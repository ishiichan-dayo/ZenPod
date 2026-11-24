//! PC → iPod の転送と、iPod → PC の書き出し

use ipod_core::t;
use tauri::{State, Window};
use zenpod_app::application::transfer::{ConversionCount, TransferOptions};

use super::{blocking, pick_folder, CmdResult};
use crate::state::AppState;

/// 転送する前に、iPod OS モードで変換が要る曲を数える (変換先を選んでもらうため)
#[tauri::command]
pub async fn count_conversions(state: State<'_, AppState>, paths: Vec<String>) -> CmdResult<ConversionCount> {
    let zen = state.zen.clone();
    blocking(move || zen.transfer.count_conversions(&paths)).await
}

/// 転送を始めて曲の数を返す。経過は transfer-progress、結果は transfer-done で知らせる
#[tauri::command(async)]
pub fn add_paths(
    state: State<AppState>,
    paths: Vec<String>,
    playlist_id: Option<String>,
    options: TransferOptions,
) -> CmdResult<usize> {
    Ok(state.zen.transfer.start(&paths, playlist_id.as_deref(), &options)?)
}

#[tauri::command]
pub fn cancel_transfer(state: State<AppState>) {
    state.zen.transfer.cancel();
}

/// iPod 内の曲を PC にコピー。書き出し先は Rust で開いたダイアログで選ぶ。戻り値は書き出した数 (やめたら None)
#[tauri::command]
pub async fn export_tracks(window: Window, state: State<'_, AppState>, ids: Vec<String>) -> CmdResult<Option<usize>> {
    let zen = state.zen.clone();
    blocking(move || match pick_folder(&window, t!("書き出し先フォルダ")) {
        Some(dest) => Ok(Some(zen.transfer.export(&ids, &dest.to_string_lossy())?)),
        None => Ok(None),
    })
    .await?
}
