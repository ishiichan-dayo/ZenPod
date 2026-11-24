//! 曲の詳細情報 (「詳細情報」の画面と、一覧の下の形式の要約)

use std::path::PathBuf;
use std::sync::Arc;

use ipod_core::media::details::{AudioSummary, FileDetails};
use ipod_core::domain::track::{DbDetails, TrackOptions};
use ipod_core::t;
use serde::Serialize;

use super::prefs::Prefs;
use super::session::LibrarySession;
use crate::domain::id::parse_id;
use crate::error::{AppError, AppResult};

/// iPod の曲は iTunesDB の値も返す (ファイルが読めなくても DB の値だけは出す)
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetails {
    pub file: Option<FileDetails>,
    pub file_error: Option<String>,
    /// ファイルを読めなかったときの、タグの構造 (名前と大きさだけ。報告に貼ってもらう)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_structure: Option<String>,
    pub db: Option<DbDetails>,
}

pub struct Details {
    session: Arc<LibrarySession>,
    prefs: Arc<Prefs>,
}

impl Details {
    pub fn new(session: Arc<LibrarySession>, prefs: Arc<Prefs>) -> Self {
        Details { session, prefs }
    }

    /// 「フォルダで表示」する曲のファイルの場所。id は iPod の dbid か "pc:<場所>"。
    /// PC の曲は登録フォルダの中の音声ファイルだけ (画面から好きな場所を開かせない)
    pub fn file_location(&self, id: &str) -> AppResult<PathBuf> {
        let path = match id.strip_prefix("pc:") {
            Some(p) => Some(PathBuf::from(p)).filter(|p| self.prefs.pc_scope().allows_audio(p)),
            None => {
                let id = parse_id(id)?;
                self.session.read(|lib| Ok(lib.track_file(id)))?.filter(|p| p.is_file())
            }
        };
        path.ok_or_else(|| AppError::msg(t!("曲のファイルが見つかりません")))
    }

    /// 曲のファイルの歌詞 (情報の編集の「歌詞」)。無ければ None。ファイルを読むので画面と別のスレッドで呼ぶ
    pub fn lyrics(&self, id: &str) -> AppResult<Option<String>> {
        Ok(ipod_core::media::read_lyrics(&self.file_location(id)?)?)
    }

    /// iTunes の「オプション」の今の値 (情報の編集の画面)。iPod OS の曲だけ。ids の順で、見つからない曲は None
    pub fn options(&self, ids: &[String]) -> AppResult<Vec<Option<TrackOptions>>> {
        let ids = ids.iter().map(|id| parse_id(id)).collect::<AppResult<Vec<u64>>>()?;
        self.session.read(|lib| Ok(ids.iter().map(|&id| lib.track_options(id)).collect()))
    }

    /// iPod の曲は id (dbid)、PC の曲は path を渡す。ファイルを読むので画面と別のスレッドで呼ぶ
    pub fn track(&self, id: Option<&str>, path: Option<PathBuf>) -> AppResult<TrackDetails> {
        let (file, db) = match (id, path) {
            (Some(id), _) => {
                let id = parse_id(id)?;
                self.session.read(|lib| Ok((lib.track_file(id), lib.track_db_details(id))))?
            }
            (None, Some(p)) => (Some(p), None),
            (None, None) => return Err(AppError::msg(t!("曲が指定されていません"))),
        };
        let (file, file_error, file_structure) = match file {
            Some(p) => match ipod_core::media::details::file_details(&p) {
                Ok(d) => (Some(d), None, None),
                Err(e) => (None, Some(e.to_string()), ipod_core::media::structure::tag_structure(&p).ok()),
            },
            None => (None, Some(t!("ファイルの場所が記録されていません")), None),
        };
        Ok(TrackDetails { file, file_error, file_structure, db })
    }

    /// 選択中の曲の形式の要約。ids は iPod の dbid か "pc:<場所>"。読めない曲は None
    pub fn audio_summaries(&self, ids: &[String]) -> Vec<Option<AudioSummary>> {
        let paths: Vec<Option<PathBuf>> = self
            .session
            .try_read(|lib| ids.iter().map(|id| file_of(id, Some(lib))).collect())
            .unwrap_or_else(|| ids.iter().map(|id| file_of(id, None)).collect());
        paths.iter().map(|p| ipod_core::media::details::audio_summary(p.as_deref()?).ok()).collect()
    }
}

fn file_of(id: &str, lib: Option<&ipod_core::library::Library>) -> Option<PathBuf> {
    match id.strip_prefix("pc:") {
        Some(p) => Some(PathBuf::from(p)),
        None => lib?.track_file(id.parse().ok()?),
    }
}
