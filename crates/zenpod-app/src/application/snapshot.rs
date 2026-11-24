//! 画面に返す、開いている iPod の今の状態

use ipod_core::dir_size;
use ipod_core::library::{Library, Mode};
use ipod_core::device::DeviceInfo;
use ipod_core::domain::playlist::PlaylistView;
use ipod_core::domain::track::TrackView;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub device: DeviceInfo,
    pub name: String,
    /// 曲の一覧。プレイリストだけを変えた操作では送らない (画面は今の一覧を使い回す。2 万曲で 8MB ほどあるため)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackView>>,
    pub playlists: Vec<PlaylistView>,
    pub writable_error: Option<String>,
    pub artwork_supported: bool,
    /// 容量の内訳。曲の一覧を送らないときは送らない (写真などのフォルダを数え直さない)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// iPod OS (iTunesDB) か Rockbox (ファイルそのもの) か
    pub mode: Mode,
    /// Rockbox モードのときの Rockbox の設定 (DB の自動更新など)
    pub rockbox: Option<ipod_core::rockbox::Status>,
    /// Rockbox が入っている (`.rockbox` がある)。無い iPod ではモードの切り替えを出さない
    pub has_rockbox: bool,
    /// Rockbox モードで開いたときの読み込みの内訳 (開いたときだけ。利用状況に送る)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<ipod_core::rockbox::ScanStats>,
    /// 開いたときだけ: 開くのにかかった時間の内訳 (両方のモード)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub times: Option<ipod_core::device::OpenTimes>,
}

/// 容量の内訳 (バイト)。使用量からこれらを引いた残りが「その他」
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// 曲のファイルの合計 (DB に載っている曲)
    pub music: u64,
    /// 写真 (iPod OS は /Photos の縮小画像と原寸の画像、Rockbox は /Pictures)
    pub photos: u64,
    /// 動画 (iPod OS は DB の動画、Rockbox は /Videos)
    pub videos: u64,
    /// アートワーク (iPod_Control/Artwork の画像)
    pub artwork: u64,
}

/// 写真・動画のフォルダの大きさ (容量の内訳のうち、フォルダを数えるもの)。
/// 曲の操作 (`LibrarySession::edit`) では変わらないので、そのあとは前に数えた値を使い回す
/// (Mac の FAT の iPod は写真が多いと数えるのに数秒かかり、曲の情報を 1 つ直すたびに待たされていた)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaDirs {
    /// iPod OS は /Photos、Rockbox は /Photos と /Pictures
    photos: u64,
    /// Rockbox の /Videos (iPod OS の動画は DB の大きさを使うので 0)
    videos: u64,
}

impl MediaDirs {
    pub fn measure(lib: &Library) -> MediaDirs {
        let root = lib.root();
        let rockbox = lib.mode() == Mode::Rockbox;
        MediaDirs {
            photos: dir_size(&root.join("Photos")) + if rockbox { dir_size(&root.join("Pictures")) } else { 0 },
            // Rockbox の動画は一覧を作ると長さを読むので、フォルダの大きさだけ数える
            videos: if rockbox { dir_size(&root.join("Videos")) } else { 0 },
        }
    }
}

impl Snapshot {
    /// 曲の一覧と容量の内訳も入れたもの
    pub fn full(lib: &Library) -> Snapshot {
        Self::full_with(lib, MediaDirs::measure(lib))
    }

    /// `full` と同じ。写真・動画のフォルダの大きさは `media` を使う (数え直さない)
    pub fn full_with(lib: &Library, media: MediaDirs) -> Snapshot {
        let tracks = lib.tracks();
        let rockbox = lib.mode() == Mode::Rockbox;
        let usage = Usage {
            music: tracks.iter().map(|t| t.size as u64).sum(),
            photos: media.photos,
            videos: if rockbox { media.videos } else { lib.videos().iter().map(|v| v.size).sum() },
            // アートワークは曲の操作で変わる。ファイルは数個なので毎回数える
            artwork: dir_size(&lib.root().join("iPod_Control").join("Artwork")),
        };
        Snapshot { tracks: Some(tracks), usage: Some(usage), ..Snapshot::playlists_only(lib) }
    }

    /// 曲の一覧と容量の内訳を除いたもの (プレイリストだけを変えた操作の後)
    pub fn playlists_only(lib: &Library) -> Snapshot {
        Snapshot {
            device: lib.info().clone(),
            mode: lib.mode(),
            rockbox: lib.rockbox_status(),
            has_rockbox: lib.root().join(".rockbox").is_dir(),
            name: lib.name().to_string(),
            usage: None,
            tracks: None,
            playlists: lib.playlists(),
            writable_error: lib.check_writable().err().map(|e| e.to_string()),
            artwork_supported: lib.artwork_supported(),
            scan: None,
            times: None,
        }
    }
}

/// 件数と、変えた後の状態 (読みの生成・曲情報の書き換えなど)
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditResult {
    pub count: usize,
    pub snapshot: Snapshot,
}
