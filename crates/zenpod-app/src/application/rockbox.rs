//! Rockbox モードだけの操作: 曲の置き場所の形・ファイル名の変更・データベースの作り直し

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::session::LibrarySession;
use super::snapshot::Snapshot;
use crate::domain::id::parse_id;
use crate::error::{AppError, AppResult};

#[derive(Deserialize)]
pub struct TrackRename {
    pub id: String,
    /// 新しい名前 (拡張子なし)
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RockboxDbResult {
    pub built: ipod_core::rockbox::rbdb::Built,
    pub snapshot: Snapshot,
}

pub struct Rockbox {
    session: Arc<LibrarySession>,
}

impl Rockbox {
    pub fn new(session: Arc<LibrarySession>) -> Self {
        Rockbox { session }
    }

    /// 曲を転送するときの置き場所の形を変える (画面が iPod ごとに覚えていて、開いたときにも渡す)。
    /// 転送中は 1 曲ごとに lock を持つので、待つ間に画面が止まらないよう画面と別のスレッドで呼ぶ
    pub fn set_layout(&self, template: &str) -> AppResult<()> {
        self.session.write(|lib| Ok(lib.set_rockbox_layout(template)?))
    }

    /// 置き場所の形の見本 ("Music/アーティスト/アルバム/01 曲名.flac")
    pub fn preview_layout(&self, template: &str) -> AppResult<String> {
        self.session.read(|lib| Ok(lib.preview_rockbox_layout(template)?))
    }

    /// 今ある曲の並びから置き場所の形を推測する (曲が多いと少し時間がかかる)
    pub fn guess_layout(&self) -> AppResult<Option<ipod_core::domain::layout::Guess>> {
        self.session.read(|lib| Ok(lib.guess_rockbox_layout()))
    }

    /// 曲のファイル名をまとめて変える。1 つでも問題があれば何も変えない
    pub fn rename_files(&self, renames: Vec<TrackRename>) -> AppResult<Snapshot> {
        let pairs = renames
            .into_iter()
            .map(|r| Ok((parse_id(&r.id)?, r.name)))
            .collect::<AppResult<Vec<(u64, String)>>>()?;
        Ok(self.session.edit(|lib| lib.rename_files(&pairs))?.1)
    }

    /// データベースを PC で作り直す。曲が多いとタグの読み取りに時間がかかるので画面と別のスレッドで呼ぶ
    pub fn rebuild_db(&self) -> AppResult<RockboxDbResult> {
        let _busy = self.session.try_begin_exclusive().ok_or(AppError::Busy)?;
        // 作っている間は iPod の lock を持たない (つながっているかの確認などを待たせない)
        let (root, cache) = self.session.read(|lib| Ok((lib.root().to_path_buf(), lib.rockbox_cache_path())))?;
        let built = self.session.rebuild_rockbox_db_now(&root, cache.as_deref());
        self.session.write(|lib| {
            lib.set_rockbox_db_error(built.as_ref().err().map(|e| e.to_string()));
            Ok(RockboxDbResult { built: built?, snapshot: Snapshot::full(lib) })
        })
    }
}
