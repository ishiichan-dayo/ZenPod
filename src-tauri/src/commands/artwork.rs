//! iPod の曲のジャケットと、ネットのジャケット候補

use ipod_core::online::artsearch::{ImageProbe, SearchResult};
use tauri::AppHandle;
use zenpod_app::application::snapshot::Snapshot;

use super::{blocking, zen, CmdResult};

/// ジャケットを data URL で返す。`large` なら最大サイズ
/// (アルバム表示では一度に何十枚も読むので、画面を止めないよう別スレッドで読む)
#[tauri::command]
pub async fn get_artwork(app: AppHandle, image_id: u32, large: bool) -> Option<String> {
    let zen = zen(&app);
    blocking(move || zen.artwork.get(image_id, large)).await.ok().flatten()
}

/// 画像の縮小と ithmb への書き込みに時間がかかるので、画面を止めないよう別スレッドで行う。
/// `shrink` は曲のファイルに埋め込む画像を縮める大きさ (長い辺の px。無ければそのまま)
#[tauri::command]
pub async fn set_artwork(app: AppHandle, ids: Vec<String>, path: String, shrink: Option<u32>) -> CmdResult<Snapshot> {
    let zen = zen(&app);
    Ok(blocking(move || zen.artwork.set_from_file(&ids, &path, shrink)).await??)
}

/// 曲のファイルに埋め込まれた画像の大きさ (情報の編集のアートワークのタブ)
#[tauri::command]
pub async fn embedded_artwork(app: AppHandle, id: String, size: Option<u32>) -> CmdResult<Option<zenpod_app::application::artwork::EmbeddedArt>> {
    let zen = zen(&app);
    Ok(blocking(move || zen.artwork.embedded(&id, size)).await??)
}

/// iPod の曲の埋め込みのジャケットを、長い辺 `size` px までに縮める
#[tauri::command]
pub async fn shrink_artwork(app: AppHandle, ids: Vec<String>, size: Option<u32>) -> CmdResult<zenpod_app::application::artwork::Shrunk> {
    let zen = zen(&app);
    Ok(blocking(move || zen.artwork.shrink(&ids, size)).await??)
}

#[tauri::command]
pub async fn remove_artwork(app: AppHandle, ids: Vec<String>) -> CmdResult<Snapshot> {
    let zen = zen(&app);
    Ok(blocking(move || zen.artwork.remove(&ids)).await??)
}

/// ネット上のジャケット候補を検索 (通信するので別スレッドで実行)
#[tauri::command]
pub async fn search_artwork(app: AppHandle, artist: String, album: String, token: Option<u32>) -> SearchResult {
    let zen = zen(&app);
    blocking(move || zen.artwork.search(&artist, &album, token.unwrap_or(0))).await.unwrap_or_default()
}

/// 候補画像の解像度・ファイルサイズ
#[tauri::command]
pub async fn probe_artwork(app: AppHandle, url: String) -> Option<ImageProbe> {
    let zen = zen(&app);
    blocking(move || zen.artwork.probe(&url)).await.ok().flatten()
}

/// 候補のサムネイルを data URL で返す (WebView が直接読めない環境向けの代替)
#[tauri::command]
pub async fn fetch_image(app: AppHandle, url: String) -> Option<String> {
    let zen = zen(&app);
    blocking(move || zen.artwork.fetch_image(&url)).await.ok().flatten()
}

/// 候補画像をダウンロードして曲のアートワークにする
#[tauri::command]
pub async fn apply_artwork_url(app: AppHandle, ids: Vec<String>, url: String, shrink: Option<u32>) -> CmdResult<Snapshot> {
    let zen = zen(&app);
    Ok(blocking(move || zen.artwork.apply_url(&ids, &url, shrink)).await??)
}
