//! 先行版の確認と更新 (設定の「先行版」の「確認」)。先行版は GitHub の「プレリリース」で、
//! 設定ファイルの更新確認先 (`releases/latest`) には出ないので、リリースの一覧で一番新しい版の latest.json を updater に読ませる
//! (`zenpod_app::domain::releases`)。普通の確認 (起動時・「アップデートを確認」) は画面から updater プラグインを直接呼ぶ

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use zenpod_app::domain::releases::{latest_json_url, newest_release_tag, releases_api_url};

use super::{blocking, CmdResult};

/// 確かめた先行版 (更新するときに使う)
#[derive(Default)]
pub struct EarlyUpdate(Mutex<Option<Update>>);

/// 画面に返す新しい版 (updater プラグインの Update と同じ名前)
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EarlyUpdateInfo {
    version: String,
    current_version: String,
    body: Option<String>,
}

/// ダウンロードの進み具合 (updater プラグインの DownloadEvent と同じ形)
#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum DownloadEvent {
    #[serde(rename_all = "camelCase")]
    Started { content_length: Option<u64> },
    #[serde(rename_all = "camelCase")]
    Progress { chunk_length: usize },
    Finished,
}

fn newest_tag() -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(concat!("ZenPod/", env!("CARGO_PKG_VERSION"), " ( https://github.com/ishiichan-dayo/ZenPod )"))
        .build()
        .into();
    let mut resp = agent
        .get(&releases_api_url())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let list: serde_json::Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    // tr: 訳さない (起きない前提。画面は「確認できませんでした: …」に続けて出す)
    newest_release_tag(&list).ok_or_else(|| "no release".to_string())
}

/// 一番新しいリリース (先行版を含む) を確かめる。今の版より新しくなければ None
#[tauri::command]
pub async fn check_early_update(app: AppHandle, early: State<'_, EarlyUpdate>) -> CmdResult<Option<EarlyUpdateInfo>> {
    let tag = blocking(newest_tag).await??;
    let url = tauri::Url::parse(&latest_json_url(&tag)).map_err(|e| e.to_string())?;
    let update = app
        .updater_builder()
        .endpoints(vec![url])
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let info = update.as_ref().map(|u| EarlyUpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        body: u.body.clone(),
    });
    *early.0.lock().unwrap() = update;
    Ok(info)
}

/// `check_early_update` で見つけた版をダウンロードして入れる (再起動は画面がする)
#[tauri::command]
pub async fn install_early_update(early: State<'_, EarlyUpdate>, on_event: Channel<DownloadEvent>) -> CmdResult<()> {
    // tr: 訳さない (起きない前提)
    let update = early.0.lock().unwrap().clone().ok_or("no update")?;
    let mut started = false;
    update
        .download_and_install(
            |chunk, total| {
                if !started {
                    started = true;
                    let _ = on_event.send(DownloadEvent::Started { content_length: total });
                }
                let _ = on_event.send(DownloadEvent::Progress { chunk_length: chunk });
            },
            || {
                let _ = on_event.send(DownloadEvent::Finished);
            },
        )
        .await
        .map_err(|e| e.to_string())
}
