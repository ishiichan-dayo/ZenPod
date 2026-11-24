//! iPod の曲の曲情報の書き換えと、ネット (MusicBrainz) で曲情報を探すこと

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use ipod_core::domain::track::{TagEdit, TrackTagEdit, TrackView};
use ipod_core::online::tagsearch::{Assignments, Naming, Release, ReleaseCandidate, TagDiff};

use super::ports::OnlineCatalog;
use super::session::LibrarySession;
use super::snapshot::EditResult;
use crate::domain::id::parse_ids;
use crate::error::AppResult;

pub struct Tags {
    session: Arc<LibrarySession>,
    online: Arc<dyn OnlineCatalog>,
    /// 直近に取ったリリース (ID, 表記, 曲目)。曲の対応を選び直すたびに MusicBrainz へ問い合わせ直さないため
    release: Mutex<Option<(String, Naming, Arc<Release>)>>,
}

impl Tags {
    pub fn new(session: Arc<LibrarySession>, online: Arc<dyn OnlineCatalog>) -> Self {
        Tags { session, online, release: Mutex::default() }
    }

    /// 曲情報 (曲名・アーティスト・アルバムなど) を書き換える
    pub fn edit(&self, ids: &[String], edit: &TagEdit) -> AppResult<EditResult> {
        let ids = parse_ids(ids)?;
        let (count, snapshot) = self.session.edit(|lib| lib.edit_tags(&ids, edit))?;
        Ok(EditResult { count, snapshot })
    }

    /// 曲ごとに違う内容で曲情報を書き換える
    pub fn apply_edits(&self, edits: &[TrackTagEdit]) -> AppResult<EditResult> {
        let (count, snapshot) = self.session.edit(|lib| lib.apply_tag_edits(edits))?;
        Ok(EditResult { count, snapshot })
    }

    /// アルバムの候補 (通信するので画面と別のスレッドで呼ぶ)
    pub fn search_releases(&self, artist: &str, album: &str) -> AppResult<Vec<ReleaseCandidate>> {
        Ok(self.online.search_releases(artist, album)?)
    }

    /// リリースの曲目 (直前と同じリリース・表記なら取っておいたものを使う)
    fn release(&self, release_id: &str, naming: Naming) -> AppResult<Arc<Release>> {
        let hit = self
            .release
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(id, n, _)| id == release_id && *n == naming)
            .map(|(_, _, r)| r.clone());
        if let Some(r) = hit {
            return Ok(r);
        }
        let rel = Arc::new(self.online.fetch_release(release_id, naming)?);
        *self.release.lock().unwrap() = Some((release_id.to_string(), naming, rel.clone()));
        Ok(rel)
    }

    /// 選んだアルバムの曲目と iPod の曲を対応付けて、変わる項目を返す (変更はしない)。
    /// `assign` で曲の対応を手で決められる (曲 ID → 曲目の添字、null = 反映しない)
    pub fn preview_release(
        &self,
        ids: &[String],
        release_id: &str,
        naming: Option<Naming>,
        assign: Option<Assignments>,
    ) -> AppResult<TagDiff> {
        let naming = naming.unwrap_or_default();
        let rel = self.release(release_id, naming)?;
        let ids: HashSet<&String> = ids.iter().collect();
        let tracks: Vec<TrackView> =
            self.session.read(|lib| Ok(lib.tracks().into_iter().filter(|t| ids.contains(&t.id)).collect()))?;
        Ok(ipod_core::online::tagsearch::diff(&tracks, &rel, naming, &assign.unwrap_or_default()))
    }

    /// PC の曲とアルバムの曲目を対応付けて、変わる項目を返す (変更はしない)
    pub fn preview_release_pc(
        &self,
        tracks: &[TrackView],
        release_id: &str,
        naming: Option<Naming>,
        assign: Option<Assignments>,
    ) -> AppResult<TagDiff> {
        let naming = naming.unwrap_or_default();
        let rel = self.release(release_id, naming)?;
        Ok(ipod_core::online::tagsearch::diff(tracks, &rel, naming, &assign.unwrap_or_default()))
    }
}
