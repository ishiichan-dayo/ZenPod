//! 動画: 画面に渡す形・画面から受け取る編集・転送の設定

use serde::{Deserialize, Serialize};

/// 動画の種類。iPod OS では iTunesDB の mediatype (2 / 0x20 / 0x40)、Rockbox では使わない (すべてムービー)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoKind {
    #[default]
    Movie,
    MusicVideo,
    TvShow,
}

impl VideoKind {
    /// MP4 のタグ (stik) と番組名から決める
    pub fn from_tags(stik: Option<u8>, has_show: bool) -> Self {
        match stik {
            Some(6) => VideoKind::MusicVideo,
            Some(10) => VideoKind::TvShow,
            Some(9) | Some(0) => VideoKind::Movie,
            _ if has_show => VideoKind::TvShow,
            _ => VideoKind::Movie,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoView {
    /// iPod OS は dbid、Rockbox はルートからの場所の SHA-256 (曲と同じ。JS の Number 精度対策で文字列)
    pub id: String,
    pub title: String,
    pub kind: VideoKind,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub composer: String,
    pub grouping: String,
    pub comment: String,
    pub year: u32,
    pub track_nr: u32,
    pub disc_nr: u32,
    /// リリース日 (UNIX 秒、0 = なし)
    pub released: i64,
    /// 並べ替え用の読み (空 = 表記のまま並ぶ)
    pub sort_title: String,
    pub sort_artist: String,
    pub sort_album: String,
    pub sort_album_artist: String,
    pub sort_composer: String,
    pub sort_show: String,
    /// テレビ番組の番組名
    pub show: String,
    pub season: u32,
    pub episode: u32,
    /// エピソード ID (iTunes の「エピソード ID」。"S1E02" など、自由な文字)
    pub episode_id: String,
    /// 放送局
    pub network: String,
    pub description: String,
    /// 評価 (0〜100、★ 1 つが 20)
    pub rating: u8,
    /// シャッフルで飛ばす
    pub skip_shuffle: bool,
    /// 再生した位置を覚える
    pub remember_position: bool,
    /// 音量の調整 (-255〜255)
    pub volume: i32,
    /// 再生を始める・止める位置 (ミリ秒。止める位置の 0 は最後まで)
    pub start_ms: u32,
    pub stop_ms: u32,
    /// イコライザのプリセットの番号 (0 = なし)
    pub eq: u32,
    pub duration_ms: u32,
    pub size: u64,
    pub date_added: i64,
    /// ArtworkDB の画像 id (0 = なし。Rockbox は持たない)
    pub artwork_id: u32,
    /// アートワークを利用者が選んだ (一覧には動画の 1 コマではなくこの画像を出す)
    pub custom_art: bool,
    pub play_count: u32,
    /// 最後に再生した日時 (UNIX 秒、0 = なし)
    pub last_played: i64,
    pub date_modified: i64,
    /// iTunesDB の種類の文字 ("MPEG-4 video file") とビットレート・サンプルレート、iPod の中の場所 (iPod OS だけ)
    pub filetype: String,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub ipod_path: String,
    /// Rockbox の動画の、iPod のルートからの場所 ("/Videos/a.mpg")。iPod OS は空
    #[serde(skip_serializing_if = "String::is_empty")]
    pub rb_path: String,
}

/// iPod OS で変換するときの画質 (大きさ)。設定には "640" / "320" で持つ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoSize {
    /// 640×480 (テレビにつないでもきれい)
    #[default]
    Vga,
    /// 320×240 (画面の大きさ。容量と変換の時間が小さい)
    Qvga,
}

impl VideoSize {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "640" => Some(VideoSize::Vga),
            "320" => Some(VideoSize::Qvga),
            _ => None,
        }
    }
}

/// 動画を送るときの設定
#[derive(Debug, Clone)]
pub struct VideoOptions {
    /// iPod OS で変換するときの大きさ
    pub size: VideoSize,
    /// 画面で開いているところ (ムービー・番組など) に合わせて、種類・番組・アーティストを決める
    pub placement: VideoPlacement,
    /// 同じ動画 (題名・番組・シーズン・話数が同じで長さもほぼ同じ) は送らない
    pub skip_duplicates: bool,
    /// 一覧の画像 (埋め込みの画像か 1 コマ) を ArtworkDB に入れる
    pub artwork: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        VideoOptions { size: VideoSize::Vga, skip_duplicates: true, artwork: true, placement: VideoPlacement::default() }
    }
}

/// 動画を置く場所 (画面のビデオの左の一覧で開いているところ)。指定したものだけ、動画のタグの代わりに使う
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoPlacement {
    pub kind: Option<VideoKind>,
    /// テレビ番組の番組名
    pub show: Option<String>,
    /// ミュージックビデオのアーティスト
    pub artist: Option<String>,
}

/// 動画の情報の編集。None の項目は変えない。文字列の空文字は消す。
/// 名前 (題名・アーティスト・アルバム・番組など) を変えて読みを指定しなければ、読みは自動で付け直す (曲と同じ)
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoEdit {
    pub title: Option<String>,
    pub kind: Option<VideoKind>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub year: Option<u32>,
    pub track_nr: Option<u32>,
    pub disc_nr: Option<u32>,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
    pub sort_composer: Option<String>,
    pub sort_show: Option<String>,
    pub grouping: Option<String>,
    pub comment: Option<String>,
    /// リリース日 (UNIX 秒、0 で消す)
    pub released: Option<i64>,
    pub show: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub episode_id: Option<String>,
    pub network: Option<String>,
    pub description: Option<String>,
    pub rating: Option<u8>,
    pub skip_shuffle: Option<bool>,
    pub remember_position: Option<bool>,
    pub volume: Option<i32>,
    pub start_ms: Option<u32>,
    pub stop_ms: Option<u32>,
    /// イコライザのプリセットの番号 (0 で外す)
    pub eq: Option<u32>,
    /// 再生回数と最後に再生した日時を 0 に戻す
    #[serde(default)]
    pub reset_play_count: bool,
}

/// 動画のファイルの中身 (情報の編集の「ファイル」のページ)
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFileInfo {
    /// "H.264 Baseline Level 3.0" など
    pub video: String,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    /// "aac" など (無ければ空)
    pub audio: String,
    pub audio_rate: u32,
    pub channels: u32,
}

/// 動画を送った結果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum VideoOutcome {
    Added { video: Box<VideoView>, converted: bool },
    Duplicate { title: String },
}
