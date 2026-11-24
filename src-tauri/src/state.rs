//! Tauri に `manage` させる状態

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use zenpod_app::application::{Deps, ZenPod};
use zenpod_app::infrastructure::alias_file::AliasFile;
use zenpod_app::infrastructure::ffmpeg_transcoder::FfmpegTranscoder;
use zenpod_app::infrastructure::online::{FfmpegDownload, PodcastWeb, WebCatalog};
use zenpod_app::infrastructure::prefs_file::PrefsFile;
use zenpod_app::infrastructure::thumb_dir::ThumbDir;

use crate::host::TauriHost;

pub struct AppState {
    pub zen: Arc<ZenPod>,
    pub host: Arc<TauriHost>,
    /// 起動時に古いものを消す
    pub thumbs: Arc<ThumbDir>,
    /// 画面側が起動できたか (ui_ready が呼ばれたら true)
    pub ui_ready: Arc<AtomicBool>,
}

impl AppState {
    /// ユースケースとポートの実装を組み立てる
    pub fn new() -> Self {
        let host = Arc::new(TauriHost::default());
        let thumbs = Arc::new(ThumbDir::new(host.clone()));
        let zen = ZenPod::new(Deps {
            events: host.clone(),
            dirs: host.clone(),
            prefs: Arc::new(PrefsFile::new(host.clone())),
            thumbs: thumbs.clone(),
            aliases: Arc::new(AliasFile::new(host.clone())),
            transcoder: Arc::new(FfmpegTranscoder),
            online: Arc::new(WebCatalog),
            ffmpeg_installer: Arc::new(FfmpegDownload),
            podcast_source: Arc::new(PodcastWeb),
        });
        AppState { zen: Arc::new(zen), host, thumbs, ui_ready: Arc::default() }
    }
}
