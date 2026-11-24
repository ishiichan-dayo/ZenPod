//! PC の曲のジャケットの縮小画像 (data URL)。一覧用 (22px 表示) とアルバム表示用 (160px 表示)。
//! 曲に埋め込まれた画像か、同じフォルダの cover.jpg など。
//! 元画像のデコードと縮小は重いので、1 回で一覧用とアルバム表示用の両方を作る。
//! 作った画像は画像の内容 (SHA-1) をキーにして保存し、次の起動からは読むだけにする
//! (同じアルバムの曲は同じ画像なので 1 回で済む。画像を差し替えればキーが変わるので古いものは使われない)。
//! 曲に埋め込まれた画像は、曲の場所 → キーも保存し、次の起動では曲のファイルを開かずに縮小画像を読む
//! (キーを出すには元の画像を全部読む必要があり、1 曲 0.5〜1MB ほど読んでいた)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;

use super::ports::{ArtRef, ThumbStore};
use super::prefs::Prefs;

pub struct PcThumbs {
    pub small: String,
    pub large: String,
}

/// 高解像度の画面でもきれいに見えるよう、表示の 2 倍の大きさで作る
pub const THUMB_SIZES: [u32; 2] = [44, 320];

/// ファイルの大きさと更新日時 (読めなければ None)
type FileStamp = Option<(u64, std::time::SystemTime)>;

fn stamp(path: &Path) -> FileStamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

fn nanos(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

/// 曲の場所 → キーの対応を書き出すまで待つ時間 (続けて覚えた分をまとめて書く)
const REFS_SAVE_DELAY: Duration = Duration::from_millis(if cfg!(test) { 50 } else { 3000 });

pub struct PcArtwork {
    prefs: Arc<Prefs>,
    store: Arc<dyn ThumbStore>,
    /// 曲の場所 → (読んだときのファイルの大きさと更新日時, 画像のキー)。メモリ上だけに置く。None = ジャケットなし。
    /// ほかのソフトでジャケットを変えたら読み直すよう、大きさか更新日時が変わっていれば使わない
    by_path: Mutex<HashMap<PathBuf, (FileStamp, Option<String>)>>,
    /// 同じ画像 (同じアルバムの曲など) は 1 回だけ縮小する: 画像のキー → 縮小画像
    by_key: Mutex<HashMap<String, Arc<PcThumbs>>>,
    /// 曲に埋め込まれた画像の、曲の場所 → キー (保存して次の起動でも使う)。初めて使うときに読む
    refs: Arc<Mutex<Option<HashMap<String, ArtRef>>>>,
    /// refs を書き出す予定が入っているか
    refs_saving: Arc<AtomicBool>,
}

impl PcArtwork {
    pub fn new(prefs: Arc<Prefs>, store: Arc<dyn ThumbStore>) -> Self {
        PcArtwork {
            prefs,
            store,
            by_path: Mutex::default(),
            by_key: Mutex::default(),
            refs: Arc::default(),
            refs_saving: Arc::default(),
        }
    }

    /// 登録フォルダの曲のジャケット。読めた・無かったことを覚えておき、何度も読みに行かない
    pub fn thumbs(&self, path: &Path) -> Option<Arc<PcThumbs>> {
        let now = stamp(path);
        let known = self.by_path.lock().unwrap().get(path).cloned();
        if let Some((_, key)) = known.filter(|(was, _)| *was == now) {
            return key.and_then(|k| self.by_key.lock().unwrap().get(&k).cloned());
        }
        if !self.prefs.pc_scope().allows_audio(path) {
            return None;
        }
        let found = match self.saved_thumbs(path, now) {
            Some(hit) => Some(hit),
            None => self.thumbs_found(path).map(|(key, t, embedded)| {
                if let (true, Some((size, mtime))) = (embedded, now) {
                    self.remember_ref(path, ArtRef { size, mtime_ns: nanos(mtime), key: key.clone() });
                }
                (key, t)
            }),
        };
        // ジャケットが無い・読めない曲も覚えておき、何度も読みに行かない
        self.by_path.lock().unwrap().insert(path.to_path_buf(), (now, found.as_ref().map(|(k, _)| k.clone())));
        found.map(|(_, t)| t)
    }

    /// 曲のジャケットの縮小画像と、画像の中身のキー。読んでよい場所かは呼び出し側で確かめる
    /// (転送中の曲は、ユーザーが渡したファイルなので読んでよい)
    pub fn thumbs_of(&self, path: &Path) -> Option<(String, Arc<PcThumbs>)> {
        self.thumbs_found(path).map(|(key, t, _)| (key, t))
    }

    /// `thumbs_of` と同じ。3 つ目は曲に埋め込まれた画像か (false = 同じフォルダの cover.jpg など)
    fn thumbs_found(&self, path: &Path) -> Option<(String, Arc<PcThumbs>, bool)> {
        let (bytes, embedded) = ipod_core::media::cover::find_cover_src(path)?;
        let key = ipod_core::media::cover::content_key(&bytes);
        let hit = self.by_key.lock().unwrap().get(&key).cloned();
        if let Some(t) = hit {
            return Some((key, t, embedded));
        }
        let t = Arc::new(self.load_or_make(&key, &bytes)?);
        self.by_key.lock().unwrap().insert(key.clone(), t.clone());
        Some((key, t, embedded))
    }

    /// 前の起動までに覚えた、曲に埋め込まれた画像のキーで縮小画像を読む (曲のファイルは開かない)。
    /// 曲のファイルが変わっていたり、縮小画像が消えていたりすれば None
    fn saved_thumbs(&self, path: &Path, now: FileStamp) -> Option<(String, Arc<PcThumbs>)> {
        let (size, mtime) = now?;
        let key = {
            let mut refs = self.refs.lock().unwrap();
            let r = refs.get_or_insert_with(|| self.store.load_refs()).get(&*path.to_string_lossy())?;
            (r.size == size && r.mtime_ns == nanos(mtime)).then(|| r.key.clone())?
        };
        let hit = self.by_key.lock().unwrap().get(&key).cloned();
        if let Some(t) = hit {
            return Some((key, t));
        }
        let t = Arc::new(Self::to_urls(&self.store.load(&key, &THUMB_SIZES)?));
        self.by_key.lock().unwrap().insert(key.clone(), t.clone());
        Some((key, t))
    }

    fn remember_ref(&self, path: &Path, r: ArtRef) {
        let mut refs = self.refs.lock().unwrap();
        refs.get_or_insert_with(|| self.store.load_refs()).insert(path.to_string_lossy().into_owned(), r);
        drop(refs);
        self.save_refs_later();
    }

    /// 少し待ってから refs を書き出す (待っている間に覚えた分も一緒に書く)
    fn save_refs_later(&self) {
        if self.refs_saving.swap(true, Ordering::SeqCst) {
            return;
        }
        let (refs, saving, store) = (self.refs.clone(), self.refs_saving.clone(), self.store.clone());
        std::thread::spawn(move || {
            std::thread::sleep(REFS_SAVE_DELAY);
            // 先に印を下ろす (写したあとに覚えた分は、次の予定で書く)
            saving.store(false, Ordering::SeqCst);
            let copy = refs.lock().unwrap().clone();
            if let Some(r) = copy {
                store.save_refs(&r);
            }
        });
    }

    /// 曲のジャケットを書き換えたので、次は読み直す
    pub fn forget(&self, path: &Path) {
        self.by_path.lock().unwrap().remove(path);
        let removed = self.refs.lock().unwrap().as_mut().and_then(|r| r.remove(&*path.to_string_lossy())).is_some();
        if removed {
            self.save_refs_later();
        }
    }

    /// 画像の縮小画像 (一覧用とアルバム表示用) を、保存してあれば読み、無ければ作って保存する。
    /// 曲のジャケット以外 (ポッドキャストの番組の画像など) に使う。`key` は画像の中身から作る
    pub fn make_thumbs(&self, key: &str, bytes: &[u8]) -> Option<PcThumbs> {
        self.load_or_make(key, bytes)
    }

    /// 保存してある縮小画像を読む。無ければ作って保存する
    fn load_or_make(&self, key: &str, bytes: &[u8]) -> Option<PcThumbs> {
        let jpegs = match self.store.load(key, &THUMB_SIZES) {
            Some(j) => j,
            None => {
                let made = ipod_core::media::cover::thumbnails_jpeg(bytes, &THUMB_SIZES)?;
                self.store.save(key, &THUMB_SIZES, &made);
                made
            }
        };
        Some(Self::to_urls(&jpegs))
    }

    fn to_urls(jpegs: &[Vec<u8>]) -> PcThumbs {
        let url = |j: &[u8]| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(j));
        PcThumbs { small: url(&jpegs[0]), large: url(&jpegs[1]) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::prefs_file::PrefsFile;
    use crate::infrastructure::test_dirs::FixedDirs;
    use crate::infrastructure::thumb_dir::ThumbDir;

    /// ffmpeg がある環境だけ (Unix): 2 回目の起動では、曲に埋め込まれた画像の縮小画像を曲のファイルを開かずに出す。
    /// 曲のファイルが変われば読み直す。フォルダの cover.jpg は覚えない (曲のファイルが変わらなくても差し替わる)
    #[cfg(unix)]
    #[test]
    fn embedded_cover_is_found_without_opening_the_song_next_time() {
        use std::os::unix::fs::PermissionsExt;
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let d = tempfile::tempdir().unwrap();
        let music = d.path().join("music");
        let (embedded, plain) = (music.join("a/1.mp3"), music.join("b/1.mp3"));
        std::fs::create_dir_all(embedded.parent().unwrap()).unwrap();
        std::fs::create_dir_all(plain.parent().unwrap()).unwrap();
        let cover = d.path().join("c.png");
        image::RgbImage::from_pixel(64, 64, image::Rgb([10, 200, 30])).save(&cover).unwrap();
        std::fs::copy(&cover, music.join("b/cover.png")).unwrap();
        for (out, art) in [(&embedded, true), (&plain, false)] {
            let mut cmd = std::process::Command::new(&ff);
            cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1"]);
            if art {
                cmd.arg("-i").arg(&cover).args(["-map", "0", "-map", "1", "-c:v", "copy", "-disposition:v", "attached_pic"]);
            }
            assert!(cmd.args(["-c:a", "libmp3lame", "-id3v2_version", "3"]).arg(out).status().unwrap().success());
        }

        let dirs: Arc<dyn crate::application::ports::AppDirs> = Arc::new(FixedDirs::new(d.path().join("cache")));
        let prefs = Arc::new(Prefs::new(Arc::new(PrefsFile::new(dirs.clone()))));
        prefs.add_pc_folder(&music).unwrap();
        let store: Arc<dyn ThumbStore> = Arc::new(ThumbDir::new(dirs));
        let first = PcArtwork::new(prefs.clone(), store.clone());
        let want = first.thumbs(&embedded).unwrap().small.clone();
        assert!(first.thumbs(&plain).is_some());
        std::thread::sleep(REFS_SAVE_DELAY * 4);
        assert_eq!(store.load_refs().keys().cloned().collect::<Vec<_>>(), vec![embedded.to_string_lossy().into_owned()]);

        // 曲のファイルを読めなくしても、次の起動では同じ縮小画像が出る
        std::fs::set_permissions(&embedded, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&embedded).is_ok() {
            return; // root で動いていると読めてしまうので確かめられない
        }
        let second = PcArtwork::new(prefs.clone(), store.clone());
        assert_eq!(second.thumbs(&embedded).map(|t| t.small.clone()), Some(want));

        // 曲のファイルが変わったら、覚えたキーは使わない (読めないので None)
        std::fs::set_permissions(&embedded, std::fs::Permissions::from_mode(0o644)).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options().write(true).open(&embedded).unwrap().set_modified(later).unwrap();
        std::fs::set_permissions(&embedded, std::fs::Permissions::from_mode(0o000)).unwrap();
        let third = PcArtwork::new(prefs, store);
        assert!(third.thumbs(&embedded).is_none());
    }
}
