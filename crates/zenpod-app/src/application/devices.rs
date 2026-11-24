//! iPod を探す・開く・取り出す

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ipod_core::library::{Library, Mode};
use ipod_core::device::{self, DeviceInfo};
use ipod_core::domain::track::TrackView;
use ipod_core::t;
use serde::Serialize;

use super::events::{AppEvent, OpenPartial};
use super::ports::{AppDirs, EventSink};
use super::session::LibrarySession;
use super::snapshot::Snapshot;
use crate::error::{AppError, AppResult};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    pub root: String,
    pub label: String,
}

pub struct Devices {
    session: Arc<LibrarySession>,
    dirs: Arc<dyn AppDirs>,
    events: Arc<dyn EventSink>,
    /// 「フォルダを指定して開く」で選んだ場所 (Rust で開いたダイアログで選んだもの。起動中だけ覚える)
    picked: Mutex<HashSet<PathBuf>>,
}

impl Devices {
    pub fn new(session: Arc<LibrarySession>, dirs: Arc<dyn AppDirs>, events: Arc<dyn EventSink>) -> Self {
        Devices { session, dirs, events, picked: Mutex::default() }
    }

    pub fn scan(&self) -> Vec<DeviceEntry> {
        device::find_ipod_roots()
            .into_iter()
            .map(|r| DeviceEntry {
                label: r
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| r.to_string_lossy().into_owned()),
                root: r.to_string_lossy().into_owned(),
            })
            .collect()
    }

    /// ダイアログで選んだフォルダを、画面から開いてよい場所にする
    pub fn allow_folder(&self, root: PathBuf) {
        self.picked.lock().unwrap().insert(root);
    }

    /// 画面から頼まれて開く。開いてよいのは、つながっている iPod (`scan` で見つかるもの) とダイアログで選んだフォルダだけ。
    /// 任意のフォルダを Rockbox モードで開くと、その中の曲を消したり名前を変えたりできてしまうため
    pub fn open_requested(&self, root: &Path, mode: Option<Mode>) -> AppResult<Snapshot> {
        let known = self.picked.lock().unwrap().contains(root) || device::is_ipod_root(root);
        if !known {
            return Err(AppError::msg(t!("開けない場所です: {p}", p = root.display())));
        }
        self.open(root, mode)
    }

    /// DB の読み込みに数秒かかることがあるので、画面と別のスレッドで呼ぶ。
    /// `mode` を省くと、Rockbox が入っている iPod は Rockbox モード、それ以外は iPod OS モードで開く
    pub fn open(&self, root: &Path, mode: Option<Mode>) -> AppResult<Snapshot> {
        let started = std::time::Instant::now();
        let r = self.open_inner(root, mode);
        match &r {
            Ok(s) => log::info!("opened: {} tracks ({} ms)", s.tracks.as_ref().map_or(0, Vec::len), started.elapsed().as_millis()),
            Err(e) => log::warn!("open failed ({} ms): {e}", started.elapsed().as_millis()),
        }
        r
    }

    fn open_inner(&self, root: &Path, mode: Option<Mode>) -> AppResult<Snapshot> {
        let mode = mode.unwrap_or(if ipod_core::rockbox::is_installed(root) { Mode::Rockbox } else { Mode::Ipod });
        log::info!("open: {} ({mode:?})", root.display());
        // 前に開いたときの裏の読み直しが、これから読む iPod のファイルと取り合わないように
        self.session.stop_reread();
        log::info!("stopped the background reread");
        // Rockbox モードは曲のタグをファイルから読むので、PC のフォルダと同じくキャッシュを使う
        // タグのキャッシュは iPod ごとのファイル (rockbox::cache_file)。前の版の共通のファイルは使わない (ドライブ文字込みで覚えていた)
        let cache = self.dirs.cache_dir();
        if let Some(d) = &cache {
            let _ = std::fs::remove_file(d.join("rockbox-tracks-v1.json"));
        }
        // 初めて Rockbox モードで開くときは全曲のタグを読むので、読んだ数を画面に知らせる
        let progress = |done: usize, total: usize| self.events.emit(AppEvent::OpenProgress(done, total));
        // 読めた曲から先に一覧に出す (Rockbox を初めて開くときは、全曲を読み終わるまで何十秒もかかることがある)。
        // 前に開いたことがあれば、iPod の中をたどり終わる前に前回の一覧を出す (`cached`)
        let partial = |device: &DeviceInfo, name: &str, tracks: Vec<TrackView>, cached: bool| {
            self.events.emit(AppEvent::OpenPartial(OpenPartial { device: device.clone(), name: name.to_string(), tracks, cached }));
        };
        let mut lib = Library::open_with_progress(root, mode, cache, &progress, &partial)?;
        log::info!("library loaded ({} tracks)", lib.tracks().len());
        // 以前の版が書いたアートワークを直して保存し直す (書き込めるときだけ):
        // - mhii を同じアルバムの曲で共有していて、iPod video 5G などでは 1 曲にしか表示されなかった → 曲ごとに分ける
        // - iPod classic の小さいジャケット (1061) が本体に出る形で書かれていなかった (Issue #12) → 作り直す
        if lib.check_writable().is_ok() {
            let fixed = lib.unshare_artwork() + lib.repair_artwork();
            if fixed > 0 {
                lib.save()?;
            }
        }
        let started = std::time::Instant::now();
        let mut snap = Snapshot { scan: lib.scan_stats(), ..Snapshot::full(&lib) };
        let mut times = lib.open_times();
        times.snapshot_ms = started.elapsed().as_millis() as u64;
        snap.times = Some(times);
        self.session.install(lib);
        Ok(snap)
    }

    pub fn close(&self) {
        self.session.close();
    }

    pub fn present(&self) -> bool {
        self.session.present()
    }

    /// 保存して OS に取り出してもらう。戻り値は OS に取り出してもらったか (false はフォルダで開いた iPod を閉じただけ)。
    /// 取り出しの完了確認で最大 15 秒待つので、画面と別のスレッドで呼ぶ。その間はほかの操作を受け付けない
    pub fn eject(&self) -> AppResult<bool> {
        let _busy = self.session.try_begin_exclusive().ok_or_else(|| AppError::msg(t!("転送中は取り出せません")))?;
        // 裏の読み直しは止め、Rockbox のデータベースは作りかけのまま取り出さない
        self.session.stop_reread();
        self.session.flush_rockbox_db();
        let root = self.session.write(|lib| {
            if lib.is_dirty() {
                lib.save()?;
            }
            Ok(lib.root().to_path_buf())
        })?;
        let ejected = ipod_core::device::eject(&root)?;
        // 取り出せたときだけ閉じる (失敗したら開いたままにして、続けて操作できるようにする)
        self.session.close_if_root(&root);
        Ok(ejected)
    }

    pub fn set_firewire_id(&self, hex: &str) -> AppResult<Snapshot> {
        self.session.write(|lib| {
            lib.set_firewire_id(hex)?;
            Ok(Snapshot::full(lib))
        })
    }

    pub fn rename(&self, name: &str) -> AppResult<Snapshot> {
        Ok(self.session.edit(|lib| lib.rename_ipod(name))?.1)
    }

    /// iPod が USB につながっているか (画面の起動時に、イベントを受ける前の状態を知るため)
    pub fn usb_connected(&self) -> bool {
        device::usb_disk_ipod_connected()
    }

    /// USB で認識したのにドライブとして見えないときの理由 (macOS だけ。`device::drive_wait_reason`)
    pub fn drive_wait_reason(&self) -> Option<&'static str> {
        device::drive_wait_reason()
    }

    /// iPod が USB につながったか・外れたかを 1 秒ごとに調べ、変わったら画面に伝える。
    /// ドライブとして見えるまで (Windows だと数秒〜十数秒) 何も出ないと、認識されていないように見えるため
    pub fn watch_usb(&self) {
        let events = self.events.clone();
        std::thread::spawn(move || {
            let mut last = false;
            loop {
                let now = device::usb_disk_ipod_connected();
                if now != last {
                    last = now;
                    events.emit(AppEvent::UsbIpod(now));
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }
}
