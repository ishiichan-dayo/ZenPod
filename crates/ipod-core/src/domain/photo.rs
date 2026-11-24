//! 写真: 画面に渡す形

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoView {
    /// iPod OS は Photo Database の画像 id、Rockbox はルートからの場所の SHA-256 (どちらも文字列)
    pub id: String,
    /// 一覧に出す大きさの縦横 (iPod OS は一番大きいサムネイル、Rockbox は置いた画像)
    pub width: u32,
    pub height: u32,
    /// 撮った日時 (UNIX 秒。分からなければ 0)
    pub date: i64,
    /// 元の画像の大きさ (バイト)
    pub size: u64,
    /// ファイル名 (Rockbox だけ。iPod OS の DB には名前が無い)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl PhotoView {
    /// PC に書き出すときの名前 (拡張子なし)。ファイル名があればそれ、無ければ撮った日時 ("2024-05-06 07.08.09")
    pub fn export_stem(&self) -> String {
        if !self.name.is_empty() {
            return std::path::Path::new(&self.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        match chrono::DateTime::from_timestamp(self.date, 0) {
            Some(d) if self.date > 0 => d.format("%Y-%m-%d %H.%M.%S").to_string(),
            _ => format!("Photo {}", self.id),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbumView {
    pub id: String,
    pub name: String,
    /// アルバムに直接入っている写真 (子のアルバムの写真は入れない)
    pub photo_ids: Vec<String>,
    /// 親のアルバム (Rockbox のフォルダの階層。iPod OS のアルバムは平らなので無い)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// iPod の写真全部。`albums` に「フォトライブラリ」(全部の写真) は入れない
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLibrary {
    pub photos: Vec<PhotoView>,
    pub albums: Vec<PhotoAlbumView>,
    /// 写真を置けるか (iPod OS で写真を表示できない機種は false)
    pub supported: bool,
}

/// 写真を送った結果 (1 枚ごと)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PhotoOutcome {
    Added { id: String },
    Duplicate,
}
