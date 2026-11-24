//! ログ。アプリのログのフォルダの zenpod.log に書く (固まったときなどに、何が起きていたかを後から見るため)。
//! ZenPod のクレートと画面 (`ui`) は info から、ほかのクレートは warn から残す。文言は英語で書く

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// ログのファイルがこれより大きくなったら、起動したときに zenpod.old.log へ回す
const MAX_FILE: u64 = 1 << 20;

struct Logger {
    file: Mutex<Option<File>>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn ours(target: &str) -> bool {
    ["ipod_core", "zenpod_app", "zenpod_lib", "ui"]
        .iter()
        .any(|p| target == *p || target.strip_prefix(p).is_some_and(|r| r.starts_with("::")))
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= if ours(m.target()) { log::Level::Info } else { log::Level::Warn }
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!("{} {:<5} {}: {}", chrono::Local::now().format("%m-%d %H:%M:%S%.3f"), r.level(), r.target(), r.args());
        if let Some(f) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }

    fn flush(&self) {}
}

fn logger() -> &'static Logger {
    LOGGER.get_or_init(|| Logger { file: Mutex::new(None) })
}

/// 起動のいちばん初めに呼ぶ。panic もログに残す (元の処理はそのまま呼ぶ)
pub fn init() {
    if log::set_logger(logger()).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        prev(info);
    }));
}

/// ログのファイルを開く (アプリのフォルダが分かってから)。それまでのログは捨てる
pub fn attach_file(dir: &Path) {
    let path = dir.join("zenpod.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) {
        let _ = std::fs::rename(&path, dir.join("zenpod.old.log"));
    }
    let file = std::fs::create_dir_all(dir).and_then(|_| OpenOptions::new().create(true).append(true).open(&path));
    match file {
        Ok(f) => *logger().file.lock().unwrap_or_else(|e| e.into_inner()) = Some(f),
        Err(e) => log::warn!("cannot open the log file: {e}"),
    }
    log::info!("started: ZenPod {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
}
