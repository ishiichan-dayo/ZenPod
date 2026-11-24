//! ポートの実装のうち Tauri が要らないもの

pub mod alias_file;
pub mod ffmpeg_transcoder;
pub mod online;
pub mod prefs_file;
pub mod thumb_dir;

#[cfg(test)]
pub(crate) mod test_dirs {
    use std::path::PathBuf;

    use crate::application::ports::AppDirs;

    /// テスト用: どのフォルダも同じ場所 (None なら決められない)
    pub struct FixedDirs(Option<PathBuf>);

    impl FixedDirs {
        pub fn new(dir: PathBuf) -> Self {
            FixedDirs(Some(dir))
        }

        pub fn none() -> Self {
            FixedDirs(None)
        }
    }

    impl AppDirs for FixedDirs {
        fn config_dir(&self) -> Option<PathBuf> {
            self.0.clone()
        }
        fn cache_dir(&self) -> Option<PathBuf> {
            self.0.clone()
        }
        fn local_data_dir(&self) -> Option<PathBuf> {
            self.0.clone()
        }
        fn resource_dir(&self) -> Option<PathBuf> {
            self.0.clone()
        }
    }
}
