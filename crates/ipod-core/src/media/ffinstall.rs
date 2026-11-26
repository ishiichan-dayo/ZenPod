//! ffmpeg の自動インストール (Windows / macOS)。
//!
//! - Windows: BtbN/FFmpeg-Builds の GitHub リリースから LGPL の shared 版 (約 80MB) を取得し、
//!   配布元の checksums.sha256 と照合してから ffmpeg.exe と DLL だけを取り出す
//! - macOS: martin-riedl.de のビルド (GPL 版・署名と公証済み、約 30MB の zip に ffmpeg 1 つ) を取得し、
//!   配布元の .sha256 と照合してから取り出し、codesign で Developer ID の署名 (チーム) を確かめる。BtbN は macOS 版を出していない
//!
//! どちらもアプリ専用のフォルダに入れる。管理者権限・PATH の変更は不要で、フォルダを消せばアンインストールできる。

use crate::online::podcast::{STALL, read_watched};
use crate::t;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

const RELEASES: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";
/// 最新の安定版へ転送される URL。転送先 (/download/macos/<arch>/<版>/ffmpeg.zip) の隣に .sha256 がある
const MAC_LATEST: &str = "https://ffmpeg.martin-riedl.de/redirect/latest/macos/{arch}/release/ffmpeg.zip";
/// macOS の配布元。転送先がこれ以外 (http や別のサーバー) なら落とさない
const MAC_ORIGIN: &str = "https://ffmpeg.martin-riedl.de/";
/// martin-riedl.de の Developer ID のチーム。2026-10-07 に arm64 / amd64 の 9.0.2 の署名から読んだ
/// (証明書は "Developer ID Application: Martin Riedl (KU3N25YGLU)")
#[cfg(target_os = "macos")]
const MAC_TEAM_ID: &str = "KU3N25YGLU";

/// 小さいもの (リリースの一覧・チェックサム) の本文を受け取る時間 (ヘッダを受け取ってからの合計)
const SMALL_BODY_TIMEOUT: Duration = Duration::from_secs(60);
/// zip の本文を受け取る時間の上限 (合計。遅い回線でも通るよう長く取り、止まったかは `read_watched` で見る)
const ZIP_BODY_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// zip の大きさの上限 (今は 30〜80MB。おかしな応答で PC の空きを使い切らないように)
const MAX_ZIP_BYTES: u64 = 512 << 20;

/// 取り出す実行ファイルの名前
const EXE: &str = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// 配布元が公開している SHA-256 (小文字 16 進)
    pub sha256: String,
}

/// `recv_body` は ureq では無通信の時間ではなく、ヘッダを受け取ってからの合計
fn agent(recv_body: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(recv_body))
        // 転送も含めて https だけ
        .https_only(true)
        .user_agent(concat!("ZenPod/", env!("CARGO_PKG_VERSION"), " (desktop app)"))
        .build()
        .into()
}

fn net_err(e: impl std::fmt::Display) -> Error {
    Error::Other(t!("ダウンロードに失敗しました: {e}", e = e))
}

/// "ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip" → Some((8, 1))
fn stable_version(name: &str) -> Option<(u32, u32)> {
    let rest = name.strip_prefix("ffmpeg-n")?;
    let (ver, tail) = rest.split_once("-latest-win64-lgpl-shared-")?;
    if !tail.ends_with(".zip") {
        return None;
    }
    let (a, b) = ver.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// リリース一覧から使うファイルを選ぶ: 安定版 (nX.Y) の一番新しいもの、無ければ開発版
pub fn pick_asset(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|n| stable_version(n).map(|v| (v, *n)))
        .max()
        .map(|(_, n)| n.to_string())
        .or_else(|| {
            names
                .iter()
                .find(|n| **n == "ffmpeg-master-latest-win64-lgpl-shared.zip")
                .map(|n| n.to_string())
        })
}

/// checksums.sha256 ("<hash>  <file>" の行) から指定ファイルのハッシュを探す
pub fn checksum_for(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (hash, file) = (it.next()?, it.next()?);
        (file.trim_start_matches('*') == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

/// 最新版のダウンロード先とハッシュを調べる
pub fn latest_asset() -> Result<Asset> {
    if cfg!(target_os = "macos") {
        latest_mac_asset(std::env::consts::ARCH)
    } else {
        latest_windows_asset()
    }
}

/// Rust のアーキテクチャ名 → 配布元の名前 (Apple Silicon は arm64、Intel は amd64)
fn mac_arch(arch: &str) -> &'static str {
    if arch == "aarch64" { "arm64" } else { "amd64" }
}

fn latest_mac_asset(arch: &str) -> Result<Asset> {
    let latest = MAC_LATEST.replace("{arch}", mac_arch(arch));
    // 転送先 (版の入った本当の場所) を知るため、転送をたどらずに Location を読む (配布元は HEAD に応じない)
    let no_redirect: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .https_only(true)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .user_agent(concat!("ZenPod/", env!("CARGO_PKG_VERSION"), " (desktop app)"))
        .build()
        .into();
    let resp = no_redirect.get(&latest).call().map_err(net_err)?;
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| Error::Other(t!("配布ファイルが見つかりませんでした")))?;
    let url = mac_download_url(&latest, location)?;
    let sums = agent(SMALL_BODY_TIMEOUT)
        .get(&format!("{url}.sha256"))
        .call()
        .map_err(net_err)?
        .body_mut()
        .read_to_string()
        .map_err(net_err)?;
    let sha256 = checksum_for(&sums, "ffmpeg.zip").ok_or_else(|| Error::Other(t!("チェックサムが見つかりませんでした")))?;
    // 大きさはダウンロードの応答からわかる
    Ok(Asset {
        name: "ffmpeg.zip".into(),
        url,
        size: 0,
        sha256,
    })
}

/// Location が "/download/..." のようにサーバー内の場所だけのとき、元の URL のサーバー名を補う
fn absolute_url(base: &str, location: &str) -> String {
    if location.starts_with("https://") || location.starts_with("http://") {
        return location.to_string();
    }
    let origin_end = base.find("://").map(|i| i + 3).and_then(|i| base[i..].find('/').map(|j| i + j)).unwrap_or(base.len());
    format!("{}{}", &base[..origin_end], location)
}

/// 転送先を配布元の https の URL にする。http や別のサーバーを指していたら落とさない
/// (zip とチェックサムを同じ場所から取るので、ここが書き換えられると照合の意味が無くなる)
fn mac_download_url(base: &str, location: &str) -> Result<String> {
    let url = absolute_url(base, location);
    let ok = url.starts_with(MAC_ORIGIN) && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    if !ok {
        return Err(Error::Other(t!("配布元が想定と違う場所を指しています")));
    }
    Ok(url)
}

/// 取り出した ffmpeg が martin-riedl.de の Developer ID で署名されているか (Apple の証明書から続く署名か) を
/// codesign で確かめる。チェックサムは zip と同じ配布元から取るので、それだけでは配布元が乗っ取られたときに防げない
#[cfg(target_os = "macos")]
fn verify_signature(exe: &Path) -> Result<()> {
    let fail = || Error::Other(t!("ffmpeg の署名を確かめられませんでした"));
    // Developer ID の証明書 (Apple のルートから続く) で、チームが MAC_TEAM_ID のもの
    let requirement = format!(
        "=anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{MAC_TEAM_ID}\""
    );
    let verified = crate::host::command("/usr/bin/codesign")
        .args(["--verify", "--strict", "-R"])
        .arg(&requirement)
        .arg(exe)
        .output()
        .map_err(|_| fail())?;
    if !verified.status.success() {
        log::warn!("codesign --verify: {}", String::from_utf8_lossy(&verified.stderr).trim());
        return Err(fail());
    }
    // 念のため表示上のチームも見る (結果は標準エラーに出る)
    let shown = crate::host::command("/usr/bin/codesign").args(["-dv", "--verbose=2"]).arg(exe).output().map_err(|_| fail())?;
    if team_identifier(&String::from_utf8_lossy(&shown.stderr)) != Some(MAC_TEAM_ID) {
        return Err(fail());
    }
    Ok(())
}

/// macOS のほか (Windows の BtbN は署名していない) はチェックサムだけ
#[cfg(not(target_os = "macos"))]
fn verify_signature(_exe: &Path) -> Result<()> {
    Ok(())
}

/// `codesign -dv` の出力から TeamIdentifier を読む
#[cfg(any(target_os = "macos", test))]
fn team_identifier(text: &str) -> Option<&str> {
    text.lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).map(str::trim)
}

fn latest_windows_asset() -> Result<Asset> {
    let a = agent(SMALL_BODY_TIMEOUT);
    let rel: Value = a
        .get(RELEASES)
        .call()
        .map_err(net_err)?
        .body_mut()
        .read_json()
        .map_err(net_err)?;
    let assets = rel["assets"].as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = assets.iter().filter_map(|x| x["name"].as_str()).collect();
    let name = pick_asset(&names).ok_or_else(|| Error::Other(t!("配布ファイルが見つかりませんでした")))?;
    let url_of = |n: &str| {
        assets
            .iter()
            .find(|x| x["name"].as_str() == Some(n))
            .and_then(|x| x["browser_download_url"].as_str())
            .map(str::to_string)
    };
    // チェックサムは zip と同じリリース (同じ配布元) から取るので、壊れた転送は防げるが配布元の差し替えは防げない
    let sums_url = url_of("checksums.sha256").ok_or_else(|| Error::Other(t!("チェックサムが見つかりませんでした")))?;
    let sums = a
        .get(&sums_url)
        .call()
        .map_err(net_err)?
        .body_mut()
        .read_to_string()
        .map_err(net_err)?;
    let sha256 = checksum_for(&sums, &name).ok_or_else(|| Error::Other(t!("チェックサムが見つかりませんでした")))?;
    let size = assets
        .iter()
        .find(|x| x["name"].as_str() == Some(&*name))
        .and_then(|x| x["size"].as_u64())
        .unwrap_or(0);
    Ok(Asset {
        url: url_of(&name).unwrap_or_default(),
        name,
        size,
        sha256,
    })
}

/// zip から ffmpeg を `dest` に取り出す (ffprobe・ffplay は使わないので除く)。
/// Windows は `*/bin/` にある ffmpeg.exe と DLL、macOS は zip の直下にある ffmpeg 1 つ
pub fn extract(zip_path: &Path, dest: &Path) -> Result<PathBuf> {
    extract_checked(zip_path, dest, |_| Ok(()))
}

/// `extract` と同じで、差し替える前に取り出した ffmpeg を `check` で確かめる (だめなら前の版を残す)
fn extract_checked(zip_path: &Path, dest: &Path, check: impl FnOnce(&Path) -> Result<()>) -> Result<PathBuf> {
    let file = fs::File::open(zip_path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Other(t!("zip を開けません: {e}", e = e)))?;
    let tmp = dest.with_extension("tmp");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| Error::Other(t!("zip を読めません: {e}", e = e)))?;
        let Some(path) = f.enclosed_name() else { continue };
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
        let wanted = if cfg!(windows) {
            let in_bin = path.parent().and_then(|p| p.file_name()).is_some_and(|d| d == "bin");
            in_bin && (file_name.eq_ignore_ascii_case(EXE) || file_name.to_ascii_lowercase().ends_with(".dll"))
        } else {
            file_name == EXE
        };
        if !f.is_file() || !wanted {
            continue;
        }
        let out_path = tmp.join(&file_name);
        let mut out = fs::File::create(&out_path)?;
        std::io::copy(&mut f, &mut out)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&out_path, fs::Permissions::from_mode(0o755))?;
        }
    }
    if !tmp.join(EXE).is_file() {
        let _ = fs::remove_dir_all(&tmp);
        return Err(Error::Other(t!("zip に {exe} が入っていませんでした", exe = EXE)));
    }
    if let Err(e) = check(&tmp.join(EXE)) {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    // 取り出しが全部終わってから差し替える (途中で失敗しても前の版が残る)
    let _ = fs::remove_dir_all(dest);
    fs::rename(&tmp, dest)?;
    Ok(dest.join(EXE))
}

/// ダウンロード → ハッシュ照合 → 取り出し。`progress(受信バイト, 全体)`。戻り値は ffmpeg の場所
pub fn install(
    asset: &Asset,
    dest: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    let parent = dest.parent().unwrap_or(dest);
    fs::create_dir_all(parent)?;
    let part = parent.join(format!("{}.part", asset.name));
    let result = (|| {
        let resp = agent(ZIP_BODY_TIMEOUT).get(&asset.url).call().map_err(net_err)?;
        // 大きさが前もってわからなかったとき (macOS の配布元) は、応答のヘッダから
        let total = match asset.size {
            0 => resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0),
            n => n,
        };
        let body = resp.into_body().into_with_config().limit(MAX_ZIP_BYTES).reader();
        let mut out = fs::File::create(&part)?;
        let mut hasher = Sha256::new();
        let mut got = 0u64;
        // 止まったら (STALL の間なにも届かなければ) やめる。取りやめは届かない間も見る
        read_watched(body, STALL, |data| {
            if cancel.load(Ordering::SeqCst) {
                return Err(Error::Other(t!("中止しました")));
            }
            let Some(data) = data else { return Ok(()) };
            out.write_all(data).map_err(|e| Error::Other(t!("I/O エラー: {e}", e = e)))?;
            hasher.update(data);
            got += data.len() as u64;
            progress(got, total);
            Ok(())
        })
        .map_err(|e| match e {
            Error::Io(e) => net_err(e),
            e => e,
        })?;
        out.sync_all()?;
        drop(out);
        let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if hash != asset.sha256 {
            return Err(Error::Other(
                t!("ダウンロードしたファイルが配布元のチェックサムと一致しませんでした。もう一度お試しください"),
            ));
        }
        extract_checked(&part, dest, verify_signature)
    })();
    let _ = fs::remove_file(&part);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_newest_stable_shared_build() {
        let names = [
            "checksums.sha256",
            "ffmpeg-master-latest-win64-lgpl-shared.zip",
            "ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip",
            "ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip",
            "ffmpeg-n9.0-latest-win64-lgpl-9.0.zip",
            "ffmpeg-n10.0-latest-win64-gpl-shared-10.0.zip",
        ];
        assert_eq!(pick_asset(&names).as_deref(), Some("ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip"));
        assert_eq!(
            pick_asset(&["ffmpeg-master-latest-win64-lgpl-shared.zip"]).as_deref(),
            Some("ffmpeg-master-latest-win64-lgpl-shared.zip")
        );
        assert_eq!(pick_asset(&["ffmpeg-n9.0-latest-win64-gpl-shared-9.0.zip"]), None);
    }

    #[test]
    fn reads_checksum_file() {
        let h = "a".repeat(64);
        let text = format!("{}  other.zip\n{h}  ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip\n", "b".repeat(64));
        assert_eq!(checksum_for(&text, "ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip"), Some(h));
        assert_eq!(checksum_for(&text, "missing.zip"), None);
    }

    #[test]
    fn joins_redirect_location() {
        let base = "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip";
        assert_eq!(
            absolute_url(base, "/download/macos/arm64/1_9.0.2/ffmpeg.zip"),
            "https://ffmpeg.martin-riedl.de/download/macos/arm64/1_9.0.2/ffmpeg.zip"
        );
        assert_eq!(absolute_url(base, "https://example.com/a.zip"), "https://example.com/a.zip");
    }

    /// 転送先は配布元の https だけ (http・別のサーバーは落とさない)
    #[test]
    fn mac_location_must_be_origin_https() {
        let base = "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip";
        assert_eq!(
            mac_download_url(base, "/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip").unwrap(),
            "https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip"
        );
        assert!(mac_download_url(base, "https://ffmpeg.martin-riedl.de/download/macos/arm64/1_9.0.2/ffmpeg.zip").is_ok());
        for bad in [
            "http://ffmpeg.martin-riedl.de/download/macos/arm64/1_9.0.2/ffmpeg.zip",
            "https://example.com/ffmpeg.zip",
            "https://ffmpeg.martin-riedl.de.example.com/ffmpeg.zip",
            "https://ffmpeg.martin-riedl.de@example.com/ffmpeg.zip",
            "HTTP://ffmpeg.martin-riedl.de/ffmpeg.zip",
            "ftp://ffmpeg.martin-riedl.de/ffmpeg.zip",
            "/download/a b.zip",
        ] {
            assert!(mac_download_url(base, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn reads_codesign_team() {
        let text = "Executable=/x/ffmpeg\nIdentifier=ffmpeg\nAuthority=Developer ID Application: Martin Riedl (KU3N25YGLU)\nTeamIdentifier=KU3N25YGLU\n";
        assert_eq!(team_identifier(text), Some("KU3N25YGLU"));
        assert_eq!(team_identifier("TeamIdentifier=not set\n"), Some("not set"));
        assert_eq!(team_identifier("Identifier=ffmpeg\n"), None);
    }

    /// 確かめられなければ前の版を残す
    #[cfg(unix)]
    #[test]
    fn keeps_old_build_when_check_fails() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("ffmpeg.zip");
        let mut w = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        w.start_file("ffmpeg", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b"new").unwrap();
        w.finish().unwrap();
        let dest = dir.path().join("ffmpeg");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("ffmpeg"), b"old").unwrap();
        assert!(extract_checked(&zip_path, &dest, |_| Err(Error::Other("unsigned".into()))).is_err());
        assert_eq!(fs::read(dest.join("ffmpeg")).unwrap(), b"old");
        assert!(!dest.with_extension("tmp").exists());
    }

    #[test]
    fn mac_arch_names() {
        assert_eq!(mac_arch("aarch64"), "arm64");
        assert_eq!(mac_arch("x86_64"), "amd64");
    }

    /// macOS の配布元 (zip の直下に ffmpeg だけ) から取り出せて、実行できる権限が付く
    #[cfg(unix)]
    #[test]
    fn extracts_mac_zip() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("zenpod-ffzip-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("ffmpeg.zip");
        let mut w = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        w.start_file("ffmpeg", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b"binary").unwrap();
        w.finish().unwrap();
        let exe = extract(&zip_path, &dir.join("ffmpeg")).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"binary");
        assert_eq!(fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        let _ = fs::remove_dir_all(&dir);
    }

    /// ネットワークを使う: `cargo test -p ipod-core ffinstall -- --ignored`
    #[test]
    #[ignore]
    fn resolves_mac_download() {
        for arch in ["aarch64", "x86_64"] {
            let a = latest_mac_asset(arch).unwrap();
            assert!(a.url.contains(&format!("/download/macos/{}/", mac_arch(arch))), "{}", a.url);
            assert_eq!(a.sha256.len(), 64);
            assert!(a.url.ends_with("/ffmpeg.zip"), "{}", a.url);
        }
        // 実際に落として照合・取り出しまで (約 30MB)。取り出しの規則が違う Windows では確かめない
        if !cfg!(windows) {
            let dir = std::env::temp_dir().join(format!("zenpod-ffdl-{}", std::process::id()));
            let mut last = (0, 0);
            let exe = install(&latest_mac_asset("aarch64").unwrap(), &dir.join("ffmpeg"), &AtomicBool::new(false), |g, t| last = (g, t)).unwrap();
            assert!(fs::metadata(&exe).unwrap().len() > 10 << 20);
            assert!(last.1 > 0 && last.0 == last.1, "{last:?}");
            let _ = fs::remove_dir_all(&dir);
        }
    }
}
