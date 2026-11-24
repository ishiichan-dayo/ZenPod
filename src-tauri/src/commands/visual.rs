//! iPod の動画と写真

use ipod_core::domain::photo::PhotoLibrary;
use ipod_core::domain::video::{VideoEdit, VideoFileInfo, VideoView};
use ipod_core::t;
use tauri::{AppHandle, Window};
use zenpod_app::application::video_play::VideoPlan;
use zenpod_app::application::visual::{PhotoEdit, VideoEditResult};

use super::{blocking, pick_folder, zen, CmdResult};

// ---------------- 動画 ----------------

/// 動画の再生のしかた (ファイルのまま / 変換して流す)。formats は画面の MediaSource で再生できる形式 (使いたい順)
#[tauri::command]
pub async fn video_play_plan(app: AppHandle, id: String, formats: Vec<String>, force_stream: bool) -> CmdResult<VideoPlan> {
    let zen = zen(&app);
    Ok(blocking(move || zen.video_player.plan(&id, &formats, force_stream)).await??)
}

/// 変換して流し始める。戻り値は video_stream_read に渡す番号
#[tauri::command]
pub async fn video_stream_start(app: AppHandle, id: String, format: String, start_ms: u32) -> CmdResult<u64> {
    let zen = zen(&app);
    Ok(blocking(move || zen.video_player.start(&id, &format, start_ms)).await??)
}

/// 流している動画の続き。先頭の 1 バイトは終わりの印 (1 = 最後まで渡した)、その後ろが動画のデータ
#[tauri::command]
pub async fn video_stream_read(app: AppHandle, stream: u64) -> CmdResult<tauri::ipc::Response> {
    let zen = zen(&app);
    let chunk = blocking(move || zen.video_player.read(stream)).await??;
    let mut out = Vec::with_capacity(chunk.data.len() + 1);
    out.push(chunk.ended as u8);
    out.extend_from_slice(&chunk.data);
    Ok(tauri::ipc::Response::new(out))
}

#[tauri::command]
pub fn video_stream_stop(app: AppHandle) {
    zen(&app).video_player.stop();
}

#[tauri::command]
pub async fn list_videos(app: AppHandle) -> CmdResult<Vec<VideoView>> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.videos()).await??)
}

#[tauri::command]
pub async fn edit_videos(app: AppHandle, ids: Vec<String>, edit: VideoEdit) -> CmdResult<VideoEditResult> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.edit_videos(&ids, &edit)).await??)
}

/// 動画のファイルの中身 (情報の編集の「ファイル」のページ)
#[tauri::command]
pub async fn video_file_info(app: AppHandle, id: String) -> CmdResult<Option<VideoFileInfo>> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.video_file_info(&id)).await??)
}

#[tauri::command]
pub async fn remove_videos(app: AppHandle, ids: Vec<String>) -> CmdResult<VideoEditResult> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.remove_videos(&ids)).await??)
}

/// 一覧の画像 (data URL)。Rockbox は ffmpeg で 1 コマ取り出すので、別スレッドで
#[tauri::command]
pub async fn video_thumb(app: AppHandle, id: String) -> Option<String> {
    let zen = zen(&app);
    blocking(move || zen.visual.video_thumb(&id)).await.ok().flatten()
}

/// 書き出し先は Rust で開いたダイアログで選ぶ。戻り値は書き出した数 (やめたら None)
#[tauri::command]
pub async fn export_videos(window: Window, app: AppHandle, ids: Vec<String>) -> CmdResult<Option<usize>> {
    let zen = zen(&app);
    blocking(move || match pick_folder(&window, t!("書き出し先フォルダ")) {
        Some(dest) => Ok(Some(zen.visual.export_videos(&ids, &dest.to_string_lossy())?)),
        None => Ok(None),
    })
    .await?
}

// ---------------- 写真 ----------------

/// 貼り付けた画像を一時ファイルにして、その場所を返す (画面がそれを転送する)
#[tauri::command]
pub async fn save_pasted_image(app: AppHandle, data: String, ext: String) -> CmdResult<String> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.save_pasted_image(&data, &ext)).await??)
}

#[tauri::command]
pub async fn photo_library(app: AppHandle) -> CmdResult<PhotoLibrary> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.photo_library()).await??)
}

/// 写真の画像 (data URL)。`large` は 1 枚を大きく見るとき
#[tauri::command]
pub async fn photo_thumb(app: AppHandle, id: String, large: bool) -> Option<String> {
    let zen = zen(&app);
    blocking(move || zen.visual.photo_thumb(&id, large)).await.ok().flatten()
}

#[tauri::command]
pub async fn remove_photos(app: AppHandle, ids: Vec<String>) -> CmdResult<PhotoEdit> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.remove_photos(&ids)).await??)
}

#[tauri::command]
pub async fn create_photo_album(app: AppHandle, name: String, parent: Option<String>, ids: Vec<String>) -> CmdResult<PhotoEdit> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.create_album(&name, parent.as_deref(), &ids)).await??)
}

#[tauri::command]
pub async fn rename_photo_album(app: AppHandle, id: String, name: String) -> CmdResult<PhotoEdit> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.rename_album(&id, &name)).await??)
}

#[tauri::command]
pub async fn delete_photo_album(app: AppHandle, id: String) -> CmdResult<PhotoEdit> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.delete_album(&id)).await??)
}

#[tauri::command]
pub async fn add_to_photo_album(app: AppHandle, id: String, ids: Vec<String>) -> CmdResult<PhotoEdit> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.add_to_album(&id, &ids)).await??)
}

#[tauri::command]
pub async fn remove_from_photo_album(app: AppHandle, id: String, ids: Vec<String>) -> CmdResult<PhotoEdit> {
    let zen = zen(&app);
    Ok(blocking(move || zen.visual.remove_from_album(&id, &ids)).await??)
}

/// 書き出し先は Rust で開いたダイアログで選ぶ。戻り値は書き出した数 (やめたら None)
#[tauri::command]
pub async fn export_photos(window: Window, app: AppHandle, ids: Vec<String>) -> CmdResult<Option<usize>> {
    let zen = zen(&app);
    blocking(move || match pick_folder(&window, t!("書き出し先フォルダ")) {
        Some(dest) => Ok(Some(zen.visual.export_photos(&ids, &dest.to_string_lossy())?)),
        None => Ok(None),
    })
    .await?
}
