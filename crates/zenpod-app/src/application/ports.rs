//! ユースケースが外の世界に触るための窓口。実装は infrastructure (Tauri が要らないもの) と src-tauri (Tauri のもの) にある

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ipod_core::online::artsearch::{self, ImageProbe, SearchResult};
use ipod_core::online::podcast::{Episode, Fetched};
use ipod_core::online::tagsearch::{Naming, Release, ReleaseCandidate};

use super::events::AppEvent;
use crate::error::AppResult;

/// 画面に経過や完了を知らせる
pub trait EventSink: Send + Sync {
    fn emit(&self, event: AppEvent);
}

/// アプリが使うフォルダ。決められなければ None (その場合は保存やキャッシュを諦める)
pub trait AppDirs: Send + Sync {
    /// 設定 (prefs.json) を置く
    fn config_dir(&self) -> Option<PathBuf>;
    /// 消えても困らないもの (タグや縮小画像のキャッシュ)
    fn cache_dir(&self) -> Option<PathBuf>;
    /// 自動で入れた ffmpeg を置く
    fn local_data_dir(&self) -> Option<PathBuf>;
    /// アプリに同梱したもの (bin/ffmpeg)
    fn resource_dir(&self) -> Option<PathBuf>;
}

/// 画面の設定 (キー → 値の文字列)
pub trait PrefsStore: Send + Sync {
    /// 読めなければ空
    fn load(&self) -> HashMap<String, String>;
    fn save(&self, prefs: &HashMap<String, String>) -> AppResult<()>;
}

/// PC の曲のジャケットの縮小画像 (JPEG) の置き場所。キーは元画像の中身から作る
pub trait ThumbStore: Send + Sync {
    /// `sizes` の大きさのものが全部そろっていれば返す (使った日時も更新する)
    fn load(&self, key: &str, sizes: &[u32]) -> Option<Vec<Vec<u8>>>;
    /// 失敗しても表示はできるので、結果は返さない
    fn save(&self, key: &str, sizes: &[u32], jpegs: &[Vec<u8>]);
    /// 曲の場所 → その曲に埋め込まれたジャケットのキー。次の起動で、曲のファイルを開かずに縮小画像を読むため
    fn load_refs(&self) -> HashMap<String, ArtRef>;
    fn save_refs(&self, refs: &HashMap<String, ArtRef>);
}

/// 曲に埋め込まれたジャケットのキー。曲のファイルの大きさと更新日時が同じ間だけ使う
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArtRef {
    pub size: u64,
    /// 更新日時 (UNIX 時刻のナノ秒)
    pub mtime_ns: u64,
    pub key: String,
}

/// MusicBrainz で調べたアーティストの別名 (表記 → 別名。空 = 見つからなかった)
pub trait AliasStore: Send + Sync {
    fn load(&self) -> HashMap<String, Vec<String>>;
    fn save(&self, aliases: &HashMap<String, Vec<String>>);
}

/// ネットの検索 (ジャケット・曲情報・アーティストの別名)。どれも通信するので、画面と別のスレッドで呼ぶ
pub trait OnlineCatalog: Send + Sync {
    /// ジャケットの候補 (見つからなければ空)。経過を `progress` に知らせる
    fn search_artwork(&self, artist: &str, album: &str, progress: artsearch::Progress) -> SearchResult;
    /// 候補画像の解像度・ファイルサイズ
    fn probe_image(&self, url: &str) -> ipod_core::Result<ImageProbe>;
    fn download_image(&self, url: &str) -> ipod_core::Result<Vec<u8>>;
    /// MusicBrainz のアーティストの別名 (見つからなければ空。通信の失敗は Err)
    fn artist_aliases(&self, artist: &str) -> ipod_core::Result<Vec<String>>;
    /// MusicBrainz のアルバムの候補
    fn search_releases(&self, artist: &str, album: &str) -> ipod_core::Result<Vec<ReleaseCandidate>>;
    /// アルバムの曲目
    fn fetch_release(&self, id: &str, naming: Naming) -> ipod_core::Result<Release>;
}

/// ポッドキャストのフィードと回を取ってくる
pub trait PodcastSource: Send + Sync {
    /// `etag` / `last_modified` は前回の値 (変わっていなければ `Fetched::NotModified`)
    fn fetch_feed(&self, url: &str, etag: Option<&str>, last_modified: Option<&str>) -> ipod_core::Result<Fetched>;
    /// 番組や回の画像 (無ければ、または取れなければ None)
    fn fetch_image(&self, url: &str) -> Option<Vec<u8>>;
    /// 回を `dir` に落として、置いたファイルの名前を返す。`progress` は (受け取った量, 全体)。`cancel` が立ったらやめる
    fn download_episode(&self, ep: &Episode, dir: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> ipod_core::Result<String>;
}

/// ffmpeg をダウンロードして `dest` に入れる (Windows / macOS)
pub trait FfmpegInstaller: Send + Sync {
    /// `progress` は (受け取った量, 全体の量)。`cancel` が立ったら途中でやめる
    fn install(&self, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> ipod_core::Result<PathBuf>;
}

/// ffmpeg を動かす: WebView が再生できない形式を再生用に FLAC に変換する・選んだファイルが ffmpeg か確かめる
pub trait Transcoder: Send + Sync {
    /// 失敗したら None
    fn to_flac(&self, ffmpeg: &Path, src: &Path) -> Option<Vec<u8>>;
    /// `-version` で動かして ffmpeg と名乗るか (少し待って答えなければ false)
    fn is_ffmpeg(&self, path: &Path) -> bool;
}
