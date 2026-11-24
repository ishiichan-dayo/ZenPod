//! 曲: 画面に渡す形・画面から受け取る編集・転送の設定と結果

use serde::Serialize;

use super::audio::{ConvertTarget, MediaKind};

/// 重複判定のキー: 正規化したタイトル・アーティスト・アルバム
pub(crate) type DupKey = (String, String, String);

pub(crate) fn dup_key(title: Option<&str>, artist: Option<&str>, album: Option<&str>) -> DupKey {
    let norm = |s: Option<&str>| s.unwrap_or("").trim().to_lowercase();
    (norm(title), norm(artist), norm(album))
}

/// 重複判定の番号 (ディスク番号, トラック番号)。どちらにもあって違えば別の曲 (シングル版とアルバム版はタグが同じで番号だけ違うことがある。
/// デラックス版は別のディスクに同じ曲名の曲がある)
pub(crate) fn same_number(a: (u32, u32), b: (u32, u32)) -> bool {
    let eq = |x: u32, y: u32| x == 0 || y == 0 || x == y;
    eq(a.0, b.0) && eq(a.1, b.1)
}

/// 転送時のオプション
#[derive(Debug, Clone, Copy)]
pub struct AddOptions {
    pub skip_duplicates: bool,
    /// 日本語のタイトル等からソート用の読みを自動生成
    pub auto_reading: bool,
    /// ジャケット画像を転送
    pub artwork: bool,
    /// 同じ曲があれば、スキップせずに iPod の曲を新しいファイルで置き換える
    /// (再生回数・評価・追加日・プレイリストはそのまま)。skip_duplicates より優先
    pub replace_duplicates: bool,
    /// 可逆圧縮の曲 (FLAC など) の変換先
    pub lossless_to: ConvertTarget,
    /// 非可逆の曲 (Ogg / Opus など) の変換先
    pub lossy_to: ConvertTarget,
    /// 「変換して転送」: 形式を選んで送る。ロスレスの曲と、これより高いビットレートの曲もこの形式にする
    /// (`ConvertTarget::should_convert`)。None ならふだんどおり (再生できない形式だけ変換)
    pub convert_all_to: Option<ConvertTarget>,
    /// iPod に置くコピーの埋め込みのジャケットを、長い辺がこの大きさまでのベースラインの JPEG にする
    /// (`media::shrink_embedded_cover`。PC の元のファイルは変えない)。None ならそのまま
    pub shrink_artwork: Option<u32>,
}

impl Default for AddOptions {
    fn default() -> Self {
        AddOptions {
            skip_duplicates: true,
            auto_reading: true,
            artwork: true,
            replace_duplicates: false,
            lossless_to: ConvertTarget::LOSSLESS_DEFAULT,
            lossy_to: ConvertTarget::LOSSY_DEFAULT,
            convert_all_to: None,
            shrink_artwork: None,
        }
    }
}

/// 読みの自動生成で変わる内容 (プレビュー用)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingChange {
    pub id: String,
    /// 曲名 (一覧表示用)
    pub track_title: String,
    /// "title" | "artist" | "album" | "albumArtist" | "composer"
    pub field: String,
    /// 元の表記 (例: 椎名林檎)
    pub source: String,
    /// 現在の読み (空 = 未設定)
    pub before: String,
    pub after: String,
}

/// プレビューで確定した変更 (after を手で直すこともできる)
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingEdit {
    pub id: String,
    pub field: String,
    pub value: String,
}

/// 表記ゆれの統一: `from` のどれかと完全一致するアーティスト・アルバムアーティストを `to` に置き換える
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ArtistUnify {
    pub from: Vec<String>,
    pub to: String,
}

/// 読み (ソート用フィールド) の編集内容。空文字は削除
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Readings {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
}

/// 画面から受け取る (PC の曲の曲情報取得) ときは、足りない項目を既定値で読む
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackView {
    /// dbid (JS の Number 精度対策で文字列)
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub duration_ms: u32,
    pub track_nr: u32,
    pub disc_nr: u32,
    pub year: u32,
    pub size: u32,
    pub bitrate: u32,
    pub play_count: u32,
    pub rating: u8,
    pub filetype: String,
    pub date_added: i64,
    /// ArtworkDB の画像 id (0 = なし)。同じ画像は同じ id
    pub artwork_id: u32,
    pub sort_title: String,
    pub sort_artist: String,
    pub sort_album: String,
    pub sort_album_artist: String,
    pub composer: String,
    /// Rockbox モードの曲の、iPod のルートからの場所 ("/Music/A/B/01 曲.flac")。iPod OS の曲は空
    /// (ファイル名の変更の画面で、今の名前とぶつかる名前を調べるため)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub rb_path: String,
    /// 曲・ポッドキャスト・オーディオブック
    #[serde(default)]
    pub kind: MediaKind,
}

/// iTunesDB の mhit に入っている値。日時は UNIX 秒 (0 = なし)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbDetails {
    pub dbid: String,
    pub ipod_path: String,
    pub filetype: String,
    pub size: u32,
    pub length_ms: u32,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub mediatype: u32,
    pub compilation: bool,
    pub has_artwork: bool,
    pub rating: u8,
    pub play_count: u32,
    pub skip_count: u32,
    pub date_added: i64,
    pub date_modified: i64,
    pub last_played: i64,
    pub last_skipped: i64,
}

/// 曲情報の編集。None の項目は変更しない。文字列の空文字は削除
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagEdit {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub year: Option<u32>,
    pub track_nr: Option<u32>,
    pub disc_nr: Option<u32>,
    /// 並べ替え用の読み。None なら、表記を変えた項目は自動で付け直す (iPod) / 消す (PC)
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
    /// 評価 (0〜100、★ 1 つが 20)
    pub rating: Option<u8>,
    /// 曲の種類
    pub kind: Option<MediaKind>,
    /// 歌詞 (同期しない歌詞)。曲のファイルに書く (iPod OS も iPod の中のファイル)。空文字は削除
    pub lyrics: Option<String>,
    /// iTunes の「オプション」(iPod OS だけ。種類を変えるときは、その後で書く)
    pub skip_shuffle: Option<bool>,
    pub remember_position: Option<bool>,
    /// 音量の調整 (-255〜255)
    pub volume: Option<i32>,
    /// 開始 / 停止の位置 (ミリ秒。停止の 0 は最後まで)
    pub start_ms: Option<u32>,
    pub stop_ms: Option<u32>,
    /// イコライザのプリセットの番号 (0 = なし)
    pub eq: Option<u32>,
}

/// iTunes の「オプション」の今の値 (iPod OS の曲。情報の編集の画面を開いたときに読む)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackOptions {
    pub skip_shuffle: bool,
    pub remember_position: bool,
    pub volume: i32,
    pub start_ms: u32,
    pub stop_ms: u32,
    pub eq: u32,
}

/// 1 曲分の編集
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TrackTagEdit {
    pub id: String,
    pub edit: TagEdit,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AddOutcome {
    Added {
        track: Box<TrackView>,
        converted: bool,
    },
    Duplicate {
        title: String,
        /// iPod にある方の曲 (「転送してプレイリストに追加」では、この曲をプレイリストに入れる)
        id: String,
    },
    /// 同じ曲を新しいファイルで置き換えた
    Replaced {
        track: Box<TrackView>,
        converted: bool,
    },
}
