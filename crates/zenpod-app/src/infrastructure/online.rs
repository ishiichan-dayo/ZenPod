//! ネットの検索と ffmpeg のダウンロードを ipod-core で行う

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ipod_core::media::ffinstall;
use ipod_core::online::artsearch::{self, ImageProbe, SearchResult};
use ipod_core::online::podcast::{self, Episode, Fetched};
use ipod_core::online::tagsearch::{self, Naming, Release, ReleaseCandidate};

use crate::application::ports::{FfmpegInstaller, OnlineCatalog, PodcastSource};

/// iTunes / Deezer / MusicBrainz + Cover Art Archive
pub struct WebCatalog;

impl OnlineCatalog for WebCatalog {
    fn search_artwork(&self, artist: &str, album: &str, progress: artsearch::Progress) -> SearchResult {
        artsearch::search_with(artist, album, progress)
    }

    fn probe_image(&self, url: &str) -> ipod_core::Result<ImageProbe> {
        artsearch::probe(url)
    }

    fn download_image(&self, url: &str) -> ipod_core::Result<Vec<u8>> {
        artsearch::download(url)
    }

    fn artist_aliases(&self, artist: &str) -> ipod_core::Result<Vec<String>> {
        artsearch::artist_aliases(artist)
    }

    fn search_releases(&self, artist: &str, album: &str) -> ipod_core::Result<Vec<ReleaseCandidate>> {
        tagsearch::search_releases(artist, album)
    }

    fn fetch_release(&self, id: &str, naming: Naming) -> ipod_core::Result<Release> {
        tagsearch::fetch_release(id, naming)
    }
}

/// ポッドキャストの配信元 (RSS と音声のファイル)
pub struct PodcastWeb;

impl PodcastSource for PodcastWeb {
    fn fetch_feed(&self, url: &str, etag: Option<&str>, last_modified: Option<&str>) -> ipod_core::Result<Fetched> {
        podcast::fetch_feed(url, etag, last_modified)
    }

    fn fetch_image(&self, url: &str) -> Option<Vec<u8>> {
        podcast::fetch_image(url)
    }

    fn download_episode(&self, ep: &Episode, dir: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> ipod_core::Result<String> {
        podcast::download_episode(ep, dir, cancel, progress)
    }
}

/// Windows は BtbN、macOS は martin-riedl.de の配布物 (どちらも ipod-core が確かめる)
pub struct FfmpegDownload;

impl FfmpegInstaller for FfmpegDownload {
    fn install(&self, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> ipod_core::Result<PathBuf> {
        let asset = ffinstall::latest_asset()?;
        ffinstall::install(&asset, dest, cancel, progress)
    }
}
