//! アーティスト名の表記ゆれ (MusicBrainz の別名も使う)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ipod_core::domain::track::ArtistUnify;
use ipod_core::t;
use ipod_core::domain::variants::VariantGroup;
use serde::Serialize;

use super::events::{AppEvent, Count};
use super::ports::{AliasStore, EventSink, OnlineCatalog};
use super::session::LibrarySession;
use super::snapshot::EditResult;
use crate::error::{AppError, AppResult};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantsResult {
    pub groups: Vec<VariantGroup>,
    /// MusicBrainz でまだ調べていないアーティスト数
    pub unchecked: usize,
}

pub struct Aliases {
    session: Arc<LibrarySession>,
    store: Arc<dyn AliasStore>,
    online: Arc<dyn OnlineCatalog>,
    events: Arc<dyn EventSink>,
    cancel: Arc<AtomicBool>,
}

impl Aliases {
    pub fn new(
        session: Arc<LibrarySession>,
        store: Arc<dyn AliasStore>,
        online: Arc<dyn OnlineCatalog>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Aliases { session, store, online, events, cancel: Arc::default() }
    }

    /// 表記ゆれの候補。調べ済みの MusicBrainz の別名も使う (通信はしない)
    pub fn variants(&self) -> AppResult<VariantsResult> {
        let cache = self.store.load();
        self.session.read(|lib| {
            let unchecked = lib.artist_names().iter().filter(|n| !cache.contains_key(*n)).count();
            Ok(VariantsResult { groups: lib.artist_variants(&cache), unchecked })
        })
    }

    /// まだ調べていないアーティストの別名を MusicBrainz で調べて覚える (1 秒 1 件。画面と別のスレッドで呼ぶ)。
    /// 調べた数を返す
    pub fn lookup(&self) -> AppResult<usize> {
        let names = self.session.read(|lib| Ok(lib.artist_names()))?;
        self.cancel.store(false, Ordering::SeqCst);
        let mut cache = self.store.load();
        let todo: Vec<String> = names.into_iter().filter(|n| !cache.contains_key(n)).collect();
        let total = todo.len();
        let mut done = 0;
        let mut failed_in_row = 0;
        for name in todo {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }
            match self.online.artist_aliases(&name) {
                Ok(list) => {
                    cache.insert(name, list);
                    failed_in_row = 0;
                }
                // 通信の失敗は覚えず次回また調べる。3 回続けて失敗したら諦める
                Err(_) => {
                    failed_in_row += 1;
                    if failed_in_row >= 3 {
                        self.store.save(&cache);
                        return Err(AppError::msg(t!("MusicBrainz に接続できませんでした")));
                    }
                }
            }
            done += 1;
            self.events.emit(AppEvent::AliasProgress(Count { done, total }));
            if done % 20 == 0 {
                self.store.save(&cache);
            }
        }
        self.store.save(&cache);
        Ok(done)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// 表記ゆれを統一する
    pub fn unify(&self, edits: &[ArtistUnify]) -> AppResult<EditResult> {
        let (count, snapshot) = self.session.edit(|lib| lib.unify_artists(edits))?;
        Ok(EditResult { count, snapshot })
    }
}
