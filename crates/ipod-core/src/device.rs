//! iPod のドライブ: マウント位置の検出・モデル / 署名方式の判定・取り出し。

use crate::t;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::ipod::hash58::parse_firewire_id;

const APPLE_VID: u16 = 0x05AC;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChecksumKind {
    /// 署名不要 (5G 以前)
    None,
    /// iPod Classic / nano 3G・4G
    Hash58,
    /// hash72 / hashAB が必要な機種 (nano 5G 以降) — 非対応
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub root: PathBuf,
    pub model_number: Option<String>,
    pub model_name: String,
    /// 16 桁 hex
    pub firewire_id: Option<String>,
    /// firewire_id をどこから取ったか (画面に出して、手で変える必要が無いことを示す)
    pub firewire_id_source: Option<FirewireSource>,
    pub usb_product_id: Option<u16>,
    pub checksum: ChecksumKind,
    /// アートワーク形式 (空ならアートワーク非対応)
    #[serde(skip)]
    pub art_formats: Vec<crate::ipod::artwork::ArtFormat>,
    /// 写真の形式 (空なら機種からは分からない。既存の Photo Database があればそちらを使う)
    #[serde(skip)]
    pub photo_formats: Vec<crate::ipod::artwork::ArtFormat>,
    /// iPod OS で写真を表示できる機種か (型番が分からなければ false。既存の Photo Database があれば写真の一覧は出る)
    pub photos: bool,
    /// iPod OS で動画を再生できる機種か (型番が分からなければ true)
    pub video: bool,
    /// iPod_Control/Music の Fxx フォルダの数 (型番から。分からなければ None)
    #[serde(skip)]
    pub music_dirs: Option<u32>,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FirewireSource {
    /// iPod_Control/Device/SysInfoExtended の FireWireGUID
    SysInfoExtended,
    /// iPod_Control/Device/SysInfo の FirewireGuid
    SysInfo,
    /// USB のシリアル番号 (iPod では FireWire GUID と同じ)
    Usb,
    /// 画面で入力した値
    Manual,
}

impl DeviceInfo {
    pub fn firewire_id_bytes(&self) -> Option<[u8; 8]> {
        self.firewire_id.as_deref().and_then(parse_firewire_id)
    }
}

/// iPod_Control/iTunes を持つボリュームを列挙する
pub fn find_ipod_roots() -> Vec<PathBuf> {
    candidate_roots().into_iter().filter(|r| looks_like_ipod(r)).collect()
}

/// `root` が `find_ipod_roots` に入るか。ほかのドライブ・ボリュームの中は見ない
/// (ネットワークドライブや眠っている外付けディスクがあると、全部を調べるだけで待たされることがある)
pub fn is_ipod_root(root: &Path) -> bool {
    candidate_roots().iter().any(|r| r == root) && looks_like_ipod(root)
}

/// Rockbox だけを入れた iPod (iTunes を使ったことがない) は iPod_Control が無いことがある
fn looks_like_ipod(root: &Path) -> bool {
    root.join("iPod_Control").join("iTunes").is_dir() || root.join(".rockbox").is_dir()
}

/// USB で iPod を認識したのに、ドライブとして見えないときの理由 (分かる範囲。Windows と macOS)。
/// - `"ejected"` (Windows): 取り出したあと、ケーブルを挿したままになっている (iPod のディスクはあるが中身 (メディア) が無い。
///   ZenPod を起動し直したときや、ZenPod の外で取り出したとき)。挿し直すまでドライブは出ない
/// - `"fsck"` (Mac): Mac がディスクを検査している (取り出さずに抜いたあとなど。容量が大きいと数分かかり、終わるとドライブが出る)
/// - `"denied"` (Mac): ドライブはあるが、このアプリに中を読む許可が無い (システム設定のプライバシーの「リムーバブルボリューム」)
/// - `"none"`: どれでもない
///
/// Linux は None
pub fn drive_wait_reason() -> Option<&'static str> {
    let run = |cmd: &str, args: &[&str]| {
        let mut c = std::process::Command::new(cmd);
        c.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    if cfg!(windows) {
        let serials = usb_ipod_guids();
        if serials.is_empty() {
            return Some("none");
        }
        // USB の外付けディスクの PNP ID と大きさ。iPod のディスクの PNP ID には USB のシリアル (FireWire GUID) が入る
        let disks = run(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_DiskDrive -Filter \"InterfaceType='USB'\" | ForEach-Object { $_.PNPDeviceID + '|' + $_.Size }",
            ],
        );
        return Some(if media_ejected(&disks, &serials) { "ejected" } else { "none" });
    }
    if !cfg!(target_os = "macos") {
        return None;
    }
    // 名前だけ (-c) の一覧。検査は fsck_msdos (FAT32)・fsck_hfs (Mac 形式)・fsck_exfat など
    let procs = run("ps", &["-A", "-c", "-o", "comm="]);
    let procs: Vec<&str> = procs.lines().map(str::trim).collect();
    let mounts = run("mount", &[]);
    Some(wait_reason(&procs, &ipod_like_mounts(&mounts)))
}

/// `mount` の出力 ("/dev/disk4s2 on /Volumes/IPOD (msdos, local, …)") から、iPod でありうるボリューム
/// (/Volumes の下の FAT32・Mac 形式・exFAT) の場所
fn ipod_like_mounts(mount: &str) -> Vec<PathBuf> {
    mount
        .lines()
        .filter_map(|l| {
            let (_, rest) = l.split_once(" on ")?;
            let (path, opts) = rest.rsplit_once(" (")?;
            let fs = opts.split(',').next()?.trim();
            (path.starts_with("/Volumes/") && matches!(fs, "msdos" | "hfs" | "exfat")).then(|| PathBuf::from(path))
        })
        .collect()
}

/// つながっている iPod の USB のシリアルの末尾 16 桁 (FireWire GUID、大文字)
fn usb_ipod_guids() -> Vec<String> {
    use nusb::MaybeFuture;
    let Ok(devices) = nusb::list_devices().wait() else { return Vec::new() };
    devices
        .filter(|d| d.vendor_id() == APPLE_VID && (0x1200..0x1300).contains(&d.product_id()))
        .filter_map(|d| d.serial_number().map(str::to_ascii_uppercase))
        .filter(|s| s.len() >= 16)
        .map(|s| s[s.len() - 16..].to_string())
        .collect()
}

/// `Win32_DiskDrive` の「PNP ID|大きさ」の行のうち、iPod のもの (PNP ID にシリアルが入っている) の大きさが空 = 中身が無い
fn media_ejected(disks: &str, serials: &[String]) -> bool {
    disks.lines().any(|l| {
        let Some((pnp, size)) = l.trim().rsplit_once('|') else { return false };
        let pnp = pnp.to_ascii_uppercase();
        size.trim().is_empty() && serials.iter().any(|s| pnp.contains(s.as_str()))
    })
}

fn wait_reason(procs: &[&str], volumes: &[PathBuf]) -> &'static str {
    if procs.iter().any(|p| p.starts_with("fsck_")) {
        return "fsck";
    }
    let denied = volumes.iter().any(|v| fs::read_dir(v).is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied));
    if denied { "denied" } else { "none" }
}

fn candidate_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        for c in b'D'..=b'Z' {
            v.push(PathBuf::from(format!("{}:\\", c as char)));
        }
    } else if cfg!(target_os = "macos") {
        push_children(&mut v, Path::new("/Volumes"));
    } else {
        for base in ["/media", "/run/media"] {
            if let Ok(rd) = fs::read_dir(base) {
                for user in rd.flatten() {
                    push_children(&mut v, &user.path());
                }
            }
            push_children(&mut v, Path::new(base));
        }
        push_children(&mut v, Path::new("/mnt"));
    }
    v
}

fn push_children(v: &mut Vec<PathBuf>, dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            v.push(e.path());
        }
    }
}

/// iPod を開くのにかかった時間の内訳 (ミリ秒) と、速さに関わる状態。利用状況で、どこが遅いかを見るため
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTimes {
    /// 機種を調べた時間 (SysInfo の読み取り・USB の問い合わせ・容量の問い合わせ)
    pub probe_ms: u64,
    /// そのうち USB の問い合わせ
    pub usb_ms: u64,
    /// そのうち容量 (全体と空き) の問い合わせ
    pub space_ms: u64,
    /// iTunesDB を読んで解析した時間 (Rockbox モードでは iPod の名前を知るため)
    pub db_ms: u64,
    /// ArtworkDB を読んだ時間 (iPod OS モード)
    pub art_ms: u64,
    /// 開いたあと、画面に渡す一覧を作った時間 (使う側が入れる)
    pub snapshot_ms: u64,
    /// macOS だけ: Spotlight がこの iPod を索引しているか (`.Spotlight-V100` があり `.metadata_never_index` が無い)。
    /// 索引を作っている間は HDD を取り合うため
    pub spotlight: Option<bool>,
}

fn ms_since(t: std::time::Instant) -> u64 {
    t.elapsed().as_millis() as u64
}

/// マウント位置から機種情報を集める
pub fn probe(root: &Path, existing_db_scheme: Option<u16>) -> DeviceInfo {
    probe_timed(root, existing_db_scheme, &mut OpenTimes::default())
}

/// `probe` と同じ。かかった時間を `times` に入れる
pub fn probe_timed(root: &Path, existing_db_scheme: Option<u16>, times: &mut OpenTimes) -> DeviceInfo {
    let started = std::time::Instant::now();
    if cfg!(target_os = "macos") {
        times.spotlight = Some(root.join(".Spotlight-V100").is_dir() && !root.join(".metadata_never_index").exists());
    }
    let dev = root.join("iPod_Control").join("Device");
    let sysinfo = fs::read_to_string(dev.join("SysInfo")).unwrap_or_default();
    let sysinfo_ext = fs::read_to_string(dev.join("SysInfoExtended")).unwrap_or_default();

    let model_number = sysinfo_value(&sysinfo, "ModelNumStr");
    let usb_started = std::time::Instant::now();
    let usb = find_usb_ipod();
    times.usb_ms = ms_since(usb_started);

    let valid = |s: Option<String>| s.filter(|s| parse_firewire_id(s).is_some());
    let (firewire_id, firewire_id_source) = valid(plist_string(&sysinfo_ext, "FireWireGUID"))
        .map(|s| (s, FirewireSource::SysInfoExtended))
        .or_else(|| valid(sysinfo_value(&sysinfo, "FirewireGuid")).map(|s| (s, FirewireSource::SysInfo)))
        .or_else(|| valid(usb.as_ref().and_then(|u| u.1.clone())).map(|s| (s, FirewireSource::Usb)))
        .map(|(s, src)| (Some(normalize_hex(&s)), Some(src)))
        .unwrap_or((None, None));

    let usb_pid = usb.map(|u| u.0);
    let db_version = plist_integer(&sysinfo_ext, "DBVersion");
    let m = model_lookup(model_number.as_deref(), usb_pid);
    let (model_name, model_checksum, music_dirs) = (m.name, m.checksum, m.music_dirs);
    // ジャケットの形式は本体の SysInfoExtended を優先する (libgpod と同じ)。無ければ機種の既定値
    let art_formats = Some(sysinfo_art_formats(&sysinfo_ext)).filter(|f| !f.is_empty()).unwrap_or_else(|| m.art_formats.to_vec());

    let checksum = match db_version {
        Some(0..=2) => ChecksumKind::None,
        Some(3) => ChecksumKind::Hash58,
        Some(_) => ChecksumKind::Unsupported,
        None => match existing_db_scheme {
            Some(1) => ChecksumKind::Hash58,
            Some(2..) => ChecksumKind::Unsupported,
            _ => model_checksum,
        },
    };

    let space_started = std::time::Instant::now();
    let (total_bytes, free_bytes) = (fs4::total_space(root).unwrap_or(0), fs4::available_space(root).unwrap_or(0));
    times.space_ms = ms_since(space_started);
    times.probe_ms = ms_since(started);
    DeviceInfo {
        root: root.to_path_buf(),
        model_number,
        model_name,
        firewire_id,
        firewire_id_source,
        usb_product_id: usb_pid,
        checksum,
        art_formats,
        photo_formats: m.generation.map(|g| g.photo_formats().to_vec()).unwrap_or_default(),
        photos: m.generation.is_some_and(|g| !g.photo_formats().is_empty()),
        video: m.generation.is_none_or(Gen::video),
        music_dirs,
        total_bytes,
        free_bytes,
    }
}

fn normalize_hex(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s)
        .to_ascii_uppercase()
}

/// SysInfo の "Key: value" 行
fn sysinfo_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key)
            .then(|| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

/// SysInfoExtended (XML plist) から <key>K</key><string>V</string> を抜く簡易パーサ
fn plist_string(xml: &str, key: &str) -> Option<String> {
    plist_value(xml, key, "string")
}

fn plist_integer(xml: &str, key: &str) -> Option<u32> {
    plist_value(xml, key, "integer")?.parse().ok()
}

/// SysInfoExtended の AlbumArt (本体が受け付けるジャケットの形式)。書ける RGB565 (LE) のものだけ。読めなければ空
fn sysinfo_art_formats(xml: &str) -> Vec<crate::ipod::artwork::ArtFormat> {
    use crate::ipod::artwork::{MAX_THUMB_DIM, fa};
    let Ok(plist::Value::Dictionary(d)) = plist::Value::from_reader_xml(xml.as_bytes()) else { return Vec::new() };
    let Some(list) = d.get("AlbumArt").and_then(|v| v.as_array()) else { return Vec::new() };
    list.iter()
        .filter_map(|v| {
            let d = v.as_dictionary()?;
            let int = |k: &str| d.get(k).and_then(|v| v.as_unsigned_integer());
            // "4C353635" = L565 (libgpod の THUMB_FORMAT_RGB565_LE)
            if !d.get("PixelFormat")?.as_string()?.eq_ignore_ascii_case("4C353635") {
                return None;
            }
            let id = u32::try_from(int("FormatId")?).ok()?;
            let dim = |k: &str| int(k).filter(|v| (1..=MAX_THUMB_DIM as u64).contains(v)).map(|v| v as u32);
            let (w, h) = (dim("RenderWidth")?, dim("RenderHeight")?);
            // AlignRowBytes だけのときは 4 (libgpod と同じ。nano 3G など)
            let align = match int("RowBytesAlignment") {
                Some(a) => u32::try_from(a).ok().filter(|a| *a <= 64)?,
                None if d.get("AlignRowBytes").and_then(|v| v.as_boolean()) == Some(true) => 4,
                None => 0,
            };
            Some(fa(id, w, h, align))
        })
        .collect()
}

fn plist_value(xml: &str, key: &str, ty: &str) -> Option<String> {
    let k = format!("<key>{key}</key>");
    let rest = &xml[xml.find(&k)? + k.len()..];
    let open = format!("<{ty}>");
    let close = format!("</{ty}>");
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(&open)?;
    Some(rest[..rest.find(&close)?].trim().to_string())
}

/// ディスクとして使える iPod の USB の product id。0x129x 以降は iPhone・iPod touch・iPad (ディスクにならない)
const DISK_IPOD_PIDS: std::ops::Range<u16> = 0x1200..0x1290;

/// ディスクとして使える iPod が USB につながっているか。
/// ドライブとして見える前 (OS がディスクを準備している間) からわかるので、待っていることを画面に出せる
pub fn usb_disk_ipod_connected() -> bool {
    use nusb::MaybeFuture;
    nusb::list_devices()
        .wait()
        .is_ok_and(|mut l| l.any(|d| d.vendor_id() == APPLE_VID && DISK_IPOD_PIDS.contains(&d.product_id())))
}

/// 接続中の Apple 製 iPod (USB) の (product id, serial)。iPod の USB シリアルは FireWire GUID と同じ。
fn find_usb_ipod() -> Option<(u16, Option<String>)> {
    use nusb::MaybeFuture;
    let devices = nusb::list_devices().wait().ok()?;
    devices
        .filter(|d| d.vendor_id() == APPLE_VID && (0x1200..0x1300).contains(&d.product_id()))
        .map(|d| {
            let serial = d
                .serial_number()
                .map(|s| s.to_string())
                .filter(|s| parse_firewire_id(s).is_some());
            (d.product_id(), serial)
        })
        .max_by_key(|(_, s)| s.is_some())
}

/// 機種の世代。名前・署名方式・アートワーク形式を決める
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gen {
    First,
    Second,
    Third,
    Fourth,
    Photo,
    Mini1,
    Mini2,
    Nano1,
    Nano2,
    Video1,
    Video2,
    Classic1,
    Classic2,
    Classic3,
    Nano3,
    Nano4,
    /// nano 5G 以降 (署名の方式が違い、書き込めない)
    Nano5Plus,
}

impl Gen {
    fn name(self) -> &'static str {
        match self {
            Gen::First => "iPod (1G)",
            Gen::Second => "iPod (2G)",
            Gen::Third => "iPod (3G)",
            Gen::Fourth => "iPod (4G)",
            Gen::Photo => "iPod photo (4G)",
            Gen::Mini1 => "iPod mini (1G)",
            Gen::Mini2 => "iPod mini (2G)",
            Gen::Nano1 => "iPod nano (1G)",
            Gen::Nano2 => "iPod nano (2G)",
            Gen::Video1 => "iPod video (5G)",
            Gen::Video2 => "iPod video (5.5G)",
            Gen::Classic1 => "iPod classic (6G)",
            Gen::Classic2 => "iPod classic (6.5G)",
            Gen::Classic3 => "iPod classic (7G)",
            Gen::Nano3 => "iPod nano (3G)",
            Gen::Nano4 => "iPod nano (4G)",
            // tr: 使う側で訳す (model_lookup)
            Gen::Nano5Plus => "iPod nano (5G 以降)",
        }
    }

    fn checksum(self) -> ChecksumKind {
        match self {
            Gen::Classic1 | Gen::Classic2 | Gen::Classic3 | Gen::Nano3 | Gen::Nano4 => ChecksumKind::Hash58,
            Gen::Nano5Plus => ChecksumKind::Unsupported,
            _ => ChecksumKind::None,
        }
    }

    /// アートワーク形式 (空ならアートワーク非対応)。既存の ArtworkDB があればそちらが優先される。
    /// 白黒液晶の 1G〜4G と mini は画像を表示できない
    fn art_formats(self) -> &'static [crate::ipod::artwork::ArtFormat] {
        use crate::ipod::artwork::{CLASSIC, NANO, NANO4, PHOTO, VIDEO};
        match self {
            Gen::Photo => PHOTO,
            Gen::Nano1 | Gen::Nano2 => NANO,
            Gen::Video1 | Gen::Video2 => VIDEO,
            Gen::Classic1 | Gen::Classic2 | Gen::Classic3 | Gen::Nano3 => CLASSIC,
            Gen::Nano4 => NANO4,
            _ => &[],
        }
    }

    /// 写真の形式 (空なら写真を表示できない)
    fn photo_formats(self) -> &'static [crate::ipod::artwork::ArtFormat] {
        use crate::ipod::artwork::{CLASSIC_PHOTOS, NANO_PHOTOS, NANO4_PHOTOS, PHOTO_PHOTOS, VIDEO_PHOTOS};
        match self {
            Gen::Photo => PHOTO_PHOTOS,
            Gen::Nano1 | Gen::Nano2 => NANO_PHOTOS,
            Gen::Video1 | Gen::Video2 => VIDEO_PHOTOS,
            Gen::Classic1 | Gen::Classic2 | Gen::Classic3 | Gen::Nano3 => CLASSIC_PHOTOS,
            Gen::Nano4 => NANO4_PHOTOS,
            _ => &[],
        }
    }

    /// 動画を再生できるか (video 5G 以降と nano 3G 以降)
    fn video(self) -> bool {
        matches!(self, Gen::Video1 | Gen::Video2 | Gen::Classic1 | Gen::Classic2 | Gen::Classic3 | Gen::Nano3 | Gen::Nano4 | Gen::Nano5Plus)
    }
}

/// 機種の型番 (ModelNumStr の末尾 4 桁) → (世代, 容量 GB, iPod_Control/Music の Fxx フォルダの数)。
/// libgpod 0.8.3 の itdb_device.c の ipod_info_table から (容量の表記も同じ)
const MODELS: &[(&str, u32, Gen, u32)] = &[
    ("8513", 5, Gen::First, 20),
    ("8541", 5, Gen::First, 20),
    ("8697", 5, Gen::First, 20),
    ("8709", 10, Gen::First, 20),
    ("8737", 10, Gen::Second, 20),
    ("8740", 10, Gen::Second, 20),
    ("8738", 20, Gen::Second, 50),
    ("8741", 20, Gen::Second, 50),
    ("8976", 10, Gen::Third, 20),
    ("8946", 15, Gen::Third, 50),
    ("9460", 15, Gen::Third, 50),
    ("9244", 20, Gen::Third, 50),
    ("8948", 30, Gen::Third, 50),
    ("9245", 40, Gen::Third, 50),
    ("9282", 20, Gen::Fourth, 50),
    ("9787", 25, Gen::Fourth, 50),
    ("9268", 40, Gen::Fourth, 50),
    ("9160", 4, Gen::Mini1, 6),
    ("9436", 4, Gen::Mini1, 6),
    ("9435", 4, Gen::Mini1, 6),
    ("9434", 4, Gen::Mini1, 6),
    ("9437", 4, Gen::Mini1, 6),
    ("9800", 4, Gen::Mini2, 6),
    ("9802", 4, Gen::Mini2, 6),
    ("9804", 4, Gen::Mini2, 6),
    ("9806", 4, Gen::Mini2, 6),
    ("9801", 6, Gen::Mini2, 20),
    ("9803", 6, Gen::Mini2, 20),
    ("9805", 6, Gen::Mini2, 20),
    ("9807", 6, Gen::Mini2, 20),
    ("A079", 20, Gen::Photo, 50),
    ("A127", 20, Gen::Photo, 50),
    ("9829", 30, Gen::Photo, 50),
    ("9585", 40, Gen::Photo, 50),
    ("9830", 60, Gen::Photo, 50),
    ("9586", 60, Gen::Photo, 50),
    ("A350", 1, Gen::Nano1, 3),
    ("A352", 1, Gen::Nano1, 3),
    ("A004", 2, Gen::Nano1, 3),
    ("A099", 2, Gen::Nano1, 3),
    ("A005", 4, Gen::Nano1, 6),
    ("A107", 4, Gen::Nano1, 6),
    ("A002", 30, Gen::Video1, 50),
    ("A146", 30, Gen::Video1, 50),
    ("A003", 60, Gen::Video1, 50),
    ("A147", 60, Gen::Video1, 50),
    ("A452", 30, Gen::Video1, 50),
    ("A444", 30, Gen::Video2, 50),
    ("A446", 30, Gen::Video2, 50),
    ("A664", 30, Gen::Video2, 50),
    ("A448", 80, Gen::Video2, 50),
    ("A450", 80, Gen::Video2, 50),
    ("A477", 2, Gen::Nano2, 3),
    ("A426", 4, Gen::Nano2, 6),
    ("A428", 4, Gen::Nano2, 6),
    ("A487", 4, Gen::Nano2, 6),
    ("A489", 4, Gen::Nano2, 6),
    ("A725", 4, Gen::Nano2, 6),
    ("A726", 8, Gen::Nano2, 6),
    ("A497", 8, Gen::Nano2, 14),
    ("E436", 40, Gen::Fourth, 50),
    ("S492", 30, Gen::Photo, 50),
    ("B029", 80, Gen::Classic1, 50),
    ("B147", 80, Gen::Classic1, 50),
    ("B145", 160, Gen::Classic1, 50),
    ("B150", 160, Gen::Classic1, 50),
    ("B562", 120, Gen::Classic2, 50),
    ("B565", 120, Gen::Classic2, 50),
    ("C293", 160, Gen::Classic3, 50),
    ("C297", 160, Gen::Classic3, 50),
    ("A978", 4, Gen::Nano3, 6),
    ("A980", 8, Gen::Nano3, 14),
    ("B261", 8, Gen::Nano3, 14),
    ("B249", 8, Gen::Nano3, 14),
    ("B253", 8, Gen::Nano3, 14),
    ("B257", 8, Gen::Nano3, 14),
    ("B480", 4, Gen::Nano4, 14),
    ("B651", 4, Gen::Nano4, 14),
    ("B654", 4, Gen::Nano4, 14),
    ("B657", 4, Gen::Nano4, 14),
    ("B660", 4, Gen::Nano4, 14),
    ("B663", 4, Gen::Nano4, 14),
    ("B666", 4, Gen::Nano4, 14),
    ("B598", 8, Gen::Nano4, 14),
    ("B732", 8, Gen::Nano4, 14),
    ("B735", 8, Gen::Nano4, 14),
    ("B739", 8, Gen::Nano4, 14),
    ("B742", 8, Gen::Nano4, 14),
    ("B745", 8, Gen::Nano4, 14),
    ("B748", 8, Gen::Nano4, 14),
    ("B751", 8, Gen::Nano4, 14),
    ("B754", 8, Gen::Nano4, 14),
    ("B903", 16, Gen::Nano4, 14),
    ("B905", 16, Gen::Nano4, 14),
    ("B907", 16, Gen::Nano4, 14),
    ("B909", 16, Gen::Nano4, 14),
    ("B911", 16, Gen::Nano4, 14),
    ("B913", 16, Gen::Nano4, 14),
    ("B915", 16, Gen::Nano4, 14),
    ("B917", 16, Gen::Nano4, 14),
    ("B918", 16, Gen::Nano4, 14),
];

/// 型番が分からないとき、USB の製品 ID から世代を決める (usb.ids の Apple の項目)
fn gen_from_usb(pid: u16) -> Option<Gen> {
    Some(match pid {
        0x1201 => Gen::Third,
        0x1202 => Gen::Second,
        0x1203 => Gen::Fourth,
        0x1204 => Gen::Photo,
        0x1205 => Gen::Mini1,
        0x1209 => Gen::Video1,
        0x120a => Gen::Nano1,
        0x1260 => Gen::Nano2,
        0x1261 => Gen::Classic1,
        0x1262 => Gen::Nano3,
        0x1263 => Gen::Nano4,
        0x1265..=0x1267 => Gen::Nano5Plus,
        _ => return None,
    })
}

/// 型番か USB の製品 ID から分かる機種の情報
struct ModelInfo {
    name: String,
    checksum: ChecksumKind,
    art_formats: &'static [crate::ipod::artwork::ArtFormat],
    /// Fxx フォルダの数 (分からなければ None)
    music_dirs: Option<u32>,
    generation: Option<Gen>,
}

fn model_lookup(model: Option<&str>, pid: Option<u16>) -> ModelInfo {
    // ModelNumStr は "xB029" / "MB029" のような形式。末尾 4 桁で判定
    if let Some(m) = model {
        let code = m.get(m.len().saturating_sub(4)..).unwrap_or(m).to_ascii_uppercase();
        if let Some(&(_, gb, g, dirs)) = MODELS.iter().find(|(c, ..)| *c == code) {
            return ModelInfo {
                name: format!("{} {gb}GB", crate::i18n::tr(g.name())),
                checksum: g.checksum(),
                art_formats: g.art_formats(),
                music_dirs: Some(dirs),
                generation: Some(g),
            };
        }
    }
    match pid.and_then(gen_from_usb) {
        Some(g) => ModelInfo { name: crate::i18n::tr(g.name()), checksum: g.checksum(), art_formats: g.art_formats(), music_dirs: None, generation: Some(g) },
        None => ModelInfo { name: "iPod".into(), checksum: ChecksumKind::None, art_formats: &[], music_dirs: None, generation: None },
    }
}

// ---------------- 書き出し ----------------

/// 書いた中身をディスクへ渡す。macOS の `sync_all` はディスクのキャッシュまで書き出させる F_FULLFSYNC で、
/// FAT の iPod では 1 ファイルに 10ms ほどかかるので fsync にする (キャッシュは DB を書くときの `sync_all` で書き出す)
pub(crate) fn sync_file(f: &fs::File) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `f` が開いている間の fd を渡すだけ
        if unsafe { libc::fsync(f.as_raw_fd()) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
    #[cfg(not(target_os = "macos"))]
    f.sync_all()
}

/// macOS が FAT のドライブに作る `._名前` (拡張属性の置き場) を消す。戻り値は消した数。
/// アプリが書いたファイル・フォルダにも OS が付けるので、取り出しの前に片付ける。
/// OS の隠しフォルダ (.Trashes・.Spotlight-V100 など) の中は見ない
pub fn remove_apple_double(root: &Path) -> usize {
    let walk = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        e.depth() == 0 || !e.file_type().is_dir() || !name.starts_with('.') || name == ".rockbox"
    });
    let mut removed = 0;
    for e in walk.flatten() {
        if e.file_type().is_file() && e.file_name().to_string_lossy().starts_with("._") {
            match fs::remove_file(e.path()) {
                Ok(()) => removed += 1,
                Err(err) => log::warn!("{}: {err}", e.path().display()),
            }
        }
    }
    removed
}

// ---------------- 取り出し ----------------

/// iPod を取り出す (保存は事前に済ませること)。
/// OS に取り出しを頼んだあと、iPod のフォルダが見えなくなるまで待って確かめる
/// (Windows のエクスプローラー経由の取り出しは完了を待たずに戻り、失敗しても分からないため)。
/// ドライブそのものではない (「フォルダを指定して開く」で開いた) ときは何もせず false を返す
/// (OS の取り出しが空振りし、フォルダが消えるのを 30 秒待ったうえで失敗していた)
pub fn eject(root: &Path) -> Result<bool> {
    use std::process::Command;
    use std::time::{Duration, Instant};
    const TIMEOUT: Duration = Duration::from_secs(15);
    if !is_volume_root(root) {
        return Ok(false);
    }
    if cfg!(target_os = "macos") {
        remove_apple_double(root);
    }
    let marker = root.join("iPod_Control");
    let status = if cfg!(target_os = "macos") {
        Command::new("diskutil").arg("eject").arg(root).status()
    } else if cfg!(windows) {
        let drive = root.to_string_lossy().trim_end_matches('\\').to_string();
        // InvokeVerb は非同期なので、PowerShell がすぐ終わると取り出しが途中で止まることがある。
        // ドライブが消えるまで PowerShell 側でも待つ
        let script = format!(
            "(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{drive}').InvokeVerb('Eject'); \
             $t = [DateTime]::Now.AddSeconds({}); \
             while ((Test-Path '{drive}\\iPod_Control') -and [DateTime]::Now -lt $t) {{ Start-Sleep -Milliseconds 300 }}",
            TIMEOUT.as_secs()
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &script]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.status()
    } else {
        Command::new("gio")
            .args(["mount", "-e"])
            .arg(root)
            .status()
            .or_else(|_| Command::new("umount").arg(root).status())
    };
    let failed = || {
        Error::Other(
            t!("取り出せませんでした。iPod のファイルを開いているアプリ (エクスプローラーなど) を閉じてから、もう一度お試しください。ケーブルはまだ抜かないでください"),
        )
    };
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => return Err(failed()),
        Err(e) => return Err(e.into()),
    }
    let start = Instant::now();
    while marker.exists() {
        if start.elapsed() > TIMEOUT {
            return Err(failed());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(true)
}

/// ドライブ (ボリューム) そのものか。Windows は `E:\` (親が無い)、Mac は /Volumes の直下、
/// Linux はマウントした場所 (親と別のファイルシステム)
fn is_volume_root(root: &Path) -> bool {
    if cfg!(windows) {
        root.parent().is_none()
    } else if cfg!(target_os = "macos") {
        root.parent() == Some(Path::new("/Volumes"))
    } else {
        is_mount_point(root)
    }
}

#[cfg(unix)]
fn is_mount_point(root: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(root), root.parent().map(fs::metadata)) {
        (Ok(m), Some(Ok(p))) => m.dev() != p.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn is_mount_point(_root: &Path) -> bool {
    false
}

// ---------------- 前の版のファイル名 ----------------

/// 2.0.0 で名前を iPod Sync から ZenPod に変え、iPod に置くファイルの名前も変えた。
/// 前の版が置いたファイル (同じフォルダの `old_name`) があれば、新しい名前に付け替える
pub(crate) fn adopt_legacy_name(path: &Path, old_name: &str) {
    if path.exists() {
        return;
    }
    let old = path.with_file_name(old_name);
    if old.exists() {
        let _ = fs::rename(&old, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `._` で始まるファイルだけを消し、.rockbox の中は片付け、OS の隠しフォルダの中には触らない
    #[test]
    fn removes_apple_double_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["Music/A", ".rockbox", ".Trashes"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["Music/A/01.mp3", "Music/A/._01.mp3", "Music/._A", "._Music", ".rockbox/._database_0.tcd", ".Trashes/._x", "Music/A/.hidden"] {
            fs::write(root.join(f), b"x").unwrap();
        }

        assert_eq!(remove_apple_double(root), 4);
        let left = |f: &str| root.join(f).exists();
        assert!(!left("Music/A/._01.mp3") && !left("Music/._A") && !left("._Music") && !left(".rockbox/._database_0.tcd"));
        assert!(left("Music/A/01.mp3") && left(".Trashes/._x") && left("Music/A/.hidden"));
    }

    #[test]
    fn ejected_ipod_media() {
        let serials = vec!["000A27001A2B3C4D".to_string()];
        let ejected = "USBSTOR\\DISK&VEN_TOSHIBA&PROD_MK3008GAL&REV_BU11\\8&1A2B3C4&0&300000000000A27001A2B3C4D&0|\r\n";
        assert!(media_ejected(ejected, &serials));
        let mounted = "USBSTOR\\DISK&VEN_APPLE&PROD_IPOD&REV_1.62\\000A27001A2B3C4D&0|30005821440";
        assert!(!media_ejected(mounted, &serials));
        // 空のカードリーダー (シリアルが違う) は iPod ではない
        let reader = "USBSTOR\\DISK&VEN_GENERIC-&PROD_SD_MMC&REV_1.00\\20120926571200000&0|";
        assert!(!media_ejected(reader, &serials));
        assert!(!media_ejected("", &serials));
    }

    #[test]
    fn drive_wait_reasons() {
        let mount = "/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)\n\
                     /dev/disk4s2 on /Volumes/JOHN'S IPOD (msdos, local, nodev, nosuid, noowners, noatime)\n\
                     /dev/disk5s1 on /Volumes/Backup (apfs, local, nodev, nosuid, journaled)\n\
                     /dev/disk6s2 on /Volumes/Mac iPod (hfs, local, nodev, nosuid, journaled, noowners)\n";
        assert_eq!(ipod_like_mounts(mount), vec![PathBuf::from("/Volumes/JOHN'S IPOD"), PathBuf::from("/Volumes/Mac iPod")]);

        let dir = tempfile::tempdir().unwrap();
        let ipod = dir.path().join("IPOD");
        fs::create_dir_all(&ipod).unwrap();
        let vols = vec![ipod.clone()];
        assert_eq!(wait_reason(&["launchd"], &vols), "none");
        assert_eq!(wait_reason(&["launchd", "fsck_msdos"], &vols), "fsck");
        // 読めないボリューム (root で動かしているときは読めてしまうので確かめない)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&ipod, fs::Permissions::from_mode(0o000)).unwrap();
            if fs::read_dir(&ipod).is_err() {
                assert_eq!(wait_reason(&[], &vols), "denied");
            }
            fs::set_permissions(&ipod, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn parse_sysinfo() {
        let s = "BoardHwName: iPod Q98\nFirewireGuid: 0x000A27001A2B3C4D\nModelNumStr: xB029\n";
        assert_eq!(sysinfo_value(s, "ModelNumStr").as_deref(), Some("xB029"));
        assert_eq!(
            sysinfo_value(s, "FirewireGuid").as_deref(),
            Some("0x000A27001A2B3C4D")
        );
        assert_eq!(model_lookup(Some("xB029"), None).checksum, ChecksumKind::Hash58);
    }

    #[test]
    fn older_models() {
        use crate::ipod::artwork::{CLASSIC_PHOTOS, NANO, PHOTO, VIDEO};
        let l = |m: Option<&str>, p: Option<u16>| {
            let i = model_lookup(m, p);
            (i.name, i.checksum, i.art_formats, i.music_dirs)
        };
        // 型番から: 名前・署名・アートワーク・フォルダの数
        let (name, sum, art, dirs) = l(Some("xA079"), None);
        assert_eq!((name.as_str(), sum, art, dirs), ("iPod photo (4G) 20GB", ChecksumKind::None, PHOTO, Some(50)));
        let (name, sum, art, dirs) = l(Some("MA350"), None);
        assert_eq!((name.as_str(), sum, art, dirs), ("iPod nano (1G) 1GB", ChecksumKind::None, NANO, Some(3)));
        // 白黒液晶の 4G と mini はアートワーク無し
        let (_, sum, art, dirs) = l(Some("x9282"), None);
        assert_eq!((sum, art.len(), dirs), (ChecksumKind::None, 0, Some(50)));
        let (name, _, art, dirs) = l(Some("x9160"), None);
        assert_eq!((name.as_str(), art.len(), dirs), ("iPod mini (1G) 4GB", 0, Some(6)));
        // 型番が無ければ USB の製品 ID から (フォルダの数は分からない)
        let (name, _, art, dirs) = l(None, Some(0x1204));
        assert_eq!((name.as_str(), art, dirs), ("iPod photo (4G)", PHOTO, None));
        assert_eq!(l(None, Some(0x1209)).2, VIDEO);
        assert_eq!(l(None, Some(0x1266)).1, ChecksumKind::Unsupported);
        // 分からない機種
        assert_eq!(l(Some("xZZZZ"), None).0, "iPod");
        // 写真と動画
        let g = |m: &str| model_lookup(Some(m), None).generation.unwrap();
        assert_eq!(g("xB029").photo_formats(), CLASSIC_PHOTOS);
        assert!(g("xB029").video() && g("xA002").video());
        assert!(!g("xA079").video() && !g("x9282").video());
        assert!(g("x9282").photo_formats().is_empty());
    }

    #[test]
    fn art_formats_from_sysinfo_extended() {
        use crate::ipod::artwork::frame_size;
        let fmt = |id: u32, w: u32, extra: &str, pix: &str| {
            format!("<dict><key>FormatId</key><integer>{id}</integer><key>RenderWidth</key><integer>{w}</integer><key>RenderHeight</key><integer>{w}</integer><key>PixelFormat</key><string>{pix}</string>{extra}</dict>")
        };
        let x = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>FireWireGUID</key><string>000A27001A2B3C4D</string><key>AlbumArt</key><array>{}{}{}{}</array></dict></plist>",
            fmt(1061, 55, "<key>AlignRowBytes</key><true/>", "4C353635"),
            fmt(1055, 128, "", "4C353635"),
            fmt(1060, 320, "<key>RowBytesAlignment</key><integer>16</integer>", "4C353635"),
            // 書けない形式 (RGB565 BE) は使わない
            fmt(1062, 100, "", "42353635"),
        );
        let f = sysinfo_art_formats(&x);
        assert_eq!(f.iter().map(|f| f.id).collect::<Vec<_>>(), [1061, 1055, 1060]);
        // 55 画素 = 110 バイトを 4 バイトにそろえて 112 バイト × 55 行
        assert_eq!(frame_size(&f[0]), 6160);
        assert_eq!(frame_size(&f[1]), 128 * 128 * 2);
        assert_eq!(frame_size(&f[2]), 320 * 320 * 2);
        // 壊れた・無いときは空 (機種の既定値を使う)
        assert!(sysinfo_art_formats("<plist><dict><key>AlbumArt").is_empty());
        assert!(sysinfo_art_formats("").is_empty());
    }

    #[test]
    fn parse_plist() {
        let x = "<dict>\n<key>DBVersion</key>\n<integer>3</integer>\n<key>FireWireGUID</key>\n<string>000A27001A2B3C4D</string>";
        assert_eq!(plist_integer(x, "DBVersion"), Some(3));
        assert_eq!(
            plist_string(x, "FireWireGUID").as_deref(),
            Some("000A27001A2B3C4D")
        );
    }
}
