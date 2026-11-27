//! 転送する曲のうち、先の曲をいくつか同時に変換しておく (配布リポジトリの Issue #29)。
//!
//! ffmpeg の AAC の変換は 1 つのスレッドでしか動かず、1 曲ずつ変換すると CPU の 1 コアしか使わない
//! (4 分の FLAC → AAC 256kbps で 6〜7 秒)。変換の係を何人か立てて、転送する順に先の曲を一時ファイルへ
//! 変換しておく。iPod への書き込みは今までどおり 1 曲ずつ順に行い、そのとき変換済みのものを使う。
//! 変換の係は iPod のデータ (重複の判定) を読むときだけ lock を取り、変換は lock の外で行う。
//! 先に進みすぎないよう、書き込み中の曲から `LOOKAHEAD` 曲先までしか変換しない (一時ファイルが溜まらないように)。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ipod_core::domain::track::AddOptions;
use ipod_core::media::preconvert::Preconverted;

use super::session::LibrarySession;

/// 同時に変換する曲の数の上限 (CPU の半分まで。ほかの操作や再生が重くならないように)
const MAX_WORKERS: usize = 4;

pub(super) struct Ahead {
    shared: Arc<Shared>,
    /// 曲か (動画・写真は変換の係を待たない)
    is_audio: Vec<bool>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// 次に変換の係が受け持つ番号
    next: usize,
    /// 書き込み中の番号 (ここから先 `lookahead` 曲までを変換する)
    current: usize,
    lookahead: usize,
    /// 終わった番号 → 変換したもの (変換しない曲・失敗した曲は None。書き込むときに今までどおり変換する)
    done: HashMap<usize, Option<Preconverted>>,
    /// まだ動いている変換の係の数
    workers: usize,
    stopped: bool,
}

impl Ahead {
    /// `audio` は転送する順の曲の場所 (曲でないものは None)
    pub(super) fn start(
        audio: Vec<Option<PathBuf>>,
        session: Arc<LibrarySession>,
        ffmpeg: Option<PathBuf>,
        opts: AddOptions,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        let count = audio.iter().filter(|a| a.is_some()).count();
        let workers = (cores / 2).clamp(1, MAX_WORKERS).min(count.max(1));
        let shared = Arc::new(Shared {
            state: Mutex::new(State { lookahead: workers * 2, workers, ..Default::default() }),
            changed: Condvar::new(),
        });
        let is_audio = audio.iter().map(Option::is_some).collect();
        let audio = Arc::new(audio);
        for _ in 0..workers {
            let (shared, audio, session, ffmpeg, cancel) =
                (shared.clone(), audio.clone(), session.clone(), ffmpeg.clone(), cancel.clone());
            std::thread::spawn(move || {
                // 途中で panic しても、書き込む側が待ち続けないように数を減らす
                let _alive = WorkerGuard(shared.clone());
                while let Some(i) = shared.claim(audio.len(), &cancel) {
                    let pre = audio[i].as_ref().and_then(|src| {
                        let plan = session.try_write(|lib| lib.conversion_plan(src, &opts)).and_then(Result::ok).flatten()?;
                        plan.run(src, ffmpeg.as_deref())
                            .map_err(|e| log::warn!("preconvert {}: {e}", src.display()))
                            .ok()
                    });
                    shared.finish(i, pre);
                }
            });
        }
        Ahead { shared, is_audio }
    }

    /// `i` 番目の変換を待って受け取る。変換しない曲・失敗した曲・曲でないものは None。
    /// 転送するものごとに順に呼ぶ (変換の係がどこまで先に進んでよいかが決まる)
    pub(super) fn take(&self, i: usize) -> Option<Preconverted> {
        let mut st = self.shared.state.lock().unwrap();
        st.current = i;
        self.shared.changed.notify_all();
        if !self.is_audio.get(i).copied().unwrap_or(false) {
            st.done.remove(&i);
            return None;
        }
        loop {
            if let Some(pre) = st.done.remove(&i) {
                return pre;
            }
            // 係がもういなければ (取り消し・panic) 待たない
            if st.workers == 0 || st.stopped {
                return None;
            }
            st = self.shared.changed.wait_timeout(st, Duration::from_millis(500)).unwrap().0;
        }
    }
}

impl Drop for Ahead {
    /// 転送が終わったら (取り消し・iPod が外れたときも) 係を止め、変換済みの一時ファイルを消す
    fn drop(&mut self) {
        let mut st = self.shared.state.lock().unwrap();
        st.stopped = true;
        st.done.clear();
        self.shared.changed.notify_all();
    }
}

impl Shared {
    /// 次に変換する番号。先に進みすぎていれば待つ。終わり・止められたら None
    fn claim(&self, len: usize, cancel: &AtomicBool) -> Option<usize> {
        let mut st = self.state.lock().unwrap();
        loop {
            if st.stopped || cancel.load(Ordering::SeqCst) || st.next >= len {
                return None;
            }
            if st.next < st.current + st.lookahead {
                st.next += 1;
                return Some(st.next - 1);
            }
            st = self.changed.wait_timeout(st, Duration::from_millis(500)).unwrap().0;
        }
    }

    fn finish(&self, i: usize, pre: Option<Preconverted>) {
        let mut st = self.state.lock().unwrap();
        // 止めたあとに終わった変換は捨てる (Drop で一時ファイルを消す)
        if !st.stopped {
            st.done.insert(i, pre);
        }
        self.changed.notify_all();
    }
}

struct WorkerGuard(Arc<Shared>);

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let mut st = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        st.workers -= 1;
        self.0.changed.notify_all();
    }
}
