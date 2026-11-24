//! 画面から呼ぶ Tauri のコマンド。引数を受け取ってユースケース (zenpod-app) を呼ぶだけにする。
//! コマンドの名前・引数・戻り値は画面 (src/main.ts) との約束なので変えない。
//!
//! 画面から呼ぶ重いコマンドは `#[tauri::command(async)]` か `async fn` + `blocking` にする
//! (ただの `fn` は画面と同じスレッドで動き、その間は画面が止まる)

pub mod artwork;
pub mod devices;
pub mod library;
pub mod pc;
pub mod podcasts;
pub mod rockbox;
pub mod settings;
pub mod transfer;
pub mod update;
pub mod visual;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager, Window};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder};
use zenpod_app::application::ZenPod;

use crate::state::AppState;

pub type CmdResult<T> = Result<T, String>;

/// `async fn` のコマンドから、ユースケースを画面と別のスレッドで呼ぶ
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

/// 別のスレッドに渡すためのユースケース一式
pub fn zen(app: &AppHandle) -> Arc<ZenPod> {
    app.state::<AppState>().zen.clone()
}

/// ファイルを書く先・ffmpeg・PC の音楽フォルダ・開く iPod は、Rust で開いたダイアログで選んでもらう
/// (画面から場所を受け取ると、乗っ取られた画面が好きな場所に書けてしまうため)。
/// ダイアログは閉じるまで待つので `blocking` の中で呼ぶ
pub fn file_dialog(window: &Window, title: String) -> FileDialogBuilder<tauri::Wry> {
    let dialog = window.dialog().file().set_title(title);
    // 画面の上に出す (プラグインの open と同じ。Linux は親を指定しない)
    #[cfg(any(windows, target_os = "macos"))]
    let dialog = dialog.set_parent(window);
    dialog
}

pub fn pick_folder(window: &Window, title: String) -> Option<PathBuf> {
    file_dialog(window, title).blocking_pick_folder()?.into_path().ok()
}
