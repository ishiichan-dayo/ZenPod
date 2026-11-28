//! Rockbox モード: iTunesDB を使わず、iPod の中の音声ファイルをそのまま曲として扱う。
//!
//! - 曲: iPod の中のフォルダ (iPod_Control・.rockbox などは除く) の音声ファイル。タグは PC のフォルダと同じく
//!   ファイルから読む (キャッシュ付き)。Rockbox は本体でタグから独自の DB を作るので、書くのはファイルだけ
//! - 転送: `Music/アーティスト/アルバム/01 曲名.拡張子` に変換せずにコピーする (Rockbox は FLAC・Ogg なども再生できる)
//! - ジャケット: アルバムのフォルダの cover.jpg (Rockbox はプログレッシブ JPEG を読めないので、ベースラインで書く)
//! - プレイリスト: `Playlists/名前.m3u8` (UTF-8、iPod のルートからの "/Music/..." のパス)
//! - 曲 ID: iPod のルートからのパス (大文字小文字を区別しない) のハッシュ。画面の ID と同じく u64

mod visual;
pub mod rbdb;

use crate::t;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::device::{self, DeviceInfo};
use crate::dirlist::Kind;
use crate::domain::audio::{Format, MediaKind};
use crate::domain::playlist::{PlaylistView, SmartPreview};
use crate::domain::track::{dup_key, same_number, AddOptions, AddOutcome, TagEdit, TrackTagEdit, TrackView};
use crate::error::{Error, Result};
use crate::media::{self, convert, cover, preconvert};
use crate::pc::folder::{self as pcfolder, PcTrack};
use crate::smartpl::{self, EvalContext, Rules, SmartDef, SmartPrefs, SplTrack};

/// 転送した曲を置くフォルダ (iPod のルートから)
const MUSIC_DIR: &str = "Music";
/// ポッドキャストとオーディオブックを置くフォルダ。Rockbox には種類の分け方が無いので、置き場所で分ける
/// (Podcasts は「自動で続きから再生」の既定の対象 "/podcast:/podcasts" に入る)
const PODCAST_DIR: &str = "Podcasts";
const AUDIOBOOK_DIR: &str = "Audiobooks";
/// Rockbox がプレイリストを置くフォルダ
const PLAYLIST_DIR: &str = "Playlists";
/// 曲を探さないフォルダ (iPod のルート直下。大文字小文字は区別しない)。
/// iPod_Control の曲は iPod OS の曲なので、ここで消すと iTunesDB と食い違う
const SKIP_DIRS: &[&str] = &[
    "ipod_control",
    ".rockbox",
    "system volume information",
    "$recycle.bin",
    "photos",
    // 動画と写真 (rockbox/visual.rs が扱う)
    "videos",
    "pictures",
    "calendars",
    "contacts",
    "notes",
];
/// 転送時に残しておく空き容量
const FREE_MARGIN: u64 = 20 * 1024 * 1024;
/// cover.jpg の大きさの上限 (Rockbox は表示のたびに縮小するので、大きすぎると遅い)
const COVER_MAX: u32 = 500;
/// Rockbox が再生できない形式 (転送するときだけ変換する)
const NEEDS_CONVERT: &[&str] = &["mka", "webm"];

struct RbTrack {
    id: u64,
    path: PathBuf,
    view: TrackView,
    /// ジャケットが埋め込まれているか
    has_art: bool,
    /// Rockbox のデータベースから埋めた曲 (ファイルから読み直すまで、並べ替え用のタグと has_art が無い)
    from_db: bool,
}

/// 転送の重複チェックの索引: (曲名, アーティスト, アルバム) を小文字にしたもの → (曲 ID, 長さ, (ディスク番号, トラック番号), 曲名)
type DupIndex = HashMap<(String, String, String), Vec<(u64, u32, (u32, u32), String)>>;

struct RbPlaylist {
    id: u64,
    name: String,
    path: PathBuf,
    items: Vec<u64>,
    /// 一覧に無い曲の行 (iPod_Control の iPod OS の曲・見つからないファイルなど)。(その前にあった曲の数, 元の行)。
    /// 画面には出さないが、書き直すときに消さない (本体で足した曲がアプリの編集で消えないように)
    other: Vec<(usize, String)>,
    dirty: bool,
    /// スマートプレイリストの設定と条件 (.m3u8 の先頭のコメント行に書く。中身は普通の .m3u8 の行)
    smart: Option<(SmartPrefs, Rules)>,
}

/// スマートプレイリストの条件を書くコメント行 (Rockbox は # の行を読み飛ばす)。値は iTunes の XML と同じ形 (Smart Info / SLst) の base64
const SMART_INFO_LINE: &str = "#ZENPOD-SMART-INFO:";
const SMART_RULES_LINE: &str = "#ZENPOD-SMART-RULES:";

fn b64(d: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(d)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()
}

/// .m3u8 の中のスマートプレイリストの条件
fn smart_of(text: &str) -> Option<(SmartPrefs, Rules)> {
    let line = |prefix: &str| text.lines().find_map(|l| l.trim().strip_prefix(prefix)).and_then(unb64);
    let prefs = SmartPrefs::from_xml_info(&line(SMART_INFO_LINE)?).ok()?;
    let rules = Rules::parse(&line(SMART_RULES_LINE)?).ok()?;
    Some((prefs, rules))
}

impl SplTrack for RbTrack {
    fn spl_id(&self) -> u64 {
        self.id
    }
    fn spl_text(&self, field: u32) -> Option<&str> {
        use smartpl::*;
        let v = &self.view;
        Some(match field {
            F_NAME => &v.title,
            F_ALBUM => &v.album,
            F_ARTIST => &v.artist,
            F_GENRE => &v.genre,
            F_KIND => &v.filetype,
            F_COMPOSER => &v.composer,
            F_ALBUM_ARTIST => &v.album_artist,
            F_SORT_NAME => &v.sort_title,
            F_SORT_ALBUM => &v.sort_album,
            F_SORT_ARTIST => &v.sort_artist,
            F_SORT_ALBUM_ARTIST => &v.sort_album_artist,
            _ => return None,
        })
    }
    fn spl_num(&self, field: u32) -> Option<i64> {
        use smartpl::*;
        let v = &self.view;
        Some(match field {
            F_BITRATE => v.bitrate as i64,
            F_YEAR => v.year as i64,
            F_TRACK_NUMBER => v.track_nr as i64,
            F_DISC_NUMBER => v.disc_nr as i64,
            F_SIZE => v.size as i64,
            F_TIME => v.duration_ms as i64,
            F_PLAY_COUNT => v.play_count as i64,
            F_RATING => v.rating as i64,
            // 追加日はファイルの更新日時 (転送した日時)
            F_DATE_ADDED => {
                if v.date_added > 0 {
                    unix_to_mac_local(v.date_added)
                } else {
                    0
                }
            }
            F_MEDIA_KIND => match v.kind {
                MediaKind::Music => 1,
                MediaKind::Podcast => 4,
                MediaKind::Audiobook => 8,
            },
            F_ARTWORK => self.has_art as i64,
            F_CHECKED => 1,
            // 値が無い (iPod OS と同じく iCloud の状況・場所はどの「である」にも合わない)
            F_ICLOUD | F_LOCATION => return None,
            _ => 0,
        })
    }
    fn spl_checked(&self) -> bool {
        true
    }
    fn spl_length_ms(&self) -> u64 {
        self.view.duration_ms as u64
    }
    fn spl_size(&self) -> u64 {
        self.view.size as u64
    }
}

/// UNIX 秒 → 判定に使う 1904 年からの秒 (PC の現地時刻のまま。iTunes と同じ)
fn unix_to_mac_local(t: i64) -> i64 {
    let off = chrono::Local::now().offset().local_minus_utc() as i64;
    crate::ipod::itunesdb::unix_to_mac(t + off) as i64
}

pub struct RockboxLib {
    pub info: DeviceInfo,
    name: String,
    tracks: Vec<RbTrack>,
    playlists: Vec<RbPlaylist>,
    /// 消したプレイリストのファイル (保存するときに消す)
    removed_playlists: Vec<PathBuf>,
    /// ジャケット: artwork_id - 1 → 画像を読む曲 (その曲の埋め込み画像か、同じフォルダの cover.jpg など)
    covers: Vec<PathBuf>,
    /// ジャケットのあるフォルダ → artwork_id
    dir_covers: HashMap<PathBuf, u32>,
    /// 画像ファイル (jpg / png) のあるフォルダ。曲を探すときに一緒に集め、変えたフォルダだけ調べ直す
    image_dirs: HashSet<PathBuf>,
    /// タグの読み取りのキャッシュ (PC のフォルダと同じ形式)
    cache_path: Option<PathBuf>,
    /// 曲を変えたので、保存のときに Rockbox のデータベースを作り直す
    db_dirty: bool,
    /// 転送の重複チェックの索引: (曲名, アーティスト, アルバム) を小文字にしたもの → (曲 ID, 長さ, 曲名)。
    /// 1 曲ごとに全曲と比べないよう、初めて要るときに作り、足した曲は足す。消す・編集する・名前を変えると作り直す
    dup_index: Option<DupIndex>,
    /// 最後にデータベースを作り直せなかった理由 (画面に出す)
    db_error: Option<String>,
    /// 曲を転送するときの置き場所の形 (layout.rs)。iPod ごとに画面で選ぶ
    layout: String,
    /// 最後に曲の一覧を読み直したときの内訳 (利用状況で、開くのが遅いときにどこで時間がかかったかを見る)
    scan_stats: ScanStats,
    /// 開くのにかかった時間の内訳 (曲の一覧の読み直しは scan_stats)
    open_times: device::OpenTimes,
    /// Videos の動画 (初めて使うときに読む。変えたら読み直す)
    video_cache: std::cell::OnceCell<Vec<visual::RbVideo>>,
    /// Pictures の写真 (同じ)
    photo_cache: std::cell::OnceCell<Vec<visual::RbPhoto>>,
}

/// 開いている途中で、読めた曲を知らせる (iPod の情報・名前・曲・前回の一覧か。ジャケットはまだ無い)。
/// 前回の一覧 (`true`) は、iPod の中をたどり終わる前に、前に開いたときの曲をまとめて 1 回だけ知らせる
/// (キャッシュが効いていても、HDD の iPod はフォルダをたどるだけで 3〜30 秒かかる)。そのあとは読めた曲を知らせない
pub type Partial<'a> = &'a (dyn Fn(&DeviceInfo, &str, Vec<TrackView>, bool) + Sync);

/// 開いている途中で、読めた曲を知らせる間隔
const PARTIAL_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// iPod の中をたどり終わるのをこれだけ待って、終わらなければ前回の一覧を先に知らせる
/// (すぐ終わるときに、前回の一覧を送ってすぐ置き換えることのないように)
const PREVIEW_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

/// 前回たどった iPod の中の音声ファイル (iPod のルートからの場所 ("/" 区切り)・更新日時 (秒)・大きさ)。
/// 開くときに、たどり終わる前の仮の一覧を作るのに使う (タグのキャッシュは消えた曲も残っているので、それだけでは作れない)
#[derive(serde::Serialize, serde::Deserialize)]
struct Listing {
    ver: u32,
    files: Vec<(String, u64, u64)>,
}

const LISTING_VER: u32 = 1;

/// タグのキャッシュの隣に置く、前回たどった一覧のファイル
fn listing_file(cache: &Path) -> PathBuf {
    cache.with_extension("files.json")
}

fn save_listing(path: &Path, root: &Path, files: &[(PathBuf, std::time::SystemTime, u64)]) {
    let files = files
        .iter()
        .filter_map(|(p, m, size)| {
            let rel = p.strip_prefix(root).ok()?;
            let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_str().map(str::to_string)).collect::<Option<_>>()?;
            Some((rel.join("/"), m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0), *size))
        })
        .collect();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(&Listing { ver: LISTING_VER, files }) {
        let _ = fs::write(path, json);
    }
}

fn load_listing(path: &Path, root: &Path) -> Option<Vec<(PathBuf, std::time::SystemTime, u64)>> {
    let l: Listing = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (l.ver == LISTING_VER).then(|| {
        l.files
            .into_iter()
            .map(|(rel, secs, size)| {
                let path = rel.split('/').fold(root.to_path_buf(), |p, c| p.join(c));
                (path, std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs), size)
            })
            .collect()
    })
}

/// 曲の一覧の読み直しの内訳
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    /// フォルダをたどって音声ファイルを集めた時間
    pub walk_ms: u64,
    /// タグを読んだ時間 (キャッシュの読み込み・書き出しを含む)
    pub read_ms: u64,
    /// 音声ファイルの数
    pub files: usize,
    /// そのうちキャッシュが使えずタグを読み直した数
    pub reread: usize,
    /// そのうちタグを読まずに Rockbox のデータベースから埋めた数 (あとで裏で読み直す)
    pub from_db: usize,
}

/// キャッシュに無い曲がこれより少なければ、データベースを読まずにタグを読む (データベースは曲が多いと 20MB ほどある)
const SEED_MIN: usize = 50;

/// iPod のルートからのパスを、ID と照合用の文字列にする ("music/artist/album/01 title.flac")。
/// 濁点・アクセントは合成した形 (NFC) にそろえる (macOS は FAT の名前を分けた形 (NFD) で返し、プレイリストの行にも NFD がある)
fn rel_key(root: &Path, path: &Path) -> String {
    use unicode_normalization::UnicodeNormalization;
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().nfc().collect::<String>().to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

/// 本体と同じく、拡張子が .m3u のときだけ昔の形式 (BOM が無ければ UTF-8 ではない) とみなす
fn is_m3u8_name(path: &Path) -> bool {
    !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("m3u"))
}

/// プレイリストのファイルの文字。BOM があるか .m3u8 なら UTF-8。.m3u は本体が Latin-1 (既定の文字コード) で読むが、
/// PC のアプリが BOM 無しの UTF-8 で書いたものもあるので、UTF-8 として読めればそれを使う
fn decode_playlist(path: &Path, bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) if !is_m3u8_name(path) => bytes.iter().map(|&b| b as char).collect(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 本体はドライブが複数ある機種のビルドだと "/<HDD0>/Music/..." のようにボリュームを付けて書く (firmware/common/pathfuncs.c)。
/// 0 番 (開いているドライブ) なら外して "/Music/..." にする。ほかの番号は別のドライブなのでそのまま (曲にはならない)
fn strip_volume(p: &str) -> &str {
    let Some(rest) = p.strip_prefix("/<") else { return p };
    let Some((vol, after)) = rest.split_once('>') else { return p };
    let name = vol.trim_end_matches(|c: char| c.is_ascii_digit());
    let num = &vol[name.len()..];
    let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic()) && !num.is_empty() && num.bytes().all(|b| b == b'0');
    match after {
        "" if ok => "/",
        a if ok && a.starts_with('/') => a,
        _ => p,
    }
}

fn id_of(key: &str) -> u64 {
    let h = Sha256::digest(key.as_bytes());
    // 0 は「なし」に使うので避ける。JS には文字列で渡すので桁数は気にしなくてよい
    u64::from_be_bytes(h[..8].try_into().unwrap()).max(1)
}

/// フォルダ名・ファイル名として使えるように直す (FAT32 と Windows で使えない文字・末尾の . と空白・長すぎる名前)
pub(crate) fn clean_name(s: &str, fallback: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    // 長すぎるとパス全体が Windows の上限を超えるので、1 つの名前は 80 文字まで
    if out.chars().count() > 80 {
        out = out.chars().take(80).collect();
    }
    let out = out.trim().trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() {
        return fallback.to_string();
    }
    // Windows の予約名 (CON / NUL / COM1 など) はそのままでは作れない
    let upper = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4 && (upper.starts_with("COM") || upper.starts_with("LPT")) && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        format!("_{out}")
    } else {
        out
    }
}

/// 転送先のファイル名 (拡張子なし): "01 曲名" / 複数枚組は "2-01 曲名" / 番号が無ければ "曲名"
fn file_stem(info: &media::MediaInfo, src: &Path) -> String {
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title = clean_name(info.title.as_deref().unwrap_or(&stem), "Track");
    match (info.disc_total > 1, info.disc_nr, info.track_nr) {
        (_, _, 0) => title,
        (true, d, n) if d > 0 => format!("{d}-{n:02} {title}"),
        (_, _, n) => format!("{n:02} {title}"),
    }
}

/// Rockbox が曲と同じフォルダ・同じ名前で読む歌詞のファイルの拡張子
const LYRICS_EXTS: [&str; 2] = ["lrc", "lrc8"];

/// 曲と同じフォルダにある、同じ名前の歌詞のファイル (大文字小文字は区別しない)
fn lyrics_files(audio: &Path) -> Vec<PathBuf> {
    DirLyrics::default().of(audio)
}

/// フォルダごとの歌詞のファイルの控え。何曲も扱う操作 (削除・名前の変更・種類の変更) で、
/// 同じアルバムのフォルダを曲の数だけ読まないようにする (iPod の HDD では 1 回ごとに待つ)
#[derive(Default)]
struct DirLyrics(HashMap<PathBuf, Vec<PathBuf>>);

impl DirLyrics {
    fn of(&mut self, audio: &Path) -> Vec<PathBuf> {
        let (Some(dir), Some(stem)) = (audio.parent(), audio.file_stem()) else { return Vec::new() };
        let stem = stem.to_string_lossy().to_lowercase();
        let files = self.0.entry(dir.to_path_buf()).or_insert_with(|| {
            let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| LYRICS_EXTS.contains(&e.to_string_lossy().to_ascii_lowercase().as_str())))
                .collect()
        });
        files.iter().filter(|p| p.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase() == stem)).cloned().collect()
    }
}

/// 歌詞のファイルを、曲の新しい場所 `audio` に合わせた名前にする
fn lyrics_dest(lyrics: &Path, audio: &Path) -> PathBuf {
    let ext = lyrics.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    audio.with_extension(ext)
}

/// 同じ名前のファイルがあれば " (2)" などを付ける
/// 同じファイルの名前か (iPod の FAT32 は大文字・小文字を区別しない)
fn same_file_name(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let p = dir.join(format!("{stem}.{ext}"));
    if !p.exists() {
        return p;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        // tr: 訳さない (起きない前提の panic の文言)
        .expect("いつかは空いている")
}

/// cover.jpg 用: 長い辺を COVER_MAX までに縮め (切り抜かない)、ベースラインの JPEG にする
fn cover_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    cover::fit_jpeg(bytes, COVER_MAX)
}

/// フォルダにジャケットの画像ファイル (cover.jpg / folder.jpg など) があるか
/// ジャケットに使う画像ファイルか (cover::find_cover がフォルダから探すのと同じ拡張子)
fn is_image(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png")
}

fn has_cover_file(dir: &Path) -> bool {
    ["cover.jpg", "folder.jpg", "cover.jpeg", "cover.png", "folder.png"]
        .iter()
        .any(|n| dir.join(n).is_file())
}

/// 曲の一覧から除くファイル (ジャケット・Windows と macOS が作る隠しファイル)
fn is_leftover(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("._")
        || matches!(n.as_str(), "thumbs.db" | "desktop.ini" | ".ds_store")
        || n.ends_with(".jpg")
        || n.ends_with(".jpeg")
        || n.ends_with(".png")
}

fn view_of(id: u64, t: &PcTrack) -> TrackView {
    let ext = Path::new(&t.path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    TrackView {
        id: id.to_string(),
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        album_artist: t.album_artist.clone(),
        genre: t.genre.clone(),
        composer: t.composer.clone(),
        duration_ms: t.duration_ms,
        track_nr: t.track_nr,
        disc_nr: t.disc_nr,
        year: t.year,
        size: t.size.min(u32::MAX as u64) as u32,
        filetype: ext.to_uppercase(),
        bitrate: t.bitrate_kbps,
        sort_title: t.sort_title.clone(),
        sort_artist: t.sort_artist.clone(),
        sort_album: t.sort_album.clone(),
        sort_album_artist: t.sort_album_artist.clone(),
        ..Default::default()
    }
}

/// 置き場所から曲の種類を決める (ルート直下の Podcasts / Audiobooks の中)
fn kind_of(root: &Path, path: &Path) -> MediaKind {
    let first = path.strip_prefix(root).ok().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().to_lowercase());
    match first.as_deref() {
        Some(d) if d == PODCAST_DIR.to_lowercase() => MediaKind::Podcast,
        Some(d) if d == AUDIOBOOK_DIR.to_lowercase() => MediaKind::Audiobook,
        _ => MediaKind::Music,
    }
}

/// 種類ごとの置き場所 (フォルダ): 曲は転送先の形 (`layout`) のフォルダの部分、ポッドキャストは Podcasts/番組 (アルバム)、
/// オーディオブックは Audiobooks/作者/書名
fn kind_dir(root: &Path, layout: &str, kind: MediaKind, f: &crate::domain::layout::Fields) -> PathBuf {
    let artist = if f.album_artist.trim().is_empty() { &f.artist } else { &f.album_artist };
    match kind {
        MediaKind::Music => {
            let parts = crate::domain::layout::render(layout, f, clean_name);
            parts[..parts.len() - 1].iter().fold(root.to_path_buf(), |p, c| p.join(c))
        }
        MediaKind::Podcast => {
            let show = if f.album.trim().is_empty() { artist } else { &f.album };
            root.join(PODCAST_DIR).join(clean_name(show, "Unknown Podcast"))
        }
        MediaKind::Audiobook => root.join(AUDIOBOOK_DIR).join(clean_name(artist, "Unknown Author")).join(clean_name(&f.album, "Unknown Book")),
    }
}

/// 一覧の曲の置き場所の情報 (`discs` は `RockboxLib::disc_totals`)
/// 種類のフォルダへ動かした曲 (後始末をまとめて行うため)
#[derive(Default)]
struct Moves {
    /// 古い曲 ID → 新しい曲 ID
    ids: HashMap<u64, u64>,
    /// 古い場所 → 新しい場所
    paths: HashMap<PathBuf, PathBuf>,
    /// 曲が出た・入ったフォルダ
    dirs: HashSet<PathBuf>,
    /// 作った (あると分かった) 種類のフォルダと、ジャケットの画像があるか。曲ごとに調べ直さない
    made: HashMap<PathBuf, bool>,
    /// 出たフォルダのジャケットの画像 (無ければ None)
    covers: HashMap<PathBuf, Option<PathBuf>>,
    lyrics: DirLyrics,
}

fn fields_of_view(v: &TrackView, discs: &HashMap<(String, String), u32>) -> crate::domain::layout::Fields {
    crate::domain::layout::Fields {
        artist: v.artist.clone(),
        album_artist: v.album_artist.clone(),
        album: v.album.clone(),
        title: v.title.clone(),
        genre: v.genre.clone(),
        year: v.year,
        track: v.track_nr,
        disc: v.disc_nr,
        disc_total: discs.get(&(v.album.clone(), v.album_artist.clone())).copied().unwrap_or(0),
    }
}

/// 転送するファイルの置き場所に使う情報
fn fields_of_info(info: &media::MediaInfo) -> crate::domain::layout::Fields {
    let s = |o: &Option<String>| o.clone().unwrap_or_default();
    crate::domain::layout::Fields {
        artist: s(&info.artist),
        album_artist: s(&info.album_artist),
        album: s(&info.album),
        title: s(&info.title),
        genre: s(&info.genre),
        year: info.year,
        track: info.track_nr,
        disc: info.disc_nr,
        disc_total: info.disc_total,
    }
}

/// 表示用の曲。場所 (rb_path)・種類・データベースにある評価 (0〜10 → 0〜100) と再生回数を入れる
fn make_view(root: &Path, id: u64, t: &PcTrack, path: &Path, stats: &HashMap<String, rbdb::Stats>) -> TrackView {
    let mut v = view_of(id, t);
    v.rb_path = rbdb::rel_path(root, path);
    v.kind = kind_of(root, path);
    let st = stats.get(&rbdb::path_key(&v.rb_path)).copied().unwrap_or_default();
    v.rating = st.rating * 10;
    v.play_count = st.play_count;
    v
}

/// タグの項目 (曲名など) を変える編集か (評価と種類だけなら、ファイルのタグは書き換えない)
fn edits_tags(e: &TagEdit) -> bool {
    e.title.is_some() || e.artist.is_some() || e.album.is_some() || e.album_artist.is_some() || e.genre.is_some()
        || e.composer.is_some() || e.year.is_some() || e.track_nr.is_some() || e.disc_nr.is_some()
        || e.sort_title.is_some() || e.sort_artist.is_some() || e.sort_album.is_some() || e.sort_album_artist.is_some()
        || e.lyrics.is_some()
}

fn not_in_rockbox() -> Error {
    Error::Unsupported(t!("Rockbox モードでは使えません"))
}

impl RockboxLib {
    /// iPod を Rockbox モードで開く。`cache_path` はタグの読み取りのキャッシュの置き場所
    pub fn open(root: &Path, cache_path: Option<PathBuf>) -> Result<Self> {
        Self::open_with_progress(root, cache_path, &|_, _| {})
    }

    /// `open` と同じ。タグを読み直す曲があるときは `progress(読んだ数, 読む数)` を呼ぶ
    /// (初めて Rockbox モードで開くときは全曲を読むので時間がかかる)
    pub fn open_with_progress(root: &Path, cache_path: Option<PathBuf>, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Self> {
        Self::open_inner(root, cache_path.map(CacheAt::File), progress, None, PREVIEW_AFTER)
    }

    /// `open_with_progress` と同じ。タグのキャッシュは `cache_dir` の中の、この iPod 用のファイル (`cache_file`)
    pub fn open_in(root: &Path, cache_dir: Option<PathBuf>, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Self> {
        Self::open_inner(root, cache_dir.map(CacheAt::Dir), progress, None, PREVIEW_AFTER)
    }

    /// `open_in` と同じ。読み終わる前から一覧を出せるよう、前回の一覧か読めた曲を `partial` で知らせる
    pub fn open_streaming(
        root: &Path,
        cache_dir: Option<PathBuf>,
        progress: &(dyn Fn(usize, usize) + Sync),
        partial: Partial,
    ) -> Result<Self> {
        Self::open_inner(root, cache_dir.map(CacheAt::Dir), progress, Some(partial), PREVIEW_AFTER)
    }

    /// `preview_after` は前回の一覧を知らせるまでに待つ時間 (0 ならたどり終わっていても知らせる。テスト用)
    fn open_inner(
        root: &Path,
        cache: Option<CacheAt>,
        progress: &(dyn Fn(usize, usize) + Sync),
        partial: Option<Partial>,
        preview_after: std::time::Duration,
    ) -> Result<Self> {
        if !root.is_dir() {
            return Err(Error::Other(t!("フォルダが見つかりません: {p}", p = root.display())));
        }
        let mut times = device::OpenTimes::default();
        let info = device::probe_timed(root, None, &mut times);
        let cache_path = cache.map(|c| match c {
            CacheAt::File(f) => f,
            CacheAt::Dir(d) => cache_file(&d, &info),
        });
        // iPod OS 側の名前があればそれを使う (デュアルブートの iPod)
        let db_started = Instant::now();
        let name = fs::read(root.join("iPod_Control").join("iTunes").join("iTunesDB"))
            .ok()
            .and_then(|d| crate::ipod::itunesdb::ITunesDb::parse(&d).ok())
            .map(|db| db.master().name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "iPod".into());
        times.db_ms = db_started.elapsed().as_millis() as u64;
        let mut lib = RockboxLib {
            info,
            name,
            tracks: Vec::new(),
            playlists: Vec::new(),
            removed_playlists: Vec::new(),
            covers: Vec::new(),
            dir_covers: HashMap::new(),
            image_dirs: HashSet::new(),
            cache_path,
            db_dirty: false,
            db_error: None,
            dup_index: None,
            layout: crate::domain::layout::DEFAULT.to_string(),
            scan_stats: ScanStats::default(),
            open_times: times,
            video_cache: std::cell::OnceCell::new(),
            photo_cache: std::cell::OnceCell::new(),
        };
        lib.rescan(progress, partial, preview_after);
        lib.load_playlists();
        Ok(lib)
    }

    fn root(&self) -> &Path {
        &self.info.root
    }

    /// iPod の中の音声ファイル (除くフォルダは SKIP_DIRS。更新日時と大きさ付き) と、画像ファイルのあるフォルダ。
    /// 更新日時などはフォルダを読んだときにまとめて得る (`dirlist`。ファイルごとに問い合わせない)
    fn audio_files(root: &Path) -> (Vec<(PathBuf, std::time::SystemTime, u64)>, HashSet<PathBuf>) {
        let mut files = Vec::new();
        let mut image_dirs = HashSet::new();
        let is_audio = |name: &std::ffi::OsStr| media::is_audio_file(Path::new(name));
        let mut dirs = vec![(root.to_path_buf(), 0)];
        while let Some((dir, depth)) = dirs.pop() {
            // 開発版だけ: 遅い iPod を真似る (ZENPOD_DEV_WALK_DELAY_MS に 1 フォルダあたりの待ち時間)
            #[cfg(debug_assertions)]
            if let Some(ms) = std::env::var("ZENPOD_DEV_WALK_DELAY_MS").ok().and_then(|v| v.parse().ok()) {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            let Ok(entries) = crate::dirlist::read(&dir, &is_audio) else { continue };
            for e in entries {
                let name = e.name.to_string_lossy().to_lowercase();
                match e.kind {
                    // 隠しフォルダ (.Trashes / .Spotlight-V100 など) も見ない
                    Kind::Dir if !(name.starts_with('.') || (depth == 0 && SKIP_DIRS.contains(&name.as_str()))) => {
                        dirs.push((dir.join(&e.name), depth + 1));
                    }
                    Kind::File if !name.starts_with("._") => {
                        if is_audio(&e.name) {
                            files.push((dir.join(&e.name), e.modified, e.len));
                        } else if is_image(Path::new(&e.name)) {
                            image_dirs.insert(dir.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        (files, image_dirs)
    }
    /// 前回たどった一覧のうち、タグのキャッシュにある曲 (開くときの仮の一覧。ジャケットは無い)
    fn preview(&self, files: &[(PathBuf, std::time::SystemTime, u64)], stats: &HashMap<String, rbdb::Stats>) -> Vec<TrackView> {
        let Some(cache) = self.cache_path.as_deref() else { return Vec::new() };
        let root = self.root();
        files
            .iter()
            .zip(pcfolder::cached_device_tracks(files, cache, root))
            .filter_map(|((path, m, _), t)| {
                let mut view = make_view(root, id_of(&rel_key(root, path)), &t?, path, stats);
                view.date_added = m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                Some(view)
            })
            .collect()
    }

    /// ファイルを読み直して曲の一覧を作る。`partial` があれば、たどり終わるまで `preview_after` より長くかかるとき
    /// 前回の一覧を先に知らせ、無ければ (初めて開く) 読めた曲を少しずつ知らせる
    fn rescan(&mut self, progress: &(dyn Fn(usize, usize) + Sync), partial: Option<Partial>, preview_after: std::time::Duration) {
        let started = Instant::now();
        let root = self.root().to_path_buf();
        let listing = self.cache_path.as_deref().map(listing_file);
        let mut previewed = false;
        let mut preview_ms = None;
        let mut stats = None;
        let (files, image_dirs) = match (partial, &listing) {
            (Some(partial), Some(listing)) => {
                let this = &*self;
                std::thread::scope(|sc| {
                    let walk = sc.spawn(|| Self::audio_files(&root));
                    // たどっている間に、前回の一覧と評価・再生回数 (本体のデータベース) から仮の一覧を作る
                    let st = rbdb::stats(&root);
                    let views = load_listing(listing, &root).map(|f| this.preview(&f, &st)).unwrap_or_default();
                    preview_ms = Some((started.elapsed().as_millis(), views.len()));
                    stats = Some(st);
                    while !walk.is_finished() && started.elapsed() < preview_after {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    if (preview_after.is_zero() || !walk.is_finished()) && !views.is_empty() {
                        partial(&this.info, &this.name, views, true);
                        previewed = true;
                    }
                    walk.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
                })
            }
            _ => Self::audio_files(&root),
        };
        let walked = Instant::now();
        if let Some(listing) = &listing {
            save_listing(listing, &root, &files);
        }
        // 曲の追加日はファイルの更新日時 (転送した日時) にする (タグのキャッシュの形は変えない)
        let mtimes: HashMap<PathBuf, i64> = files
            .iter()
            .map(|(p, m, _)| (p.clone(), m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)))
            .collect();
        self.image_dirs = image_dirs;
        let count = files.len();
        let stats = stats.unwrap_or_else(|| rbdb::stats(&root));
        // 読み直す数は進み具合の分母 (読み直す曲が無ければ呼ばれない)
        let reread = AtomicUsize::new(0);
        // 読めた曲はためておき、前に知らせてから PARTIAL_EVERY たったら知らせる (すぐ読み終わるときは知らせない)
        let pending = Mutex::new((Vec::new(), started));
        // キャッシュに無い曲は、Rockbox のデータベースにあって更新日時が同じなら、そこから埋める
        // (HDD の iPod では 1 曲ずつタグを読むと 1000 曲で 30 秒以上かかる。データベースは数個のファイルを続けて読むだけ)
        let seed = |asked: &[(PathBuf, std::time::SystemTime)]| -> Vec<Option<PcTrack>> {
            if asked.len() < SEED_MIN {
                return Vec::new();
            }
            let db = rbdb::seed_tracks(&root);
            asked
                .iter()
                .map(|(p, m)| {
                    let (mtime, t) = db.get(&rel_key(&root, p))?;
                    (*mtime == rbdb::device_mtime(*m)).then(|| t.clone())
                })
                .collect()
        };
        let found = pcfolder::scan_device_files_streaming(
            files,
            self.cache_path.as_deref(),
            &root,
            &seed,
            |done, total| {
                reread.store(total, Ordering::Relaxed);
                progress(done, total)
            },
            |t| {
                // 前回の一覧を出したあとは知らせない (同じ曲が二重になる。変わった曲は読み終わったときの一覧で入れ替わる)
                let Some(partial) = partial.filter(|_| !previewed) else { return };
                let path = Path::new(&t.path);
                let view = make_view(&root, id_of(&rel_key(&root, path)), t, path, &stats);
                let batch = {
                    let mut p = pending.lock().unwrap();
                    p.0.push(view);
                    if p.1.elapsed() < PARTIAL_EVERY {
                        return;
                    }
                    p.1 = Instant::now();
                    std::mem::take(&mut p.0)
                };
                partial(&self.info, &self.name, batch, false);
            },
        );
        self.scan_stats = ScanStats {
            walk_ms: (walked - started).as_millis() as u64,
            read_ms: walked.elapsed().as_millis() as u64,
            files: count,
            reread: reread.into_inner(),
            from_db: found.iter().filter(|t| t.from_db).count(),
        };
        // 開く処理の段階ごとの時間 (固まったとき・遅いときにどこかを見る)
        log::info!(
            "rockbox scan: {} files, walk {} ms, read {} ms, reread {}, from db {}, last list {}",
            count,
            self.scan_stats.walk_ms,
            self.scan_stats.read_ms,
            self.scan_stats.reread,
            self.scan_stats.from_db,
            match preview_ms {
                Some((ms, n)) => format!("{n} tracks ready at {ms} ms, {}", if previewed { "shown" } else { "not needed" }),
                None => "none".into(),
            }
        );
        self.tracks = found
            .iter()
            .map(|t| {
                let path = PathBuf::from(&t.path);
                let id = id_of(&rel_key(&root, &path));
                let mut view = make_view(&root, id, t, &path, &stats);
                view.date_added = mtimes.get(&path).copied().unwrap_or(0);
                RbTrack { id, view, path, has_art: t.has_picture, from_db: t.from_db }
            })
            .collect();
        self.assign_covers(None);
    }

    /// ジャケットの ID をフォルダごとに振り直す (同じフォルダの曲は同じ画像)。
    /// 画像ファイル (cover.jpg など) のあるフォルダか、先頭の曲に画像が埋め込まれているフォルダ。
    /// `dirs` を渡すとそのフォルダだけやり直す (曲を 1 曲足すたびに全部のフォルダを調べ直さないように)。
    /// ファイルは開かない (埋め込みの有無はタグを読んだときに覚えた has_art、画像ファイルは image_dirs)
    fn assign_covers(&mut self, dirs: Option<&HashSet<PathBuf>>) {
        if dirs.is_none() {
            self.covers.clear();
            self.dir_covers.clear();
        }
        // フォルダごとの曲 (曲の順のまま)
        let mut by_dir: Vec<(PathBuf, Vec<usize>)> = Vec::new();
        let mut pos: HashMap<PathBuf, usize> = HashMap::new();
        for (i, t) in self.tracks.iter().enumerate() {
            let dir = t.path.parent().map(Path::to_path_buf).unwrap_or_default();
            if dirs.is_some_and(|d| !d.contains(&dir)) {
                continue;
            }
            match pos.get(&dir) {
                Some(&k) => by_dir[k].1.push(i),
                None => {
                    pos.insert(dir.clone(), by_dir.len());
                    by_dir.push((dir, vec![i]));
                }
            }
        }
        // 曲が 1 つも無くなったフォルダ
        if let Some(d) = dirs {
            for dir in d.iter().filter(|d| !pos.contains_key(*d)) {
                self.dir_covers.remove(dir);
            }
        }
        for (dir, idx) in by_dir {
            let first = &self.tracks[idx[0]];
            let id = if self.image_dirs.contains(&dir) || first.has_art {
                let src = first.path.clone();
                Some(match self.dir_covers.get(&dir) {
                    Some(&id) => {
                        self.covers[id as usize - 1] = src;
                        id
                    }
                    None => {
                        self.covers.push(src);
                        let id = self.covers.len() as u32;
                        self.dir_covers.insert(dir.clone(), id);
                        id
                    }
                })
            } else {
                self.dir_covers.remove(&dir);
                None
            };
            for i in idx {
                self.tracks[i].view.artwork_id = id.unwrap_or(0);
            }
        }
    }

    /// フォルダに画像ファイルがあるか調べ直す (ジャケットを書いた・消したとき)
    fn refresh_image_dir(&mut self, dir: &Path) {
        let has = fs::read_dir(dir).is_ok_and(|rd| rd.flatten().any(|e| is_image(&e.path())));
        if has {
            self.image_dirs.insert(dir.to_path_buf());
        } else {
            self.image_dirs.remove(dir);
        }
    }

    fn playlist_dir(&self) -> PathBuf {
        self.root().join(PLAYLIST_DIR)
    }

    /// Playlists の .m3u8 / .m3u を読む。曲のパスは "/Music/..." (ルートから) か、プレイリストのフォルダからの相対
    fn load_playlists(&mut self) {
        let root = self.root().to_path_buf();
        let by_key: HashMap<String, u64> = self.tracks.iter().map(|t| (rel_key(&root, &t.path), t.id)).collect();
        let Ok(rd) = fs::read_dir(self.playlist_dir()) else { return };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                p.is_file() && (ext == "m3u8" || ext == "m3u")
            })
            .collect();
        files.sort();
        for path in files {
            let Ok(bytes) = fs::read(&path) else { continue };
            let text = decode_playlist(&path, &bytes);
            let dir = path.parent().unwrap_or(&root).to_path_buf();
            let mut items = Vec::new();
            let mut other = Vec::new();
            for l in text.lines().map(|l| l.trim().trim_start_matches('\u{feff}')).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                let p = l.replace('\\', "/");
                // 本体と同じく、"E:/Music/..." のドライブ文字は外して iPod のルートからとみなす ("E:Music/..." は相対)
                let p = match p.as_bytes() {
                    [d, b':', ..] if d.is_ascii_alphabetic() => &p[2..],
                    _ => strip_volume(&p),
                };
                let file = match p.strip_prefix('/') {
                    Some(rest) => root.join(rest),
                    None => dir.join(p),
                };
                match by_key.get(&rel_key(&root, &normalize(&file))) {
                    Some(&id) => items.push(id),
                    None => other.push((items.len(), l.to_string())),
                }
            }
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let smart = smart_of(&text);
            self.playlists.push(RbPlaylist { id: id_of(&rel_key(&root, &path)), name, path, items, other, dirty: false, smart });
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.playlists.iter().any(|p| p.dirty) || !self.removed_playlists.is_empty()
    }

    /// 曲を変えたので Rockbox のデータベースを作り直したいか (1 回だけ true を返す)。
    /// 作り直しは時間がかかるので、保存のたびではなく、呼び出し側が裏でまとめて行う
    pub fn take_db_request(&mut self) -> bool {
        std::mem::take(&mut self.db_dirty) && rbdb::supported(self.root())
    }

    pub fn cache_path(&self) -> Option<&Path> {
        self.cache_path.as_deref()
    }

    /// 裏で作り直した結果 (失敗の理由は画面に出す)
    pub fn set_db_error(&mut self, e: Option<String>) {
        self.db_error = e;
    }

    /// Rockbox のデータベースを PC で作り直して iPod に書く (本体で何時間も待たなくて済む)
    pub fn rebuild_db(&mut self) -> Result<rbdb::Built> {
        self.db_dirty = false;
        let r = rbdb::rebuild(self.root(), self.cache_path.as_deref());
        self.db_error = r.as_ref().err().map(|e| e.to_string());
        r
    }

    /// Rockbox のデータベースの様子 (画面用)
    /// Rockbox のデータベースから埋めた曲のファイル (裏で読み直す。`apply_reread`)
    pub fn db_filled_files(&self) -> Vec<PathBuf> {
        self.tracks.iter().filter(|t| t.from_db).map(|t| t.path.clone()).collect()
    }

    /// データベースから埋めた曲を、ファイルから読み直した情報に替える (`pcfolder::reread_device_files` の結果)。
    /// その間に動かした・編集した曲 (場所が違う・印が消えた) はそのまま。戻り値は替えた曲の数
    pub fn apply_reread(&mut self, read: &[PcTrack]) -> usize {
        let by_path: HashMap<&Path, &PcTrack> = read.iter().map(|t| (Path::new(&t.path), t)).collect();
        let mut dirs = HashSet::new();
        let mut n = 0;
        for rt in self.tracks.iter_mut().filter(|t| t.from_db) {
            let Some(t) = by_path.get(rt.path.as_path()) else { continue };
            let mut view = view_of(rt.id, t);
            // 場所・種類・評価・再生回数・追加日・ジャケットは今のまま (ファイルのタグとは別に決まる)
            view.rb_path = std::mem::take(&mut rt.view.rb_path);
            view.kind = rt.view.kind;
            view.rating = rt.view.rating;
            view.play_count = rt.view.play_count;
            view.date_added = rt.view.date_added;
            view.artwork_id = rt.view.artwork_id;
            rt.view = view;
            rt.from_db = false;
            if rt.has_art != t.has_picture {
                rt.has_art = t.has_picture;
                if let Some(d) = rt.path.parent() {
                    dirs.insert(d.to_path_buf());
                }
            }
            n += 1;
        }
        if !dirs.is_empty() {
            self.assign_covers(Some(&dirs));
        }
        if n > 0 {
            self.dup_index = None;
        }
        n
    }

    pub fn open_times(&self) -> device::OpenTimes {
        self.open_times
    }

    pub fn scan_stats(&self) -> ScanStats {
        self.scan_stats
    }

    pub fn status(&self) -> Status {
        let mut st = status(self.root());
        st.db_error = self.db_error.clone();
        st
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 曲を転送するときの置き場所の形
    pub fn layout(&self) -> &str {
        &self.layout
    }

    pub fn set_layout(&mut self, template: &str) -> Result<()> {
        crate::domain::layout::validate(template)?;
        self.layout = template.trim().to_string();
        Ok(())
    }

    /// 今ある曲の並びから置き場所の形を推測する (ポッドキャスト・オーディオブックは除く)
    pub fn guess_layout(&self) -> Option<crate::domain::layout::Guess> {
        let root = self.root();
        let discs = self.disc_totals();
        let tracks: Vec<(Vec<String>, crate::domain::layout::Fields)> = self
            .tracks
            .iter()
            .filter(|t| t.view.kind == MediaKind::Music)
            .filter_map(|t| {
                let rel = t.path.strip_prefix(root).ok()?.with_extension("");
                let parts = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                Some((parts, fields_of_view(&t.view, &discs)))
            })
            .collect();
        crate::domain::layout::guess(&tracks, clean_name)
    }

    /// 形の見本: 今ある曲 (無ければ架空の曲) を当てはめた場所 ("Music/アーティスト/アルバム/01 曲名.flac")
    pub fn preview_layout(&self, template: &str) -> Result<String> {
        crate::domain::layout::validate(template)?;
        let (f, ext) = match self.tracks.iter().position(|t| t.view.kind == MediaKind::Music && !t.view.album.is_empty()) {
            Some(i) => (self.view_fields(i), self.tracks[i].path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()),
            None => (
                crate::domain::layout::Fields {
                    artist: "Artist".into(),
                    album_artist: "Artist".into(),
                    album: "Album".into(),
                    title: "Song".into(),
                    genre: "Pop".into(),
                    year: 2024,
                    track: 1,
                    disc: 1,
                    disc_total: 1,
                },
                "flac".to_string(),
            ),
        };
        Ok(format!("{}.{ext}", crate::domain::layout::render(template, &f, clean_name).join("/")))
    }

    /// 一覧の曲の置き場所の情報。何枚組かは同じアルバムの曲だけ数える (全曲の表を作ると 2 万曲で 1 回 2.5ms)
    fn view_fields(&self, i: usize) -> crate::domain::layout::Fields {
        let v = &self.tracks[i].view;
        let total = self
            .tracks
            .iter()
            .filter(|t| t.view.album == v.album && t.view.album_artist == v.album_artist)
            .map(|t| t.view.disc_nr)
            .max()
            .unwrap_or(0);
        let discs = HashMap::from([((v.album.clone(), v.album_artist.clone()), total)]);
        fields_of_view(v, &discs)
    }

    /// アルバム (アルバム名, アルバムアーティスト) → 何枚組か (曲の一番大きいディスク番号)
    fn disc_totals(&self) -> HashMap<(String, String), u32> {
        let mut m: HashMap<(String, String), u32> = HashMap::new();
        for t in &self.tracks {
            let e = m.entry((t.view.album.clone(), t.view.album_artist.clone())).or_default();
            *e = (*e).max(t.view.disc_nr);
        }
        m
    }

    /// プレイリストを書き出す (変えたものだけ)
    pub fn save(&mut self) -> Result<()> {
        // 曲の追加・削除・編集をスマートプレイリストに反映する
        self.refresh_smart_playlists();
        let root = self.root().to_path_buf();
        let paths: HashMap<u64, String> = self
            .tracks
            .iter()
            .map(|t| (t.id, rbdb::rel_path(&root, &t.path)))
            .collect();
        for p in self.removed_playlists.drain(..) {
            let _ = fs::remove_file(p);
        }
        for pl in self.playlists.iter_mut().filter(|p| p.dirty) {
            if let Some(dir) = pl.path.parent() {
                fs::create_dir_all(dir)?;
            }
            let mut text = String::from("#EXTM3U\n");
            if let Some((prefs, rules)) = &pl.smart {
                text.push_str(&format!("{SMART_INFO_LINE}{}\n{SMART_RULES_LINE}{}\n", b64(&prefs.to_xml_info(None)), b64(&rules.to_bytes())));
                // 中身は条件で決まるので、一覧に無い曲の行は残さない
                pl.other.clear();
            }
            // 一覧に無い曲の行は、前にあった曲の数の位置に戻す (曲を外して数が減ったら後ろに)
            let mut other = pl.other.iter().peekable();
            for (k, id) in pl.items.iter().enumerate() {
                while let Some((_, line)) = other.next_if(|(at, _)| *at <= k) {
                    text.push_str(line);
                    text.push('\n');
                }
                if let Some(p) = paths.get(id) {
                    text.push_str(p);
                    text.push('\n');
                }
            }
            for (_, line) in other {
                text.push_str(line);
                text.push('\n');
            }
            // 本体は .m3u を BOM が無ければ UTF-8 ではなく Latin-1 として読むので、BOM を付ける (本体が書くときと同じ)
            if !is_m3u8_name(&pl.path) {
                text.insert(0, '\u{feff}');
            }
            let tmp = pl.path.with_extension("m3u8.tmp");
            fs::write(&tmp, text)?;
            fs::rename(&tmp, &pl.path)?;
            pl.dirty = false;
        }
        self.info.free_bytes = fs4::available_space(self.root()).unwrap_or(self.info.free_bytes);
        Ok(())
    }

    pub fn tracks(&self) -> Vec<TrackView> {
        self.tracks.iter().map(|t| t.view.clone()).collect()
    }

    pub fn playlists(&self) -> Vec<PlaylistView> {
        // 画面は iPod OS と同じく、先頭に全曲の「ライブラリ」を置く前提
        let master = PlaylistView {
            id: "0".into(),
            name: self.name.clone(),
            track_ids: self.tracks.iter().map(|t| t.id.to_string()).collect(),
            master: true,
            smart: false,
            podcast: false,
        };
        std::iter::once(master)
            .chain(self.playlists.iter().map(|p| PlaylistView {
                id: p.id.to_string(),
                name: p.name.clone(),
                track_ids: p.items.iter().map(|i| i.to_string()).collect(),
                master: false,
                smart: p.smart.is_some(),
                podcast: false,
            }))
            .collect()
    }

    fn track(&self, id: u64) -> Option<&RbTrack> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_file(&self, id: u64) -> Option<PathBuf> {
        self.track(id).map(|t| t.path.clone())
    }

    /// 1 曲転送する (変換はしない。Rockbox が再生できない形式だけ変換する)
    pub fn add_file(&mut self, src: &Path, ffmpeg: Option<&Path>, opts: &AddOptions) -> Result<AddOutcome> {
        self.add_file_preconverted(src, ffmpeg, opts, None)
    }

    /// 同じ曲 (Rockbox にあれば (ID, 曲名))。曲名・アーティスト・アルバムが同じで、長さの差が 2 秒未満、
    /// 番号がどちらにもあれば同じ (iPod OS と同じ)。重複を調べない設定なら None
    fn duplicate_for(&mut self, info: &media::MediaInfo, opts: &AddOptions) -> Option<(u64, String)> {
        if !(opts.skip_duplicates || opts.replace_duplicates) {
            return None;
        }
        let key = dup_key(info.title.as_deref(), info.artist.as_deref(), info.album.as_deref());
        let tracks = &self.tracks;
        let index = self.dup_index.get_or_insert_with(|| {
            let mut m: DupIndex = HashMap::new();
            for t in tracks {
                let k = dup_key(Some(&t.view.title), Some(&t.view.artist), Some(&t.view.album));
                m.entry(k).or_default().push((t.id, t.view.duration_ms, (t.view.disc_nr, t.view.track_nr), t.view.title.clone()));
            }
            m
        });
        index
            .get(&key)
            .and_then(|v| {
                v.iter()
                    .find(|(_, d, nr, _)| d.abs_diff(info.duration_ms) < 2000 && same_number(*nr, (info.disc_nr, info.track_nr)))
            })
            .map(|(id, _, _, title)| (*id, title.clone()))
    }

    /// 変換先。Rockbox はたいていの形式を再生できるので、変換するのは「変換して転送」の曲と、再生できない形式だけ。
    /// 「変換して転送」でも、Rockbox が再生できる非可逆の形式 (Ogg など) は MP3・AAC と同じく、ビットレートが高いときだけ
    fn conversion_target(src: &Path, info: &media::MediaInfo, opts: &AddOptions) -> Option<crate::domain::audio::ConvertTarget> {
        let ext = src.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let needs = NEEDS_CONVERT.contains(&ext.as_str());
        let forced = opts.convert_all_to.filter(|t| match media::classify(src) {
            Ok(Format::ConvertLossy) if !needs => t.should_convert(Format::Mp3, info.bitrate_kbps),
            Ok(f) => t.should_convert(f, info.bitrate_kbps),
            Err(_) => false,
        });
        forced.or(needs.then_some(opts.lossy_to))
    }

    /// 転送の前に変換しておけるか (`media::preconvert`)。重複で送らない曲・変換の要らない曲は None
    pub fn conversion_plan(&mut self, src: &Path, opts: &AddOptions) -> Result<Option<preconvert::ConversionPlan>> {
        let info = media::read_info(src)?;
        if self.duplicate_for(&info, opts).is_some() && !opts.replace_duplicates {
            return Ok(None);
        }
        Ok(Self::conversion_target(src, &info, opts)
            .map(|target| preconvert::ConversionPlan { target, sample_rate: info.sample_rate, afconvert: false }))
    }

    /// 1 曲転送する。`pre` に同じ変換をしておいた一時ファイルがあれば、変換せずにそれを写す
    pub fn add_file_preconverted(
        &mut self,
        src: &Path,
        ffmpeg: Option<&Path>,
        opts: &AddOptions,
        pre: Option<&preconvert::Preconverted>,
    ) -> Result<AddOutcome> {
        let info = media::read_info(src)?;
        let dup = self.duplicate_for(&info, opts);
        if let Some((id, title)) = &dup
            && !opts.replace_duplicates
        {
            return Ok(AddOutcome::Duplicate { title: title.clone(), id: id.to_string() });
        }

        let ext = src.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let convert_to = Self::conversion_target(src, &info, opts);
        let pre = pre.filter(|p| {
            convert_to.is_some_and(|target| {
                p.matches(&preconvert::ConversionPlan { target, sample_rate: info.sample_rate, afconvert: false })
            })
        });
        let size = fs::metadata(src)?.len();
        if size > u32::MAX as u64 {
            return Err(Error::Unsupported(t!("4GB を超えるファイルは転送できません")));
        }
        let free = fs4::available_space(self.root()).unwrap_or(u64::MAX);
        if size + FREE_MARGIN > free {
            return Err(Error::NoSpace);
        }

        // 置き換えるときは元の曲の種類に合わせる (手で変えたかもしれないので)
        let kind = match dup.as_ref().and_then(|(id, _)| self.track(*id)) {
            Some(old) => old.view.kind,
            None => media::detect_kind(src, &info),
        };
        let fields = fields_of_info(&info);
        let dir = kind_dir(self.root(), &self.layout, kind, &fields);
        fs::create_dir_all(&dir)?;
        let out_ext = convert_to.map(|t| t.ext().to_string()).unwrap_or_else(|| ext.clone());
        // 曲は転送先の形のファイル名、ポッドキャスト・オーディオブックは "01 曲名"
        let stem = match kind {
            MediaKind::Music => crate::domain::layout::render(&self.layout, &fields, clean_name).pop().unwrap_or_default(),
            _ => file_stem(&info, src),
        };
        // 置き換える曲が、新しい曲を置きたい名前にそのままあるときは、同じ名前で入れ替える
        // (先に一時の名前に書き、古い曲を消してから名前を変える。そうしないと「曲名 (2)」になる。配布リポジトリの Issue #30)
        let replaced_file = dup.as_ref().and_then(|(id, _)| self.track_file(*id));
        let wanted = dir.join(format!("{stem}.{out_ext}"));
        let same_name = replaced_file.as_ref().is_some_and(|p| same_file_name(p, &wanted));
        let (dest, staging) = if same_name {
            (wanted, Some(unique_path(&dir, &format!(".zenpod-new {stem}"), &out_ext)))
        } else {
            (unique_path(&dir, &stem, &out_ext), None)
        };
        // 書く先 (入れ替えるときは一時の名前)
        let write_to = staging.clone().unwrap_or_else(|| dest.clone());
        let copied = match (pre, convert_to) {
            (Some(pre), Some(_)) => fs::copy(pre.path(), &write_to).map(|_| ()).map_err(Error::from),
            (None, Some(target)) => {
                let ff = ffmpeg.ok_or_else(|| {
                    Error::Unsupported(t!(
                        "{ext} の変換には ffmpeg が必要です (設定 → 変換 から自動で入れられます)",
                        ext = ext
                    ))
                })?;
                convert::convert(ff, src, &write_to, target, info.sample_rate)
            }
            (_, None) => {
                // 埋め込みのジャケットを縮めるときは、PC で縮めた一時ファイルを送る
                let shrunk = opts.shrink_artwork.and_then(|max| media::shrunk_copy(src, max));
                let r = fs::copy(shrunk.as_deref().unwrap_or(src), &write_to).map(|_| ()).map_err(Error::from);
                if let Some(t) = shrunk {
                    let _ = fs::remove_file(t);
                }
                r
            }
        };
        if let Err(e) = copied {
            let _ = fs::remove_file(&write_to);
            return Err(e);
        }
        // 変換したファイルは iPod の上にあるので、そこで縮める (ジャケットの失敗で転送は止めない)
        if let (Some(max), Some(_)) = (opts.shrink_artwork, convert_to)
            && let Err(e) = media::shrink_embedded_cover(&write_to, max)
        {
            log::warn!("shrink artwork: {e}");
        }
        // 歌詞: 曲の名前を変えて送るので、同じ名前の .lrc も新しい名前で写す
        for l in lyrics_files(src) {
            let _ = fs::copy(&l, lyrics_dest(&l, &dest));
        }
        // ジャケット: フォルダにまだ無ければ cover.jpg を書く
        if opts.artwork
            && !has_cover_file(&dir)
            && let Some(jpg) = cover::find_cover(src).and_then(|b| cover_jpeg(&b))
            && fs::write(dir.join("cover.jpg"), jpg).is_ok()
        {
            self.image_dirs.insert(dir.clone());
        }
        // ジャケットを振り直すのは、この曲のフォルダと、置き換えた曲のフォルダだけ
        let mut changed: HashSet<PathBuf> = HashSet::from([dir.clone()]);

        let replaced = dup.map(|(id, _)| id);
        if let Some(p) = replaced.and_then(|old| self.track_file(old)) {
            // 新しい曲が同じ名前の歌詞を写したかもしれないので、古い曲の歌詞は場所が違うときだけ消す
            for l in lyrics_files(&p) {
                if lyrics_dest(&l, &dest) != l {
                    let _ = fs::remove_file(&l);
                }
            }
            let _ = fs::remove_file(&p);
            if let Some(tmp) = &staging
                && let Err(e) = fs::rename(tmp, &dest)
            {
                let _ = fs::remove_file(tmp);
                return Err(e.into());
            }
            cleanup_dirs(self.root(), &self.layout, p.parent());
            if let Some(d) = p.parent() {
                changed.insert(d.to_path_buf());
            }
        }
        let size = fs::metadata(&dest)?.len();
        // タグは PC の元のファイルから読む (iPod の HDD から読み直さない)。変換したときは変換後のファイルから
        let mut t = if convert_to.is_none() { pcfolder::read_track(src, size) } else { pcfolder::read_track(&dest, size) };
        t.path = dest.to_string_lossy().into_owned();
        let id = id_of(&rel_key(self.root(), &dest));
        let mut view = make_view(self.root(), id, &t, &dest, &HashMap::new());
        view.date_added = crate::ipod::itunesdb::now_unix();
        // プレイリストの中の古い曲は新しい曲に差し替える
        if let Some(old) = replaced {
            self.tracks.retain(|x| x.id != old);
            for pl in &mut self.playlists {
                if pl.items.contains(&old) {
                    pl.items.iter_mut().filter(|i| **i == old).for_each(|i| *i = id);
                    pl.dirty = true;
                }
            }
        }
        if let Some(index) = &mut self.dup_index {
            if let Some(old) = replaced {
                for v in index.values_mut() {
                    v.retain(|(i, _, _, _)| *i != old);
                }
            }
            let k = dup_key(Some(&view.title), Some(&view.artist), Some(&view.album));
            index.entry(k).or_default().push((id, view.duration_ms, (view.disc_nr, view.track_nr), view.title.clone()));
        }
        self.tracks.push(RbTrack { id, path: dest, view, has_art: t.has_picture, from_db: false });
        self.assign_covers(Some(&changed));
        self.db_dirty = true;
        let track = Box::new(self.track(id).map(|t| t.view.clone()).unwrap_or_default());
        let converted = convert_to.is_some();
        Ok(if replaced.is_some() {
            AddOutcome::Replaced { track, converted }
        } else {
            AddOutcome::Added { track, converted }
        })
    }

    /// 曲のファイルを消す。空になったアルバム・アーティストのフォルダも消す
    pub fn remove_tracks(&mut self, ids: &[u64]) -> Result<usize> {
        let set: HashSet<u64> = ids.iter().copied().collect();
        let mut n = 0;
        let mut dirs = Vec::new();
        let mut lyrics = DirLyrics::default();
        for t in self.tracks.iter().filter(|t| set.contains(&t.id)) {
            for l in lyrics.of(&t.path) {
                let _ = fs::remove_file(&l);
            }
            fs::remove_file(&t.path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })?;
            dirs.push(t.path.parent().map(Path::to_path_buf));
            n += 1;
        }
        self.tracks.retain(|t| !set.contains(&t.id));
        self.db_dirty |= n > 0;
        self.dup_index = None;
        for pl in &mut self.playlists {
            let before = pl.items.len();
            pl.items.retain(|i| !set.contains(i));
            pl.dirty |= pl.items.len() != before;
        }
        // 空になったフォルダの片付けは、同じフォルダを 1 回だけ見る
        let changed: HashSet<PathBuf> = dirs.into_iter().flatten().collect();
        for d in &changed {
            cleanup_dirs(self.root(), &self.layout, Some(d));
        }
        self.assign_covers(Some(&changed));
        self.info.free_bytes = fs4::available_space(self.root()).unwrap_or(self.info.free_bytes);
        Ok(n)
    }

    /// 曲のファイル名を変える (拡張子とフォルダはそのまま)。`renames` は (曲 ID, 新しい名前 (拡張子を除く))。
    /// 曲 ID は場所から作るので変わる。プレイリストの中の曲も新しい ID に差し替えて書き直す。戻り値は変えた曲の数
    pub fn rename_files(&mut self, renames: &[(u64, String)]) -> Result<usize> {
        let pos: HashMap<u64, usize> = self.tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        let mut pairs = Vec::with_capacity(renames.len());
        let mut idx = Vec::with_capacity(renames.len());
        let mut lyrics = Vec::new();
        let mut dir_lyrics = DirLyrics::default();
        for (id, name) in renames {
            let &i = pos.get(id).ok_or_else(|| Error::Other(t!("曲が見つかりません")))?;
            pairs.push((self.tracks[i].path.clone(), name.clone()));
            lyrics.extend(dir_lyrics.of(&self.tracks[i].path).into_iter().map(|l| (l, name.clone())));
            idx.push(i);
        }
        // 歌詞のファイルも同じ名前に変える (曲と一緒に変えて、名前の重なりもまとめて調べる)
        let n = pairs.len();
        pairs.extend(lyrics);
        let mut dsts = pcfolder::rename_many(&pairs)?;
        dsts.truncate(n);
        let root = self.root().to_path_buf();
        let mut new_ids: HashMap<u64, u64> = HashMap::new();
        for (&i, dst) in idx.iter().zip(&dsts) {
            let t = &mut self.tracks[i];
            let old_path = std::mem::replace(&mut t.path, dst.clone());
            let new_id = id_of(&rel_key(&root, dst));
            new_ids.insert(t.id, new_id);
            t.id = new_id;
            t.view.id = new_id.to_string();
            t.view.rb_path = rbdb::rel_path(&root, dst);
            // ジャケットを読む曲にしていたら、新しい場所にする
            for c in self.covers.iter_mut().filter(|c| **c == old_path) {
                *c = dst.clone();
            }
        }
        for pl in &mut self.playlists {
            for item in pl.items.iter_mut() {
                if let Some(&n) = new_ids.get(item) {
                    *item = n;
                    pl.dirty = true;
                }
            }
        }
        self.db_dirty = true;
        self.dup_index = None;
        Ok(dsts.len())
    }

    /// ファイルのタグを書き換えて読み直し、種類が変わったらファイルを種類のフォルダへ動かす。
    /// 評価は `ratings` に集める (データベースにまとめて書く)。戻り値は変えたか
    fn edit_one(&mut self, id: u64, e: &TagEdit, ratings: &mut HashMap<String, u8>, moves: &mut Moves) -> Result<bool> {
        let Some(mut i) = self.tracks.iter().position(|t| t.id == id) else { return Ok(false) };
        let mut changed = false;
        if edits_tags(e) {
            let path = self.tracks[i].path.clone();
            media::write_tags(&path, e)?;
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let (artwork_id, rating) = (self.tracks[i].view.artwork_id, self.tracks[i].view.rating);
            let read = pcfolder::read_track(&path, size);
            self.tracks[i].view = make_view(self.root(), id, &read, &path, &HashMap::new());
            self.tracks[i].view.artwork_id = artwork_id;
            self.tracks[i].view.rating = rating;
            // データベースから埋めた曲は、埋め込みのジャケットの有無がここで分かる
            if self.tracks[i].from_db {
                self.tracks[i].from_db = false;
                if self.tracks[i].has_art != read.has_picture {
                    self.tracks[i].has_art = read.has_picture;
                    let dirs: HashSet<PathBuf> = path.parent().map(Path::to_path_buf).into_iter().collect();
                    self.assign_covers(Some(&dirs));
                }
            }
            self.db_dirty = true;
            self.dup_index = None;
            changed = true;
        }
        if let Some(kind) = e.kind
            && kind != self.tracks[i].view.kind
        {
            i = self.move_to_kind(i, kind, moves)?;
            changed = true;
        }
        if let Some(r) = e.rating.map(|r| r.min(100) / 20 * 2)
            && self.tracks[i].view.rating != r * 10
        {
            ratings.insert(rbdb::path_key(&self.tracks[i].view.rb_path), r);
            self.tracks[i].view.rating = r * 10;
            changed = true;
        }
        Ok(changed)
    }

    /// 曲のファイルを種類のフォルダへ動かす (ジャケットの画像ファイルも一緒に写す)。
    /// 曲 ID は場所から作るので変わる。プレイリストの中の曲の差し替え・ジャケットの振り直し・空のフォルダの片付けは
    /// 全曲を見るので、`moves` に貯めて最後に `finish_moves` で 1 回だけ行う (1 曲ごとだと 1000 曲で 5 秒かかっていた)。
    /// 戻り値は動かした曲の位置
    fn move_to_kind(&mut self, i: usize, kind: MediaKind, moves: &mut Moves) -> Result<usize> {
        let root = self.root().to_path_buf();
        let old_path = self.tracks[i].path.clone();
        let dir = kind_dir(&root, &self.layout, kind, &self.view_fields(i));
        let old_dir = old_path.parent().map(Path::to_path_buf).unwrap_or_else(|| root.clone());
        // 行き先のフォルダを作るのと、ジャケットがあるかを調べるのは、フォルダごとに 1 回
        let has_cover = match moves.made.get(&dir) {
            Some(&c) => c,
            None => {
                fs::create_dir_all(&dir)?;
                let c = has_cover_file(&dir);
                moves.made.insert(dir.clone(), c);
                c
            }
        };
        let stem = old_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = old_path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let dst = unique_path(&dir, &stem, &ext);
        fs::rename(&old_path, &dst)?;
        for l in moves.lyrics.of(&old_path) {
            let _ = fs::rename(&l, lyrics_dest(&l, &dst));
        }
        if !has_cover
            && let Some(cover) = moves
                .covers
                .entry(old_dir.clone())
                .or_insert_with(|| ["cover.jpg", "folder.jpg", "cover.jpeg", "cover.png", "folder.png"].iter().map(|n| old_dir.join(n)).find(|p| p.is_file()))
                .clone()
            && let Some(name) = cover.file_name()
            && fs::copy(&cover, dir.join(name)).is_ok()
        {
            self.image_dirs.insert(dir.clone());
            moves.made.insert(dir.clone(), true);
        }
        let new_id = id_of(&rel_key(&root, &dst));
        let old_id = self.tracks[i].id;
        let t = &mut self.tracks[i];
        t.path = dst.clone();
        t.id = new_id;
        t.view.id = new_id.to_string();
        t.view.rb_path = rbdb::rel_path(&root, &dst);
        t.view.kind = kind;
        moves.ids.insert(old_id, new_id);
        moves.paths.insert(old_path, dst);
        moves.dirs.insert(old_dir);
        moves.dirs.insert(dir);
        self.db_dirty = true;
        self.dup_index = None;
        Ok(i)
    }

    /// `move_to_kind` で動かした曲の後始末をまとめて行う
    fn finish_moves(&mut self, moves: Moves) {
        if moves.ids.is_empty() {
            return;
        }
        for c in self.covers.iter_mut() {
            if let Some(dst) = moves.paths.get(c) {
                *c = dst.clone();
            }
        }
        for pl in &mut self.playlists {
            for item in pl.items.iter_mut() {
                if let Some(&n) = moves.ids.get(item) {
                    *item = n;
                    pl.dirty = true;
                }
            }
        }
        let root = self.root().to_path_buf();
        for d in &moves.dirs {
            cleanup_dirs(&root, &self.layout, Some(d));
        }
        self.assign_covers(Some(&moves.dirs));
    }

    /// 評価をデータベースに書く。まだデータベースに入っていない曲 (転送した直後など) があれば、作り直してから書く
    fn write_ratings(&mut self, ratings: &HashMap<String, u8>) -> Result<()> {
        if ratings.is_empty() {
            return Ok(());
        }
        if !rbdb::supported(self.root()) {
            return Err(Error::Unsupported(t!("この Rockbox の版では、アプリから評価を付けられません")));
        }
        let missing = rbdb::set_ratings(self.root(), ratings)?;
        if !missing.is_empty() {
            self.rebuild_db()?;
            let rest: HashMap<String, u8> = ratings.iter().filter(|(k, _)| missing.contains(k)).map(|(k, v)| (k.clone(), *v)).collect();
            rbdb::set_ratings(self.root(), &rest)?;
        }
        Ok(())
    }

    pub fn edit_tags(&mut self, ids: &[u64], e: &TagEdit) -> Result<usize> {
        let mut n = 0;
        let mut ratings = HashMap::new();
        let mut moves = Moves::default();
        // 途中で失敗しても、それまでに動かした曲の後始末はする
        let r = ids.iter().try_for_each(|&id| {
            n += self.edit_one(id, e, &mut ratings, &mut moves)? as usize;
            Ok::<_, Error>(())
        });
        self.finish_moves(moves);
        r?;
        self.write_ratings(&ratings)?;
        Ok(n)
    }

    pub fn apply_tag_edits(&mut self, edits: &[TrackTagEdit]) -> Result<usize> {
        let mut n = 0;
        let mut ratings = HashMap::new();
        let mut moves = Moves::default();
        let r = edits.iter().try_for_each(|x| {
            let id = x.id.parse::<u64>().map_err(|_| Error::Other(t!("不正な ID: {id}", id = x.id)))?;
            n += self.edit_one(id, &x.edit, &mut ratings, &mut moves)? as usize;
            Ok::<_, Error>(())
        });
        self.finish_moves(moves);
        r?;
        self.write_ratings(&ratings)?;
        Ok(n)
    }

    pub fn set_artwork_from_file(&mut self, ids: &[u64], image: &Path) -> Result<usize> {
        let bytes = fs::read(image)?;
        self.set_artwork_from_bytes(ids, &bytes)
    }

    /// 曲に画像を埋め込み、アルバムのフォルダの cover.jpg も差し替える
    pub fn set_artwork_from_bytes(&mut self, ids: &[u64], bytes: &[u8]) -> Result<usize> {
        let jpg = cover_jpeg(bytes).ok_or_else(|| Error::Other(t!("画像を読めません: {e}", e = "")))?;
        let set: HashSet<u64> = ids.iter().copied().collect();
        let mut n = 0;
        let mut dirs = HashSet::new();
        for t in self.tracks.iter_mut().filter(|t| set.contains(&t.id)) {
            media::write_cover(&t.path, Some(bytes))?;
            t.has_art = true;
            if let Some(d) = t.path.parent() {
                dirs.insert(d.to_path_buf());
            }
            n += 1;
        }
        for d in &dirs {
            fs::write(d.join("cover.jpg"), &jpg)?;
            self.image_dirs.insert(d.clone());
        }
        self.assign_covers(Some(&dirs));
        Ok(n)
    }

    /// 曲の埋め込み画像を消す。フォルダの曲をすべて選んだときは cover.jpg なども消す
    pub fn remove_artwork(&mut self, ids: &[u64]) -> Result<usize> {
        let set: HashSet<u64> = ids.iter().copied().collect();
        let mut n = 0;
        let mut dirs: HashMap<PathBuf, usize> = HashMap::new();
        for t in self.tracks.iter_mut().filter(|t| set.contains(&t.id)) {
            media::write_cover(&t.path, None)?;
            t.has_art = false;
            if let Some(d) = t.path.parent() {
                *dirs.entry(d.to_path_buf()).or_default() += 1;
            }
            n += 1;
        }
        let mut per_dir: HashMap<PathBuf, usize> = HashMap::new();
        for t in &self.tracks {
            if let Some(d) = t.path.parent().filter(|d| dirs.contains_key(*d)) {
                *per_dir.entry(d.to_path_buf()).or_default() += 1;
            }
        }
        for (d, picked) in &dirs {
            if per_dir.get(d) == Some(picked) {
                for name in ["cover.jpg", "folder.jpg", "cover.jpeg", "cover.png", "folder.png"] {
                    let _ = fs::remove_file(d.join(name));
                }
            }
            self.refresh_image_dir(d);
        }
        let changed: HashSet<PathBuf> = dirs.into_keys().collect();
        self.assign_covers(Some(&changed));
        Ok(n)
    }

    /// 曲のファイルの埋め込みのジャケットを縮める (`media::shrink_embedded_cover`)。Rockbox は埋め込みの JPEG を
    /// cover.jpg より先に読み、表示のたびに縮小するので、大きいと表示が遅い。戻り値は (縮めた曲, 縮められなかった曲)。
    /// `progress(済んだ数, 全体)` は 1 曲ごとに呼ぶ
    pub fn shrink_artwork(&mut self, ids: &[u64], max: u32, progress: &dyn Fn(usize, usize)) -> Result<(usize, usize)> {
        let set: HashSet<u64> = ids.iter().copied().collect();
        // 埋め込みの画像が無いと分かっている曲は開かない (データベースから埋めた曲は分からないので開く)
        let targets: Vec<usize> = (0..self.tracks.len()).filter(|&i| set.contains(&self.tracks[i].id) && (self.tracks[i].has_art || self.tracks[i].from_db)).collect();
        let (mut done, mut failed) = (0, 0);
        for (n, &i) in targets.iter().enumerate() {
            progress(n, targets.len());
            let path = self.tracks[i].path.clone();
            match media::shrink_embedded_cover(&path, max) {
                Ok(true) => {
                    if let Ok(m) = fs::metadata(&path) {
                        self.tracks[i].view.size = m.len() as u32;
                    }
                    done += 1;
                }
                Ok(false) => {}
                Err(e) => {
                    log::warn!("shrink artwork: {}: {e}", path.display());
                    failed += 1;
                }
            }
        }
        // ファイルの更新日時が変わったので、データベースを作り直す
        self.db_dirty |= done > 0;
        Ok((done, failed))
    }

    /// 一覧用のジャケット (JPEG)。大きい方は 600px、小さい方は 160px の正方形
    pub fn artwork_jpeg(&self, image_id: u32, prefer_large: bool) -> Option<Vec<u8>> {
        let src = self.covers.get(image_id.checked_sub(1)? as usize)?;
        let bytes = cover::find_cover(src)?;
        cover::thumbnail_jpeg(&bytes, if prefer_large { 600 } else { 160 })
    }

    /// アーティスト名 (アルバムアーティストも)。曲数の多い順
    pub fn artist_names(&self) -> Vec<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for t in &self.tracks {
            for n in [t.view.artist.trim(), t.view.album_artist.trim()] {
                if !n.is_empty() {
                    *counts.entry(n).or_default() += 1;
                }
            }
        }
        let mut v: Vec<(&str, usize)> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v.into_iter().map(|(n, _)| n.to_string()).collect()
    }

    /// 表記ゆれの候補 (iPod OS と同じ判定)
    pub fn artist_variants(&self, aliases: &HashMap<String, Vec<String>>) -> Vec<crate::domain::variants::VariantGroup> {
        let entries: Vec<crate::domain::variants::Entry> = self
            .tracks
            .iter()
            .map(|t| crate::domain::variants::Entry {
                artist: &t.view.artist,
                album_artist: &t.view.album_artist,
                album: &t.view.album,
                disc_nr: t.view.disc_nr,
                track_nr: t.view.track_nr,
                compilation: false,
            })
            .collect();
        crate::domain::variants::find_groups(&entries, aliases)
    }

    /// 表記を統一する (ファイルのアーティスト・アルバムアーティストを書き換える)。戻り値は更新した曲数
    pub fn unify_artists(&mut self, edits: &[crate::domain::track::ArtistUnify]) -> Result<usize> {
        let mut map: HashMap<String, String> = HashMap::new();
        for e in edits {
            let to = e.to.trim();
            if to.is_empty() {
                continue;
            }
            for f in &e.from {
                if f.trim() != to {
                    map.insert(f.trim().to_string(), to.to_string());
                }
            }
        }
        let mut n = 0;
        let ids: Vec<u64> = self.tracks.iter().map(|t| t.id).collect();
        for id in ids {
            let Some(t) = self.track(id) else { continue };
            let artist = map.get(t.view.artist.trim()).cloned();
            let album_artist = map.get(t.view.album_artist.trim()).cloned();
            if artist.is_none() && album_artist.is_none() {
                continue;
            }
            let e = TagEdit { artist, album_artist, ..Default::default() };
            // 種類は変えないので、曲は動かない
            n += self.edit_one(id, &e, &mut HashMap::new(), &mut Moves::default())? as usize;
        }
        Ok(n)
    }

    // ---------------- プレイリスト ----------------

    fn playlist_mut(&mut self, id: u64) -> Result<&mut RbPlaylist> {
        self.playlists
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| Error::Other(t!("プレイリストが見つかりません")))
    }

    /// プレイリストのファイルの場所 (同じ名前があれば " (2)" などを付ける)
    fn playlist_path(&self, name: &str, except: Option<u64>) -> PathBuf {
        let dir = self.playlist_dir();
        let stem = clean_name(name, "Playlist");
        let taken = |p: &Path| {
            p.exists()
                || self
                    .playlists
                    .iter()
                    .any(|x| Some(x.id) != except && x.path.to_string_lossy().to_lowercase() == p.to_string_lossy().to_lowercase())
        };
        let first = dir.join(format!("{stem}.m3u8"));
        if !taken(&first) {
            return first;
        }
        // tr: 訳さない (起きない前提の panic の文言)
        (2..).map(|n| dir.join(format!("{stem} ({n}).m3u8"))).find(|p| !taken(p)).expect("いつかは空いている")
    }

    pub fn create_playlist(&mut self, name: &str, tracks: &[u64]) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other(t!("名前を入力してください")));
        }
        let path = self.playlist_path(name, None);
        let id = id_of(&rel_key(self.root(), &path));
        let known: HashSet<u64> = self.tracks.iter().map(|t| t.id).collect();
        let items = tracks.iter().copied().filter(|i| known.contains(i)).collect();
        self.playlists.push(RbPlaylist { id, name: name.to_string(), path, items, other: Vec::new(), dirty: true, smart: None });
        Ok(id.to_string())
    }

    // ---------------- スマートプレイリスト ----------------

    fn playlist_members(&self) -> HashMap<u64, HashSet<u64>> {
        self.playlists.iter().map(|p| (p.id, p.items.iter().copied().collect())).collect()
    }

    fn evaluate_smart(&self, prefs: &SmartPrefs, rules: &Rules, previous: &[u64]) -> Vec<u64> {
        let members = self.playlist_members();
        let cx = EvalContext { now_mac: smartpl::now_mac_local(), playlists: &members };
        smartpl::evaluate(rules, prefs, &self.tracks, &cx, previous)
    }

    /// ライブアップデートのスマートプレイリストの中身を作り直す (「プレイリストが…」があるので変わらなくなるまで、最大 4 回)
    fn refresh_smart_playlists(&mut self) {
        for _ in 0..4 {
            let mut changed = false;
            for i in 0..self.playlists.len() {
                let Some((prefs, rules)) = self.playlists[i].smart.clone() else { continue };
                if !prefs.live_update {
                    continue;
                }
                let items = self.evaluate_smart(&prefs, &rules, &self.playlists[i].items);
                if items != self.playlists[i].items {
                    self.playlists[i].items = items;
                    self.playlists[i].dirty = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    pub fn smart_playlist(&self, id: u64) -> Result<SmartDef> {
        let pl = self.playlists.iter().find(|p| p.id == id).ok_or_else(|| Error::Other(t!("プレイリストが見つかりません")))?;
        let (prefs, rules) = pl.smart.clone().ok_or_else(|| Error::Other(t!("スマートプレイリストの条件を読めません")))?;
        Ok(SmartDef::from_parts(prefs, rules))
    }

    /// スマートプレイリストの名前・設定・条件 (書き出し用)
    pub fn smart_parts(&self, id: u64) -> Option<(String, SmartPrefs, Rules)> {
        let pl = self.playlists.iter().find(|p| p.id == id)?;
        let (prefs, rules) = pl.smart.clone()?;
        Some((pl.name.clone(), prefs, rules))
    }

    /// スマートプレイリストを作る (`id` が None) か、名前と条件を変える。中身はすぐ条件から決める
    pub fn save_smart_playlist(&mut self, id: Option<u64>, name: &str, def: SmartDef) -> Result<String> {
        let (prefs, rules) = def.into_parts();
        let id = match id {
            Some(id) => {
                if self.playlist_mut(id)?.smart.is_none() {
                    return Err(Error::Other(t!("プレイリストが見つかりません")));
                }
                if self.playlist_mut(id)?.name != name.trim() {
                    self.rename_playlist(id, name)?;
                }
                id
            }
            None => self.create_playlist(name, &[])?.parse().unwrap_or(0),
        };
        let previous = self.playlist_mut(id)?.items.clone();
        let items = self.evaluate_smart(&prefs, &rules, &previous);
        let pl = self.playlist_mut(id)?;
        pl.items = items;
        pl.other.clear();
        pl.smart = Some((prefs, rules));
        pl.dirty = true;
        Ok(id.to_string())
    }

    /// 条件の画面で、今の条件に合う曲の数と合計の長さ・大きさを出す
    pub fn preview_smart(&self, def: SmartDef, current: Option<u64>) -> SmartPreview {
        let (prefs, rules) = def.into_parts();
        let previous = current.and_then(|id| self.playlists.iter().find(|p| p.id == id)).map(|p| p.items.clone()).unwrap_or_default();
        let ids = self.evaluate_smart(&prefs, &rules, &previous);
        let set: HashSet<u64> = ids.iter().copied().collect();
        let (ms, bytes) = self
            .tracks
            .iter()
            .filter(|t| set.contains(&t.id))
            .fold((0u64, 0u64), |(ms, b), t| (ms + t.view.duration_ms as u64, b + t.view.size as u64));
        SmartPreview {
            sample: ids.iter().take(5).map(|d| d.to_string()).collect(),
            count: ids.len(),
            length_ms: ms,
            size: bytes,
            not_on_ipod: rules.uses_any_field(smartpl::FIELDS_NOT_ON_IPOD),
        }
    }

    /// 名前を変える (ファイル名も変える。ID は変えない)
    pub fn rename_playlist(&mut self, id: u64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other(t!("名前を入力してください")));
        }
        let new_path = self.playlist_path(name, Some(id));
        let pl = self.playlist_mut(id)?;
        if pl.path.exists() {
            fs::rename(&pl.path, &new_path)?;
        }
        pl.path = new_path;
        pl.name = name.to_string();
        pl.dirty = true;
        Ok(())
    }

    pub fn delete_playlist(&mut self, id: u64) -> Result<()> {
        let i = self
            .playlists
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| Error::Other(t!("プレイリストが見つかりません")))?;
        let pl = self.playlists.remove(i);
        self.removed_playlists.push(pl.path);
        Ok(())
    }

    pub fn add_to_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<usize> {
        let known: HashSet<u64> = self.tracks.iter().map(|t| t.id).collect();
        let valid: Vec<u64> = tracks.iter().copied().filter(|i| known.contains(i)).collect();
        let pl = self.playlist_mut(id)?;
        let before = pl.items.len();
        let mut have: HashSet<u64> = pl.items.iter().copied().collect();
        for i in valid {
            if have.insert(i) {
                pl.items.push(i);
            }
        }
        let added = pl.items.len() - before;
        pl.dirty |= added > 0;
        Ok(added)
    }

    pub fn remove_from_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<()> {
        let tracks: HashSet<u64> = tracks.iter().copied().collect();
        let pl = self.playlist_mut(id)?;
        pl.items.retain(|i| !tracks.contains(i));
        pl.dirty = true;
        Ok(())
    }

    pub fn reorder_playlist(&mut self, id: u64, order: &[u64]) -> Result<()> {
        let pl = self.playlist_mut(id)?;
        let (mut a, mut b) = (pl.items.clone(), order.to_vec());
        a.sort_unstable();
        b.sort_unstable();
        if a != b {
            return Err(Error::Other(t!("並び替え対象がプレイリストの内容と一致しません")));
        }
        pl.items = order.to_vec();
        pl.dirty = true;
        Ok(())
    }

    // ---------------- iPod OS だけの機能 ----------------

    pub fn rename_ipod(&mut self, _name: &str) -> Result<()> {
        Err(not_in_rockbox())
    }
}

/// `a/./b/../c` のようなパスを文字の上だけで整える (ファイルが無くてもよい)
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// 曲を消したあと、曲が無くなったフォルダ (ジャケットと隠しファイルだけ残っているものも) を
/// Music (と Podcasts・Audiobooks) の中で上へたどって消す。それら自体とそれより上は消さない
fn cleanup_dirs(root: &Path, layout: &str, dir: Option<&Path>) {
    // 転送先の形の一番上のフォルダ (MUSIC など) も、それ自体は消さない
    let top = layout.split('/').next().unwrap_or(MUSIC_DIR);
    let tops: Vec<PathBuf> = [MUSIC_DIR, top, PODCAST_DIR, AUDIOBOOK_DIR].iter().map(|d| root.join(d)).collect();
    let mut cur = dir.map(Path::to_path_buf);
    while let Some(d) = cur {
        if !tops.iter().any(|t| d.starts_with(t) && d != *t) {
            break;
        }
        let Ok(rd) = fs::read_dir(&d) else { break };
        let entries: Vec<_> = rd.flatten().collect();
        let only_leftovers = entries
            .iter()
            .all(|e| e.file_type().is_ok_and(|t| t.is_file()) && is_leftover(&e.file_name().to_string_lossy()));
        if !only_leftovers {
            break;
        }
        for e in entries {
            let _ = fs::remove_file(e.path());
        }
        if fs::remove_dir(&d).is_err() {
            break;
        }
        cur = d.parent().map(Path::to_path_buf);
    }
}

/// Rockbox の設定のうち、画面の案内に使うもの
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// 起動時に DB を自動で更新する設定か (「データベース → 自動更新」)
    pub db_autoupdate: bool,
    /// アプリでデータベースを作れる版の Rockbox か (できないときは本体で更新してもらう)
    pub db_supported: bool,
    /// iPod にあるデータベースの曲の数 (消した印の付いた曲も含む)。無ければ None
    pub db_tracks: Option<usize>,
    /// 最後にデータベースを作り直せなかった理由
    pub db_error: Option<String>,
    /// 入っている Rockbox の版 ("4.0" など)。rockbox-info.txt が無ければ None
    pub version: Option<String>,
    /// 「自動で続きから再生」(Automatic Resume) がオンか。ポッドキャスト・オーディオブックの案内に使う
    pub autoresume: bool,
}

/// `.rockbox/config.cfg` ("名前: 値" の行) から読む。無ければ既定値 (自動更新はオフ)
pub fn status(root: &Path) -> Status {
    let cfg = fs::read_to_string(root.join(".rockbox").join("config.cfg")).unwrap_or_default();
    let value = |key: &str| {
        cfg.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_ascii_lowercase())
        })
    };
    Status {
        db_autoupdate: value("tagcache_autoupdate").as_deref() == Some("on"),
        db_supported: rbdb::supported(root),
        db_tracks: rbdb::entry_count(root),
        db_error: None,
        version: installed(root).map(|i| i.version).filter(|v| !v.is_empty()),
        autoresume: value("autoresume enable").as_deref() == Some("on"),
    }
}

/// iPod に入っている Rockbox (`.rockbox/rockbox-info.txt`)
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    /// 機種名 (ipod6g = iPod classic、ipodvideo = iPod video など)
    pub target: String,
    /// "4.0" のようなリリース番号か、開発版の "2745e0b973-260928"
    pub version: String,
}

/// `.rockbox/rockbox-info.txt` の "Target:" と "Version:" を読む
pub fn installed(root: &Path) -> Option<Installed> {
    let text = fs::read_to_string(root.join(".rockbox").join("rockbox-info.txt")).ok()?;
    let value = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim().eq_ignore_ascii_case(key)).then(|| v.trim().to_string())
        })
    };
    let target = value("Target").filter(|t| !t.is_empty())?;
    let version = value("Version").unwrap_or_default();
    Some(Installed { target, version: version.trim_start_matches('v').to_string() })
}

/// タグのキャッシュの置き場所
enum CacheAt {
    File(PathBuf),
    Dir(PathBuf),
}

/// iPod ごとのタグのキャッシュのファイル (`dir` の中)。iPod は FireWire GUID で見分け、取れなければ容量で見分ける。
/// 曲は iPod の中の場所で覚えるので、ドライブ文字が変わっても使える
pub fn cache_file(dir: &Path, info: &DeviceInfo) -> PathBuf {
    let id: String = match &info.firewire_id {
        Some(g) => g.chars().filter(|c| c.is_ascii_alphanumeric()).collect(),
        None => format!("size-{}", info.total_bytes),
    };
    dir.join(format!("rockbox-tracks-{}.json", id.to_ascii_lowercase()))
}

/// iPod を Rockbox モードで開くのがよさそうか (Rockbox が入っている)
pub fn is_installed(root: &Path) -> bool {
    root.join(".rockbox").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_names_for_fat32() {
        assert_eq!(clean_name("AC/DC", "x"), "AC_DC");
        assert_eq!(clean_name("What?", "x"), "What_");
        assert_eq!(clean_name("  ...  ", "Unknown"), "Unknown");
        assert_eq!(clean_name("Mr. ", "x"), "Mr");
        assert_eq!(clean_name("CON", "x"), "_CON");
        assert_eq!(clean_name("com1.txt", "x"), "_com1.txt");
        assert_eq!(clean_name("米津玄師", "x"), "米津玄師");
        assert_eq!(clean_name(&"あ".repeat(100), "x").chars().count(), 80);
    }

    #[test]
    fn file_names_from_tags() {
        let src = Path::new("/x/foo.flac");
        let mut i = media::MediaInfo { title: Some("曲".into()), track_nr: 3, ..Default::default() };
        assert_eq!(file_stem(&i, src), "03 曲");
        i.disc_nr = 2;
        i.disc_total = 2;
        assert_eq!(file_stem(&i, src), "2-03 曲");
        i.track_nr = 0;
        assert_eq!(file_stem(&i, src), "曲");
        i.title = None;
        assert_eq!(file_stem(&i, src), "foo");
    }

    /// ffmpeg がある環境だけ: 転送 → ジャケット → プレイリスト → 開き直し → 削除 の一通り
    #[test]
    fn transfer_playlist_and_remove() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let base = std::env::temp_dir().join(format!("zenpod-rb-{}", std::process::id()));
        let (root, src_dir) = (base.join("ipod"), base.join("src"));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::write(root.join(".rockbox/rockbox-info.txt"), "Target: ipod6g\nVersion: v4.0\n").unwrap();
        fs::create_dir_all(&src_dir).unwrap();
        // iPod OS の曲 (iPod_Control の中) は一覧に出さない
        fs::create_dir_all(root.join("iPod_Control/Music/F00")).unwrap();
        let mk = |name: &str, title: &str, nr: &str| {
            let p = src_dir.join(name);
            let ok = std::process::Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "flac"])
                .args(["-metadata", &format!("title={title}"), "-metadata", "artist=AC/DC", "-metadata", "album=アルバム?", "-metadata", &format!("track={nr}")])
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            p
        };
        let a = mk("a.flac", "曲 1", "1");
        let b = mk("b.flac", "曲 2", "2");
        fs::copy(&a, root.join("iPod_Control/Music/F00/ABCD.flac")).unwrap();
        // ジャケットはプログレッシブ JPEG でも、cover.jpg はベースラインで書く
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(900, 900, image::Rgb([200, 30, 30])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        media::write_cover(&a, Some(png.get_ref())).unwrap();
        assert!(is_installed(&root));

        let mut lib = RockboxLib::open(&root, None).unwrap();
        assert!(lib.tracks().is_empty(), "iPod_Control の曲は出さない");
        let opts = AddOptions::default();
        for f in [&a, &b] {
            assert!(matches!(lib.add_file(f, Some(&ff), &opts).unwrap(), AddOutcome::Added { converted: false, .. }));
        }
        // 同じ曲は重複として飛ばす
        assert!(matches!(lib.add_file(&a, Some(&ff), &opts).unwrap(), AddOutcome::Duplicate { .. }));
        let dir = root.join("Music").join("AC_DC").join("アルバム_");
        assert!(dir.join("01 曲 1.flac").is_file());
        assert!(dir.join("02 曲 2.flac").is_file());
        let cover = fs::read(dir.join("cover.jpg")).unwrap();
        assert!(cover.windows(2).any(|w| w == [0xFF, 0xC0]), "ベースライン (SOF0)");
        assert!(!cover.windows(2).any(|w| w == [0xFF, 0xC2]), "プログレッシブではない");
        let img = image::load_from_memory(&cover).unwrap();
        assert_eq!((img.width(), img.height()), (COVER_MAX, COVER_MAX));
        let tracks = lib.tracks();
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|t| t.artwork_id == 1), "同じフォルダは同じジャケット");
        assert!(lib.artwork_jpeg(1, false).is_some());

        // プレイリスト: 2 曲目 → 1 曲目 の順で保存し、開き直しても同じ
        let ids: Vec<u64> = tracks.iter().map(|t| t.id.parse().unwrap()).collect();
        let pl = lib.create_playlist("お気に入り/テスト", &[ids[1], ids[0]]).unwrap();
        lib.save().unwrap();
        // データベースの作り直しは呼び出し側が裏で行う (ここではその場で)
        let rebuild = |lib: &mut RockboxLib| {
            if lib.take_db_request() {
                lib.rebuild_db().unwrap();
            }
        };
        rebuild(&mut lib);
        let text = fs::read_to_string(root.join("Playlists").join("お気に入り_テスト.m3u8")).unwrap();
        assert_eq!(text, "#EXTM3U\n/Music/AC_DC/アルバム_/02 曲 2.flac\n/Music/AC_DC/アルバム_/01 曲 1.flac\n");
        // 保存のときに Rockbox のデータベースも作る。本体と同じく iPod_Control の曲も入る
        let titles = |root: &Path| {
            let mut v: Vec<String> =
                rbdb::read(&root.join(".rockbox")).unwrap().entries.iter().map(|e| e.strings[&3].clone()).collect();
            v.sort();
            v
        };
        assert_eq!(titles(&root), ["曲 1", "曲 1", "曲 2"]);
        assert_eq!(status(&root).db_tracks, Some(3));
        let mut lib = RockboxLib::open(&root, None).unwrap();
        let pls = lib.playlists();
        assert_eq!(pls.len(), 2);
        assert!(pls[0].master);
        assert_eq!(pls[1].id, pl);
        assert_eq!(pls[1].track_ids, [ids[1].to_string(), ids[0].to_string()]);
        lib.rename_playlist(pl.parse().unwrap(), "新しい名前").unwrap();
        lib.save().unwrap();
        assert!(root.join("Playlists/新しい名前.m3u8").is_file());
        assert!(!root.join("Playlists/お気に入り_テスト.m3u8").exists());

        // タグの編集はファイルに書く
        let e = TagEdit { title: Some("変えた".into()), ..Default::default() };
        lib.edit_tags(&[ids[0]], &e).unwrap();
        assert_eq!(media::read_info(&dir.join("01 曲 1.flac")).unwrap().title.as_deref(), Some("変えた"));
        lib.save().unwrap();
        rebuild(&mut lib);
        assert_eq!(titles(&root), ["変えた", "曲 1", "曲 2"]);

        // 全曲を消すと、アルバムとアーティストのフォルダ (cover.jpg だけ残る) も消える。Music は残す
        assert_eq!(lib.remove_tracks(&ids).unwrap(), 2);
        assert!(!root.join("Music/AC_DC").exists());
        assert!(root.join("Music").is_dir());
        lib.save().unwrap();
        rebuild(&mut lib);
        assert_eq!(fs::read_to_string(root.join("Playlists/新しい名前.m3u8")).unwrap(), "#EXTM3U\n");
        assert_eq!(titles(&root), ["曲 1"]);
        let _ = fs::remove_dir_all(&base);
    }

    /// 歌詞 (.lrc) は曲と同じ名前で写し、名前の変更・種類のフォルダへの移動・削除にもついていく
    #[test]
    fn lyrics_follow_tracks() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let base = tempfile::tempdir().unwrap();
        let (root, src_dir) = (base.path().join("ipod"), base.path().join("src"));
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::write(root.join(".rockbox/rockbox-info.txt"), "Target: ipod6g\nVersion: v4.0\n").unwrap();
        fs::create_dir_all(&src_dir).unwrap();
        let a = src_dir.join("a.flac");
        let ok = std::process::Command::new(&ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "flac"])
            .args(["-metadata", "title=曲", "-metadata", "artist=A", "-metadata", "album=B", "-metadata", "track=1"])
            .arg(&a)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        fs::write(src_dir.join("A.LRC"), "[00:00.00]歌詞").unwrap();
        fs::write(src_dir.join("other.lrc"), "ほかの曲").unwrap();

        let mut lib = RockboxLib::open(&root, None).unwrap();
        assert!(matches!(lib.add_file(&a, Some(&ff), &AddOptions::default()).unwrap(), AddOutcome::Added { .. }));
        let dir = root.join("Music/A/B");
        assert_eq!(fs::read_to_string(dir.join("01 曲.lrc")).unwrap(), "[00:00.00]歌詞");
        assert!(!dir.join("other.lrc").exists());

        let id: u64 = lib.tracks()[0].id.parse().unwrap();
        lib.rename_files(&[(id, "新しい名前".into())]).unwrap();
        assert!(dir.join("新しい名前.flac").is_file());
        assert!(dir.join("新しい名前.lrc").is_file());
        assert!(!dir.join("01 曲.lrc").exists());

        let id: u64 = lib.tracks()[0].id.parse().unwrap();
        lib.edit_tags(&[id], &TagEdit { kind: Some(MediaKind::Podcast), ..Default::default() }).unwrap();
        let moved = lib.track_file(lib.tracks()[0].id.parse().unwrap()).unwrap();
        assert!(moved.starts_with(root.join(PODCAST_DIR)));
        assert!(moved.with_extension("lrc").is_file());
        assert!(!dir.join("新しい名前.lrc").exists());

        let id: u64 = lib.tracks()[0].id.parse().unwrap();
        lib.remove_tracks(&[id]).unwrap();
        assert!(!moved.with_extension("lrc").exists());
        assert!(!moved.parent().unwrap().exists(), "歌詞が残ってフォルダが消えないことがない");
    }

    /// ジャケットはフォルダごと。曲を足す・画像を消すと、そのフォルダだけ振り直す (ほかのフォルダの ID は変わらない)
    #[test]
    fn covers_per_folder_incremental() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (root, src) = (dir.path().join("ipod"), dir.path().join("src"));
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::create_dir_all(&src).unwrap();
        let mk = |name: &str, album: &str| {
            let p = src.join(name);
            let ok = std::process::Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"])
                .args(["-metadata", &format!("title={name}"), "-metadata", "artist=A", "-metadata", &format!("album={album}")])
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            p
        };
        let (a1, a2, b1) = (mk("a1.flac", "X"), mk("a2.flac", "X"), mk("b1.flac", "Y"));
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(64, 64, image::Rgb([1, 2, 3])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        media::write_cover(&a1, Some(png.get_ref())).unwrap();

        let mut lib = RockboxLib::open(&root, None).unwrap();
        // cover.jpg は書かない: 埋め込みの画像 (タグを読んだときに覚えた has_art) でジャケットが付く
        let opts = AddOptions { artwork: false, ..Default::default() };
        for f in [&a1, &b1, &a2] {
            lib.add_file(f, None, &opts).unwrap();
        }
        let art = |lib: &RockboxLib, title: &str| lib.tracks().iter().find(|t| t.title == title).unwrap().artwork_id;
        assert_eq!(art(&lib, "a1.flac"), 1);
        assert_eq!(art(&lib, "a2.flac"), 1, "同じフォルダは先頭の曲の画像");
        assert_eq!(art(&lib, "b1.flac"), 0);
        assert!(lib.artwork_jpeg(1, false).is_some());
        // 開き直しても同じ (キャッシュ無し = タグから)
        let mut lib = RockboxLib::open(&root, None).unwrap();
        assert_eq!((art(&lib, "a1.flac"), art(&lib, "b1.flac")), (1, 0));
        // Y のフォルダに画像を付けても、X の ID はそのまま
        let b = lib.tracks().iter().find(|t| t.title == "b1.flac").unwrap().id.parse::<u64>().unwrap();
        lib.set_artwork_from_bytes(&[b], png.get_ref()).unwrap();
        assert_eq!((art(&lib, "a1.flac"), art(&lib, "b1.flac")), (1, 2));
        // 画像を消すと、そのフォルダだけ無くなる (フォルダの曲を全部選んだので cover.jpg も消える)
        lib.remove_artwork(&[b]).unwrap();
        assert_eq!((art(&lib, "a1.flac"), art(&lib, "b1.flac")), (1, 0));
        assert!(!root.join("Music/A/Y/cover.jpg").exists());
    }

    /// 初めて開くとき、キャッシュに無い曲は Rockbox のデータベースから埋め (タグを読まない)、
    /// あとで読み直すと並べ替え用のタグと埋め込みのジャケットが付く。更新日時が違う曲はデータベースを使わない
    #[test]
    fn seeds_from_database_then_rereads() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ipod");
        let cache = dir.path().join("cache").join("rockbox-tracks.json");
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::write(root.join(".rockbox/rockbox-info.txt"), "Target: ipod6g\nVersion: v4.0\n").unwrap();
        let src = dir.path().join("src.flac");
        let ok = std::process::Command::new(&ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"])
            .args(["-metadata", "title=曲", "-metadata", "artist=A", "-metadata", "ARTISTSORT=Sort A", "-metadata", "album=X"])
            .args(["-metadata", "track=3", "-metadata", "date=2001"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(64, 64, image::Rgb([1, 2, 3])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        media::write_cover(&src, Some(png.get_ref())).unwrap();
        let album = root.join("Music/A/X");
        fs::create_dir_all(&album).unwrap();
        let n = SEED_MIN + 10;
        for i in 0..n {
            fs::copy(&src, album.join(format!("{i:02}.flac"))).unwrap();
        }
        rbdb::rebuild(&root, None).unwrap();
        // データベースを作ったあとで変わったファイルは、データベースを使わずにタグを読む
        let changed = album.join("00.flac");
        fs::File::options()
            .write(true)
            .open(&changed)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();

        let mut lib = RockboxLib::open(&root, Some(cache.clone())).unwrap();
        let st = lib.scan_stats();
        assert_eq!((st.files, st.from_db, st.reread), (n, n - 1, 1));
        let tracks = lib.tracks();
        let t = tracks.iter().find(|t| t.rb_path.ends_with("/01.flac")).unwrap();
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str(), t.track_nr, t.year), ("曲", "A", "X", 3, 2001));
        assert_eq!(t.sort_artist, "", "データベースに並べ替え用のタグは無い");
        assert!(t.duration_ms > 900);
        // フォルダの先頭の曲 (00) はタグを読んだので、埋め込みのジャケットが付く
        assert!(tracks.iter().all(|t| t.artwork_id == 1));
        let files = lib.db_filled_files();
        assert_eq!(files.len(), n - 1);

        let read = pcfolder::reread_device_files(&files, Some(&cache), &root, &|| false, &|| false);
        assert_eq!(read.len(), n - 1);
        assert_eq!(lib.apply_reread(&read), n - 1);
        assert!(lib.db_filled_files().is_empty());
        let t = lib.tracks().into_iter().find(|t| t.rb_path.ends_with("/01.flac")).unwrap();
        assert_eq!((t.sort_artist.as_str(), t.rb_path.as_str()), ("Sort A", "/Music/A/X/01.flac"));
        // 読み直した結果はキャッシュに入っているので、開き直してもデータベースもタグも読まない
        let lib = RockboxLib::open(&root, Some(cache.clone())).unwrap();
        let st = lib.scan_stats();
        assert_eq!((st.from_db, st.reread), (0, 0));
        assert!(lib.tracks().iter().all(|t| t.sort_artist == "Sort A"));
    }

    /// macOS は iPod (FAT) の名前を濁点を分けた形 (NFD) で返すが、本体のデータベースは合成した形 (NFC)。
    /// NFD の名前でもデータベースから埋め、評価を引く。iPod へ書く場所は、macOS では NFC、ほかの OS ではファイル名のまま
    #[test]
    fn nfd_names_match_database() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ipod");
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::write(root.join(".rockbox/rockbox-info.txt"), "Target: ipod6g\nVersion: v4.0\n").unwrap();
        let data = vec![0u8; 1600];
        let mut w = Vec::new();
        for part in [&b"RIFF"[..], &(36 + data.len() as u32).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(),
            &1u16.to_le_bytes(), &8000u32.to_le_bytes(), &16000u32.to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes(),
            b"data", &(data.len() as u32).to_le_bytes(), &data] {
            w.extend_from_slice(part);
        }
        let nfc_album = root.join("Music/A/ガ");
        fs::create_dir_all(&nfc_album).unwrap();
        for i in 0..SEED_MIN {
            fs::write(nfc_album.join(format!("{i:02}.wav")), &w).unwrap();
        }
        rbdb::rebuild(&root, None).unwrap();
        let rating = 8;
        rbdb::set_ratings(&root, &HashMap::from([("/music/a/ガ/00.wav".to_string(), rating)])).unwrap();
        // Mac から見える名前にする (名前を変えてもファイルの更新日時は変わらない)
        fs::rename(&nfc_album, root.join("Music/A/カ\u{3099}")).unwrap();

        let lib = RockboxLib::open(&root, Some(dir.path().join("cache.json"))).unwrap();
        assert_eq!(lib.scan_stats().from_db, SEED_MIN);
        let tracks = lib.tracks();
        let t = tracks.iter().find(|t| t.rb_path.ends_with("/00.wav")).unwrap();
        let written = if cfg!(target_os = "macos") { "/Music/A/ガ/00.wav" } else { "/Music/A/カ\u{3099}/00.wav" };
        assert_eq!((t.rb_path.as_str(), t.rating), (written, rating * 10));
    }

    /// ファイル名を変えると、曲 ID も変わり、プレイリストの中の曲も新しい名前になる
    #[test]
    fn rename_files_updates_playlists() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (root, src) = (dir.path().join("ipod"), dir.path().join("src"));
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::create_dir_all(&src).unwrap();
        let mk = |name: &str, nr: &str| {
            let p = src.join(format!("{name}.flac"));
            let ok = std::process::Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"])
                .args(["-metadata", &format!("title={name}"), "-metadata", "artist=A", "-metadata", "album=X", "-metadata", &format!("track={nr}")])
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            p
        };
        let (a, b) = (mk("a", "1"), mk("b", "2"));
        let mut lib = RockboxLib::open(&root, None).unwrap();
        let opts = AddOptions { artwork: false, ..Default::default() };
        lib.add_file(&a, None, &opts).unwrap();
        lib.add_file(&b, None, &opts).unwrap();
        let ids: Vec<u64> = lib.tracks().iter().map(|t| t.id.parse().unwrap()).collect();
        let pl: u64 = lib.create_playlist("P", &ids).unwrap().parse().unwrap();
        lib.save().unwrap();
        let first = lib.tracks().into_iter().find(|t| t.title == "a").unwrap();
        assert_eq!(first.rb_path, "/Music/A/X/01 a.flac");

        assert_eq!(lib.rename_files(&[(first.id.parse().unwrap(), "01 - 新しい名前".into())]).unwrap(), 1);
        lib.save().unwrap();
        let dir_x = root.join("Music/A/X");
        assert!(dir_x.join("01 - 新しい名前.flac").is_file());
        assert!(!dir_x.join("01 a.flac").exists());
        let renamed = lib.tracks().into_iter().find(|t| t.title == "a").unwrap();
        assert_ne!(renamed.id, first.id, "場所から作る ID は変わる");
        assert_eq!(renamed.rb_path, "/Music/A/X/01 - 新しい名前.flac");
        let pls = lib.playlists();
        let p = pls.iter().find(|p| p.id == pl.to_string()).unwrap();
        assert!(p.track_ids.contains(&renamed.id));
        assert_eq!(
            fs::read_to_string(root.join("Playlists/P.m3u8")).unwrap(),
            "#EXTM3U\n/Music/A/X/01 - 新しい名前.flac\n/Music/A/X/02 b.flac\n"
        );
        // 開き直しても同じ ID
        let lib = RockboxLib::open(&root, None).unwrap();
        assert!(lib.tracks().iter().any(|t| t.id == renamed.id));
        // 同じ名前にはできない
        let mut lib = lib;
        let b_id: u64 = lib.tracks().iter().find(|t| t.title == "b").unwrap().id.parse().unwrap();
        assert!(lib.rename_files(&[(b_id, "01 - 新しい名前".into())]).is_err());
    }

    #[test]
    /// ffmpeg がある環境だけ: ポッドキャスト・オーディオブックの置き場所、種類を変えたときの移動、評価
    fn kinds_and_ratings() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let base = std::env::temp_dir().join(format!("zenpod-rbkind-{}", std::process::id()));
        let (root, src_dir) = (base.join("ipod"), base.join("src"));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::write(root.join(".rockbox/rockbox-info.txt"), "Target: ipod6g\nVersion: v4.0\n").unwrap();
        fs::create_dir_all(&src_dir).unwrap();
        let mk = |name: &str, codec: &str, meta: &[&str]| {
            let p = src_dir.join(name);
            let mut cmd = std::process::Command::new(&ff);
            cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", codec]);
            for m in meta {
                cmd.args(["-metadata", m]);
            }
            assert!(cmd.arg(&p).status().unwrap().success());
            p
        };
        let pod = mk("pod.mp3", "libmp3lame", &["title=第 1 回", "artist=番組の人", "album=ラジオ A", "genre=Podcast"]);
        let book = mk("book.m4b", "aac", &["title=第 1 章", "artist=作者", "album=本"]);
        let song = mk("song.flac", "flac", &["title=曲", "artist=バンド", "album=盤", "track=1"]);
        let mut lib = RockboxLib::open(&root, None).unwrap();
        for f in [&pod, &book, &song] {
            lib.add_file(f, Some(&ff), &AddOptions::default()).unwrap();
        }
        let by_title = |lib: &RockboxLib, t: &str| lib.tracks().into_iter().find(|v| v.title == t).unwrap();
        assert!(root.join("Podcasts/ラジオ A/第 1 回.mp3").is_file());
        assert!(root.join("Audiobooks/作者/本/第 1 章.m4b").is_file());
        assert!(root.join("Music/バンド/盤/01 曲.flac").is_file());
        assert_eq!(by_title(&lib, "第 1 回").kind, MediaKind::Podcast);
        assert_eq!(by_title(&lib, "第 1 章").kind, MediaKind::Audiobook);
        assert_eq!(by_title(&lib, "曲").kind, MediaKind::Music);

        // 曲をポッドキャストにすると Podcasts/アルバム へ動き、プレイリストの中も差し替わる。空になったフォルダは消す
        let song_id: u64 = by_title(&lib, "曲").id.parse().unwrap();
        let pl = lib.create_playlist("リスト", &[song_id]).unwrap();
        let e = TagEdit { kind: Some(MediaKind::Podcast), ..Default::default() };
        assert_eq!(lib.edit_tags(&[song_id], &e).unwrap(), 1);
        assert!(root.join("Podcasts/盤/01 曲.flac").is_file());
        assert!(!root.join("Music/バンド").exists());
        let moved = by_title(&lib, "曲");
        assert_eq!(moved.kind, MediaKind::Podcast);
        let pls = lib.playlists();
        assert_eq!(pls.iter().find(|p| p.id == pl).unwrap().track_ids, vec![moved.id.clone()]);

        // 評価: データベースがまだ無いので作ってから書く。開き直しても、作り直しても残る
        let book_id: u64 = by_title(&lib, "第 1 章").id.parse().unwrap();
        let e = TagEdit { rating: Some(80), ..Default::default() };
        lib.edit_tags(&[book_id], &e).unwrap();
        assert_eq!(by_title(&lib, "第 1 章").rating, 80);
        assert_eq!(rbdb::stats(&root).get("/audiobooks/作者/本/第 1 章.m4b").map(|s| s.rating), Some(8));
        lib.save().unwrap();
        lib.rebuild_db().unwrap();
        let lib = RockboxLib::open(&root, None).unwrap();
        assert_eq!(by_title(&lib, "第 1 章").rating, 80);
        assert_eq!(by_title(&lib, "第 1 回").rating, 0);

        // 「自動で続きから再生」の設定
        assert!(!lib.status().autoresume);
        fs::write(root.join(".rockbox/config.cfg"), "autoresume enable: on\n").unwrap();
        assert!(lib.status().autoresume);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    /// 本体で足した一覧に無い曲 (iPod_Control の曲など) の行は、アプリでプレイリストを編集しても消さない
    fn playlists_keep_unknown_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::create_dir_all(root.join("Music/A")).unwrap();
        fs::create_dir_all(root.join("Playlists")).unwrap();
        // 短い無音の WAV (タグが無いので曲名はファイル名)
        let data = vec![0u8; 1600];
        let mut w = Vec::new();
        for part in [&b"RIFF"[..], &(36 + data.len() as u32).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(),
            &1u16.to_le_bytes(), &8000u32.to_le_bytes(), &16000u32.to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes(),
            b"data", &(data.len() as u32).to_le_bytes(), &data] {
            w.extend_from_slice(part);
        }
        for n in ["a", "b", "c"] {
            fs::write(root.join(format!("Music/A/{n}.wav")), &w).unwrap();
        }
        fs::write(
            root.join("Playlists/list.m3u8"),
            "#EXTM3U\n/Music/A/a.wav\n/iPod_Control/Music/F00/ABCD.mp3\n/Music/A/b.wav\n/Missing/x.flac\n",
        )
        .unwrap();
        let mut lib = RockboxLib::open(root, None).unwrap();
        let id_of_title = |lib: &RockboxLib, t: &str| lib.tracks().into_iter().find(|v| v.title == t).unwrap().id.parse::<u64>().unwrap();
        let pl = lib.playlists().into_iter().find(|p| p.name == "list").unwrap();
        assert_eq!(pl.track_ids.len(), 2, "画面には一覧にある曲だけ");
        let pid: u64 = pl.id.parse().unwrap();
        let c = id_of_title(&lib, "c");
        lib.add_to_playlist(pid, &[c]).unwrap();
        lib.save().unwrap();
        let text = fs::read_to_string(root.join("Playlists/list.m3u8")).unwrap();
        assert_eq!(
            text,
            "#EXTM3U\n/Music/A/a.wav\n/iPod_Control/Music/F00/ABCD.mp3\n/Music/A/b.wav\n/Missing/x.flac\n/Music/A/c.wav\n"
        );
        // 曲を外しても、知らない行は残る
        let a = id_of_title(&lib, "a");
        lib.remove_from_playlist(pid, &[a]).unwrap();
        lib.save().unwrap();
        let text = fs::read_to_string(root.join("Playlists/list.m3u8")).unwrap();
        assert!(text.contains("/iPod_Control/Music/F00/ABCD.mp3\n") && text.contains("/Missing/x.flac\n"));
        assert!(!text.contains("/Music/A/a.wav"));
        // ビットレートも出す
        assert!(lib.tracks().iter().all(|t| t.bitrate > 0));
    }

    #[test]
    /// PC のアプリが書いたプレイリスト: Latin-1 の .m3u・ドライブ文字付きのパス・NFD のファイル名・本体のボリューム付きのパスも曲として読む。
    /// 書き直した .m3u には BOM を付ける (本体は BOM の無い .m3u を Latin-1 として読む)
    fn playlists_from_pc_apps() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::create_dir_all(root.join("Music/A")).unwrap();
        fs::create_dir_all(root.join("Playlists")).unwrap();
        let data = vec![0u8; 1600];
        let mut w = Vec::new();
        for part in [&b"RIFF"[..], &(36 + data.len() as u32).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(),
            &1u16.to_le_bytes(), &8000u32.to_le_bytes(), &16000u32.to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes(),
            b"data", &(data.len() as u32).to_le_bytes(), &data] {
            w.extend_from_slice(part);
        }
        for n in ["Café", "b", "がく"] {
            fs::write(root.join(format!("Music/A/{n}.wav")), &w).unwrap();
        }
        // Latin-1 の "Café"、ドライブ文字付きの Windows のパス、NFD の「がく」(か + 濁点)
        let mut m3u = b"#EXTM3U\r\n/Music/A/Caf\xe9.wav\r\nE:\\Music\\A\\b.wav\r\n".to_vec();
        m3u.extend_from_slice("../Music/A/か\u{3099}く.wav\r\n".as_bytes());
        // Latin-1 の行があるので、ファイル全体は UTF-8 として読めない → Latin-1 として読む。NFD の行は別のファイルで確かめる
        fs::write(root.join("Playlists/pc.m3u"), &m3u[..m3u.len() - "../Music/A/か\u{3099}く.wav\r\n".len()]).unwrap();
        fs::write(root.join("Playlists/nfd.m3u8"), "/Music/A/か\u{3099}く.wav\n../Music/A/b.wav\n").unwrap();
        // 本体が書くボリューム付きのパス。0 番だけがこのドライブ
        fs::write(root.join("Playlists/vol.m3u8"), "/<HDD0>/Music/A/b.wav\n/Music/A/Café.wav\n/<HDD1>/Music/A/b.wav\n").unwrap();
        let mut lib = RockboxLib::open(root, None).unwrap();
        let pls = lib.playlists();
        let pc = pls.iter().find(|p| p.name == "pc").unwrap();
        assert_eq!(pc.track_ids.len(), 2, "Latin-1 とドライブ文字のパス");
        let nfd = pls.iter().find(|p| p.name == "nfd").unwrap();
        assert_eq!(nfd.track_ids.len(), 2, "NFD と相対パス");
        let vol = pls.iter().find(|p| p.name == "vol").unwrap();
        assert_eq!(vol.track_ids.len(), 2, "/<HDD0>/ のパス");
        let pid: u64 = pc.id.parse().unwrap();
        let ids: Vec<u64> = pc.track_ids.iter().rev().map(|i| i.parse().unwrap()).collect();
        lib.reorder_playlist(pid, &ids).unwrap();
        lib.save().unwrap();
        let text = fs::read_to_string(root.join("Playlists/pc.m3u")).unwrap();
        assert_eq!(text, "\u{feff}#EXTM3U\n/Music/A/b.wav\n/Music/A/Café.wav\n");
        // 書き直したものを読み直しても同じ
        let lib = RockboxLib::open(root, None).unwrap();
        assert_eq!(lib.playlists().iter().find(|p| p.name == "pc").unwrap().track_ids.len(), 2);
    }

    #[test]
    /// ffmpeg がある環境だけ: 転送先の形を変えると、その形で置く。今ある曲の並びから形を推測する
    fn custom_layout_and_guess() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let base = std::env::temp_dir().join(format!("zenpod-rblayout-{}", std::process::id()));
        let (root, src_dir) = (base.join("ipod"), base.join("src"));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::create_dir_all(&src_dir).unwrap();
        let mk = |name: &str, meta: &[&str]| {
            let p = src_dir.join(name);
            let mut cmd = std::process::Command::new(&ff);
            cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"]);
            for m in meta {
                cmd.args(["-metadata", m]);
            }
            assert!(cmd.arg(&p).status().unwrap().success());
            p
        };
        let a = mk("a.flac", &["title=712PM", "artist=Future", "album=I NEVER LIKED YOU", "date=2022", "track=1"]);
        let b = mk("b.flac", &["title=SICKO MODE", "artist=Travis Scott", "album=ASTROWORLD", "date=2018", "track=3"]);
        let c = mk("c.flac", &["title=Still D.R.E.", "artist=Dr. Dre", "album=2001", "date=1999", "track=2"]);
        let mut lib = RockboxLib::open(&root, None).unwrap();
        assert!(lib.set_layout("MUSIC/{album}").is_err(), "ファイル名に曲名が無い形は使えない");
        lib.set_layout("MUSIC/{album} [{year}]/{title} - {artist}").unwrap();
        assert_eq!(lib.preview_layout(lib.layout()).unwrap(), "MUSIC/Album [2024]/Song - Artist.flac", "曲が無いときは架空の曲で見本");
        for f in [&a, &b, &c] {
            lib.add_file(f, Some(&ff), &AddOptions::default()).unwrap();
        }
        assert!(root.join("MUSIC/I NEVER LIKED YOU [2022]/712PM - Future.flac").is_file());
        assert!(root.join("MUSIC/ASTROWORLD [2018]/SICKO MODE - Travis Scott.flac").is_file());
        assert_eq!(lib.preview_layout(crate::domain::layout::DEFAULT).unwrap().split('/').count(), 4);
        // 開き直して推測すると同じ形 (今は既定の形なので、案内する候補になる)
        let lib = RockboxLib::open(&root, None).unwrap();
        let g = lib.guess_layout().unwrap();
        assert_eq!(g.template, "MUSIC/{album} [{year}]/{title} - {artist}");
        assert_eq!((g.matched, g.total), (3, 3));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn reads_db_autoupdate() {
        let root = std::env::temp_dir().join(format!("zenpod-rbcfg-{}", std::process::id()));
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        assert!(!status(&root).db_autoupdate);
        fs::write(root.join(".rockbox/config.cfg"), "# comment\nvolume: -25\ntagcache_autoupdate: on\n").unwrap();
        assert!(status(&root).db_autoupdate);
        fs::write(root.join(".rockbox/config.cfg"), "tagcache_autoupdate: off\n").unwrap();
        assert!(!status(&root).db_autoupdate);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ids_ignore_case_and_root() {
        let root = Path::new("/media/IPOD");
        let a = rel_key(root, Path::new("/media/IPOD/Music/A/01 x.flac"));
        assert_eq!(a, "music/a/01 x.flac");
        assert_eq!(id_of(&a), id_of(&rel_key(root, Path::new("/media/IPOD/MUSIC/a/01 X.FLAC"))));
    }

    /// ffmpeg がある環境だけ: 前に開いたことがあれば、たどり終わる前に前回の一覧を 1 回だけ知らせ、
    /// 読み終わったときの一覧は今の中身 (消した曲は消え、足した曲は入る)
    #[test]
    fn reopen_shows_last_listing_first() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (root, cache) = (dir.path().join("ipod"), dir.path().join("cache"));
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        let album = root.join("Music").join("A").join("X");
        fs::create_dir_all(&album).unwrap();
        let mk = |name: &str, title: &str| {
            let ok = std::process::Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"])
                .args(["-metadata", &format!("title={title}"), "-metadata", "artist=A", "-metadata", "album=X"])
                .arg(album.join(name))
                .status()
                .unwrap()
                .success();
            assert!(ok);
        };
        mk("01.flac", "一");
        mk("02.flac", "二");
        mk("03.flac", "三");

        // 初めて開く: 前回の一覧は無い
        let calls = Mutex::new(Vec::new());
        let partial = |_: &DeviceInfo, _: &str, v: Vec<TrackView>, cached: bool| calls.lock().unwrap().push((v.len(), cached));
        let lib = RockboxLib::open_inner(&root, Some(CacheAt::Dir(cache.clone())), &|_, _| {}, Some(&partial), std::time::Duration::ZERO).unwrap();
        assert_eq!(lib.tracks().len(), 3);
        assert!(calls.lock().unwrap().iter().all(|c| !c.1), "初めて開くときは前回の一覧を出さない");

        // 1 曲消して 1 曲足してから開き直す
        fs::remove_file(album.join("02.flac")).unwrap();
        mk("04.flac", "四");
        calls.lock().unwrap().clear();
        let lib = RockboxLib::open_inner(&root, Some(CacheAt::Dir(cache.clone())), &|_, _| {}, Some(&partial), std::time::Duration::ZERO).unwrap();
        // 前回の一覧 (3 曲) を 1 回だけ。そのあと読めた曲 (足した曲) は知らせない
        assert_eq!(*calls.lock().unwrap(), vec![(3, true)]);
        let mut titles: Vec<_> = lib.tracks().iter().map(|t| t.title.clone()).collect();
        titles.sort();
        assert_eq!(titles, ["一", "三", "四"]);

        // partial を渡さない開き方 (計測など) では作らない
        calls.lock().unwrap().clear();
        RockboxLib::open_inner(&root, Some(CacheAt::Dir(cache)), &|_, _| {}, None, std::time::Duration::ZERO).unwrap();
        assert!(calls.lock().unwrap().is_empty());
    }
}
