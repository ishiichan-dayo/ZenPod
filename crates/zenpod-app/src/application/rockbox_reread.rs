//! Rockbox の iPod を初めて開くときにデータベースから埋めた曲を、裏でファイルから読み直す。
//! データベースには並べ替え用のタグと埋め込みのジャケットの有無が無いため。
//! - iPod を開いたあとに始め、1 曲ずつ順に読む。転送などの最中 (busy) は待つ
//! - 別の iPod を開く・閉じる・取り出すときは `stop` で止める (取り出すときにファイルを開いたままにしない)。
//!   `stop` は終わるまで待つので、iPod の lock を持ったまま呼ばない
//! - 読み終えたら一覧に反映し、`rockbox-reread-done` で画面に知らせる

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ipod_core::library::Library;

use super::events::AppEvent;
use super::ports::EventSink;

/// 読み直す iPod (場所, タグのキャッシュ, 曲のファイル)
pub type RockboxRereadJob = (PathBuf, Option<PathBuf>, Vec<PathBuf>);

pub(crate) struct RockboxRereader {
    /// 動いている読み直し (止める印, スレッド)
    running: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
    library: Arc<Mutex<Option<Library>>>,
    busy: Arc<AtomicBool>,
    events: Arc<dyn EventSink>,
}

impl RockboxRereader {
    pub fn new(library: Arc<Mutex<Option<Library>>>, busy: Arc<AtomicBool>, events: Arc<dyn EventSink>) -> Self {
        RockboxRereader { running: Mutex::default(), library, busy, events }
    }

    /// 読み直しを始める (前のものは止める)
    pub fn start(&self, job: Option<RockboxRereadJob>) {
        self.stop();
        let Some((root, cache, files)) = job else { return };
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, library, busy, events) = (stop.clone(), self.library.clone(), self.busy.clone(), self.events.clone());
        let handle = std::thread::spawn(move || {
            let read = ipod_core::pc::folder::reread_device_files(
                &files,
                cache.as_deref(),
                &root,
                &|| busy.load(Ordering::SeqCst),
                &|| flag.load(Ordering::SeqCst),
            );
            if read.is_empty() || flag.load(Ordering::SeqCst) {
                return;
            }
            let changed = library
                .lock()
                .unwrap()
                .as_mut()
                .filter(|l| l.root() == root)
                .map(|l| l.apply_rockbox_reread(&read))
                .unwrap_or(0);
            if changed > 0 {
                events.emit(AppEvent::RockboxReread(changed));
            }
        });
        *self.running.lock().unwrap() = Some((stop, handle));
    }

    /// 読み直しを止めて、終わるまで待つ (読んでいる 1 曲の分だけ)
    pub fn stop(&self) {
        let running = self.running.lock().unwrap().take();
        if let Some((stop, handle)) = running {
            stop.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
    }
}
