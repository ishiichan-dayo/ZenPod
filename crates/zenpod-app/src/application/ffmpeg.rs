//! 変換に使う ffmpeg の場所と、自動インストール (Windows / macOS)

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ipod_core::media::convert;
use ipod_core::t;
use serde::Serialize;

use super::events::{AppEvent, FfmpegProgress};
use super::ports::{AppDirs, EventSink, FfmpegInstaller, Transcoder};
use super::prefs::Prefs;
use crate::error::{AppError, AppResult};

pub const CAN_INSTALL_FFMPEG: bool = cfg!(windows) || cfg!(target_os = "macos");

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub ffmpeg: Option<String>,
    /// 設定で選んだ ffmpeg の場所 (無ければ自動で探す)
    pub ffmpeg_chosen: Option<String>,
    /// 使っている ffmpeg がアプリの自動インストールしたものか (削除ボタンを出す)
    pub ffmpeg_installed_by_app: bool,
    /// 自動インストールに対応しているか (Windows / macOS)
    pub can_install_ffmpeg: bool,
    /// ffmpeg が無くても OS の機能 (macOS の afconvert) で FLAC を変換できるか
    pub native_flac: bool,
}

pub struct Ffmpeg {
    dirs: Arc<dyn AppDirs>,
    installer: Arc<dyn FfmpegInstaller>,
    events: Arc<dyn EventSink>,
    /// 選んだファイルが ffmpeg かを確かめる
    transcoder: Arc<dyn Transcoder>,
    /// 設定で選んだ ffmpeg (prefs.json の ffmpegPath。無ければ同梱・自動インストール・PATH の順に探す)
    prefs: Arc<Prefs>,
    cancel: Arc<AtomicBool>,
}

impl Ffmpeg {
    pub fn new(
        dirs: Arc<dyn AppDirs>,
        installer: Arc<dyn FfmpegInstaller>,
        events: Arc<dyn EventSink>,
        transcoder: Arc<dyn Transcoder>,
        prefs: Arc<Prefs>,
    ) -> Self {
        Ffmpeg { dirs, installer, events, transcoder, prefs, cancel: Arc::default() }
    }

    /// 「ffmpeg を自動で入れる」で入れる場所
    fn install_dir(&self) -> Option<PathBuf> {
        Some(self.dirs.local_data_dir()?.join("ffmpeg"))
    }

    /// 選んだ場所に無くなっていれば (is_file でなければ) 自動で探す
    pub fn path(&self) -> Option<PathBuf> {
        let explicit = self.prefs.ffmpeg_path();
        let dirs: Vec<PathBuf> = [self.dirs.resource_dir().map(|d| d.join("bin")), self.install_dir()]
            .into_iter()
            .flatten()
            .collect();
        convert::find_ffmpeg(explicit.as_deref(), &dirs)
    }

    pub fn settings(&self) -> Settings {
        let ffmpeg = self.path();
        let dir = self.install_dir();
        Settings {
            ffmpeg_installed_by_app: matches!((&ffmpeg, &dir), (Some(f), Some(d)) if f.starts_with(d)),
            can_install_ffmpeg: CAN_INSTALL_FFMPEG,
            native_flac: convert::find_afconvert().is_some(),
            ffmpeg: ffmpeg.map(|p| p.to_string_lossy().into_owned()),
            ffmpeg_chosen: self.prefs.ffmpeg_path().map(|p| p.to_string_lossy().into_owned()),
        }
    }

    /// 使う ffmpeg を決める。`path` は Rust で開いたダイアログで選んだもの (画面からの場所は受け取らない)。
    /// 動かして ffmpeg と分かったときだけ覚える
    pub fn choose(&self, path: &Path) -> AppResult<Settings> {
        if !path.is_file() || !self.transcoder.is_ffmpeg(path) {
            return Err(AppError::msg(t!("選んだファイルは ffmpeg ではありません")));
        }
        self.prefs.set_ffmpeg_path(Some(path))?;
        Ok(self.settings())
    }

    /// 選んだ場所を忘れて、自動で探すのに戻す
    pub fn forget_choice(&self) -> AppResult<Settings> {
        self.prefs.set_ffmpeg_path(None)?;
        Ok(self.settings())
    }

    /// ダウンロードしてアプリ専用のフォルダに入れる。ダウンロードに時間がかかるので画面と別のスレッドで呼ぶ
    pub fn install(&self) -> AppResult<Settings> {
        if !CAN_INSTALL_FFMPEG {
            return Err(AppError::msg(t!("自動インストールは Windows と Mac のみ対応しています")));
        }
        let dest = self.install_dir().ok_or_else(|| AppError::msg(t!("インストール先が決められません")))?;
        self.cancel.store(false, Ordering::SeqCst);
        let mut last = 0u64;
        self.installer.install(&dest, &self.cancel, &mut |received, total| {
            // 通知は 1MB ごと
            if received - last >= 1 << 20 || received == total {
                last = received;
                self.events.emit(AppEvent::FfmpegProgress(FfmpegProgress { received, total }));
            }
        })?;
        Ok(self.settings())
    }

    pub fn cancel_install(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// 自動で入れた ffmpeg を削除する
    pub fn uninstall(&self) -> AppResult<Settings> {
        if let Some(d) = self.install_dir() {
            if d.exists() {
                std::fs::remove_dir_all(&d)
                    .map_err(|e| AppError::msg(t!("削除できませんでした (変換中ではありませんか): {e}", e = e)))?;
            }
        }
        Ok(self.settings())
    }
}
