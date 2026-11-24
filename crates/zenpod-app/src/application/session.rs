//! 開いている iPod (1 台まで) と、それを使う処理どうしの順番

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ipod_core::library::Library;

use super::ports::EventSink;
use super::rockbox_db::{RockboxDbJob, RockboxDbWorker};
use super::rockbox_reread::RockboxRereader;
use super::snapshot::{MediaDirs, Snapshot};
use crate::error::{AppError, AppResult};

pub struct LibrarySession {
    library: Arc<Mutex<Option<Library>>>,
    /// 転送・取り出しなど、終わるまでほかの変更を受け付けない処理の最中
    busy: Arc<AtomicBool>,
    /// iPod の曲のジャケット ((画像 ID, 大きい方か) → data URL)。iPod を開き直したり、
    /// 曲・ジャケットを変えたりしたら空にする (画像 ID が別の画像を指すことがあるため)
    artwork: Mutex<HashMap<(u32, bool), Arc<String>>>,
    /// 動画と写真の一覧の画像 ((ID, 大きい方か) → data URL)。ジャケットと一緒に空にする
    /// (iPod OS の写真の ID は保存のたびに振り直すため)
    visual: Mutex<HashMap<(u64, bool), Arc<String>>>,
    rbdb: Arc<RockboxDbWorker>,
    /// データベースから埋めた曲を裏でファイルから読み直す (Rockbox を初めて開いたとき)
    reread: RockboxRereader,
    /// 前に数えた写真・動画のフォルダの大きさ。曲の操作 (`edit`) のあとはこれを使い、
    /// 写真・動画を変えうる操作 (`edit_playlists`・転送など busy の処理) と開き直しで捨てる
    media: Mutex<Option<MediaDirs>>,
}

/// 終わるまでほかの変更を受け付けない処理の間だけ持つ (落とすと受け付けに戻る)
pub struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl LibrarySession {
    pub fn new(events: Arc<dyn EventSink>) -> Self {
        let library = Arc::new(Mutex::new(None));
        let busy: Arc<AtomicBool> = Arc::default();
        LibrarySession {
            rbdb: Arc::new(RockboxDbWorker::new(library.clone(), events.clone())),
            reread: RockboxRereader::new(library.clone(), busy.clone(), events),
            library,
            busy,
            artwork: Mutex::default(),
            visual: Mutex::default(),
            media: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Library>> {
        self.library.lock().unwrap()
    }

    /// 開いた iPod をこのセッションのものにする (前の iPod は閉じる)
    /// Rockbox のデータベースから埋めた曲があれば、裏でファイルから読み直し始める
    pub fn install(&self, lib: Library) {
        self.reread.stop();
        self.clear_artwork_cache();
        self.forget_media();
        let job = lib.rockbox_reread_job();
        *self.lock() = Some(lib);
        self.reread.start(job);
    }

    pub fn close(&self) {
        self.reread.stop();
        *self.lock() = None;
        self.clear_artwork_cache();
        self.forget_media();
    }

    /// 裏の読み直しを止める (取り出す前に。iPod のファイルを開いたままにしない)
    pub fn stop_reread(&self) {
        self.reread.stop();
    }

    /// 開いている iPod を閉じる。ただし別の iPod に替わっていたら何もしない
    pub fn close_if_root(&self, root: &Path) {
        let mut guard = self.lock();
        if guard.as_ref().is_some_and(|i| i.root() == root) {
            *guard = None;
            self.forget_media();
        }
    }

    /// 曲の一覧と容量の内訳を含む状態。写真・動画のフォルダも数え直す (転送のあとなどに画面が呼ぶ)
    pub fn snapshot(&self) -> Option<Snapshot> {
        let guard = self.lock();
        let lib = guard.as_ref()?;
        let media = MediaDirs::measure(lib);
        // 転送などの途中なら覚えない (このあとも写真・動画が増える)
        if !self.is_busy() {
            *self.media.lock().unwrap() = Some(media);
        }
        Some(Snapshot::full_with(lib, media))
    }

    /// 前に数えた写真・動画のフォルダの大きさ (無ければ数えて覚える)
    fn media_dirs(&self, lib: &Library) -> MediaDirs {
        *self.media.lock().unwrap().get_or_insert_with(|| MediaDirs::measure(lib))
    }

    fn forget_media(&self) {
        *self.media.lock().unwrap() = None;
    }

    /// 開いている iPod がまだ接続されているか。
    /// 2.5 秒ごとに呼ばれる。ほかの処理 (保存・転送など) が iPod を使っているあいだは待たずに「ある」と答える
    /// (待つと画面ごと止まる。外されていれば、次の確認で分かる)
    pub fn present(&self) -> bool {
        let root = match self.library.try_lock() {
            Ok(g) => g.as_ref().map(|i| i.root().to_path_buf()),
            Err(_) => return true,
        };
        // Rockbox だけを入れた iPod には iPod_Control が無い
        root.is_some_and(|r| r.join("iPod_Control").is_dir() || r.join(".rockbox").is_dir())
    }

    /// 開いている iPod を読む。開いていなければ `NotConnected`
    pub fn read<T>(&self, f: impl FnOnce(&Library) -> AppResult<T>) -> AppResult<T> {
        f(self.lock().as_ref().ok_or(AppError::NotConnected)?)
    }

    /// 開いている iPod を変える (保存はしない)。開いていなければ `NotConnected`
    pub fn write<T>(&self, f: impl FnOnce(&mut Library) -> AppResult<T>) -> AppResult<T> {
        f(self.lock().as_mut().ok_or(AppError::NotConnected)?)
    }

    /// 開いていれば読む。開いていなければ None
    pub fn try_read<T>(&self, f: impl FnOnce(&Library) -> T) -> Option<T> {
        self.lock().as_ref().map(f)
    }

    /// 開いていれば変える (保存はしない)。開いていなければ None
    pub fn try_write<T>(&self, f: impl FnOnce(&mut Library) -> T) -> Option<T> {
        self.lock().as_mut().map(f)
    }

    /// 開いている iPod に対して操作 → 保存 → 曲の一覧を含む状態を返す。
    /// ジャケットのキャッシュは捨てる。Rockbox のデータベースの作り直しを頼む
    pub fn edit<T, E: Into<AppError>>(&self, f: impl FnOnce(&mut Library) -> Result<T, E>) -> AppResult<(T, Snapshot)> {
        if self.is_busy() {
            return Err(AppError::Busy);
        }
        let mut guard = self.lock();
        let lib = guard.as_mut().ok_or(AppError::NotConnected)?;
        let res = f(lib);
        self.clear_artwork_cache();
        let value = res.map_err(Into::into)?;
        if lib.is_dirty() {
            lib.save()?;
        }
        self.rbdb.request(lib.take_rockbox_db_request());
        // 曲の操作では写真・動画のフォルダは変わらないので数え直さない
        let media = self.media_dirs(lib);
        Ok((value, Snapshot::full_with(lib, media)))
    }

    /// `edit` と同じ。プレイリストだけを変える操作用で、曲の一覧は返さない
    pub fn edit_playlists<T, E: Into<AppError>>(&self, f: impl FnOnce(&mut Library) -> Result<T, E>) -> AppResult<(T, Snapshot)> {
        if self.is_busy() {
            return Err(AppError::Busy);
        }
        let mut guard = self.lock();
        let lib = guard.as_mut().ok_or(AppError::NotConnected)?;
        // 写真・動画の操作もここを通るので、次に容量の内訳を出すときは数え直す
        self.forget_media();
        let value = f(lib).map_err(Into::into)?;
        if lib.is_dirty() {
            lib.save()?;
        }
        Ok((value, Snapshot::playlists_only(lib)))
    }

    /// 終わるまでほかの変更を受け付けない処理を始める。ほかの処理の最中なら None
    pub fn try_begin_exclusive(&self) -> Option<BusyGuard> {
        let guard = (!self.busy.swap(true, Ordering::SeqCst)).then(|| BusyGuard(self.busy.clone()));
        // 転送などは写真・動画を足すことがあるので、次に容量の内訳を出すときは数え直す
        if guard.is_some() {
            self.forget_media();
        }
        guard
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    // ---- ジャケットのキャッシュ ----

    pub fn cached_artwork(&self, key: (u32, bool)) -> Option<Arc<String>> {
        self.artwork.lock().unwrap().get(&key).cloned()
    }

    pub fn remember_artwork(&self, key: (u32, bool), url: Arc<String>) {
        self.artwork.lock().unwrap().insert(key, url);
    }

    pub fn clear_artwork_cache(&self) {
        self.artwork.lock().unwrap().clear();
        self.visual.lock().unwrap().clear();
    }

    pub fn cached_visual(&self, key: (u64, bool)) -> Option<Arc<String>> {
        self.visual.lock().unwrap().get(&key).cloned()
    }

    pub fn remember_visual(&self, key: (u64, bool), url: Arc<String>) {
        self.visual.lock().unwrap().insert(key, url);
    }

    // ---- Rockbox のデータベース ----

    pub fn request_rockbox_db(&self, job: Option<RockboxDbJob>) {
        self.rbdb.request(job);
    }

    /// 頼まれている作り直しをすぐ行い、作っている最中なら終わるまで待つ (取り出しの前に)
    pub fn flush_rockbox_db(&self) {
        self.rbdb.flush();
    }

    /// 頼まれている分を取りやめて、今すぐ作り直す (手で作り直すとき)
    pub fn rebuild_rockbox_db_now(&self, root: &Path, cache: Option<&Path>) -> ipod_core::Result<ipod_core::rockbox::rbdb::Built> {
        self.rbdb.rebuild_now(root, cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::events::AppEvent;

    struct Silent;
    impl EventSink for Silent {
        fn emit(&self, _: AppEvent) {}
    }

    #[test]
    fn not_connected_and_busy() {
        let s = LibrarySession::new(Arc::new(Silent));
        assert!(matches!(s.read(|_| Ok(())), Err(AppError::NotConnected)));
        assert!(matches!(s.edit(|_| Ok::<_, AppError>(())), Err(AppError::NotConnected)));
        assert!(s.try_read(|_| ()).is_none());
        assert!(!s.present());
        let g = s.try_begin_exclusive().unwrap();
        assert!(s.try_begin_exclusive().is_none());
        assert!(matches!(s.edit(|_| Ok::<_, AppError>(())), Err(AppError::Busy)));
        assert!(matches!(s.edit_playlists(|_| Ok::<_, AppError>(())), Err(AppError::Busy)));
        drop(g);
        assert!(!s.is_busy());
        assert!(s.try_begin_exclusive().is_some());
    }
}
