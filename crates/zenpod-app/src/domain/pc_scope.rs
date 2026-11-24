//! PC の曲のうち、アプリが再生・編集・名前の変更をしてよいもの = 登録フォルダ (設定の pcFolders) の中の音声ファイル

use std::path::{Path, PathBuf};

pub struct PcScope {
    /// 登録フォルダ (実際の場所に直したもの。無くなったフォルダは入れない)
    folders: Vec<PathBuf>,
}

impl PcScope {
    /// 設定の pcFolders (フォルダの場所の JSON の配列) から作る
    pub fn from_pref(json: Option<&str>) -> Self {
        let folders: Vec<String> = json.and_then(|v| serde_json::from_str(v).ok()).unwrap_or_default();
        PcScope { folders: folders.iter().filter_map(|f| Path::new(f).canonicalize().ok()).collect() }
    }

    /// 登録フォルダの中のファイルか (シンボリックリンクなどは実際の場所で比べる)
    pub fn contains(&self, path: &Path) -> bool {
        let Ok(path) = path.canonicalize() else {
            return false;
        };
        self.folders.iter().any(|f| path.starts_with(f))
    }

    /// 登録フォルダの中の音声ファイルか
    pub fn allows_audio(&self, path: &Path) -> bool {
        ipod_core::media::is_audio_file(path) && self.contains(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_inside_registered_folders() {
        let root = tempfile::tempdir().unwrap();
        let music = root.path().join("music");
        let other = root.path().join("other");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        for d in [&music, &other] {
            std::fs::write(d.join("a.mp3"), b"").unwrap();
            std::fs::write(d.join("a.txt"), b"").unwrap();
        }
        let json = serde_json::to_string(&[music.to_string_lossy()]).unwrap();
        let scope = PcScope::from_pref(Some(&json));
        assert!(scope.allows_audio(&music.join("a.mp3")));
        assert!(!scope.allows_audio(&music.join("a.txt")));
        assert!(!scope.allows_audio(&other.join("a.mp3")));
        // 「..」で外に出る場所も実際の場所で比べる
        assert!(!scope.allows_audio(&music.join("..").join("other").join("a.mp3")));
        assert!(!scope.allows_audio(&music.join("missing.mp3")));
        assert!(!PcScope::from_pref(None).contains(&music.join("a.mp3")));
        assert!(!PcScope::from_pref(Some("not json")).contains(&music.join("a.mp3")));
    }
}
