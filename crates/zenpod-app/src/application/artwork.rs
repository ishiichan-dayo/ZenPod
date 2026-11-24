//! iPod の曲のジャケットと、ネットのジャケット候補

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use ipod_core::online::artsearch::{ImageProbe, SearchResult};
use serde::Serialize;

use super::events::{AppEvent, ArtworkProgress, ArtworkSearch};
use super::ports::{EventSink, OnlineCatalog};
use super::session::LibrarySession;
use super::snapshot::Snapshot;
use crate::domain::id::parse_ids;
use crate::domain::media_type::image_mime;
use crate::error::AppResult;

/// 曲のファイルに埋め込まれた画像 (情報の編集のアートワークのタブに出す)
#[derive(Serialize)]
pub struct EmbeddedArt {
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    /// 縮めなくてよい (ベースラインの JPEG で長い辺が指定の大きさ以下)
    pub fits: bool,
}

impl EmbeddedArt {
    /// `path` の曲に埋め込まれた画像 (無い・読めなければ None)。`fits` は長い辺 `size` で縮めなくてよいか
    pub(crate) fn of(path: &Path, size: Option<u32>) -> Option<EmbeddedArt> {
        use ipod_core::media::cover;
        let Some((bytes, true)) = cover::find_cover_src(path) else {
            return None;
        };
        let (width, height) = cover::dimensions(&bytes)?;
        Some(EmbeddedArt { width, height, bytes: bytes.len(), fits: cover::fits(&bytes, cover::embed_size(size)) })
    }
}

/// `shrink` (長い辺の px) があれば、画像をその大きさまでのベースラインの JPEG にする (読めない画像はそのまま)
pub(crate) fn prepared(bytes: Vec<u8>, shrink: Option<u32>) -> Vec<u8> {
    use ipod_core::media::cover::{embed_size, fit_jpeg, fits};
    let Some(max) = shrink.map(|px| embed_size(Some(px))) else {
        return bytes;
    };
    if fits(&bytes, max) {
        return bytes;
    }
    fit_jpeg(&bytes, max).unwrap_or(bytes)
}

/// iPod の曲の埋め込みのジャケットを縮めた結果
#[derive(Serialize)]
pub struct Shrunk {
    pub snapshot: Snapshot,
    /// 縮めた曲 (もともと小さい曲・画像の無い曲は数えない)
    pub shrunk: usize,
    /// 書き換えに失敗した曲
    pub failed: usize,
}

pub struct Artwork {
    session: Arc<LibrarySession>,
    online: Arc<dyn OnlineCatalog>,
    events: Arc<dyn EventSink>,
}

impl Artwork {
    pub fn new(session: Arc<LibrarySession>, online: Arc<dyn OnlineCatalog>, events: Arc<dyn EventSink>) -> Self {
        Artwork { session, online, events }
    }

    /// ジャケットを data URL (JPEG) で返す。`large` なら最大サイズ。
    /// アルバム表示では一度に何十枚も読むので、画面と別のスレッドで呼ぶ
    pub fn get(&self, image_id: u32, large: bool) -> Option<String> {
        if let Some(hit) = self.session.cached_artwork((image_id, large)) {
            return Some(hit.to_string());
        }
        let jpg = self.session.try_read(|lib| lib.artwork_jpeg(image_id, large)).flatten()?;
        let url = Arc::new(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)));
        self.session.remember_artwork((image_id, large), url.clone());
        Some(url.to_string())
    }

    /// 画像ファイルを曲のジャケットにする。縮小と ithmb への書き込みに時間がかかるので画面と別のスレッドで呼ぶ。
    /// `shrink` (長い辺の px) があれば、曲のファイルに埋め込む画像を先に縮める
    /// (Rockbox。iPod OS は埋め込まず、ArtworkDB は 320px までなので変わらない)
    pub fn set_from_file(&self, ids: &[String], path: &str, shrink: Option<u32>) -> AppResult<Snapshot> {
        let ids = parse_ids(ids)?;
        self.events.emit(ArtworkProgress::write(0, ids.len()));
        if shrink.is_none() {
            return Ok(self.session.edit(|lib| lib.set_artwork_from_file(&ids, Path::new(path)))?.1);
        }
        let bytes = prepared(std::fs::read(path)?, shrink);
        Ok(self.session.edit(|lib| lib.set_artwork_from_bytes(&ids, &bytes))?.1)
    }

    /// 曲のファイルに埋め込まれた画像の大きさ (無ければ None)。`fits` は長い辺 `size` で縮めなくてよいか
    pub fn embedded(&self, id: &str, size: Option<u32>) -> AppResult<Option<EmbeddedArt>> {
        let id = crate::domain::id::parse_id(id)?;
        let Some(path) = self.session.read(|lib| Ok(lib.track_file(id)))? else {
            return Ok(None);
        };
        Ok(EmbeddedArt::of(&path, size))
    }

    /// iPod の曲の埋め込みのジャケットを縮める (Rockbox は表示が速くなり、どちらのモードも容量が減る)。
    /// 曲のファイルを 1 曲ずつ書き直すので画面と別のスレッドで呼ぶ
    pub fn shrink(&self, ids: &[String], size: Option<u32>) -> AppResult<Shrunk> {
        let ids = parse_ids(ids)?;
        let max = ipod_core::media::cover::embed_size(size);
        let events = self.events.clone();
        let progress = move |done, total| events.emit(ArtworkProgress::write(done, total));
        let ((shrunk, failed), snapshot) =
            self.session.edit(|lib| lib.shrink_artwork(&ids, max, &progress))?;
        Ok(Shrunk { snapshot, shrunk, failed })
    }

    pub fn remove(&self, ids: &[String]) -> AppResult<Snapshot> {
        let ids = parse_ids(ids)?;
        self.events.emit(ArtworkProgress::write(0, ids.len()));
        Ok(self.session.edit(|lib| lib.remove_artwork(&ids))?.1)
    }

    /// 候補画像をダウンロードして曲のジャケットにする (`shrink` は `set_from_file` と同じ)
    pub fn apply_url(&self, ids: &[String], url: &str, shrink: Option<u32>) -> AppResult<Snapshot> {
        let ids = parse_ids(ids)?;
        self.events.emit(ArtworkProgress::download(ids.len()));
        let bytes = prepared(self.online.download_image(url)?, shrink);
        self.events.emit(ArtworkProgress::write(0, ids.len()));
        Ok(self.session.edit(|lib| lib.set_artwork_from_bytes(&ids, &bytes))?.1)
    }

    /// ネット上のジャケット候補を検索する (通信するので画面と別のスレッドで呼ぶ)。経過は `token` を付けて画面に知らせる
    /// (アルバムごとに探すときは、次のアルバムも並行して探すので、どの検索の経過かを見分ける)
    pub fn search(&self, artist: &str, album: &str, token: u32) -> SearchResult {
        let events = self.events.clone();
        self.online.search_artwork(artist, album, Arc::new(move |progress| events.emit(AppEvent::ArtworkSearch(ArtworkSearch { token, progress }))))
    }

    /// 候補画像の解像度・ファイルサイズ
    pub fn probe(&self, url: &str) -> Option<ImageProbe> {
        self.online.probe_image(url).ok()
    }

    /// 候補のサムネイルを data URL で返す (WebView が直接読めない環境向けの代替)
    pub fn fetch_image(&self, url: &str) -> Option<String> {
        let bytes = self.online.download_image(url).ok()?;
        Some(format!("data:{};base64,{}", image_mime(&bytes), base64::engine::general_purpose::STANDARD.encode(&bytes)))
    }
}
