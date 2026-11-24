//! 動画と写真: 一覧・情報の編集・削除・アルバム・PC への書き出し・一覧の画像

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use ipod_core::domain::photo::PhotoLibrary;
use ipod_core::domain::video::{VideoEdit, VideoFileInfo, VideoView};
use ipod_core::media::video::VideoCodec;
use ipod_core::library::PhotoExport;
use serde::Serialize;

use super::ffmpeg::Ffmpeg;
use super::session::LibrarySession;
use super::snapshot::Snapshot;
use crate::domain::export_name::{export_stem, unique_path};
use crate::domain::id::{parse_id, parse_ids};
use crate::error::{AppError, AppResult};

/// 写真を変えた後の一覧と、iPod の状態 (空き容量)
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoEdit {
    pub library: PhotoLibrary,
    pub snapshot: Snapshot,
    /// 作ったアルバムの ID (アルバムを作ったときだけ)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
}

/// 動画を変えた後の一覧と、iPod の状態 (空き容量)
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoEditResult {
    pub videos: Vec<VideoView>,
    pub snapshot: Snapshot,
}

fn data_url(jpeg: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg))
}

pub struct Visual {
    session: Arc<LibrarySession>,
    ffmpeg: Arc<Ffmpeg>,
}

impl Visual {
    pub fn new(session: Arc<LibrarySession>, ffmpeg: Arc<Ffmpeg>) -> Self {
        Visual { session, ffmpeg }
    }

    // ---------------- 動画 ----------------

    pub fn videos(&self) -> AppResult<Vec<VideoView>> {
        self.session.read(|lib| Ok(lib.videos()))
    }

    /// 動画を変える操作 → 保存 → 動画の一覧と状態
    fn edit_videos_with(&self, f: impl FnOnce(&mut ipod_core::library::Library) -> ipod_core::Result<usize>) -> AppResult<VideoEditResult> {
        let (_, snapshot) = self.session.edit_playlists(f)?;
        self.session.clear_artwork_cache();
        Ok(VideoEditResult { videos: self.videos()?, snapshot })
    }

    pub fn edit_videos(&self, ids: &[String], edit: &VideoEdit) -> AppResult<VideoEditResult> {
        let ids = parse_ids(ids)?;
        self.edit_videos_with(|lib| lib.edit_videos(&ids, edit))
    }

    pub fn remove_videos(&self, ids: &[String]) -> AppResult<VideoEditResult> {
        let ids = parse_ids(ids)?;
        self.edit_videos_with(|lib| lib.remove_videos(&ids))
    }

    /// 動画のファイルの中身 (情報の編集の「ファイル」のページ)。MP4 と .mpg は ffmpeg を使わずに読む
    pub fn video_file_info(&self, id: &str) -> AppResult<Option<VideoFileInfo>> {
        let id = parse_id(id)?;
        let Some(file) = self.session.read(|lib| Ok(lib.video_file(id)))? else { return Ok(None) };
        let ff = self.ffmpeg.path();
        let Ok(info) = ipod_core::media::video::probe_quick(&file, ff.as_deref()) else { return Ok(None) };
        let video = match &info.codec {
            VideoCodec::None => String::new(),
            VideoCodec::H264 { profile, level } => {
                let p = match profile {
                    66 => "Baseline",
                    77 => "Main",
                    88 => "Extended",
                    100 => "High",
                    _ => "",
                };
                format!("H.264 {p} Level {}.{}", level / 10, level % 10).replace("  ", " ")
            }
            VideoCodec::Mpeg4 => "MPEG-4".into(),
            VideoCodec::Mpeg1 => "MPEG-1".into(),
            VideoCodec::Mpeg2 => "MPEG-2".into(),
            VideoCodec::Other(s) => s.clone(),
        };
        let audio = info.audio.unwrap_or_default();
        Ok(Some(VideoFileInfo {
            video,
            width: info.width,
            height: info.height,
            fps: info.fps,
            audio: audio.codec,
            audio_rate: audio.sample_rate,
            channels: audio.channels,
        }))
    }

    /// 動画の一覧の画像 (data URL)。ffmpeg があれば動画のファイルから 1 コマ取り出す (元の縦横比のまま。埋め込みの画像があればそれ)。
    /// 無ければ iPod OS は ArtworkDB の画像 (iPod に置くときに正方形に切り抜いたもの)。
    /// アートワークを利用者が選んだ動画は ArtworkDB の画像を先に使う
    pub fn video_thumb(&self, id: &str) -> Option<String> {
        let id = parse_id(id).ok()?;
        if let Some(hit) = self.session.cached_visual((id, false)) {
            return Some(hit.to_string());
        }
        let (artwork_id, custom_art, file) = self.session.try_read(|lib| {
            let v = lib.videos().into_iter().find(|v| v.id == id.to_string())?;
            Some((v.artwork_id, v.custom_art, lib.video_file(id)))
        })??;
        let artwork = || (artwork_id != 0).then(|| self.session.try_read(|lib| lib.artwork_jpeg(artwork_id, true)).flatten()).flatten();
        let from_file = || {
            let ff = self.ffmpeg.path()?;
            let file = file.as_ref()?;
            let info = ipod_core::media::video::probe(file, Some(&ff)).ok()?;
            ipod_core::media::video::thumbnail(Some(&ff), file, &info)
        };
        // 利用者が選んだアートワークは、動画のファイルの画像より先に出す
        let jpeg = if custom_art { artwork().or_else(from_file) } else { from_file().or_else(artwork) }?;
        let url = Arc::new(data_url(&jpeg));
        self.session.remember_visual((id, false), url.clone());
        Some(url.to_string())
    }

    /// 動画を PC のフォルダにコピーする (名前は題名)。コピーした数
    pub fn export_videos(&self, ids: &[String], dest: &str) -> AppResult<usize> {
        let ids = parse_ids(ids)?;
        let files: Vec<(std::path::PathBuf, String)> = self.session.read(|lib| {
            let videos = lib.videos();
            Ok(ids
                .iter()
                .filter_map(|id| {
                    let v = videos.iter().find(|v| v.id == id.to_string())?;
                    Some((lib.video_file(*id)?, v.title.clone()))
                })
                .collect())
        })?;
        let mut n = 0;
        for (src, title) in files {
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("m4v").to_string();
            std::fs::copy(&src, unique_path(Path::new(dest), &export_stem("", &title), &ext))?;
            n += 1;
        }
        Ok(n)
    }

    // ---------------- 写真 ----------------

    /// 貼り付けた画像 (ブラウザで「画像をコピー」したものなど) を一時ファイルに書き、場所を返す (それを転送する)。
    /// ブラウザからのドラッグは画像のデータで届き、Tauri のドロップ (ファイルの場所だけ) では受け取れないので、貼り付けで受ける
    pub fn save_pasted_image(&self, data_b64: &str, ext: &str) -> AppResult<String> {
        let ext = ext.to_ascii_lowercase();
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
            return Err(AppError::msg(ipod_core::t!("未対応の形式です: .{e}", e = ext)));
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(data_b64).map_err(|e| AppError::msg(e.to_string()))?;
        let dir = std::env::temp_dir().join("zenpod").join("pasted");
        std::fs::create_dir_all(&dir)?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let path = unique_path(&dir, &format!("Pasted {now}"), &ext);
        std::fs::write(&path, bytes)?;
        Ok(path.to_string_lossy().into_owned())
    }

    pub fn photo_library(&self) -> AppResult<PhotoLibrary> {
        self.session.read(|lib| Ok(lib.photo_library()))
    }

    /// 写真を変える操作 → 保存 → 写真の一覧と状態
    fn edit_photos(&self, f: impl FnOnce(&mut ipod_core::library::Library) -> ipod_core::Result<Option<String>>) -> AppResult<PhotoEdit> {
        let (album_id, snapshot) = self.session.edit_playlists(f)?;
        self.session.clear_artwork_cache();
        Ok(PhotoEdit { library: self.photo_library()?, snapshot, album_id })
    }

    pub fn remove_photos(&self, ids: &[String]) -> AppResult<PhotoEdit> {
        let ids = parse_ids(ids)?;
        self.edit_photos(|lib| lib.remove_photos(&ids).map(|_| None))
    }

    /// アルバムを作る。`parent` (その中に作る) は Rockbox だけ
    pub fn create_album(&self, name: &str, parent: Option<&str>, ids: &[String]) -> AppResult<PhotoEdit> {
        let ids = parse_ids(ids)?;
        let parent = crate::domain::id::parse_opt_id(parent)?;
        self.edit_photos(|lib| lib.create_photo_album(name, parent, &ids).map(Some))
    }

    pub fn rename_album(&self, id: &str, name: &str) -> AppResult<PhotoEdit> {
        let id = parse_id(id)?;
        self.edit_photos(|lib| lib.rename_photo_album(id, name).map(|_| None))
    }

    pub fn delete_album(&self, id: &str) -> AppResult<PhotoEdit> {
        let id = parse_id(id)?;
        self.edit_photos(|lib| lib.delete_photo_album(id).map(|_| None))
    }

    pub fn add_to_album(&self, id: &str, ids: &[String]) -> AppResult<PhotoEdit> {
        let (id, ids) = (parse_id(id)?, parse_ids(ids)?);
        self.edit_photos(|lib| lib.add_to_photo_album(id, &ids).map(|_| None))
    }

    pub fn remove_from_album(&self, id: &str, ids: &[String]) -> AppResult<PhotoEdit> {
        let (id, ids) = (parse_id(id)?, parse_ids(ids)?);
        self.edit_photos(|lib| lib.remove_from_photo_album(id, &ids).map(|_| None))
    }

    /// 写真の画像 (data URL)。一覧に出す小さい方と、見るときの大きい方
    pub fn photo_thumb(&self, id: &str, large: bool) -> Option<String> {
        let id = parse_id(id).ok()?;
        if let Some(hit) = self.session.cached_visual((id, large)) {
            return Some(hit.to_string());
        }
        let jpeg = self.session.try_read(|lib| lib.photo_jpeg(id, large)).flatten()?;
        let url = Arc::new(data_url(&jpeg));
        self.session.remember_visual((id, large), url.clone());
        Some(url.to_string())
    }

    /// 写真を PC のフォルダに書き出す。Rockbox は置いたファイル、iPod OS は原寸を置かないので一番大きい画像。書き出した数
    pub fn export_photos(&self, ids: &[String], dest: &str) -> AppResult<usize> {
        let ids = parse_ids(ids)?;
        let items: Vec<(PhotoExport, String)> = self.session.read(|lib| {
            let pl = lib.photo_library();
            Ok(ids
                .iter()
                .filter_map(|id| {
                    let p = pl.photos.iter().find(|p| p.id == id.to_string())?;
                    Some((lib.photo_export(*id)?, p.export_stem()))
                })
                .collect())
        })?;
        let mut n = 0;
        for (item, stem) in items {
            let stem = export_stem("", &stem);
            match item {
                PhotoExport::File(src) => {
                    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_string();
                    std::fs::copy(&src, unique_path(Path::new(dest), &stem, &ext))?;
                }
                PhotoExport::Bytes(b) => std::fs::write(unique_path(Path::new(dest), &stem, "jpg"), b)?,
            }
            n += 1;
        }
        Ok(n)
    }
}
