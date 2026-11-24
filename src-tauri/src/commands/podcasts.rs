//! ポッドキャストの購読

use ipod_core::online::podcast::Feed;
use tauri::{AppHandle, State};
use zenpod_app::application::podcasts::{Cover, DownloadResult, EpRef, RefreshResult};

use super::{blocking, zen, CmdResult};
use crate::state::AppState;

#[tauri::command]
pub async fn podcast_list(app: AppHandle) -> CmdResult<Vec<Feed>> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.list()).await??)
}

/// 番組を登録する。フィードを読めたら番組の ID を返す (同じ URL がもうあればその ID)
#[tauri::command]
pub async fn podcast_subscribe(app: AppHandle, url: String) -> CmdResult<String> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.subscribe(&url)).await??)
}

/// 番組の登録をやめる (落としたファイルも消す。iPod の回はそのまま)
#[tauri::command]
pub async fn podcast_unsubscribe(app: AppHandle, id: String) -> CmdResult<()> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.unsubscribe(&id)).await??)
}

/// フィードを読み直す (`ids` が無ければ全部)。`min_age` 秒より前に確かめた番組だけにする (起動時)
#[tauri::command]
pub async fn podcast_refresh(app: AppHandle, ids: Option<Vec<String>>, min_age: Option<i64>) -> CmdResult<Vec<RefreshResult>> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.refresh(ids.as_deref(), min_age)).await??)
}

/// 同期で iPod に置いておく、聞いていない回の数 (0 = 自動では送らない) と、古い方から選ぶか
#[tauri::command]
pub async fn podcast_set_keep(app: AppHandle, id: String, keep: u32, oldest: Option<bool>) -> CmdResult<()> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.set_keep(&id, keep, oldest)).await??)
}

/// 回に「聞いた」の印を付ける / 外す
#[tauri::command]
pub async fn podcast_mark(app: AppHandle, items: Vec<EpRef>, played: bool) -> CmdResult<()> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.mark(&items, played)).await??)
}

/// 落としたファイルを消す (iPod の回はそのまま)
#[tauri::command]
pub async fn podcast_delete_files(app: AppHandle, items: Vec<EpRef>) -> CmdResult<()> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.delete_files(&items)).await??)
}

/// 落としたファイルの合計 (設定に出す)
#[tauri::command]
pub async fn podcast_files_size(app: AppHandle) -> CmdResult<u64> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.files_size()).await??)
}

/// 番組の画像 (一覧用の小さいものと、見出し用の大きいもの)
#[tauri::command]
pub async fn podcast_cover(app: AppHandle, id: String) -> Option<Cover> {
    let zen = zen(&app);
    blocking(move || zen.podcasts.cover(&id)).await.ok().flatten()
}

/// 回を落としてタグを書く。もう落としてある回は落とし直さない。経過は podcast-download で知らせる
#[tauri::command]
pub async fn podcast_download(app: AppHandle, items: Vec<EpRef>) -> CmdResult<DownloadResult> {
    let zen = zen(&app);
    Ok(blocking(move || zen.podcasts.download(&items)).await??)
}

#[tauri::command]
pub fn cancel_podcast_download(state: State<AppState>) {
    state.zen.podcasts.cancel_download();
}

/// 回の画像 (一覧用の小さいもの、data URL)
#[tauri::command]
pub async fn podcast_image(app: AppHandle, feed_id: String, url: String) -> Option<String> {
    let zen = zen(&app);
    blocking(move || zen.podcasts.image(&feed_id, &url)).await.ok().flatten()
}

/// 番組のサイト (フィードに書いてあるリンク) をブラウザで開く。登録している番組の http / https のリンクだけ
#[tauri::command]
pub async fn podcast_open_link(app: AppHandle, id: String) -> CmdResult<()> {
    let link = zen(&app).podcasts.link(&id)?;
    crate::platform::open_in_browser(&link).map_err(|e| e.to_string())
}
