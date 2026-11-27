//! 画面に知らせる出来事。イベント名と中身 (JSON) は分ける前の src-tauri と同じ

use ipod_core::device::DeviceInfo;
use ipod_core::domain::track::TrackView;
use ipod_core::online::artsearch::SearchProgress;
use ipod_core::pc::folder::PcTrack;
use serde::Serialize;

/// 中身はそのまま JSON にする (`OpenProgress(done, total)` は `[done, total]`)
#[derive(Serialize, Clone)]
#[serde(untagged)]
pub enum AppEvent {
    /// iPod を開いている途中で、タグを読んだ曲の数 (done, total)
    OpenProgress(usize, usize),
    /// 開いている途中で読めた曲 (Rockbox を初めて開くときは全曲を読み終わるまで何十秒もかかるので、先に出す)
    OpenPartial(OpenPartial),
    TransferProgress(TransferProgress),
    TransferDone(TransferSummary),
    FfmpegProgress(FfmpegProgress),
    ArtworkProgress(ArtworkProgress),
    /// ジャケットの検索の経過
    ArtworkSearch(ArtworkSearch),
    /// Rockbox のデータベースを裏で作り直し終わった
    RockboxDbDone(RockboxDbDone),
    /// Rockbox のデータベースから埋めた曲を裏でファイルから読み直し、一覧に反映した (替えた曲の数)
    RockboxReread(usize),
    PcScanProgress(Count),
    /// PC の音楽フォルダの読み込みで、新しく読めた曲 (初めて開くときは時間がかかるので、読めた順に少しずつ出す)
    PcScanTracks(PcScanTracks),
    AliasProgress(Count),
    /// iPod が USB につながった / 外れた
    UsbIpod(bool),
    /// ポッドキャストの回を落としている途中
    PodcastDownload(PodcastDownloadProgress),
}

impl AppEvent {
    pub fn name(&self) -> &'static str {
        match self {
            AppEvent::OpenProgress(..) => "open-progress",
            AppEvent::OpenPartial(_) => "open-partial",
            AppEvent::TransferProgress(_) => "transfer-progress",
            AppEvent::TransferDone(_) => "transfer-done",
            AppEvent::FfmpegProgress(_) => "ffmpeg-progress",
            AppEvent::ArtworkProgress(_) => "artwork-progress",
            AppEvent::ArtworkSearch(_) => "artwork-search",
            AppEvent::RockboxDbDone(_) => "rockbox-db-done",
            AppEvent::RockboxReread(_) => "rockbox-reread-done",
            AppEvent::PcScanProgress(_) => "pc-scan-progress",
            AppEvent::PcScanTracks(_) => "pc-scan-tracks",
            AppEvent::AliasProgress(_) => "alias-progress",
            AppEvent::UsbIpod(_) => "usb-ipod",
            AppEvent::PodcastDownload(_) => "podcast-download",
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpenPartial {
    pub device: DeviceInfo,
    pub name: String,
    pub tracks: Vec<TrackView>,
    /// 前回開いたときの一覧 (iPod の中をたどり終わる前にまとめて 1 回)。false は初めて開くときの読めた曲
    pub cached: bool,
}

#[derive(Serialize, Clone)]
pub struct PcScanTracks {
    /// 読んでいる登録フォルダ
    pub folder: String,
    pub tracks: Vec<PcTrack>,
}

#[derive(Serialize, Clone)]
pub struct Count {
    pub done: usize,
    pub total: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub index: usize,
    pub total: usize,
    pub file: String,
    /// ファイルの場所 (PC の一覧で、終わった曲から「転送済み」を付けるため)
    pub path: String,
    /// "working" | "converting" | "added" | "duplicate" | "replaced" | "error"
    pub status: &'static str,
    pub message: Option<String>,
    /// 転送し始めた曲のジャケット (下のバーに出す小さな data URL)。"working" のときだけ
    pub art: Option<String>,
    /// 動画の変換の進み具合 (0.0〜1.0)。"converting" のときだけ
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f32>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct TransferSummary {
    pub added: usize,
    pub duplicates: usize,
    /// 同じ曲を新しいファイルで置き換えた数
    pub replaced: usize,
    pub errors: Vec<String>,
    pub cancelled: bool,
    pub save_error: Option<String>,
    /// 送った動画の数 (added には入れない)
    pub videos: usize,
    /// 送った写真の数 (added には入れない)
    pub photos: usize,
}

#[derive(Serialize, Clone)]
pub struct FfmpegProgress {
    pub received: u64,
    pub total: u64,
}

/// アートワークの反映の経過 (画面に「ダウンロード中」「書き込み中 3 / 10 曲」を出す)
#[derive(Serialize, Clone)]
pub struct ArtworkProgress {
    /// "download" (画像をダウンロード中) / "write" (曲に書き込み中)
    pub phase: &'static str,
    pub done: usize,
    pub total: usize,
}

impl ArtworkProgress {
    pub fn download(total: usize) -> AppEvent {
        AppEvent::ArtworkProgress(ArtworkProgress { phase: "download", done: 0, total })
    }

    pub fn write(done: usize, total: usize) -> AppEvent {
        AppEvent::ArtworkProgress(ArtworkProgress { phase: "write", done, total })
    }
}

/// ジャケットの検索の経過。`token` は画面が検索ごとに付けた番号
#[derive(Serialize, Clone)]
pub struct ArtworkSearch {
    pub token: u32,
    #[serde(flatten)]
    pub progress: SearchProgress,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PodcastDownloadProgress {
    pub index: usize,
    pub total: usize,
    pub feed_id: String,
    pub ep_id: String,
    pub received: u64,
    pub size: u64,
}

/// 裏で作り直したことを画面に知らせる (失敗したときの理由も)
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RockboxDbDone {
    pub tracks: usize,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_match_previous_json() {
        let j = |e: &AppEvent| serde_json::to_string(e).unwrap();
        assert_eq!(j(&AppEvent::OpenProgress(3, 10)), "[3,10]");
        assert_eq!(j(&AppEvent::UsbIpod(true)), "true");
        assert_eq!(j(&AppEvent::PcScanProgress(Count { done: 1, total: 2 })), r#"{"done":1,"total":2}"#);
        assert_eq!(j(&ArtworkProgress::write(1, 4)), r#"{"phase":"write","done":1,"total":4}"#);
        assert_eq!(
            j(&AppEvent::TransferDone(TransferSummary::default())),
            r#"{"added":0,"duplicates":0,"replaced":0,"errors":[],"cancelled":false,"saveError":null,"videos":0,"photos":0}"#
        );
        assert_eq!(
            j(&AppEvent::RockboxDbDone(RockboxDbDone { tracks: 5, error: None })),
            r#"{"tracks":5,"error":null}"#
        );
    }
}
