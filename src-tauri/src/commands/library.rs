//! iPod の曲とプレイリストの編集・読み・曲情報・表記ゆれ・曲の詳細

use ipod_core::pc::itunesxml::ImportCandidate;
use ipod_core::domain::track::{ArtistUnify, ReadingChange, ReadingEdit, Readings, TagEdit, TrackOptions, TrackTagEdit};
use ipod_core::domain::playlist::SmartPreview;
use ipod_core::smartpl::SmartDef;
use ipod_core::online::tagsearch::{Assignments, Naming, ReleaseCandidate, TagDiff};
use ipod_core::t;
use std::path::PathBuf;
use tauri::{State, Window};
use zenpod_app::application::aliases::VariantsResult;
use zenpod_app::application::details::TrackDetails;
use zenpod_app::application::playlists::ImportResult;
use zenpod_app::application::snapshot::{EditResult, Snapshot};
use zenpod_app::domain::export_name::export_stem;

use super::{blocking, file_dialog, pick_folder, CmdResult};
use crate::state::AppState;

// ---------------- 曲とプレイリスト ----------------

#[tauri::command(async)]
pub fn remove_tracks(state: State<AppState>, ids: Vec<String>) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.remove_tracks(&ids)?)
}

#[tauri::command(async)]
pub fn create_playlist(state: State<AppState>, name: String, track_ids: Vec<String>) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.create(&name, &track_ids)?)
}

#[tauri::command(async)]
pub fn rename_playlist(state: State<AppState>, id: String, name: String) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.rename(&id, &name)?)
}

#[tauri::command(async)]
pub fn delete_playlist(state: State<AppState>, id: String) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.delete(&id)?)
}

#[tauri::command(async)]
pub fn add_to_playlist(state: State<AppState>, id: String, track_ids: Vec<String>) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.add_tracks(&id, &track_ids)?)
}

#[tauri::command(async)]
pub fn remove_from_playlist(state: State<AppState>, id: String, track_ids: Vec<String>) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.remove_tracks_from(&id, &track_ids)?)
}

#[tauri::command(async)]
pub fn reorder_playlist(state: State<AppState>, id: String, track_ids: Vec<String>) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.reorder_tracks(&id, &track_ids)?)
}

/// プレイリストの並びを変える (iPod OS)
#[tauri::command(async)]
pub fn reorder_playlists(state: State<AppState>, ids: Vec<String>) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.reorder(&ids)?)
}

#[tauri::command(async)]
pub fn smart_playlist(state: State<AppState>, id: String) -> CmdResult<SmartDef> {
    Ok(state.zen.playlists.smart(&id)?)
}

/// スマートプレイリストを作る (`id` が無い) か、名前と条件を変える
#[tauri::command(async)]
pub fn save_smart_playlist(state: State<AppState>, id: Option<String>, name: String, def: SmartDef) -> CmdResult<Snapshot> {
    Ok(state.zen.playlists.save_smart(id.as_deref(), &name, def)?)
}

/// 条件の画面で、今の条件に合う曲の数を出す (保存はしない)
#[tauri::command(async)]
pub fn preview_smart_playlist(state: State<AppState>, id: Option<String>, def: SmartDef) -> CmdResult<SmartPreview> {
    Ok(state.zen.playlists.preview_smart(id.as_deref(), def)?)
}

/// プレイリストの取り込み (iTunes / ミュージックの XML・.m3u / .m3u8) の前の一覧
#[tauri::command(async)]
pub fn playlist_import_preview(state: State<AppState>, paths: Vec<String>) -> CmdResult<Vec<ImportCandidate>> {
    Ok(state.zen.playlists.import_preview(&paths)?)
}

/// 選んだプレイリストを取り込む (`indexes` は一覧の index)
#[tauri::command(async)]
pub fn playlist_import(state: State<AppState>, paths: Vec<String>, indexes: Vec<usize>) -> CmdResult<ImportResult> {
    Ok(state.zen.playlists.import(&paths, &indexes)?)
}

/// スマートプレイリストを iTunes / ミュージックで読み込める XML に書き出す。書く先は Rust で開いたダイアログで選ぶ
/// (`name` は最初に入れておくファイル名)。戻り値は書き出したか (やめたら false)
#[tauri::command]
pub async fn export_smart_xml(window: Window, state: State<'_, AppState>, ids: Vec<String>, name: String) -> CmdResult<bool> {
    let zen = state.zen.clone();
    blocking(move || {
        let Some(path) = file_dialog(&window, t!("iTunes 用に書き出す"))
            .set_file_name(format!("{}.xml", export_stem("", &name)))
            .add_filter("XML", &["xml"])
            .blocking_save_file()
            .and_then(|p| p.into_path().ok())
        else {
            return Ok(false);
        };
        zen.playlists.export_smart_xml(&ids, &path.to_string_lossy())?;
        Ok(true)
    })
    .await?
}

/// プレイリストを .m3u8 に書き出す。1 つなら保存のダイアログ (`name` は最初に入れておくファイル名)、
/// いくつもならフォルダを選ぶ。書く先は Rust で開いたダイアログで選ぶ。戻り値は書き出した数 (やめたら 0)
#[tauri::command]
pub async fn export_m3u8(window: Window, state: State<'_, AppState>, ids: Vec<String>, name: String) -> CmdResult<usize> {
    let zen = state.zen.clone();
    blocking(move || {
        let folder = ids.len() > 1;
        let path = if folder {
            pick_folder(&window, t!("書き出し先フォルダ"))
        } else {
            file_dialog(&window, t!(".m3u8 で書き出す"))
                .set_file_name(format!("{}.m3u8", export_stem("", &name)))
                .add_filter("M3U8", &["m3u8"])
                .blocking_save_file()
                .and_then(|p| p.into_path().ok())
        };
        let Some(path) = path else { return Ok(0) };
        Ok(zen.playlists.export_m3u8(&ids, &path.to_string_lossy(), folder)?)
    })
    .await?
}

// ---------------- 読み ----------------

/// 読みを自動生成。`ids` が None なら全曲
#[tauri::command(async)]
pub fn generate_readings(state: State<AppState>, ids: Option<Vec<String>>, overwrite: bool) -> CmdResult<EditResult> {
    Ok(state.zen.readings.generate(ids.as_deref(), overwrite)?)
}

/// 読みの自動生成のプレビュー (変更はしない)
#[tauri::command(async)]
pub fn preview_readings(state: State<AppState>, ids: Option<Vec<String>>, overwrite: bool) -> CmdResult<Vec<ReadingChange>> {
    Ok(state.zen.readings.preview(ids.as_deref(), overwrite)?)
}

/// プレビューで選んだ読みを反映
#[tauri::command(async)]
pub fn apply_readings(state: State<AppState>, edits: Vec<ReadingEdit>) -> CmdResult<EditResult> {
    Ok(state.zen.readings.apply(&edits)?)
}

#[tauri::command(async)]
pub fn set_readings(state: State<AppState>, id: String, readings: Readings) -> CmdResult<Snapshot> {
    Ok(state.zen.readings.set(&id, &readings)?)
}

/// 表記から並べ替え用の読みを作る (曲情報の編集画面の「読みを自動で入れる」)。作れなければ None
#[tauri::command(async)]
pub fn make_readings(state: State<AppState>, texts: Vec<String>) -> Vec<Option<String>> {
    state.zen.readings.make(&texts)
}

// ---------------- 曲情報 ----------------

/// 曲情報 (曲名・アーティスト・アルバムなど) を書き換える
#[tauri::command(async)]
pub fn edit_tags(state: State<AppState>, ids: Vec<String>, edit: TagEdit) -> CmdResult<EditResult> {
    Ok(state.zen.tags.edit(&ids, &edit)?)
}

/// 曲ごとに違う内容で曲情報を書き換える
#[tauri::command(async)]
pub fn apply_tag_edits(state: State<AppState>, edits: Vec<TrackTagEdit>) -> CmdResult<EditResult> {
    Ok(state.zen.tags.apply_edits(&edits)?)
}

/// 曲情報をネット (MusicBrainz) で探す: アルバムの候補
#[tauri::command]
pub async fn search_releases(state: State<'_, AppState>, artist: String, album: String) -> CmdResult<Vec<ReleaseCandidate>> {
    let zen = state.zen.clone();
    Ok(blocking(move || zen.tags.search_releases(&artist, &album)).await??)
}

/// 選んだアルバムの曲目と iPod の曲を対応付けて、変わる項目を返す (変更はしない)。
/// `assign` で曲の対応を手で決められる (曲 ID → 曲目の添字、null = 反映しない)
#[tauri::command]
pub async fn preview_release(
    state: State<'_, AppState>,
    ids: Vec<String>,
    release_id: String,
    naming: Option<Naming>,
    assign: Option<Assignments>,
) -> CmdResult<TagDiff> {
    let zen = state.zen.clone();
    Ok(blocking(move || zen.tags.preview_release(&ids, &release_id, naming, assign)).await??)
}

// ---------------- 表記ゆれ ----------------

/// 表記ゆれの候補。調べ済みの MusicBrainz の別名も使う (通信はしない)
#[tauri::command(async)]
pub fn artist_variants(state: State<AppState>) -> CmdResult<VariantsResult> {
    Ok(state.zen.aliases.variants()?)
}

/// まだ調べていないアーティストの別名を MusicBrainz で調べて覚える (1 秒 1 件)
#[tauri::command]
pub async fn lookup_artist_aliases(state: State<'_, AppState>) -> CmdResult<usize> {
    let zen = state.zen.clone();
    Ok(blocking(move || zen.aliases.lookup()).await??)
}

#[tauri::command]
pub fn cancel_alias_lookup(state: State<AppState>) {
    state.zen.aliases.cancel();
}

/// 表記ゆれを統一する
#[tauri::command(async)]
pub fn unify_artists(state: State<AppState>, edits: Vec<ArtistUnify>) -> CmdResult<EditResult> {
    Ok(state.zen.aliases.unify(&edits)?)
}

// ---------------- 曲の詳細 ----------------

/// 曲の詳細情報。iPod の曲は id (dbid)、PC の曲は path を渡す
#[tauri::command]
pub async fn track_details(state: State<'_, AppState>, id: Option<String>, path: Option<String>) -> CmdResult<TrackDetails> {
    let zen = state.zen.clone();
    Ok(blocking(move || zen.details.track(id.as_deref(), path.map(PathBuf::from))).await??)
}

/// 曲のファイルの歌詞。id は iPod の dbid か "pc:<場所>"
#[tauri::command(async)]
pub fn track_lyrics(state: State<AppState>, id: String) -> CmdResult<Option<String>> {
    Ok(state.zen.details.lyrics(&id)?)
}

/// iTunes の「オプション」の今の値 (情報の編集の画面)。iPod OS の曲だけ
#[tauri::command(async)]
pub fn track_options(state: State<AppState>, ids: Vec<String>) -> CmdResult<Vec<Option<TrackOptions>>> {
    Ok(state.zen.details.options(&ids)?)
}

/// 曲のファイルを、入っているフォルダを開いて見せる。id は iPod の dbid か "pc:<場所>"
#[tauri::command(async)]
pub fn reveal_track(state: State<AppState>, id: String) -> CmdResult<()> {
    let path = state.zen.details.file_location(&id)?;
    crate::platform::reveal_in_folder(&path).map_err(|e| e.to_string())
}

/// 選択中の曲の形式の要約 (一覧の下に出す)。ids は iPod の dbid か "pc:<場所>"。読めない曲は None
#[tauri::command]
pub async fn audio_summaries(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> CmdResult<Vec<Option<ipod_core::media::details::AudioSummary>>> {
    let zen = state.zen.clone();
    blocking(move || zen.details.audio_summaries(&ids)).await
}
