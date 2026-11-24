//! zenpod-app のポートのうち Tauri が要るもの: 画面へのイベントと、アプリのフォルダ

use std::path::PathBuf;
use std::sync::OnceLock;

use tauri::{AppHandle, Emitter, Manager};
use zenpod_app::application::events::AppEvent;
use zenpod_app::application::ports::{AppDirs, EventSink};

/// AppHandle は `setup` で渡す (状態を `manage` するときにはまだ無いため)。
/// 画面はウィンドウを作った直後の `setup` が終わってから動き出すので、それより前に使われることはない
#[derive(Default)]
pub struct TauriHost {
    app: OnceLock<AppHandle>,
}

impl TauriHost {
    pub fn bind(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }
}

impl EventSink for TauriHost {
    fn emit(&self, event: AppEvent) {
        if let Some(app) = self.app.get() {
            let _ = app.emit(event.name(), event);
        }
    }
}

impl AppDirs for TauriHost {
    fn config_dir(&self) -> Option<PathBuf> {
        self.app.get()?.path().app_config_dir().ok()
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        self.app.get()?.path().app_cache_dir().ok()
    }

    fn local_data_dir(&self) -> Option<PathBuf> {
        self.app.get()?.path().app_local_data_dir().ok()
    }

    fn resource_dir(&self) -> Option<PathBuf> {
        self.app.get()?.path().resource_dir().ok()
    }
}
