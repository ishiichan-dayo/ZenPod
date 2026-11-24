//! Rockbox のデータベース (tagcache) を裏で作り直す。曲が多いと数秒かかるので、保存のたびにその場では作らない。
//! - 続けて頼まれたら、少し待ってまとめて 1 回にする (タグを 1 曲ずつ直したときなど)
//! - 作るのは同時に 1 つだけ (`lock`)。手で作り直すときも同じ lock を使う
//! - iPod を取り出す前には `flush` で必ず作り終える

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ipod_core::library::Library;

use super::events::{AppEvent, RockboxDbDone};
use super::ports::EventSink;

/// 作り直す iPod (場所, タグのキャッシュ)
pub type RockboxDbJob = (PathBuf, Option<PathBuf>);

pub(crate) struct RockboxDbWorker {
    /// 作り直したい iPod。新しい頼みで上書きする
    pending: Mutex<Option<RockboxDbJob>>,
    running: AtomicBool,
    lock: Mutex<()>,
    /// 作り終えたら、その iPod がまだ開いていれば失敗の理由を覚えさせる
    library: Arc<Mutex<Option<Library>>>,
    events: Arc<dyn EventSink>,
}

impl RockboxDbWorker {
    /// まとめる間の待ち時間
    const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(1500);

    pub fn new(library: Arc<Mutex<Option<Library>>>, events: Arc<dyn EventSink>) -> Self {
        RockboxDbWorker { pending: Mutex::default(), running: AtomicBool::new(false), lock: Mutex::default(), library, events }
    }

    pub fn request(self: &Arc<Self>, job: Option<RockboxDbJob>) {
        let Some(job) = job else { return };
        *self.pending.lock().unwrap() = Some(job);
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Self::DEBOUNCE);
                let job = me.pending.lock().unwrap().take();
                match job {
                    Some(job) => me.run(job),
                    None => {
                        me.running.store(false, Ordering::SeqCst);
                        // 止めた直後に頼まれていたら続ける
                        if me.pending.lock().unwrap().is_none() || me.running.swap(true, Ordering::SeqCst) {
                            break;
                        }
                    }
                }
            }
        });
    }

    fn run(&self, (root, cache): RockboxDbJob) {
        let _g = self.lock.lock().unwrap();
        let r = ipod_core::rockbox::rbdb::rebuild(&root, cache.as_deref());
        let error = r.as_ref().err().map(|e| e.to_string());
        if let Some(lib) = self.library.lock().unwrap().as_mut().filter(|l| l.root() == root) {
            lib.set_rockbox_db_error(error.clone());
        }
        self.events.emit(AppEvent::RockboxDbDone(RockboxDbDone { tracks: r.map(|b| b.tracks).unwrap_or(0), error }));
    }

    /// 頼まれている作り直しをすぐ行い、作っている最中なら終わるまで待つ
    pub fn flush(&self) {
        let job = self.pending.lock().unwrap().take();
        match job {
            Some(job) => self.run(job),
            None => drop(self.lock.lock().unwrap()),
        }
    }

    /// 頼まれている分を取りやめて、今すぐ作り直す (終わったことは知らせない。呼んだ側が結果を使う)
    pub fn rebuild_now(&self, root: &Path, cache: Option<&Path>) -> ipod_core::Result<ipod_core::rockbox::rbdb::Built> {
        let _g = self.lock.lock().unwrap();
        self.pending.lock().unwrap().take();
        ipod_core::rockbox::rbdb::rebuild(root, cache)
    }
}
