//! 画面の設定を設定のフォルダの prefs.json に置く

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use ipod_core::t;

use crate::application::ports::{AppDirs, PrefsStore};
use crate::error::{AppError, AppResult};

pub struct PrefsFile {
    dirs: Arc<dyn AppDirs>,
}

impl PrefsFile {
    pub fn new(dirs: Arc<dyn AppDirs>) -> Self {
        PrefsFile { dirs }
    }

    fn path(&self) -> Option<PathBuf> {
        Some(self.dirs.config_dir()?.join("prefs.json"))
    }
}

impl PrefsStore for PrefsFile {
    fn load(&self) -> HashMap<String, String> {
        self.path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, prefs: &HashMap<String, String>) -> AppResult<()> {
        let path = self.path().ok_or_else(|| AppError::msg(t!("設定の保存先が決められません")))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // 書きかけのファイルが残らないよう、一時ファイルに書いてから置き換える
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(prefs)?)?;
        Ok(std::fs::rename(&tmp, &path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_dirs::FixedDirs;

    #[test]
    fn round_trip() {
        let d = tempfile::tempdir().unwrap();
        let store = PrefsFile::new(Arc::new(FixedDirs::new(d.path().join("config"))));
        assert!(store.load().is_empty());
        let prefs = HashMap::from([("lang".to_string(), "en".to_string())]);
        store.save(&prefs).unwrap();
        assert_eq!(store.load(), prefs);
        assert!(!d.path().join("config/prefs.json.tmp").exists());
    }

    #[test]
    fn no_dir_is_an_error() {
        let store = PrefsFile::new(Arc::new(FixedDirs::none()));
        assert!(store.load().is_empty());
        assert!(store.save(&HashMap::new()).is_err());
    }
}
