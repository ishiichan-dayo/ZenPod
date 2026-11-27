//! PC の音楽フォルダ: 読み込み・タグとジャケットの書き換え・ファイル名の変更。
//! 書き換えてよいのは登録フォルダ (設定の pcFolders) の中の曲だけ

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ipod_core::domain::track::TagEdit;
use ipod_core::pc::folder::PcTrack;
use ipod_core::{media, t};
use serde::Deserialize;

use super::artwork::{prepared, EmbeddedArt};
use super::events::{AppEvent, ArtworkProgress, Count, PcScanTracks};
use super::pc_artwork::PcArtwork;
use super::ports::{AppDirs, EventSink, OnlineCatalog};
use super::prefs::Prefs;
use crate::error::{AppError, AppResult};

/// PC の曲 1 曲分の編集
#[derive(Deserialize)]
pub struct PcTagEdit {
    pub path: String,
    pub edit: TagEdit,
}

#[derive(Deserialize)]
pub struct PcRename {
    pub path: String,
    /// 新しい名前 (拡張子なし)
    pub name: String,
}

pub struct PcLibrary {
    prefs: Arc<Prefs>,
    artwork: Arc<PcArtwork>,
    online: Arc<dyn OnlineCatalog>,
    dirs: Arc<dyn AppDirs>,
    events: Arc<dyn EventSink>,
}

impl PcLibrary {
    pub fn new(
        prefs: Arc<Prefs>,
        artwork: Arc<PcArtwork>,
        online: Arc<dyn OnlineCatalog>,
        dirs: Arc<dyn AppDirs>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        PcLibrary { prefs, artwork, online, dirs, events }
    }

    /// 曲のタグのキャッシュの場所。v2: 作曲者を追加、v3: リリースの種類を追加 (古いキャッシュは使わず読み直す)
    fn cache_path(&self) -> Option<PathBuf> {
        self.dirs.cache_dir().map(|d| d.join("pc-tracks-v3.json"))
    }

    fn check_registered(&self, path: &str) -> AppResult<()> {
        if !self.prefs.pc_folders().iter().any(|f| f == path) {
            return Err(AppError::msg(t!("登録したフォルダではありません")));
        }
        Ok(())
    }

    /// 前回読んだときの一覧 (フォルダをたどらない)。開いてすぐ出し、`scan` の結果で置き換える (配布リポジトリの Issue #16)
    pub fn cached(&self, path: &str) -> AppResult<Vec<PcTrack>> {
        self.check_registered(path)?;
        Ok(self.cache_path().map(|c| ipod_core::pc::folder::cached_folder_tracks(Path::new(path), &c)).unwrap_or_default())
    }

    /// 登録フォルダ (サブフォルダ含む) の曲を読み取る。前回から変わっていないファイルはキャッシュを使う。
    /// 新しく読めた曲は、少しずつ (0.3 秒ごと) `PcScanTracks` で知らせる
    pub fn scan(&self, path: &str) -> AppResult<Vec<PcTrack>> {
        self.check_registered(path)?;
        let cache = self.cache_path();
        let batch = std::sync::Mutex::new((Vec::new(), std::time::Instant::now()));
        let flush = |force: bool| {
            let mut b = batch.lock().unwrap();
            if !b.0.is_empty() && (force || b.1.elapsed().as_millis() >= 300) {
                let tracks = std::mem::take(&mut b.0);
                b.1 = std::time::Instant::now();
                self.events.emit(AppEvent::PcScanTracks(PcScanTracks { folder: path.to_string(), tracks }));
            }
        };
        let tracks = ipod_core::pc::folder::scan_streaming(
            Path::new(path),
            cache.as_deref(),
            |done, total| self.events.emit(AppEvent::PcScanProgress(Count { done, total })),
            |t| {
                batch.lock().unwrap().0.push(t.clone());
                flush(false);
            },
        )?;
        flush(true);
        Ok(tracks)
    }

    /// 一覧用の小さいジャケット
    pub fn small_artwork(&self, path: &str) -> Option<String> {
        self.artwork.thumbs(Path::new(path)).map(|t| t.small.clone())
    }

    /// アルバム表示用のジャケット (一覧用より大きい正方形)
    pub fn large_artwork(&self, path: &str) -> Option<String> {
        self.artwork.thumbs(Path::new(path)).map(|t| t.large.clone())
    }

    /// 画像ファイルを曲のジャケットにする。`shrink` (長い辺の px) があれば先に縮める
    pub fn set_artwork_from_file(&self, paths: &[String], image: &str, shrink: Option<u32>) -> AppResult<usize> {
        let bytes = prepared(std::fs::read(image)?, shrink);
        self.write_covers(paths, Some(&bytes))
    }

    /// ネットの候補画像をダウンロードして曲のジャケットにする (`shrink` は `set_artwork_from_file` と同じ)
    pub fn apply_artwork_url(&self, paths: &[String], url: &str, shrink: Option<u32>) -> AppResult<usize> {
        self.events.emit(ArtworkProgress::download(paths.len()));
        let bytes = prepared(self.online.download_image(url)?, shrink);
        self.write_covers(paths, Some(&bytes))
    }

    /// 曲に埋め込まれた画像の大きさ (情報の編集のアートワークのタブ)
    pub fn embedded_artwork(&self, path: &str, size: Option<u32>) -> AppResult<Option<EmbeddedArt>> {
        if !self.prefs.pc_scope().allows_audio(Path::new(path)) {
            return Err(AppError::msg(t!("{p}: 登録したフォルダの曲ではありません", p = path)));
        }
        Ok(EmbeddedArt::of(Path::new(path), size))
    }

    /// 曲に埋め込まれた画像を、長い辺 `size` px までの JPEG に縮める (元のファイルを変更する。元の画像には戻せない)。
    /// 戻り値は縮めた曲の数。もともと小さい曲・画像の無い曲は数えない。一部が失敗したら、縮めた数と失敗の一覧を Err で返す
    pub fn shrink_artwork(&self, paths: &[String], size: Option<u32>) -> AppResult<usize> {
        let max = ipod_core::media::cover::embed_size(size);
        let scope = self.prefs.pc_scope();
        let mut report = BatchReport::default();
        for (i, p) in paths.iter().enumerate() {
            self.events.emit(ArtworkProgress::write(i, paths.len()));
            let path = Path::new(p);
            if !scope.allows_audio(path) {
                report.errors.push(t!("{p}: 登録したフォルダの曲ではありません", p = p));
                continue;
            }
            match media::shrink_embedded_cover(path, max) {
                Ok(true) => {
                    self.artwork.forget(path);
                    report.done += 1;
                }
                Ok(false) => {}
                Err(e) => report.record(path, Err(e)),
            }
        }
        report.finish()
    }

    /// 曲に埋め込まれたジャケットを消す
    pub fn remove_artwork(&self, paths: &[String]) -> AppResult<usize> {
        self.write_covers(paths, None)
    }

    /// ジャケットを元のファイルに埋め込む (None なら削除)。一部が失敗したら成功数と失敗の一覧を Err で返す
    fn write_covers(&self, paths: &[String], image: Option<&[u8]>) -> AppResult<usize> {
        let scope = self.prefs.pc_scope();
        let mut report = BatchReport::default();
        for (i, p) in paths.iter().enumerate() {
            self.events.emit(ArtworkProgress::write(i, paths.len()));
            let path = Path::new(p);
            if !scope.allows_audio(path) {
                report.errors.push(t!("{p}: 登録したフォルダの曲ではありません", p = p));
                continue;
            }
            let res = media::write_cover(path, image);
            // 一覧の小さい画像は読み直させる
            self.artwork.forget(path);
            report.record(path, res);
        }
        report.finish()
    }

    /// 複数の曲に同じ内容を書く (情報の編集)
    pub fn edit_tags(&self, paths: Vec<String>, edit: &TagEdit) -> AppResult<usize> {
        let edits: Vec<PcTagEdit> = paths.into_iter().map(|path| PcTagEdit { path, edit: edit.clone() }).collect();
        self.write_tags(&edits)
    }

    /// 曲ごとに違う内容を書く (ネットで取得した曲情報の反映)。元のファイルを変更する。
    /// 一部が失敗したら、成功数と失敗の一覧を Err で返す
    pub fn write_tags(&self, edits: &[PcTagEdit]) -> AppResult<usize> {
        let scope = self.prefs.pc_scope();
        let mut report = BatchReport::default();
        for e in edits {
            let path = Path::new(&e.path);
            if !scope.allows_audio(path) {
                report.errors.push(t!("{p}: 登録したフォルダの曲ではありません", p = e.path));
                continue;
            }
            report.record(path, media::write_tags(path, &e.edit));
        }
        report.finish()
    }

    /// ファイル名を変える (拡張子はそのまま)。戻り値は新しい場所
    pub fn rename(&self, path: &str, name: &str) -> AppResult<String> {
        let p = Path::new(path);
        if !self.prefs.pc_scope().allows_audio(p) {
            return Err(AppError::msg(t!("登録したフォルダの曲だけ名前を変えられます")));
        }
        let dst = ipod_core::pc::folder::rename(p, name)?;
        Ok(dst.to_string_lossy().into_owned())
    }

    /// ファイル名をまとめて変える。1 つでも問題があれば何も変えない
    pub fn rename_many(&self, renames: Vec<PcRename>) -> AppResult<Vec<String>> {
        let scope = self.prefs.pc_scope();
        if let Some(r) = renames.iter().find(|r| !scope.allows_audio(Path::new(&r.path))) {
            return Err(AppError::msg(t!("{p}: 登録したフォルダの曲ではありません", p = r.path)));
        }
        let pairs: Vec<(PathBuf, String)> = renames.into_iter().map(|r| (PathBuf::from(r.path), r.name)).collect();
        let out = ipod_core::pc::folder::rename_many(&pairs)?;
        Ok(out.into_iter().map(|p| p.to_string_lossy().into_owned()).collect())
    }
}

/// 何曲かまとめて書き換えた結果
#[derive(Default)]
struct BatchReport {
    done: usize,
    errors: Vec<String>,
}

impl BatchReport {
    fn record(&mut self, path: &Path, res: ipod_core::Result<()>) {
        match res {
            Ok(()) => self.done += 1,
            Err(err) => self
                .errors
                .push(format!("{}: {err}", path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default())),
        }
    }

    fn finish(self) -> AppResult<usize> {
        if self.errors.is_empty() {
            Ok(self.done)
        } else {
            Err(AppError::msg(t!(
                "{n} 曲を更新しました。{f} 曲は失敗しました:\n{list}",
                n = self.done,
                f = self.errors.len(),
                list = self.errors.join("\n")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_report_lists_failures() {
        ipod_core::i18n::set_lang("ja");
        let mut r = BatchReport::default();
        r.record(Path::new("/a/ok.mp3"), Ok(()));
        assert_eq!(BatchReport { done: 1, errors: vec![] }.finish().unwrap(), 1);
        r.record(Path::new("/a/bad.mp3"), Err(ipod_core::Error::Other("x".into())));
        let msg = r.finish().unwrap_err().to_string();
        assert_eq!(msg, "1 曲を更新しました。1 曲は失敗しました:\nbad.mp3: x");
    }
}
