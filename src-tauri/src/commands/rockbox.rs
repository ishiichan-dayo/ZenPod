//! Rockbox モードだけの操作

use tauri::State;
use zenpod_app::application::rockbox::{RockboxDbResult, TrackRename};
use zenpod_app::application::snapshot::Snapshot;

use super::{blocking, CmdResult};
use crate::state::AppState;

/// Rockbox のデータベースを PC で作り直す。曲が多いとタグの読み取りに時間がかかるので別スレッドで
#[tauri::command]
pub async fn rebuild_rockbox_db(state: State<'_, AppState>) -> CmdResult<RockboxDbResult> {
    let zen = state.zen.clone();
    Ok(blocking(move || zen.rockbox.rebuild_db()).await??)
}

/// 曲を転送するときの置き場所の形を変える (画面が iPod ごとに覚えていて、開いたときにも渡す)。
/// 転送中は 1 曲ごとに lock を持つので、待つ間に画面が止まらないよう別のスレッドで
#[tauri::command(async)]
pub fn set_rockbox_layout(state: State<AppState>, template: String) -> CmdResult<()> {
    Ok(state.zen.rockbox.set_layout(&template)?)
}

/// 置き場所の形の見本 ("Music/アーティスト/アルバム/01 曲名.flac")
#[tauri::command(async)]
pub fn preview_rockbox_layout(state: State<AppState>, template: String) -> CmdResult<String> {
    Ok(state.zen.rockbox.preview_layout(&template)?)
}

/// 今ある曲の並びから置き場所の形を推測する (曲が多いと少し時間がかかるので画面と別のスレッドで)
#[tauri::command(async)]
pub fn guess_rockbox_layout(state: State<AppState>) -> CmdResult<Option<ipod_core::domain::layout::Guess>> {
    Ok(state.zen.rockbox.guess_layout()?)
}

/// 曲のファイル名をまとめて変える。1 つでも問題があれば何も変えない
#[tauri::command(async)]
pub fn rename_rockbox_files(state: State<AppState>, renames: Vec<TrackRename>) -> CmdResult<Snapshot> {
    Ok(state.zen.rockbox.rename_files(renames)?)
}
