//! プレイリスト: 画面に渡す形

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistView {
    pub id: String,
    pub name: String,
    pub track_ids: Vec<String>,
    pub master: bool,
    pub smart: bool,
    pub podcast: bool,
}

/// 条件の画面に出す、今の条件に合う曲の数など
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartPreview {
    pub count: usize,
    pub length_ms: u64,
    pub size: u64,
    /// iPod の DB に値が無い項目を使っている (購入した項目・ラブなど。無し・0 として判定する)
    pub not_on_ipod: bool,
    /// 入る曲の先頭いくつか (画面で曲名を見せる)
    pub sample: Vec<String>,
}
