//! 動画をアプリの中で再生する。WebView がそのまま再生できる動画 (H.264 と AAC の MP4) は
//! ipodaudio:// でファイルのまま少しずつ読ませ (`Playback`)、それ以外は ffmpeg で変換しながら流して、
//! 画面が Media Source Extensions で少しずつ読む。流すのは 1 本だけ (新しく始めたら前のは止める)

use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ipod_core::media::video::{self, PreviewFormat, PreviewProcess, VideoInfo};
use ipod_core::t;
use serde::Serialize;

use super::ffmpeg::Ffmpeg;
use super::session::LibrarySession;
use crate::domain::id::parse_id;
use crate::error::{AppError, AppResult};

/// 読んでもらっていない分がこれを超えたら、ffmpeg からの読み込みを待たせる (変換は再生よりずっと速いので)
const AHEAD_LIMIT: usize = 16 * 1024 * 1024;
/// 1 回に渡す大きさ
const READ_MAX: usize = 1024 * 1024;
/// 渡すものがまだ無いときに待つ長さ
const READ_WAIT: Duration = Duration::from_secs(2);

/// 再生のしかた
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VideoPlan {
    /// ファイルのまま再生する (ipodaudio://.../video/<ID>)
    pub native: bool,
    /// 変換して流すときの形式 ("webm" / "mp4") と、MediaSource に渡す MIME タイプ
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    pub duration_ms: u32,
}

/// 読んだ分
pub struct StreamChunk {
    pub data: Vec<u8>,
    /// 最後まで渡した
    pub ended: bool,
}

#[derive(Default)]
struct Pipe {
    data: VecDeque<u8>,
    /// ffmpeg が書き終えた
    eof: bool,
    /// 止めた (読むスレッドを終わらせる)
    closed: bool,
}

struct Running {
    id: u64,
    pipe: Arc<(Mutex<Pipe>, Condvar)>,
    process: PreviewProcess,
}

impl Running {
    fn close(&self) {
        self.pipe.0.lock().unwrap().closed = true;
        self.pipe.1.notify_all();
    }
}

pub struct VideoPlayer {
    session: Arc<LibrarySession>,
    ffmpeg: Arc<Ffmpeg>,
    current: Mutex<Option<Running>>,
    next_id: Mutex<u64>,
    /// 直前に読んだ動画の情報 (再生のしかたを決めたときのものを、流し始めでも使う)
    last_info: Mutex<Option<(PathBuf, VideoInfo)>>,
}

impl VideoPlayer {
    pub fn new(session: Arc<LibrarySession>, ffmpeg: Arc<Ffmpeg>) -> Self {
        VideoPlayer { session, ffmpeg, current: Mutex::default(), next_id: Mutex::new(1), last_info: Mutex::default() }
    }

    fn file(&self, id: &str) -> AppResult<PathBuf> {
        let id = parse_id(id)?;
        let path = self.session.read(|lib| Ok(lib.video_file(id)))?;
        path.filter(|p| p.is_file()).ok_or_else(|| AppError::msg(t!("動画のファイルが見つかりません")))
    }

    /// 動画の情報。同じ動画なら読み直さない (ffmpeg の起動と iPod の読み込みは、流し始めを遅らせる)
    fn info(&self, path: &PathBuf, ff: Option<&std::path::Path>) -> AppResult<VideoInfo> {
        if let Some((_, i)) = self.last_info.lock().unwrap().as_ref().filter(|(p, _)| p == path) {
            return Ok(i.clone());
        }
        let info = video::probe_quick(path, ff)?;
        *self.last_info.lock().unwrap() = Some((path.clone(), info.clone()));
        Ok(info)
    }

    /// 再生のしかたを決める。formats は画面が MediaSource で再生できる形式 (使いたい順)。
    /// force_stream はファイルのままでは再生できなかったとき (WebKitGTK に H.264 が無いなど)
    pub fn plan(&self, id: &str, formats: &[String], force_stream: bool) -> AppResult<VideoPlan> {
        let path = self.file(id)?;
        let ff = self.ffmpeg.path();
        let info = self.info(&path, ff.as_deref())?;
        if !force_stream && video::webview_playable(&info) {
            return Ok(VideoPlan { native: true, format: None, mime: None, duration_ms: info.duration_ms });
        }
        let Some(ff) = ff else {
            return Err(AppError::msg(t!("この動画の再生には ffmpeg が必要です (設定 → 変換 から自動で入れられます)")));
        };
        for f in formats {
            let Some(format) = PreviewFormat::parse(f) else { continue };
            if let Some(plan) = video::preview_plan(&ff, format, info.audio.is_some()) {
                let key = if format == PreviewFormat::Webm { "webm" } else { "mp4" };
                return Ok(VideoPlan { native: false, format: Some(key), mime: Some(plan.mime), duration_ms: info.duration_ms });
            }
        }
        Err(AppError::msg(t!("この動画を再生できません (ffmpeg に再生用の変換に使うエンコーダがありません)")))
    }

    /// start_ms の位置から変換して流し始める。前に流していたものは止める。戻り値は `read` に渡す番号
    pub fn start(&self, id: &str, format: &str, start_ms: u32) -> AppResult<u64> {
        self.stop();
        let path = self.file(id)?;
        let ff = self.ffmpeg.path().ok_or_else(|| AppError::msg(t!("この動画の再生には ffmpeg が必要です (設定 → 変換 から自動で入れられます)")))?;
        let format = PreviewFormat::parse(format).ok_or_else(|| AppError::msg(t!("この動画を再生できません (ffmpeg に再生用の変換に使うエンコーダがありません)")))?;
        let has_audio = self.info(&path, Some(&ff)).map(|i| i.audio.is_some()).unwrap_or(true);
        let plan = video::preview_plan(&ff, format, has_audio)
            .ok_or_else(|| AppError::msg(t!("この動画を再生できません (ffmpeg に再生用の変換に使うエンコーダがありません)")))?;
        let mut process = video::spawn_preview(&ff, &plan, &path, start_ms)?;
        let mut out = process.stdout.take().ok_or_else(|| AppError::msg(t!("再生用の変換に失敗しました")))?;
        let pipe = Arc::new((Mutex::new(Pipe::default()), Condvar::new()));
        let p = pipe.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = out.read(&mut buf).unwrap_or(0);
                let (lock, cv) = &*p;
                let mut s = lock.lock().unwrap();
                if n == 0 {
                    s.eof = true;
                    cv.notify_all();
                    return;
                }
                while s.data.len() > AHEAD_LIMIT && !s.closed {
                    s = cv.wait(s).unwrap();
                }
                if s.closed {
                    return;
                }
                s.data.extend(&buf[..n]);
                cv.notify_all();
            }
        });
        let mut next = self.next_id.lock().unwrap();
        let id = *next;
        *next += 1;
        *self.current.lock().unwrap() = Some(Running { id, pipe, process });
        Ok(id)
    }

    /// 流している動画の続き。まだ無ければ少し待つ (待っても無ければ空を返し、画面がもう一度呼ぶ)。
    /// 止めた・新しく始めた後の古い番号には、終わりとして空を返す
    pub fn read(&self, stream: u64) -> AppResult<StreamChunk> {
        let pipe = match self.current.lock().unwrap().as_ref() {
            Some(r) if r.id == stream => r.pipe.clone(),
            _ => return Ok(StreamChunk { data: Vec::new(), ended: true }),
        };
        let (lock, cv) = &*pipe;
        let mut s = lock.lock().unwrap();
        if s.data.is_empty() && !s.eof && !s.closed {
            s = cv.wait_timeout_while(s, READ_WAIT, |s| s.data.is_empty() && !s.eof && !s.closed).unwrap().0;
        }
        let n = s.data.len().min(READ_MAX);
        let data: Vec<u8> = s.data.drain(..n).collect();
        let ended = s.data.is_empty() && (s.eof || s.closed);
        let eof = s.eof;
        drop(s);
        cv.notify_all();
        // 何も出せずに終わったら、ffmpeg のエラーを伝える
        if ended && eof && data.is_empty() {
            let mut cur = self.current.lock().unwrap();
            if let Some(r) = cur.as_mut().filter(|r| r.id == stream) {
                // 終わったかを少し待って確かめる (標準出力を閉じた直後はまだ終わっていないことがある)
                for _ in 0..20 {
                    match r.process.finished_ok() {
                        Some(true) => break,
                        Some(false) => {
                            let detail = r.process.error_text();
                            return Err(AppError::msg(if detail.is_empty() { t!("再生用の変換に失敗しました") } else { format!("{}\n{detail}", t!("再生用の変換に失敗しました")) }));
                        }
                        None => std::thread::sleep(Duration::from_millis(25)),
                    }
                }
            }
        }
        Ok(StreamChunk { data, ended })
    }

    /// 流すのをやめる (ffmpeg を終わらせる)
    pub fn stop(&self) {
        if let Some(r) = self.current.lock().unwrap().take() {
            r.close();
        }
    }
}
