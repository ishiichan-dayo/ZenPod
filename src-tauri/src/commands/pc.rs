//! PC の音楽フォルダ

use ipod_core::domain::track::{TagEdit, TrackView};
use ipod_core::pc::folder::PcTrack;
use ipod_core::online::tagsearch::{Assignments, Naming, TagDiff};
use ipod_core::t;
use tauri::{AppHandle, State, Window};
use zenpod_app::application::pc_library::{PcRename, PcTagEdit};

use super::{blocking, pick_folder, zen, CmdResult};
use crate::state::AppState;

/// 音楽フォルダを登録する。フォルダは Rust で開いたダイアログで選ぶ (画面から場所は受け取らない)。
/// 戻り値は選んだフォルダ (やめたら None)
#[tauri::command]
pub async fn add_pc_folder(window: Window, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let zen = state.zen.clone();
    blocking(move || match pick_folder(&window, t!("音楽が入っているフォルダ")) {
        Some(dir) => Ok(Some(zen.prefs.add_pc_folder(&dir)?)),
        None => Ok(None),
    })
    .await?
}

/// 音楽フォルダの登録を外す (中身はそのまま)
#[tauri::command(async)]
pub fn remove_pc_folder(state: State<AppState>, path: String) -> CmdResult<()> {
    Ok(state.zen.prefs.remove_pc_folder(&path)?)
}

/// 音楽フォルダの並び順 (登録済みのフォルダの並べ替えだけ)
#[tauri::command(async)]
pub fn reorder_pc_folders(state: State<AppState>, folders: Vec<String>) -> CmdResult<()> {
    Ok(state.zen.prefs.reorder_pc_folders(folders)?)
}

/// 登録フォルダ (サブフォルダ含む) の曲を読み取る。前回から変わっていないファイルはキャッシュを使う
#[tauri::command]
pub async fn scan_music_folder(app: AppHandle, path: String) -> CmdResult<Vec<PcTrack>> {
    let zen = zen(&app);
    Ok(blocking(move || zen.pc.scan(&path)).await??)
}

/// PC の曲のジャケット (一覧用の小さい画像)
#[tauri::command]
pub async fn pc_artwork(app: AppHandle, path: String) -> Option<String> {
    let zen = zen(&app);
    blocking(move || zen.pc.small_artwork(&path)).await.ok().flatten()
}

/// アルバム表示用の PC の曲のジャケット (一覧用より大きい正方形)
#[tauri::command]
pub async fn pc_album_art(app: AppHandle, path: String) -> Option<String> {
    let zen = zen(&app);
    blocking(move || zen.pc.large_artwork(&path)).await.ok().flatten()
}

/// 画像ファイルを PC の曲のジャケットにする。`shrink` は埋め込む画像を縮める大きさ (長い辺の px。無ければそのまま)
#[tauri::command]
pub async fn set_pc_artwork(app: AppHandle, paths: Vec<String>, path: String, shrink: Option<u32>) -> CmdResult<usize> {
    let zen = zen(&app);
    Ok(blocking(move || zen.pc.set_artwork_from_file(&paths, &path, shrink)).await??)
}

/// ネットの候補画像をダウンロードして PC の曲のジャケットにする
#[tauri::command]
pub async fn apply_pc_artwork_url(app: AppHandle, paths: Vec<String>, url: String, shrink: Option<u32>) -> CmdResult<usize> {
    let zen = zen(&app);
    Ok(blocking(move || zen.pc.apply_artwork_url(&paths, &url, shrink)).await??)
}

/// PC の曲に埋め込まれた画像の大きさ
#[tauri::command]
pub async fn pc_embedded_artwork(app: AppHandle, path: String, size: Option<u32>) -> CmdResult<Option<zenpod_app::application::artwork::EmbeddedArt>> {
    let zen = zen(&app);
    Ok(blocking(move || zen.pc.embedded_artwork(&path, size)).await??)
}

/// PC の曲に埋め込まれた画像を、長い辺 `size` px までに縮める (元のファイルを変更する)
#[tauri::command]
pub async fn shrink_pc_artwork(app: AppHandle, paths: Vec<String>, size: Option<u32>) -> CmdResult<usize> {
    let zen = zen(&app);
    Ok(blocking(move || zen.pc.shrink_artwork(&paths, size)).await??)
}

/// PC の曲に埋め込まれたジャケットを消す
#[tauri::command]
pub async fn remove_pc_artwork(app: AppHandle, paths: Vec<String>) -> CmdResult<usize> {
    let zen = zen(&app);
    Ok(blocking(move || zen.pc.remove_artwork(&paths)).await??)
}

/// 複数の PC の曲に同じ内容を書く (情報の編集)
#[tauri::command]
pub async fn edit_pc_tags(app: AppHandle, paths: Vec<String>, edit: TagEdit) -> CmdResult<usize> {
    let zen = zen(&app);
    Ok(blocking(move || zen.pc.edit_tags(paths, &edit)).await??)
}

/// PC の曲ごとに違う内容を書く (ネットで取得した曲情報の反映)
#[tauri::command]
pub async fn apply_pc_tag_edits(app: AppHandle, edits: Vec<PcTagEdit>) -> CmdResult<usize> {
    let zen = zen(&app);
    Ok(blocking(move || zen.pc.write_tags(&edits)).await??)
}

/// PC の曲とアルバムの曲目を対応付けて、変わる項目を返す (変更はしない)
#[tauri::command]
pub async fn preview_release_pc(
    state: State<'_, AppState>,
    tracks: Vec<TrackView>,
    release_id: String,
    naming: Option<Naming>,
    assign: Option<Assignments>,
) -> CmdResult<TagDiff> {
    let zen = state.zen.clone();
    Ok(blocking(move || zen.tags.preview_release_pc(&tracks, &release_id, naming, assign)).await??)
}

/// PC の曲のファイル名を変える (拡張子はそのまま)。戻り値は新しい場所
#[tauri::command(async)]
pub fn rename_pc_file(state: State<AppState>, path: String, name: String) -> CmdResult<String> {
    Ok(state.zen.pc.rename(&path, &name)?)
}

/// PC の曲のファイル名をまとめて変える。1 つでも問題があれば何も変えない
#[tauri::command(async)]
pub fn rename_pc_files(state: State<AppState>, renames: Vec<PcRename>) -> CmdResult<Vec<String>> {
    Ok(state.zen.pc.rename_many(renames)?)
}
