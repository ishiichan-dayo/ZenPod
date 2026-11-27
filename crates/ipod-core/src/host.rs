//! アプリの外のプログラム (ffmpeg・gio・xdg-open など) を起動するための `Command`。
//!
//! Linux の AppImage は、同梱のライブラリを読むように `LD_LIBRARY_PATH` や GStreamer・GTK・GIO の
//! 変数に AppImage の中の場所 (`$APPDIR/...`) を入れて起動する。外のプログラムがそれを引き継ぐと、
//! システムのプログラムが同梱の古いライブラリを読んで起動できない
//! (`/usr/bin/ffmpeg: symbol lookup error: ... undefined symbol: vaMapBuffer2` など)。
//! AppImage で動いているときは、環境変数から AppImage の中の場所を取り除いてから起動する。
//! 先行版の更新では古い版のアプリが新しい版を起動するので、古い版の AppImage の場所 (もう別の
//! `/tmp/.mount_ZenPod…`) も残っている。自分の場所だけでなく、AppImage の中の場所はすべて取り除く。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// 外のプログラムを起動する `Command`。AppImage で動いているときは AppImage の中を指す環境変数を外す
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    if let Some(dir) = appimage_dir() {
        for (key, value) in cleaned_env(std::env::vars_os(), &dir) {
            match value {
                Some(v) => cmd.env(key, v),
                None => cmd.env_remove(key),
            };
        }
    }
    cmd
}

/// AppImage の中身の場所。AppImage の起動では `APPDIR` が入る。展開して `AppRun` から起動したときは
/// 入らないことがあるので、実行ファイルが `<場所>/usr/bin/` にあり `<場所>/AppRun` があればその場所
fn appimage_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    if let Some(d) = std::env::var_os("APPDIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?.parent()?;
    (exe.parent()?.ends_with("usr/bin") && dir.join("AppRun").exists()).then(|| dir.to_path_buf())
}

/// `dir` の中とほかの AppImage の中 (`is_appimage_path`) を指す項目を取り除いた環境変数 (変える変数だけ)。
/// `:` 区切りの一覧はその項目だけを除き、残りが無ければ変数ごと外す (None)
fn cleaned_env(vars: impl Iterator<Item = (OsString, OsString)>, dir: &Path) -> Vec<(OsString, Option<String>)> {
    let Some(dir) = dir.to_str().map(|d| d.trim_end_matches('/')) else {
        return Vec::new();
    };
    if dir.is_empty() {
        return Vec::new();
    }
    let inside =
        |p: &str| p == dir || p.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/')) || is_appimage_path(p);
    let mut out = Vec::new();
    for (key, value) in vars {
        let Some(v) = value.to_str() else { continue };
        if !v.split(':').any(inside) {
            continue;
        }
        let kept: Vec<&str> = v.split(':').filter(|p| !p.is_empty() && !inside(p)).collect();
        out.push((key, (!kept.is_empty()).then(|| kept.join(":"))));
    }
    out
}

/// AppImage の中の場所か。AppImage は `$TMPDIR/.mount_<名前><乱数>` に組み込まれ、
/// `--appimage-extract-and-run` では `$TMPDIR/appimage_extracted_<ハッシュ>` に展開される
fn is_appimage_path(p: &str) -> bool {
    p.starts_with('/') && p.split('/').any(|c| c.starts_with(".mount_") || c.starts_with("appimage_extracted_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect()
    }

    #[test]
    fn removes_only_entries_inside_appimage() {
        let dir = Path::new("/tmp/.mount_ZenPodAb12");
        let vars = env(&[
            ("LD_LIBRARY_PATH", "/tmp/.mount_ZenPodAb12/usr/lib:/tmp/.mount_ZenPodAb12/usr/lib/x86_64-linux-gnu"),
            ("PATH", "/tmp/.mount_ZenPodAb12/usr/bin:/usr/local/bin:/usr/bin"),
            ("XDG_DATA_DIRS", "/tmp/.mount_ZenPodAb12/usr/share/:/usr/share"),
            ("GST_PLUGIN_SCANNER", "/tmp/.mount_ZenPodAb12/usr/lib/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner"),
            ("APPDIR", "/tmp/.mount_ZenPodAb12"),
            // AppImage の外は残す
            ("OTHER", "/opt/lib:/usr/lib/x86_64-linux-gnu"),
            ("HOME", "/home/user"),
        ]);
        let got = cleaned_env(vars.into_iter(), dir);
        let get = |k: &str| got.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("LD_LIBRARY_PATH"), Some(None));
        assert_eq!(get("PATH"), Some(Some("/usr/local/bin:/usr/bin".into())));
        assert_eq!(get("XDG_DATA_DIRS"), Some(Some("/usr/share".into())));
        assert_eq!(get("GST_PLUGIN_SCANNER"), Some(None));
        assert_eq!(get("APPDIR"), Some(None));
        assert_eq!(get("OTHER"), None);
        assert_eq!(get("HOME"), None);
    }

    #[test]
    fn removes_entries_of_older_appimage() {
        // 先行版の更新: 古い版 (Old123) が新しい版 (New456) を起動し、古い版の場所が後ろに残っている
        let vars = env(&[
            ("LD_LIBRARY_PATH", "/tmp/.mount_ZenPodNew456/usr/lib:/tmp/.mount_ZenPodOld123/usr/lib"),
            ("GST_PLUGIN_PATH_1_0", "/tmp/.mount_ZenPodOld123/usr/lib/gstreamer-1.0"),
            ("PATH", "/tmp/appimage_extracted_0a1b2c/usr/bin:/usr/bin"),
            ("MOUNTS", "/home/user/mount_points:/srv/.mountain"),
        ]);
        let got = cleaned_env(vars.into_iter(), Path::new("/tmp/.mount_ZenPodNew456"));
        let get = |k: &str| got.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("LD_LIBRARY_PATH"), Some(None));
        assert_eq!(get("GST_PLUGIN_PATH_1_0"), Some(None));
        assert_eq!(get("PATH"), Some(Some("/usr/bin".into())));
        assert_eq!(get("MOUNTS"), None);
    }

    #[test]
    fn trailing_slash_in_appdir() {
        // 名前が似ているだけの場所 (/ab) は残す
        let got = cleaned_env(env(&[("LD_LIBRARY_PATH", "/a/usr/lib:/ab/lib:/usr/lib")]).into_iter(), Path::new("/a/"));
        assert_eq!(got, vec![(OsString::from("LD_LIBRARY_PATH"), Some("/ab/lib:/usr/lib".into()))]);
    }
}
