//! iPod の中のデータ (iTunesDB の曲の場所・ArtworkDB のファイル名・Rockbox の設定) に書いてある場所を PC の場所にする。
//! 中古の iPod や壊れた DB が ".." や "C:\…"・"\\server\share" を指していても、iPod のフォルダの外を触らないように、
//! ふつうの名前の部品だけを認める

use std::path::{Component, Path, PathBuf};

/// iTunesDB・ArtworkDB の区切り (":iPod_Control:Music:F00:ABCD.mp3")。"/" と "\" も区切りとみなす
pub const IPOD_SEPS: &[char] = &[':', '/', '\\'];
/// Rockbox の本体の場所 ("/.rockbox")。"\" も区切りとみなす (":" を含む部品は認めない)
pub const DEVICE_SEPS: &[char] = &['/', '\\'];

/// `rel` を `seps` で区切って `root` の下につなぐ。空の部品は飛ばす (部品が無ければ `root` そのもの)。
/// ".."・"."・ドライブ文字 ("C:")・NUL・Windows の装置名 (CON など) を含めば None
pub fn join_inside(root: &Path, rel: &str, seps: &[char]) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for part in rel.split(seps).filter(|s| !s.is_empty()) {
        if !is_plain_name(part) {
            return None;
        }
        p.push(part);
    }
    Some(p)
}

/// 1 つの部品として安全な名前か
fn is_plain_name(part: &str) -> bool {
    // "..." なども Windows では末尾の点が落ちて "." / ".." と同じになるので、点と空白だけの名前は認めない
    if part.trim_end_matches(['.', ' ']).is_empty() || part.contains(['\0', ':', '/', '\\']) || is_device_name(part) {
        return false;
    }
    // 念のため、その OS の決まりでも 1 つのふつうの名前になるか
    let mut c = Path::new(part).components();
    matches!((c.next(), c.next()), (Some(Component::Normal(n)), None) if n == part)
}

/// Windows の装置名 (拡張子が付いていても装置を指す)
fn is_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_plain_paths() {
        let root = Path::new("/ipod");
        let want = root.join("iPod_Control").join("Music").join("F00").join("ABCD.mp3");
        assert_eq!(join_inside(root, ":iPod_Control:Music:F00:ABCD.mp3", IPOD_SEPS), Some(want.clone()));
        assert_eq!(join_inside(root, "iPod_Control/Music\\F00/ABCD.mp3", IPOD_SEPS), Some(want));
        assert_eq!(join_inside(root, "/", DEVICE_SEPS), Some(root.to_path_buf()));
        assert_eq!(join_inside(root, "/.rockbox", DEVICE_SEPS), Some(root.join(".rockbox")));
        assert_eq!(join_inside(root, "/Music/a.b c", DEVICE_SEPS), Some(root.join("Music").join("a.b c")));
        assert_eq!(join_inside(root, "/Music/Console", DEVICE_SEPS), Some(root.join("Music").join("Console")));
    }

    #[test]
    fn rejects_escapes() {
        let root = Path::new("/ipod");
        for bad in [
            ":iPod_Control:..:..:etc:passwd",
            "..",
            "a/../../b",
            ":.:x",
            "...",
            "C:\\Windows\\win.ini",
            "\\\\?\\C:\\x",
            "a\0b",
            "Music/CON",
            "Music/com1.mp3",
            "Music/LPT9",
        ] {
            assert_eq!(join_inside(root, bad, DEVICE_SEPS), None, "{bad:?}");
        }
        for bad in [":iPod_Control:..:..:etc:passwd", "..\\..\\x", "a/./b", "nul.txt"] {
            assert_eq!(join_inside(root, bad, IPOD_SEPS), None, "{bad:?}");
        }
        // 絶対パス・UNC は区切りで割るので、iPod の中の名前になる
        assert_eq!(join_inside(root, "\\\\server\\share\\x", DEVICE_SEPS), Some(root.join("server").join("share").join("x")));
        assert_eq!(join_inside(root, "/etc/passwd", DEVICE_SEPS), Some(root.join("etc").join("passwd")));
    }
}
