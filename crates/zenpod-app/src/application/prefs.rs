//! 画面の設定 (prefs.json)。
//! 以前は WebView の localStorage に置いていたが、WebView2 は書き込みを遅らせてまとめるため、
//! 終了時に WebView2 が落ちると直前の変更が消えることがあった。変更のたびに保存する

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ipod_core::t;

use super::ports::PrefsStore;
use crate::domain::pc_scope::PcScope;
use crate::error::{AppError, AppResult};

/// 登録フォルダ (PC の曲を触ってよい範囲。フォルダの場所の JSON の配列)
const PC_FOLDERS: &str = "pcFolders";
/// 手で選んだ ffmpeg の場所
const FFMPEG_PATH: &str = "ffmpegPath";
/// 画面からは `set` で書けないキー。値が Rust 側で場所や動かすプログラムとして使われるので、
/// 専用のコマンド (Rust が開くダイアログで選ぶ) だけで変える。ほかのキーは画面の表示の設定で、
/// 列の幅などキーが増え続けるので、許すキーを並べるのではなく危ないキーを並べる
const PROTECTED: &[&str] = &[PC_FOLDERS, FFMPEG_PATH];

pub struct Prefs {
    store: Arc<dyn PrefsStore>,
    /// 最初に読んだ内容を持ち、変更のたびに保存する
    cache: Mutex<Option<HashMap<String, String>>>,
}

impl Prefs {
    pub fn new(store: Arc<dyn PrefsStore>) -> Self {
        Prefs { store, cache: Mutex::default() }
    }

    fn with<T>(&self, f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
        let mut guard = self.cache.lock().unwrap();
        f(guard.get_or_insert_with(|| self.store.load()))
    }

    pub fn all(&self) -> HashMap<String, String> {
        self.with(|p| p.clone())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.with(|p| p.get(key).cloned())
    }

    /// 画面からの変更。`value` が None なら削除。場所などに使うキー (`PROTECTED`) は変えられない
    pub fn set(&self, key: String, value: Option<String>) -> AppResult<()> {
        if PROTECTED.contains(&key.as_str()) {
            return Err(AppError::msg(t!("この設定はここからは変えられません: {k}", k = key)));
        }
        self.put(key, value)
    }

    fn put(&self, key: String, value: Option<String>) -> AppResult<()> {
        self.with(|p| {
            match value {
                Some(v) => p.insert(key, v),
                None => p.remove(&key),
            };
            self.store.save(p)
        })
    }

    /// PC の曲を触ってよい範囲 (登録フォルダ)
    pub fn pc_scope(&self) -> PcScope {
        self.with(|p| PcScope::from_pref(p.get(PC_FOLDERS).map(String::as_str)))
    }

    /// 登録フォルダ (画面の並び順)
    pub fn pc_folders(&self) -> Vec<String> {
        self.get(PC_FOLDERS).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default()
    }

    fn save_pc_folders(&self, folders: &[String]) -> AppResult<()> {
        self.put(PC_FOLDERS.into(), Some(serde_json::to_string(folders)?))
    }

    /// フォルダを登録する (ダイアログで選んだもの。登録済みなら何もしない)
    pub fn add_pc_folder(&self, dir: &Path) -> AppResult<String> {
        let dir = dir.to_string_lossy().into_owned();
        let mut folders = self.pc_folders();
        if !folders.contains(&dir) {
            folders.push(dir.clone());
            self.save_pc_folders(&folders)?;
        }
        Ok(dir)
    }

    /// 登録を外す (フォルダの中身はそのまま)
    pub fn remove_pc_folder(&self, dir: &str) -> AppResult<()> {
        let mut folders = self.pc_folders();
        folders.retain(|f| f != dir);
        self.save_pc_folders(&folders)
    }

    /// 登録フォルダの並び順を変える。登録済みのフォルダをちょうど並べ替えたものだけ受け付ける
    pub fn reorder_pc_folders(&self, order: Vec<String>) -> AppResult<()> {
        let mut now = self.pc_folders();
        let mut sorted = order.clone();
        now.sort();
        sorted.sort();
        if now != sorted {
            return Err(AppError::msg(t!("登録したフォルダが変わっています。もう一度お試しください")));
        }
        self.save_pc_folders(&order)
    }

    /// 手で選んだ ffmpeg の場所 (無ければ自動で探す)
    pub fn ffmpeg_path(&self) -> Option<PathBuf> {
        self.get(FFMPEG_PATH).filter(|p| !p.is_empty()).map(PathBuf::from)
    }

    /// `path` は Rust で開いたダイアログで選んで、ffmpeg だと確かめたもの。None なら自動で探すのに戻す
    pub fn set_ffmpeg_path(&self, path: Option<&Path>) -> AppResult<()> {
        self.put(FFMPEG_PATH.into(), path.map(|p| p.to_string_lossy().into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory {
        saved: Mutex<Vec<HashMap<String, String>>>,
    }
    impl PrefsStore for Memory {
        fn load(&self) -> HashMap<String, String> {
            HashMap::from([("lang".into(), "en".into())])
        }
        fn save(&self, prefs: &HashMap<String, String>) -> AppResult<()> {
            self.saved.lock().unwrap().push(prefs.clone());
            Ok(())
        }
    }

    #[test]
    fn loads_once_and_saves_every_change() {
        let store = Arc::new(Memory::default());
        let prefs = Prefs::new(store.clone());
        assert_eq!(prefs.get("lang").as_deref(), Some("en"));
        prefs.set("theme".into(), Some("dark".into())).unwrap();
        prefs.set("lang".into(), None).unwrap();
        assert_eq!(prefs.all(), HashMap::from([("theme".into(), "dark".into())]));
        let saved = store.saved.lock().unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[1], prefs.all());
    }

    #[test]
    fn screen_cannot_write_paths() {
        let prefs = Prefs::new(Arc::new(Memory::default()));
        assert!(prefs.set("pcFolders".into(), Some("[\"/\"]".into())).is_err());
        assert!(prefs.set("ffmpegPath".into(), Some("/bin/sh".into())).is_err());
        assert!(prefs.set("pcFolders".into(), None).is_err());
        assert!(prefs.pc_folders().is_empty());
        assert_eq!(prefs.ffmpeg_path(), None);
    }

    #[test]
    fn pc_folders_add_remove_reorder() {
        let prefs = Prefs::new(Arc::new(Memory::default()));
        assert_eq!(prefs.add_pc_folder(Path::new("/a")).unwrap(), "/a");
        prefs.add_pc_folder(Path::new("/b")).unwrap();
        prefs.add_pc_folder(Path::new("/a")).unwrap();
        assert_eq!(prefs.pc_folders(), ["/a", "/b"]);
        prefs.reorder_pc_folders(vec!["/b".into(), "/a".into()]).unwrap();
        assert_eq!(prefs.pc_folders(), ["/b", "/a"]);
        // 並べ替えに見せかけて足す・入れ替えるのは受け付けない
        assert!(prefs.reorder_pc_folders(vec!["/b".into(), "/a".into(), "/".into()]).is_err());
        assert!(prefs.reorder_pc_folders(vec!["/b".into(), "/".into()]).is_err());
        assert!(prefs.reorder_pc_folders(vec!["/b".into(), "/b".into()]).is_err());
        prefs.remove_pc_folder("/b").unwrap();
        assert_eq!(prefs.pc_folders(), ["/a"]);
    }
}
