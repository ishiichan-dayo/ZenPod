//! PC の曲のジャケットの縮小画像を、キャッシュのフォルダの art-thumbs に「キー-大きさ.jpg」で置く。
//! 曲の場所 → 埋め込みのジャケットのキーの対応は pc-art-refs.json に置く

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::application::ports::{AppDirs, ArtRef, ThumbStore};

pub struct ThumbDir {
    dirs: Arc<dyn AppDirs>,
}

impl ThumbDir {
    /// 90 日使っていないものは消す
    const MAX_AGE: Duration = Duration::from_secs(90 * 24 * 60 * 60);
    /// それでも合計がこれを超えていたら、使っていない期間の長いものから消す
    const MAX_BYTES: u64 = 200 * 1024 * 1024;

    pub fn new(dirs: Arc<dyn AppDirs>) -> Self {
        ThumbDir { dirs }
    }

    fn dir(&self) -> Option<PathBuf> {
        Some(self.dirs.cache_dir()?.join("art-thumbs"))
    }

    fn refs_file(&self) -> Option<PathBuf> {
        Some(self.dirs.cache_dir()?.join("pc-art-refs.json"))
    }

    fn files(&self, key: &str, sizes: &[u32]) -> Option<Vec<PathBuf>> {
        let dir = self.dir()?;
        Some(sizes.iter().map(|s| dir.join(format!("{key}-{s}.jpg"))).collect())
    }

    /// 古いものを消す (起動時に画面と別のスレッドで)
    pub fn prune(&self) {
        let Some(dir) = self.dir() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let now = SystemTime::now();
        let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                m.is_file().then(|| (m.modified().unwrap_or(now), m.len(), e.path()))
            })
            .collect();
        files.retain(|(t, _, p)| {
            let old = now.duration_since(*t).is_ok_and(|age| age > Self::MAX_AGE);
            if old {
                let _ = std::fs::remove_file(p);
            }
            !old
        });
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.0);
        let mut removed = 0;
        for (_, len, p) in &files {
            if total <= Self::MAX_BYTES {
                break;
            }
            if std::fs::remove_file(p).is_ok() {
                total -= len;
            }
            removed += 1;
        }
        // 縮小画像を消したキーは、曲の場所との対応からも外す (対応は消えたファイルの分だけ増え続けないように)
        let kept: HashSet<&str> = files[removed..]
            .iter()
            .filter_map(|(_, _, p)| p.file_stem()?.to_str()?.rsplit_once('-').map(|(k, _)| k))
            .collect();
        let mut refs = self.load_refs();
        let before = refs.len();
        refs.retain(|_, r| kept.contains(r.key.as_str()));
        if refs.len() != before {
            self.save_refs(&refs);
        }
    }
}

impl ThumbStore for ThumbDir {
    fn load(&self, key: &str, sizes: &[u32]) -> Option<Vec<Vec<u8>>> {
        let files = self.files(key, sizes)?;
        let jpegs: Vec<Vec<u8>> = files.iter().map(|p| std::fs::read(p).ok()).collect::<Option<_>>()?;
        // 使った日時を更新しておく (長く使っていないものから消すため。起動ごとに 1 回だけ)
        for f in &files {
            let _ = std::fs::File::options().write(true).open(f).and_then(|file| file.set_modified(SystemTime::now()));
        }
        Some(jpegs)
    }

    fn save(&self, key: &str, sizes: &[u32], jpegs: &[Vec<u8>]) {
        let (Some(dir), Some(files)) = (self.dir(), self.files(key, sizes)) else {
            return;
        };
        let _ = std::fs::create_dir_all(dir);
        for (f, jpg) in files.iter().zip(jpegs) {
            let _ = std::fs::write(f, jpg);
        }
    }

    fn load_refs(&self) -> HashMap<String, ArtRef> {
        self.refs_file()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_refs(&self, refs: &HashMap<String, ArtRef>) {
        let Some(p) = self.refs_file() else {
            return;
        };
        let Ok(json) = serde_json::to_vec(refs) else {
            return;
        };
        // 書きかけで落ちても前のものが残るように、別の名前に書いてから置き換える
        let tmp = p.with_extension("json.tmp");
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_dirs::FixedDirs;

    #[test]
    fn needs_every_size() {
        let d = tempfile::tempdir().unwrap();
        let store = ThumbDir::new(Arc::new(FixedDirs::new(d.path().to_path_buf())));
        assert!(store.load("k", &[1, 2]).is_none());
        store.save("k", &[1, 2], &[b"a".to_vec(), b"bb".to_vec()]);
        assert_eq!(store.load("k", &[1, 2]).unwrap(), vec![b"a".to_vec(), b"bb".to_vec()]);
        std::fs::remove_file(d.path().join("art-thumbs/k-2.jpg")).unwrap();
        assert!(store.load("k", &[1, 2]).is_none());
    }

    #[test]
    fn prune_removes_old_files() {
        let d = tempfile::tempdir().unwrap();
        let store = ThumbDir::new(Arc::new(FixedDirs::new(d.path().to_path_buf())));
        store.save("old", &[1], &[b"x".to_vec()]);
        store.save("new", &[1], &[b"y".to_vec()]);
        let old = d.path().join("art-thumbs/old-1.jpg");
        let long_ago = SystemTime::now() - ThumbDir::MAX_AGE - Duration::from_secs(60);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(long_ago).unwrap();
        let r = |key: &str| ArtRef { size: 1, mtime_ns: 2, key: key.into() };
        store.save_refs(&HashMap::from([("/a.mp3".to_string(), r("old")), ("/b.mp3".to_string(), r("new"))]));
        store.prune();
        assert!(!old.exists());
        assert!(d.path().join("art-thumbs/new-1.jpg").exists());
        // 縮小画像を消したキーの対応も消える
        assert_eq!(store.load_refs(), HashMap::from([("/b.mp3".to_string(), r("new"))]));
    }
}
