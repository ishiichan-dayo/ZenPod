//! iPod の曲を PC に書き出すときのファイル名

use std::path::{Path, PathBuf};

/// 「アーティスト - 曲名」(アーティストが空なら曲名だけ)。Windows で使えない字と制御文字は _ にする
pub fn export_stem(artist: &str, title: &str) -> String {
    let base = if artist.is_empty() { title.to_string() } else { format!("{artist} - {title}") };
    base.chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c })
        .collect()
}

/// `dir` の中でまだ使われていない名前。同じ名前があれば「名前 (2).ext」「名前 (3).ext」…
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut dst = dir.join(format!("{stem}.{ext}"));
    let mut k = 2;
    while dst.exists() {
        dst = dir.join(format!("{stem} ({k}).{ext}"));
        k += 1;
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_replaces_forbidden_chars() {
        assert_eq!(export_stem("", "Song"), "Song");
        assert_eq!(export_stem("AC/DC", "What? \"Yes\"\n"), "AC_DC - What_ _Yes__");
    }

    #[test]
    fn unique_path_counts_up() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(dir.path(), "a", "mp3"), dir.path().join("a.mp3"));
        std::fs::write(dir.path().join("a.mp3"), b"").unwrap();
        std::fs::write(dir.path().join("a (2).mp3"), b"").unwrap();
        assert_eq!(unique_path(dir.path(), "a", "mp3"), dir.path().join("a (3).mp3"));
    }
}
