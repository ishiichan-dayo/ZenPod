//! MusicBrainz で調べたアーティストの別名を、キャッシュのフォルダの artist-aliases.json に置く

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::application::ports::{AliasStore, AppDirs};

pub struct AliasFile {
    dirs: Arc<dyn AppDirs>,
}

impl AliasFile {
    pub fn new(dirs: Arc<dyn AppDirs>) -> Self {
        AliasFile { dirs }
    }

    fn path(&self) -> Option<PathBuf> {
        Some(self.dirs.cache_dir()?.join("artist-aliases.json"))
    }
}

impl AliasStore for AliasFile {
    fn load(&self) -> HashMap<String, Vec<String>> {
        self.path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// 保存できなくても次回また調べるだけなので、失敗は無視する
    fn save(&self, aliases: &HashMap<String, Vec<String>>) {
        if let Some(p) = self.path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_vec(aliases) {
                let _ = std::fs::write(p, json);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_dirs::FixedDirs;

    #[test]
    fn round_trip() {
        let d = tempfile::tempdir().unwrap();
        let store = AliasFile::new(Arc::new(FixedDirs::new(d.path().join("cache"))));
        assert!(store.load().is_empty());
        let a = HashMap::from([("宇多田ヒカル".to_string(), vec!["Hikaru Utada".to_string()]), ("x".to_string(), vec![])]);
        store.save(&a);
        assert_eq!(store.load(), a);
    }
}
