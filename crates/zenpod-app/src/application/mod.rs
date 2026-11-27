//! ユースケース。画面からの 1 つの操作が、ここの 1 つのメソッドになる

pub mod aliases;
pub mod artwork;
pub mod details;
pub mod devices;
pub mod events;
pub mod ffmpeg;
pub mod pc_artwork;
pub mod pc_library;
pub mod playback;
pub mod playlists;
pub mod podcasts;
pub mod ports;
pub mod prefs;
pub mod readings;
pub mod rockbox;
mod rockbox_db;
mod rockbox_reread;
pub mod session;
pub mod snapshot;
pub mod tags;
pub mod transfer;
mod transfer_ahead;
pub mod video_play;
pub mod visual;

use std::sync::Arc;

use ports::{AliasStore, AppDirs, EventSink, FfmpegInstaller, OnlineCatalog, PodcastSource, PrefsStore, ThumbStore, Transcoder};

/// ユースケースが使う外の世界 (ポートの実装)
pub struct Deps {
    pub events: Arc<dyn EventSink>,
    pub dirs: Arc<dyn AppDirs>,
    pub prefs: Arc<dyn PrefsStore>,
    pub thumbs: Arc<dyn ThumbStore>,
    pub aliases: Arc<dyn AliasStore>,
    pub transcoder: Arc<dyn Transcoder>,
    pub online: Arc<dyn OnlineCatalog>,
    pub ffmpeg_installer: Arc<dyn FfmpegInstaller>,
    pub podcast_source: Arc<dyn PodcastSource>,
}

/// アプリのユースケース一式 (アプリ全体で 1 つ)
pub struct ZenPod {
    pub session: Arc<LibrarySession>,
    pub prefs: Arc<prefs::Prefs>,
    pub ffmpeg: Arc<ffmpeg::Ffmpeg>,
    pub devices: devices::Devices,
    pub transfer: transfer::Transfer,
    pub playlists: playlists::Playlists,
    pub artwork: artwork::Artwork,
    pub tags: tags::Tags,
    pub readings: readings::Readings,
    pub aliases: aliases::Aliases,
    pub rockbox: rockbox::Rockbox,
    pub pc: pc_library::PcLibrary,
    pub playback: playback::Playback,
    pub details: details::Details,
    pub podcasts: podcasts::Podcasts,
    pub visual: visual::Visual,
    pub video_player: video_play::VideoPlayer,
}

pub use session::LibrarySession;

impl ZenPod {
    pub fn new(deps: Deps) -> Self {
        let Deps { events, dirs, prefs, thumbs, aliases, transcoder, online, ffmpeg_installer, podcast_source } = deps;
        let session = Arc::new(LibrarySession::new(events.clone()));
        let prefs = Arc::new(prefs::Prefs::new(prefs));
        let ffmpeg = Arc::new(ffmpeg::Ffmpeg::new(dirs.clone(), ffmpeg_installer, events.clone(), transcoder.clone(), prefs.clone()));
        let pc_artwork = Arc::new(pc_artwork::PcArtwork::new(prefs.clone(), thumbs));
        ZenPod {
            devices: devices::Devices::new(session.clone(), dirs.clone(), events.clone()),
            transfer: transfer::Transfer::new(session.clone(), ffmpeg.clone(), pc_artwork.clone(), events.clone()),
            playlists: playlists::Playlists::new(session.clone()),
            artwork: artwork::Artwork::new(session.clone(), online.clone(), events.clone()),
            tags: tags::Tags::new(session.clone(), online.clone()),
            readings: readings::Readings::new(session.clone()),
            aliases: aliases::Aliases::new(session.clone(), aliases, online.clone(), events.clone()),
            rockbox: rockbox::Rockbox::new(session.clone()),
            podcasts: podcasts::Podcasts::new(dirs.clone(), podcast_source, events.clone(), pc_artwork.clone()),
            pc: pc_library::PcLibrary::new(prefs.clone(), pc_artwork, online, dirs, events),
            playback: playback::Playback::new(session.clone(), ffmpeg.clone(), prefs.clone(), transcoder),
            details: details::Details::new(session.clone(), prefs.clone()),
            visual: visual::Visual::new(session.clone(), ffmpeg.clone()),
            video_player: video_play::VideoPlayer::new(session.clone(), ffmpeg.clone()),
            session,
            prefs,
            ffmpeg,
        }
    }
}
