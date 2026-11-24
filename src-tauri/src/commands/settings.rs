//! アプリの設定・ffmpeg・画面の起動・言語

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use ipod_core::t;
use tauri::{State, Window};
use zenpod_app::application::ffmpeg::Settings;
use zenpod_app::domain::links::is_allowed_link;

use super::{blocking, file_dialog, CmdResult};
use crate::state::AppState;

/// 「このアプリについて」のリンクを既定のブラウザで開く。画面から任意の URL を開けないよう、決めた先だけにする
#[tauri::command]
pub fn open_url(url: String) -> CmdResult<()> {
    if !is_allowed_link(&url) {
        return Err(t!("開けないリンクです"));
    }
    crate::platform::open_in_browser(&url).map_err(|e| e.to_string())
}

/// 画面のエラー (コマンドの失敗・例外) をログ (zenpod.log) に残す。Rust のログと同じ並びで見られるように
#[tauri::command]
pub fn ui_log(msg: String) {
    let msg: String = msg.chars().take(2000).collect();
    log::warn!(target: "ui", "{msg}");
}

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Settings {
    state.zen.ffmpeg.settings()
}

/// ffmpeg をダウンロードしてアプリ専用のフォルダに入れる (Windows / macOS)
#[tauri::command]
pub async fn install_ffmpeg(state: State<'_, AppState>) -> CmdResult<Settings> {
    let zen = state.zen.clone();
    Ok(blocking(move || zen.ffmpeg.install()).await??)
}

#[tauri::command]
pub fn cancel_ffmpeg_install(state: State<AppState>) {
    state.zen.ffmpeg.cancel_install();
}

/// 自動で入れた ffmpeg を削除する
#[tauri::command(async)]
pub fn uninstall_ffmpeg(state: State<AppState>) -> CmdResult<Settings> {
    Ok(state.zen.ffmpeg.uninstall()?)
}

/// 使う ffmpeg を Rust で開いたダイアログで選ぶ。動かして ffmpeg と分かったときだけ覚える。やめたら None
#[tauri::command]
pub async fn pick_ffmpeg(window: Window, state: State<'_, AppState>) -> CmdResult<Option<Settings>> {
    let zen = state.zen.clone();
    blocking(move || {
        let Some(path) = file_dialog(&window, t!("ffmpeg を選択")).blocking_pick_file().and_then(|p| p.into_path().ok()) else {
            return Ok(None);
        };
        Ok(Some(zen.ffmpeg.choose(&path)?))
    })
    .await?
}

/// 選んだ ffmpeg を忘れて、自動で探すのに戻す
#[tauri::command(async)]
pub fn reset_ffmpeg_path(state: State<AppState>) -> CmdResult<Settings> {
    Ok(state.zen.ffmpeg.forget_choice()?)
}

/// 画面の起動が終わったら呼ぶ (起動の見張りを止める)
#[tauri::command]
pub fn ui_ready(state: State<AppState>) {
    state.ui_ready.store(true, Ordering::SeqCst);
}

/// 画面の言語を Rust 側の文言 (エラーなど) にも使う。画面が起動時に呼ぶ
#[tauri::command]
pub fn set_lang(lang: String) {
    ipod_core::i18n::set_lang(&lang);
}

#[tauri::command]
pub fn load_prefs(state: State<AppState>) -> HashMap<String, String> {
    state.zen.prefs.all()
}

/// `value` が None なら削除。場所に使うキー (pcFolders・ffmpegPath) は専用のコマンドで変える
#[tauri::command(async)]
pub fn set_pref(state: State<AppState>, key: String, value: Option<String>) -> CmdResult<()> {
    Ok(state.zen.prefs.set(key, value)?)
}
