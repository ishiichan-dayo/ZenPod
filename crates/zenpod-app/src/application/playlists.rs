//! 曲の削除・プレイリスト・スマートプレイリスト・プレイリストの取り込みと書き出し

use std::path::Path;
use std::sync::Arc;

use ipod_core::pc::itunesxml::{ImportCandidate, XmlLibrary, XmlTrack};
use ipod_core::domain::playlist::SmartPreview;
use ipod_core::smartpl::SmartDef;
use serde::Serialize;

use super::session::LibrarySession;
use super::snapshot::Snapshot;
use crate::domain::export_name::{export_stem, unique_path};
use crate::domain::id::{parse_id, parse_ids, parse_opt_id};
use crate::error::AppResult;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub snapshot: Snapshot,
    pub summary: ipod_core::library::ImportSummary,
}

pub struct Playlists {
    session: Arc<LibrarySession>,
}

impl Playlists {
    pub fn new(session: Arc<LibrarySession>) -> Self {
        Playlists { session }
    }

    pub fn remove_tracks(&self, ids: &[String]) -> AppResult<Snapshot> {
        let ids = parse_ids(ids)?;
        Ok(self.session.edit(|lib| lib.remove_tracks(&ids))?.1)
    }

    pub fn create(&self, name: &str, track_ids: &[String]) -> AppResult<Snapshot> {
        let ids = parse_ids(track_ids)?;
        Ok(self.session.edit_playlists(|lib| lib.create_playlist(name, &ids))?.1)
    }

    pub fn rename(&self, id: &str, name: &str) -> AppResult<Snapshot> {
        let id = parse_id(id)?;
        Ok(self.session.edit_playlists(|lib| lib.rename_playlist(id, name))?.1)
    }

    pub fn delete(&self, id: &str) -> AppResult<Snapshot> {
        let id = parse_id(id)?;
        Ok(self.session.edit_playlists(|lib| lib.delete_playlist(id))?.1)
    }

    pub fn add_tracks(&self, id: &str, track_ids: &[String]) -> AppResult<Snapshot> {
        let (id, ids) = (parse_id(id)?, parse_ids(track_ids)?);
        Ok(self.session.edit_playlists(|lib| lib.add_to_playlist(id, &ids))?.1)
    }

    pub fn remove_tracks_from(&self, id: &str, track_ids: &[String]) -> AppResult<Snapshot> {
        let (id, ids) = (parse_id(id)?, parse_ids(track_ids)?);
        Ok(self.session.edit_playlists(|lib| lib.remove_from_playlist(id, &ids))?.1)
    }

    /// プレイリストの中の曲の並びを変える
    pub fn reorder_tracks(&self, id: &str, track_ids: &[String]) -> AppResult<Snapshot> {
        let (id, ids) = (parse_id(id)?, parse_ids(track_ids)?);
        Ok(self.session.edit_playlists(|lib| lib.reorder_playlist(id, &ids))?.1)
    }

    /// プレイリストの並びを変える (iPod OS)
    pub fn reorder(&self, ids: &[String]) -> AppResult<Snapshot> {
        let ids = parse_ids(ids)?;
        Ok(self.session.edit_playlists(|lib| lib.reorder_playlists(&ids))?.1)
    }

    pub fn smart(&self, id: &str) -> AppResult<SmartDef> {
        let id = parse_id(id)?;
        self.session.read(|lib| Ok(lib.smart_playlist(id)?))
    }

    /// スマートプレイリストを作る (`id` が無い) か、名前と条件を変える
    pub fn save_smart(&self, id: Option<&str>, name: &str, def: SmartDef) -> AppResult<Snapshot> {
        let id = parse_opt_id(id)?;
        Ok(self.session.edit_playlists(|lib| lib.save_smart_playlist(id, name, def))?.1)
    }

    /// 条件の画面で、今の条件に合う曲の数を出す (保存はしない)
    pub fn preview_smart(&self, id: Option<&str>, def: SmartDef) -> AppResult<SmartPreview> {
        let id = parse_opt_id(id)?;
        self.session.read(|lib| Ok(lib.preview_smart(def, id)?))
    }

    /// 取り込む前の一覧
    pub fn import_preview(&self, paths: &[String]) -> AppResult<Vec<ImportCandidate>> {
        let xml = read_import_files(paths)?;
        self.session.read(|lib| Ok(lib.import_candidates(&xml)))
    }

    /// 選んだプレイリストを取り込む (`indexes` は一覧の index)
    pub fn import(&self, paths: &[String], indexes: &[usize]) -> AppResult<ImportResult> {
        let xml = read_import_files(paths)?;
        let (summary, snapshot) = self.session.edit_playlists(|lib| lib.import_itunes(&xml, indexes))?;
        Ok(ImportResult { snapshot, summary })
    }

    /// スマートプレイリストを iTunes / ミュージックで読み込める XML に書き出す
    pub fn export_smart_xml(&self, ids: &[String], path: &str) -> AppResult<()> {
        let ids = parse_ids(ids)?;
        let bytes = self.session.read(|lib| Ok(lib.export_smart_playlists(&ids)?))?;
        Ok(std::fs::write(path, bytes)?)
    }

    /// プレイリストを .m3u8 に書き出す。`folder` なら `path` のフォルダに「名前.m3u8」で (同じ名前があれば「名前 (2).m3u8」)、
    /// でなければ `path` のファイルに 1 つ目を。戻り値は書き出した数
    pub fn export_m3u8(&self, ids: &[String], path: &str, folder: bool) -> AppResult<usize> {
        let ids = parse_ids(ids)?;
        let files = self.session.read(|lib| Ok(lib.export_m3u8(&ids)?))?;
        if !folder {
            std::fs::write(path, &files[0].1)?;
            return Ok(1);
        }
        for (name, text) in &files {
            std::fs::write(unique_path(Path::new(path), &export_stem("", name), "m3u8"), text)?;
        }
        Ok(files.len())
    }
}

/// 選んだファイル (XML と .m3u / .m3u8 をいくつでも) を 1 つにまとめて読む。m3u の曲はファイルのタグを読む
fn read_import_files(paths: &[String]) -> AppResult<XmlLibrary> {
    let read_tags = |p: &Path| {
        let i = ipod_core::media::read_info(p).ok()?;
        Some(XmlTrack {
            name: i.title.unwrap_or_default(),
            artist: i.artist.unwrap_or_default(),
            album: i.album.unwrap_or_default(),
            total_time_ms: i.duration_ms,
            track_number: i.track_nr,
            disc_number: i.disc_nr,
            location: None,
        })
    };
    let mut lib = XmlLibrary::default();
    for path in paths {
        let bytes = std::fs::read(path)?;
        let p = Path::new(path);
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        lib.append(if ext == "m3u" || ext == "m3u8" {
            XmlLibrary::from_m3u(&bytes, p, &read_tags)
        } else {
            XmlLibrary::parse(&bytes)?
        });
    }
    Ok(lib)
}
