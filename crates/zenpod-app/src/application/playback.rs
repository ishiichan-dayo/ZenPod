//! 曲の再生用のデータ。ファイルは PC にコピーせず、要求された範囲だけを読んで返す。
//! WebView が再生できない形式 (ALAC・AIFF など) は ffmpeg で FLAC に変換し、メモリ上に 1 曲だけ持つ

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ipod_core::{media, t};

use super::ffmpeg::Ffmpeg;
use super::ports::Transcoder;
use super::prefs::Prefs;
use super::session::LibrarySession;
use crate::domain::byte_range;
use crate::domain::media_type::native_audio_mime;

/// 再生する曲
pub enum AudioTarget {
    /// iPod の曲 (画面から渡された ID のまま)
    Track(String),
    /// PC の登録フォルダの曲
    Pc(PathBuf),
    /// iPod の動画 (WebView がそのまま再生できるもの。それ以外は `VideoPlayer` が変換して流す)
    Video(String),
}

/// 返す内容。HTTP の応答にするのは呼んだ側
pub enum AudioReply {
    Partial { mime: &'static str, start: u64, end: u64, total: u64, body: Vec<u8> },
    /// 要求された範囲が曲の外 (416)
    Unsatisfiable { total: u64 },
    Fail { status: u16, message: String },
}

impl AudioReply {
    fn fail(status: u16, message: String) -> Self {
        AudioReply::Fail { status, message }
    }
}

pub struct Playback {
    session: Arc<LibrarySession>,
    ffmpeg: Arc<Ffmpeg>,
    prefs: Arc<Prefs>,
    transcoder: Arc<dyn Transcoder>,
    /// 再生用に変換した直近の 1 曲 (曲の場所, FLAC)。メモリ上だけに置き、ディスクには書かない
    converted: Mutex<Option<(PathBuf, Arc<Vec<u8>>)>>,
}

impl Playback {
    pub fn new(session: Arc<LibrarySession>, ffmpeg: Arc<Ffmpeg>, prefs: Arc<Prefs>, transcoder: Arc<dyn Transcoder>) -> Self {
        Playback { session, ffmpeg, prefs, transcoder, converted: Mutex::default() }
    }

    /// `range` は HTTP の Range ヘッダーの値。ファイルの読み込みや変換に時間がかかるので画面と別のスレッドで呼ぶ
    pub fn serve(&self, target: AudioTarget, range: Option<&str>) -> AudioReply {
        let path = match target {
            AudioTarget::Pc(p) => {
                if !self.prefs.pc_scope().allows_audio(&p) {
                    return AudioReply::fail(403, t!("登録したフォルダの曲だけ再生できます"));
                }
                p
            }
            AudioTarget::Track(id) => {
                let Ok(id) = id.parse::<u64>() else {
                    return AudioReply::fail(400, t!("曲 ID が不正です"));
                };
                let Some(p) = self.session.try_read(|lib| lib.track_file(id)).flatten() else {
                    return AudioReply::fail(404, t!("曲が見つかりません"));
                };
                p
            }
            AudioTarget::Video(id) => {
                let Ok(id) = id.parse::<u64>() else {
                    return AudioReply::fail(400, t!("不正な ID: {id}", id = id));
                };
                let Some(p) = self.session.try_read(|lib| lib.video_file(id)).flatten() else {
                    return AudioReply::fail(404, t!("動画のファイルが見つかりません"));
                };
                return Self::serve_file(&p, "video/mp4", range);
            }
        };
        if !path.is_file() {
            return AudioReply::fail(404, t!("曲のファイルが見つかりません"));
        }
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let native = native_audio_mime(media::classify(&path).ok(), &ext, cfg!(target_os = "macos"));
        if let Some(mime) = native {
            return Self::serve_file(&path, mime, range);
        }

        // WebView が再生できない形式は、変換した FLAC (メモリ) を返す
        let data = match self.converted(&path) {
            Ok(d) => d,
            Err(fail) => return fail,
        };
        let total = data.len() as u64;
        if total == 0 {
            return AudioReply::fail(404, t!("曲のファイルが空です"));
        }
        let Some((start, end)) = byte_range::resolve(range, total) else {
            return AudioReply::Unsatisfiable { total };
        };
        AudioReply::Partial { mime: "audio/flac", start, end, total, body: data[start as usize..=end as usize].to_vec() }
    }

    /// iPod / PC のファイルの、要求された範囲だけを読んで返す
    fn serve_file(path: &std::path::Path, mime: &'static str, range: Option<&str>) -> AudioReply {
        let total = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if total == 0 {
            return AudioReply::fail(404, if mime.starts_with("video/") { t!("動画のファイルが空です") } else { t!("曲のファイルが空です") });
        }
        let Some((start, end)) = byte_range::resolve(range, total) else {
            return AudioReply::Unsatisfiable { total };
        };
        use std::io::{Read, Seek, SeekFrom};
        let mut buf = vec![0u8; (end - start + 1) as usize];
        let read = std::fs::File::open(path).and_then(|mut f| {
            f.seek(SeekFrom::Start(start))?;
            f.read_exact(&mut buf)
        });
        if read.is_err() {
            return AudioReply::fail(500, t!("iPod から読み込めませんでした"));
        }
        AudioReply::Partial { mime, start, end, total, body: buf }
    }

    /// 再生用に変換した FLAC (直前と同じ曲なら変換し直さない)
    fn converted(&self, path: &PathBuf) -> Result<Arc<Vec<u8>>, AudioReply> {
        let cached = self.converted.lock().unwrap().as_ref().filter(|(p, _)| p == path).map(|(_, d)| d.clone());
        if let Some(d) = cached {
            return Ok(d);
        }
        let Some(ff) = self.ffmpeg.path() else {
            return Err(AudioReply::fail(415, t!("この形式の再生には ffmpeg が必要です (設定 → 変換 から自動で入れられます)")));
        };
        let Some(bytes) = self.transcoder.to_flac(&ff, path) else {
            return Err(AudioReply::fail(500, t!("再生用の変換に失敗しました")));
        };
        let d = Arc::new(bytes);
        *self.converted.lock().unwrap() = Some((path.clone(), d.clone()));
        Ok(d)
    }
}
