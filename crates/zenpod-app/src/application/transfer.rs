//! PC → iPod の転送と、iPod → PC の書き出し

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ipod_core::domain::audio::{ConvertTarget, Format};
use ipod_core::domain::photo::PhotoOutcome;
use ipod_core::domain::track::{AddOptions, AddOutcome};
use ipod_core::domain::video::{VideoOptions, VideoPlacement, VideoSize};
use ipod_core::media::video::Progress;
use ipod_core::{media, t};
use serde::{Deserialize, Serialize};

use super::events::{AppEvent, TransferProgress, TransferSummary};
use super::ffmpeg::Ffmpeg;
use super::pc_artwork::PcArtwork;
use super::ports::EventSink;
use super::session::LibrarySession;
use crate::domain::export_name::{export_stem, unique_path};
use crate::domain::id::{parse_ids, parse_opt_id};
use crate::error::{AppError, AppResult};

/// 転送の設定 (画面の設定から渡す)。変換先は "alac" / "aac:256" / "mp3:320"
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferOptions {
    pub skip_duplicates: bool,
    /// 同じ曲があれば置き換える (skip_duplicates より優先)
    #[serde(default)]
    pub replace_duplicates: bool,
    pub auto_reading: bool,
    pub artwork: bool,
    /// iPod に置くコピーの埋め込みのジャケットを縮める (`AddOptions::shrink_artwork`)
    #[serde(default)]
    pub shrink_artwork: bool,
    /// 縮める大きさ (長い辺の px)。無ければ既定
    #[serde(default)]
    pub artwork_size: Option<u32>,
    pub lossless_to: Option<String>,
    pub lossy_to: Option<String>,
    /// 「変換して転送」で選んだ形式 (無ければふだんどおり)
    #[serde(default)]
    pub convert_all_to: Option<String>,
    /// iPod OS で動画を変換するときの大きさ ("640" / "320")
    #[serde(default)]
    pub video_size: Option<String>,
    /// 写真を入れるアルバムの名前 (写真の画面のアルバムに落としたとき)。無ければフォルダの名前
    #[serde(default)]
    pub photo_album: Option<String>,
    /// 動画の置き場所 (ビデオの画面で開いているところ: ムービー・番組など)。無ければ動画のタグのまま
    #[serde(default)]
    pub video_placement: VideoPlacement,
}

impl TransferOptions {
    fn to_add_options(&self) -> AddOptions {
        let target = |s: &Option<String>, default| s.as_deref().and_then(ConvertTarget::parse).unwrap_or(default);
        AddOptions {
            skip_duplicates: self.skip_duplicates,
            replace_duplicates: self.replace_duplicates,
            auto_reading: self.auto_reading,
            artwork: self.artwork,
            lossless_to: target(&self.lossless_to, ConvertTarget::LOSSLESS_DEFAULT),
            lossy_to: target(&self.lossy_to, ConvertTarget::LOSSY_DEFAULT),
            convert_all_to: self.convert_all_to.as_deref().and_then(ConvertTarget::parse),
            shrink_artwork: self.shrink_artwork.then(|| ipod_core::media::cover::embed_size(self.artwork_size)),
        }
    }

    fn to_video_options(&self) -> VideoOptions {
        VideoOptions {
            size: self.video_size.as_deref().and_then(VideoSize::parse).unwrap_or_default(),
            skip_duplicates: self.skip_duplicates || self.replace_duplicates,
            artwork: self.artwork,
            placement: self.video_placement.clone(),
        }
    }
}

/// 転送するもの (ドロップしたファイル・フォルダを分けたもの)
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Audio(PathBuf),
    Video(PathBuf),
    /// 写真。フォルダから拾ったものは、そのフォルダの名前のアルバムに入れる
    Photo { path: PathBuf, album: Option<String> },
}

impl Item {
    fn path(&self) -> &Path {
        match self {
            Item::Audio(p) | Item::Video(p) | Item::Photo { path: p, .. } => p,
        }
    }
}

#[derive(Serialize)]
pub struct ConversionCount {
    /// 可逆圧縮 (FLAC / APE など) で、変換して送る曲
    pub lossless: usize,
    /// 非可逆 (Ogg / Opus / WMA など) で、変換して送る曲
    pub lossy: usize,
}

pub struct Transfer {
    session: Arc<LibrarySession>,
    ffmpeg: Arc<Ffmpeg>,
    pc_artwork: Arc<PcArtwork>,
    events: Arc<dyn EventSink>,
    cancel: Arc<AtomicBool>,
}

impl Transfer {
    pub fn new(session: Arc<LibrarySession>, ffmpeg: Arc<Ffmpeg>, pc_artwork: Arc<PcArtwork>, events: Arc<dyn EventSink>) -> Self {
        Transfer { session, ffmpeg, pc_artwork, events, cancel: Arc::default() }
    }

    /// 転送する前に、iPod OS モードで変換が要る曲を数える (変換先を選んでもらうため)。フォルダは中の曲を数える
    pub fn count_conversions(&self, paths: &[String]) -> ConversionCount {
        let mut c = ConversionCount { lossless: 0, lossy: 0 };
        for f in expand_paths(paths).into_iter().filter_map(|i| if let Item::Audio(p) = i { Some(p) } else { None }) {
            match media::classify(&f) {
                Ok(Format::ConvertLossless) => c.lossless += 1,
                Ok(Format::ConvertLossy) => c.lossy += 1,
                _ => {}
            }
        }
        c
    }

    /// 転送を別のスレッドで始め、曲の数を返す。経過と結果はイベントで知らせる
    pub fn start(&self, paths: &[String], playlist_id: Option<&str>, options: &TransferOptions) -> AppResult<usize> {
        let opts = options.to_add_options();
        let vopts = options.to_video_options();
        let album = options.photo_album.clone().filter(|a| !a.trim().is_empty());
        let playlist = parse_opt_id(playlist_id)?;
        let busy = self.session.try_begin_exclusive().ok_or_else(|| AppError::msg(t!("別の転送が進行中です")))?;
        let files = expand_paths(paths);
        if files.is_empty() {
            return Err(AppError::msg(t!("転送できるファイル (曲・動画・写真) が見つかりません")));
        }
        let total = files.len();
        let ffmpeg = self.ffmpeg.path();
        self.cancel.store(false, Ordering::SeqCst);
        let (session, pc_artwork, events, cancel) =
            (self.session.clone(), self.pc_artwork.clone(), self.events.clone(), self.cancel.clone());

        std::thread::spawn(move || {
            let mut summary = TransferSummary::default();
            let mut added_ids = Vec::new();
            for (i, item) in files.iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    summary.cancelled = true;
                    break;
                }
                let f = item.path();
                let name = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let emit = |status, message, art, percent| {
                    events.emit(AppEvent::TransferProgress(TransferProgress {
                        index: i,
                        total,
                        file: name.clone(),
                        path: f.to_string_lossy().into_owned(),
                        status,
                        message,
                        art,
                        percent,
                    }));
                };
                let res: ipod_core::Result<&'static str> = match item {
                    Item::Audio(_) => {
                        emit("working", None, pc_artwork.thumbs_of(f).map(|(_, t)| t.small.clone()), None);
                        let res = session
                            .try_write(|lib| lib.add_file(f, ffmpeg.as_deref(), &opts))
                            .unwrap_or(Err(ipod_core::Error::Disconnected));
                        res.map(|out| match out {
                            AddOutcome::Added { track, .. } => {
                                summary.added += 1;
                                added_ids.push(track.id.parse::<u64>().unwrap_or(0));
                                "added"
                            }
                            AddOutcome::Duplicate { id, .. } => {
                                summary.duplicates += 1;
                                // プレイリストに転送した場合は、iPod にある方の曲を追加する (選んだ曲がプレイリストから漏れないように)
                                added_ids.push(id.parse::<u64>().unwrap_or(0));
                                "duplicate"
                            }
                            AddOutcome::Replaced { track, .. } => {
                                summary.replaced += 1;
                                // プレイリストに転送した場合は、置き換えた曲も追加する (既に入っていれば何もしない)
                                added_ids.push(track.id.parse::<u64>().unwrap_or(0));
                                "replaced"
                            }
                        })
                    }
                    Item::Video(_) => {
                        emit("working", None, None, None);
                        // 変換の進み具合は 0.25 秒ごとまでに間引いて知らせる
                        let last = std::cell::Cell::new(std::time::Instant::now());
                        let report = |p: f32| {
                            if last.get().elapsed().as_millis() >= 250 || p >= 1.0 {
                                last.set(std::time::Instant::now());
                                emit("converting", None, None, Some(p));
                            }
                        };
                        let cancelled = || cancel.load(Ordering::SeqCst);
                        let progress = Progress { report: &report, cancelled: &cancelled };
                        // 変換は数分かかることがあるので iPod の lock の外で行う (その間も一覧やジャケットを読めるように)
                        let ff = ffmpeg.as_deref();
                        (|| {
                            let mut info = media::video::probe(f, ff)?;
                            media::video::apply_placement(&mut info.tags, &vopts.placement);
                            if !info.has_video() {
                                return Err(ipod_core::Error::Media(t!("映像が見つかりません")));
                            }
                            let checked = session.try_read(|lib| {
                                let target = lib.video_target(vopts.size)?;
                                Ok((target, vopts.skip_duplicates.then(|| lib.video_duplicate(f, &info)).flatten()))
                            });
                            let (target, dup) = checked.unwrap_or(Err(ipod_core::Error::Disconnected))?;
                            if dup.is_some() {
                                summary.duplicates += 1;
                                return Ok("duplicate");
                            }
                            let prepared = media::video::prepare(f, ff, target, info, vopts.artwork, &progress)?;
                            session
                                .try_write(|lib| lib.add_prepared_video(f, &prepared))
                                .unwrap_or(Err(ipod_core::Error::Disconnected))?;
                            summary.videos += 1;
                            Ok("added")
                        })()
                    }
                    Item::Photo { album: from_folder, .. } => {
                        emit("working", None, None, None);
                        let album = album.as_deref().or(from_folder.as_deref());
                        let res = session
                            .try_write(|lib| lib.add_photo(f, album, ffmpeg.as_deref()))
                            .unwrap_or(Err(ipod_core::Error::Disconnected));
                        res.map(|out| match out {
                            PhotoOutcome::Added { .. } => {
                                summary.photos += 1;
                                "added"
                            }
                            PhotoOutcome::Duplicate => {
                                summary.duplicates += 1;
                                "duplicate"
                            }
                        })
                    }
                };
                match res {
                    Ok(status) => emit(status, None, None, None),
                    Err(ipod_core::Error::Cancelled) => {
                        summary.cancelled = true;
                        emit("error", Some(ipod_core::Error::Cancelled.to_string()), None, None);
                        break;
                    }
                    Err(e) => {
                        summary.errors.push(format!("{name}: {e}"));
                        emit("error", Some(e.to_string()), None, None);
                        if matches!(e, ipod_core::Error::Disconnected | ipod_core::Error::NoSpace) {
                            break;
                        }
                    }
                }
            }
            let rbdb = session.try_write(|lib| {
                if let Some(pl) = playlist {
                    if let Err(e) = lib.add_to_playlist(pl, &added_ids) {
                        summary.errors.push(e.to_string());
                    }
                }
                if lib.is_dirty() {
                    if let Err(e) = lib.save() {
                        summary.save_error = Some(e.to_string());
                    }
                }
                lib.take_rockbox_db_request()
            });
            session.request_rockbox_db(rbdb.flatten());
            session.clear_artwork_cache();
            drop(busy);
            events.emit(AppEvent::TransferDone(summary));
        });
        Ok(total)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// iPod の曲を PC のフォルダにコピーする。名前は「アーティスト - 曲名」。コピーした数を返す
    pub fn export(&self, ids: &[String], dest: &str) -> AppResult<usize> {
        let ids = parse_ids(ids)?;
        self.session.read(|lib| {
            let tracks: HashMap<String, _> = lib.tracks().into_iter().map(|t| (t.id.clone(), t)).collect();
            let mut n = 0;
            for id in ids {
                let Some(src) = lib.track_file(id) else {
                    continue;
                };
                let Some(t) = tracks.get(&id.to_string()) else {
                    continue;
                };
                let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("mp3");
                let dst = unique_path(Path::new(dest), &export_stem(&t.artist, &t.title), ext);
                std::fs::copy(&src, &dst)?;
                n += 1;
            }
            Ok(n)
        })
    }
}

/// ジャケットとして置かれている画像の名前 (曲のフォルダの cover.jpg など。写真としては送らない)
fn is_cover_name(path: &Path) -> bool {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    ["cover", "folder", "front", "albumart", "albumartsmall", "back", "thumb"].iter().any(|n| stem == *n || stem.starts_with(&format!("{n}_")))
        || stem.starts_with("albumart_")
}

/// ドロップされたファイル / フォルダを、送れるもの (曲・動画・写真) に分ける。
/// フォルダの中の画像は、曲の無いフォルダのものだけ写真にする (曲のフォルダの画像はジャケット)。
/// アルバムは、落としたフォルダからの場所 ("旅行" / 中のフォルダなら "旅行/2024")。Rockbox は階層のまま、iPod OS は 1 つの名前にする
fn expand_paths(paths: &[String]) -> Vec<Item> {
    let classify = |p: PathBuf| -> Option<Item> {
        if media::video::is_video_file(&p) {
            Some(Item::Video(p))
        } else if media::is_audio_file(&p) {
            Some(Item::Audio(p))
        } else if media::photo::is_photo_file(&p) {
            Some(Item::Photo { path: p, album: None })
        } else {
            None
        }
    };
    let mut out = Vec::new();
    for p in paths {
        let p = PathBuf::from(p);
        if !p.is_dir() {
            out.extend(classify(p));
            continue;
        }
        let files = ipod_core::pc::folder::all_files(&p);
        let audio_dirs: std::collections::HashSet<PathBuf> =
            files.iter().filter(|f| media::is_audio_file(f)).filter_map(|f| f.parent().map(Path::to_path_buf)).collect();
        for f in files {
            match classify(f) {
                Some(Item::Photo { path, .. }) => {
                    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                    if audio_dirs.contains(&dir) || is_cover_name(&path) {
                        continue;
                    }
                    let base = p.parent().unwrap_or(&p);
                    let album = dir
                        .strip_prefix(base)
                        .ok()
                        .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
                        .filter(|a| !a.is_empty());
                    out.push(Item::Photo { path, album });
                }
                Some(i) => out.push(i),
                None => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_folders_and_sorts_kinds() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("album")).unwrap();
        std::fs::create_dir_all(d.path().join("旅行/2024")).unwrap();
        std::fs::write(d.path().join("album/01.mp3"), b"").unwrap();
        std::fs::write(d.path().join("album/cover.jpg"), b"").unwrap();
        std::fs::write(d.path().join("album/booklet.jpg"), b"").unwrap();
        std::fs::write(d.path().join("旅行/IMG_1.jpg"), b"").unwrap();
        std::fs::write(d.path().join("旅行/folder.jpg"), b"").unwrap();
        std::fs::write(d.path().join("旅行/clip.mpg"), b"").unwrap();
        std::fs::write(d.path().join("旅行/2024/IMG_2.jpg"), b"").unwrap();
        std::fs::write(d.path().join("single.flac"), b"").unwrap();
        std::fs::write(d.path().join("photo.png"), b"").unwrap();
        std::fs::write(d.path().join("notes.txt"), b"").unwrap();
        let s = |p: &str| d.path().join(p).to_string_lossy().into_owned();
        let files = expand_paths(&[s("album"), s("旅行"), s("single.flac"), s("photo.png"), s("notes.txt")]);
        assert_eq!(
            files,
            vec![
                Item::Audio(d.path().join("album/01.mp3")),
                Item::Photo { path: d.path().join("旅行/2024/IMG_2.jpg"), album: Some("旅行/2024".into()) },
                Item::Photo { path: d.path().join("旅行/IMG_1.jpg"), album: Some("旅行".into()) },
                Item::Video(d.path().join("旅行/clip.mpg")),
                Item::Audio(d.path().join("single.flac")),
                Item::Photo { path: d.path().join("photo.png"), album: None },
            ]
        );
    }
}
