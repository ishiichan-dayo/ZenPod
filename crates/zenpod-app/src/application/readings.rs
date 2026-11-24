//! 並べ替え用の読み (ジャンプテーブル用)

use std::sync::Arc;

use ipod_core::domain::track::{ReadingChange, ReadingEdit, Readings as TrackReadings};

use super::session::LibrarySession;
use super::snapshot::{EditResult, Snapshot};
use crate::domain::id::{parse_id, parse_ids};
use crate::error::AppResult;

pub struct Readings {
    session: Arc<LibrarySession>,
}

impl Readings {
    pub fn new(session: Arc<LibrarySession>) -> Self {
        Readings { session }
    }

    /// 読みを自動生成する。`ids` が None なら全曲
    pub fn generate(&self, ids: Option<&[String]>, overwrite: bool) -> AppResult<EditResult> {
        let ids = ids.map(parse_ids).transpose()?;
        let (count, snapshot) = self.session.edit(|lib| lib.generate_readings(ids.as_deref(), overwrite))?;
        Ok(EditResult { count, snapshot })
    }

    /// 自動生成のプレビュー (変更はしない)
    pub fn preview(&self, ids: Option<&[String]>, overwrite: bool) -> AppResult<Vec<ReadingChange>> {
        let ids = ids.map(parse_ids).transpose()?;
        self.session.read(|lib| Ok(lib.preview_readings(ids.as_deref(), overwrite)))
    }

    /// プレビューで選んだ読みを反映する
    pub fn apply(&self, edits: &[ReadingEdit]) -> AppResult<EditResult> {
        let (count, snapshot) = self.session.edit(|lib| lib.apply_reading_edits(edits))?;
        Ok(EditResult { count, snapshot })
    }

    pub fn set(&self, id: &str, readings: &TrackReadings) -> AppResult<Snapshot> {
        let id = parse_id(id)?;
        Ok(self.session.edit(|lib| lib.set_readings(id, readings))?.1)
    }

    /// 表記から読みを作る (曲情報の編集画面の「読みを自動で入れる」)。作れなければ None
    pub fn make(&self, texts: &[String]) -> Vec<Option<String>> {
        texts.iter().map(|t| ipod_core::domain::yomi::reading(t)).collect()
    }
}
