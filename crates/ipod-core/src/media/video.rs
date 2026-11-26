//! 動画: 中身の判定 (MP4 は自前で読む・ほかは ffmpeg に聞く)・iPod / Rockbox でそのまま再生できるか・
//! ffmpeg での変換 (進み具合と中止) とサムネイル。
//!
//! - iPod OS (video 5G・classic・nano 3G 以降): H.264 Baseline (Level 3.0、640×480 まで) か MPEG-4 Part 2 と、
//!   AAC-LC の MP4 (.m4v)。それ以外は ffmpeg で変換する。H.264 のエンコーダは libx264 → VideoToolbox (Mac) →
//!   OpenH264 (Windows に自動で入れる LGPL 版の ffmpeg はこれ) → MPEG-4 Part 2 の順に、ある物を使う
//! - Rockbox: 動画のプラグイン (mpegplayer) は MPEG-1 / 2 の .mpg だけ。画面に合わせて 320×240 の MPEG-2 と MP2 にする

use crate::t;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use crate::error::{Error, Result};

/// 動画として扱う拡張子 (.mp4 / .webm / .mkv は音声だけのこともあるので中身も見る)
const VIDEO_EXTS: &[&str] = &[
    "m4v", "mp4", "mov", "mkv", "avi", "webm", "wmv", "flv", "mpg", "mpeg", "m2ts", "mts", "ts", "3gp", "ogv", "vob", "m2v",
    "mpv", "divx", "asf",
];
/// Rockbox の動画のプラグインがそのまま開ける拡張子 (MPEG-1 / 2 のプログラムストリーム)
pub const ROCKBOX_EXTS: &[&str] = &["mpg", "mpeg", "mpv", "m2v"];

fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// 動画のファイルか。拡張子で選び、音声だけのこともある形式は中身 (映像があるか) で決める
pub fn is_video_file(path: &Path) -> bool {
    let ext = ext_of(path);
    if !VIDEO_EXTS.contains(&ext.as_str()) {
        return false;
    }
    match ext.as_str() {
        "mp4" | "m4v" | "mov" | "3gp" => match probe_mp4(path) {
            Some(i) => i.has_video(),
            // 読めない MP4 は拡張子で決める (.mp4 は音声のことが多い)
            None => ext != "mp4",
        },
        "webm" | "mkv" => matroska_has_video(path).unwrap_or(ext == "mkv"),
        _ => true,
    }
}

// ============================================================
// 中身の情報
// ============================================================

#[derive(Debug, Clone, PartialEq, Default)]
pub enum VideoCodec {
    #[default]
    None,
    H264 {
        /// avcC の AVCProfileIndication (66 = Baseline, 77 = Main, 100 = High)
        profile: u8,
        /// 10 倍の値 (30 = Level 3.0)
        level: u8,
    },
    /// MPEG-4 Part 2 (Simple Profile など)
    Mpeg4,
    Mpeg1,
    Mpeg2,
    Other(String),
}

#[derive(Debug, Clone, Default)]
pub struct AudioStream {
    /// "aac" / "mp3" / "ac3" など
    pub codec: String,
    /// AAC のとき、LC (Low Complexity) か (HE-AAC は iPod で再生できない)
    pub aac_lc: bool,
    pub sample_rate: u32,
    pub channels: u32,
}

/// 動画のタグ (MP4 の ilst と同じ意味の項目)
#[derive(Debug, Clone, Default)]
pub struct VideoTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: u32,
    pub description: Option<String>,
    /// テレビ番組の番組名 (tvsh)
    pub show: Option<String>,
    /// エピソードの ID (tven。"S01E02" など)
    pub episode_id: Option<String>,
    /// 放送局 (tvnn)
    pub network: Option<String>,
    pub season: u32,
    pub episode: u32,
    /// MP4 の stik (6 = ミュージックビデオ, 9 = ムービー, 10 = テレビ番組)
    pub media_kind: Option<u8>,
    pub sort_title: Option<String>,
    pub sort_show: Option<String>,
    /// 埋め込みの画像 (covr)
    pub cover: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
pub struct VideoInfo {
    pub duration_ms: u32,
    pub width: u32,
    pub height: u32,
    /// 1 秒あたりのコマ数 (分からなければ 0)
    pub fps: f32,
    pub codec: VideoCodec,
    pub audio: Option<AudioStream>,
    /// 全体のビットレート (ファイルの大きさ ÷ 長さ)
    pub bitrate_kbps: u32,
    pub tags: VideoTags,
    /// MP4 (mp4 / m4v / mov) の入れ物
    pub mp4: bool,
}

impl VideoInfo {
    pub fn has_video(&self) -> bool {
        self.codec != VideoCodec::None
    }
}

/// テレビ番組の項目 (番組名・シーズン・話数) を外す
fn clear_episode(tags: &mut VideoTags) {
    tags.show = None;
    tags.sort_show = None;
    tags.season = 0;
    tags.episode = 0;
    tags.episode_id = None;
}

/// 置く場所 (画面で開いているところ) に合わせてタグを変える。iPod OS は種類 (stik) と番組名・アーティストから、
/// Rockbox は番組名から (Videos/番組名/) 置き場所が決まる。ムービーとミュージックビデオにするときは番組名・シーズン・話数を外す
pub fn apply_placement(tags: &mut VideoTags, p: &crate::domain::video::VideoPlacement) {
    use crate::domain::video::VideoKind;
    let given = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    match p.kind {
        Some(VideoKind::Movie) => {
            tags.media_kind = Some(9);
            clear_episode(tags);
        }
        Some(VideoKind::TvShow) => {
            tags.media_kind = Some(10);
            if let Some(show) = given(&p.show) {
                tags.show = Some(show);
                tags.sort_show = None;
            }
        }
        Some(VideoKind::MusicVideo) => {
            tags.media_kind = Some(6);
            clear_episode(tags);
            if let Some(artist) = given(&p.artist) {
                tags.artist = Some(artist);
            }
        }
        None => {}
    }
}

/// 動画の情報を読む。MP4 は自前で、それ以外は ffmpeg に聞く (無ければ分かる範囲だけ)
pub fn probe(path: &Path, ffmpeg: Option<&Path>) -> Result<VideoInfo> {
    if let Some(i) = probe_mp4(path) {
        return Ok(i);
    }
    let ext = ext_of(path);
    if ROCKBOX_EXTS.contains(&ext.as_str())
        && let Some(i) = probe_mpeg_ps(path)
        && ffmpeg.is_none()
    {
        return Ok(i);
    }
    match ffmpeg {
        Some(ff) => probe_ffmpeg(ff, path),
        None => Err(Error::Unsupported(t!(
            "{ext} の動画を読むには ffmpeg が必要です (設定 → 変換 から自動で入れられます)",
            ext = ext
        ))),
    }
}

/// 再生のための速い読み取り: MP4 と Rockbox の .mpg は自前で読み (ffmpeg を起動しない。iPod の HDD を読む量も少ない)、
/// それ以外は ffmpeg に聞く。.mpg の音声は mp2 とみなす
pub fn probe_quick(path: &Path, ffmpeg: Option<&Path>) -> Result<VideoInfo> {
    if let Some(i) = probe_mp4(path) {
        return Ok(i);
    }
    if ROCKBOX_EXTS.contains(&ext_of(path).as_str())
        && let Some(i) = probe_mpeg_ps(path)
    {
        return Ok(i);
    }
    probe(path, ffmpeg)
}

// ---------------- MP4 ----------------

fn be16(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 2).map_or(0, |s| u16::from_be_bytes([s[0], s[1]]) as u32)
}
fn be32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map_or(0, |s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}
fn be64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).map_or(0, |s| u64::from_be_bytes(s.try_into().unwrap()))
}

/// 箱 (atom) の並びを (種類, 中身) で返す。壊れていたらそこで止める
fn boxes(buf: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= buf.len() {
        let size = be32(buf, at) as u64;
        let kind = [buf[at + 4], buf[at + 5], buf[at + 6], buf[at + 7]];
        let (hdr, size) = match size {
            0 => (8, (buf.len() - at) as u64),
            1 => (16, be64(buf, at + 8)),
            s => (8, s),
        };
        // 大きさは壊れたファイルでは何でも入りうるので、足し算のあふれも壊れたものとして止める
        let end = match (at as u64).checked_add(size) {
            Some(end) if size >= hdr as u64 && end <= buf.len() as u64 => end as usize,
            _ => break,
        };
        out.push((kind, &buf[at + hdr..end]));
        at = end;
    }
    out
}

fn child<'a>(buf: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(buf).into_iter().find(|(k, _)| k == kind).map(|(_, b)| b)
}

/// ファイルの一番外の箱から moov を探して読む (大きすぎるものは読まない)。(moov, ファイルの大きさ)
fn read_moov(path: &Path) -> Option<(Vec<u8>, u64)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut at = 0u64;
    let mut first = true;
    // 小さな箱を延々と並べたファイルで読み続けないよう、見る箱の数にも上限を置く (断片化した MP4 でも十分な数)
    let mut left = 1_000_000u32;
    while at + 8 <= len {
        left = left.checked_sub(1)?;
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut size = be32(&h, 0) as u64;
        let kind = [h[4], h[5], h[6], h[7]];
        let mut hdr = 8;
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = be64(&h, 8);
            hdr = 16;
        } else if size == 0 {
            size = len - at;
        }
        // MP4 は ftyp か moov (古い QuickTime は wide / mdat なども) から始まる
        if first && !matches!(&kind, b"ftyp" | b"moov" | b"wide" | b"mdat" | b"free" | b"skip" | b"pnot") {
            return None;
        }
        first = false;
        // largesize が u64 の端に近いと足し算があふれて前に戻り、同じところを読み続けてしまう
        let end = at.checked_add(size)?;
        if size < hdr || end > len {
            return None;
        }
        if &kind == b"moov" {
            if size > 64 * 1024 * 1024 {
                return None;
            }
            let mut body = vec![0u8; (size - hdr) as usize];
            f.read_exact(&mut body).ok()?;
            return Some((body, len));
        }
        at = end;
    }
    None
}

/// MPEG-4 の記述子の長さ (1〜4 バイト、上位ビットが続きの印)
fn desc_len(b: &[u8], at: &mut usize) -> usize {
    let mut n = 0usize;
    for _ in 0..4 {
        let Some(&c) = b.get(*at) else { return 0 };
        *at += 1;
        n = (n << 7) | (c & 0x7F) as usize;
        if c & 0x80 == 0 {
            break;
        }
    }
    n
}

/// esds の (objectTypeIndication, AudioSpecificConfig の audioObjectType)
fn parse_esds(b: &[u8]) -> (u8, u8) {
    let b = b.get(4..).unwrap_or(&[]); // 版と印
    let mut at = 0usize;
    let mut oti = 0u8;
    let mut aot = 0u8;
    while at < b.len() {
        let tag = b[at];
        at += 1;
        let len = desc_len(b, &mut at);
        match tag {
            0x03 => {
                // ES_ID (2) と印 (1)
                let flags = b.get(at + 2).copied().unwrap_or(0);
                at += 3;
                if flags & 0x80 != 0 {
                    at += 2;
                }
                if flags & 0x40 != 0 {
                    at = at.saturating_add(1 + b.get(at).copied().unwrap_or(0) as usize);
                }
                if flags & 0x20 != 0 {
                    at += 2;
                }
            }
            0x04 => {
                oti = b.get(at).copied().unwrap_or(0);
                at += 13;
            }
            0x05 => {
                aot = b.get(at).map_or(0, |c| c >> 3);
                at = at.saturating_add(len);
            }
            _ => at = at.saturating_add(len),
        }
    }
    (oti, aot)
}

fn parse_trak(trak: &[u8], info: &mut VideoInfo) {
    let Some(mdia) = child(trak, b"mdia") else { return };
    let handler = child(mdia, b"hdlr").map(|h| [h.get(8).copied().unwrap_or(0), h.get(9).copied().unwrap_or(0), h.get(10).copied().unwrap_or(0), h.get(11).copied().unwrap_or(0)]);
    let (timescale, duration) = match child(mdia, b"mdhd") {
        Some(m) if m.first() == Some(&1) => (be32(m, 20), be64(m, 24)),
        Some(m) => (be32(m, 12), be32(m, 16) as u64),
        None => (0, 0),
    };
    let stbl = child(mdia, b"minf").and_then(|m| child(m, b"stbl"));
    let entry = stbl.and_then(|s| child(s, b"stsd")).and_then(|s| boxes(s.get(8..)?).into_iter().next());
    let samples = stbl.and_then(|s| child(s, b"stsz")).map(|s| be32(s, 8)).unwrap_or(0);
    let secs = if timescale > 0 { duration as f64 / timescale as f64 } else { 0.0 };
    match handler.as_ref() {
        Some(b"vide") if info.codec == VideoCodec::None => {
            let Some((kind, e)) = entry else { return };
            // 見た目の大きさは tkhd (16.16 の固定小数)。無ければ見本の記述の大きさ
            let (mut w, mut h) = match child(trak, b"tkhd") {
                Some(t) if t.first() == Some(&1) => (be32(t, 88) >> 16, be32(t, 92) >> 16),
                Some(t) => (be32(t, 76) >> 16, be32(t, 80) >> 16),
                None => (0, 0),
            };
            if w == 0 || h == 0 {
                w = be16(e, 24);
                h = be16(e, 26);
            }
            let kids = e.get(78..).unwrap_or(&[]);
            info.codec = match &kind {
                b"avc1" | b"avc3" => match child(kids, b"avcC") {
                    Some(c) => VideoCodec::H264 { profile: c.get(1).copied().unwrap_or(0), level: c.get(3).copied().unwrap_or(0) },
                    None => VideoCodec::H264 { profile: 0, level: 0 },
                },
                b"mp4v" => match child(kids, b"esds").map(parse_esds) {
                    // 0x20 = MPEG-4 Visual。0x60〜0x65 は MPEG-2、0x6A は MPEG-1
                    Some((0x60..=0x65, _)) => VideoCodec::Mpeg2,
                    Some((0x6A, _)) => VideoCodec::Mpeg1,
                    _ => VideoCodec::Mpeg4,
                },
                b"jpeg" | b"png " => return, // 画像の見本だけの道 (ジャケット扱い) は映像にしない
                k => VideoCodec::Other(String::from_utf8_lossy(k).trim().to_string()),
            };
            info.width = w;
            info.height = h;
            if secs > 0.0 && samples > 1 {
                info.fps = (samples as f64 / secs) as f32;
            }
            info.duration_ms = info.duration_ms.max((secs * 1000.0) as u32);
        }
        Some(b"soun") if info.audio.is_none() => {
            let Some((kind, e)) = entry else { return };
            let version = be16(e, 8);
            let rate = be32(e, 24) >> 16;
            let fixed = match version {
                1 => 28 + 16,
                2 => 28 + 36,
                _ => 28,
            };
            let kids = e.get(fixed..).unwrap_or(&[]);
            let mut a = AudioStream {
                codec: String::from_utf8_lossy(&kind).trim().to_string(),
                aac_lc: false,
                sample_rate: rate,
                channels: be16(e, 16),
            };
            if &kind == b"mp4a" {
                // QuickTime の v1 / v2 は wave の箱の中に esds がある
                let esds = child(kids, b"esds").or_else(|| child(kids, b"wave").and_then(|w| child(w, b"esds")));
                match esds.map(parse_esds) {
                    Some((0x40 | 0x66 | 0x67, aot)) => {
                        a.codec = "aac".into();
                        a.aac_lc = aot == 2;
                    }
                    Some((0x69 | 0x6B, _)) => a.codec = "mp3".into(),
                    _ => a.codec = "aac".into(),
                }
            }
            info.audio = Some(a);
            info.duration_ms = info.duration_ms.max((secs * 1000.0) as u32);
        }
        _ => {}
    }
}

/// ilst の項目の中身 (data の箱): (型, 値)
fn ilst_data(item: &[u8]) -> Option<(u32, &[u8])> {
    let d = child(item, b"data")?;
    Some((be32(d, 0) & 0x00FF_FFFF, d.get(8..)?))
}

fn parse_ilst(ilst: &[u8], tags: &mut VideoTags) {
    for (k, item) in boxes(ilst) {
        let Some((ty, v)) = ilst_data(item) else { continue };
        let text = || Some(String::from_utf8_lossy(v).trim().to_string()).filter(|s| !s.is_empty());
        let int = || match v.len() {
            1 => v[0] as u32,
            2 => be16(v, 0),
            4 => be32(v, 0),
            8 => be64(v, 0) as u32,
            _ => 0,
        };
        match &k {
            b"\xA9nam" => tags.title = text(),
            b"\xA9ART" => tags.artist = text(),
            b"\xA9alb" => tags.album = text(),
            b"aART" => tags.album_artist = text(),
            b"\xA9gen" => tags.genre = text(),
            b"\xA9day" => tags.year = text().and_then(|s| s.get(..4).and_then(|y| y.parse().ok())).unwrap_or(0),
            b"desc" => tags.description = tags.description.take().or_else(text),
            b"ldes" => tags.description = text().or(tags.description.take()),
            b"tvsh" => tags.show = text(),
            b"tven" => tags.episode_id = text(),
            b"tvnn" => tags.network = text(),
            b"tvsn" => tags.season = int(),
            b"tves" => tags.episode = int(),
            b"stik" => tags.media_kind = Some(int() as u8),
            b"sonm" => tags.sort_title = text(),
            b"sosn" => tags.sort_show = text(),
            b"covr" if matches!(ty, 13 | 14 | 0) && !v.is_empty() => tags.cover = Some(v.to_vec()),
            _ => {}
        }
    }
}

/// MP4 (mp4 / m4v / mov) を読む。MP4 でなければ None
pub fn probe_mp4(path: &Path) -> Option<VideoInfo> {
    let (moov, file_len) = read_moov(path)?;
    let mut info = VideoInfo { mp4: true, ..Default::default() };
    for (k, b) in boxes(&moov) {
        match &k {
            b"trak" => parse_trak(b, &mut info),
            b"mvhd" => {
                let (scale, dur) = if b.first() == Some(&1) { (be32(b, 20), be64(b, 24)) } else { (be32(b, 12), be32(b, 16) as u64) };
                if scale > 0 {
                    info.duration_ms = info.duration_ms.max((dur as f64 * 1000.0 / scale as f64) as u32);
                }
            }
            b"udta" => {
                if let Some(meta) = child(b, b"meta") {
                    // meta は版と印の 4 バイトの後に子の箱が続く (付いていない作りもある)
                    let kids = if meta.get(..4) == Some(&[0, 0, 0, 0]) { &meta[4..] } else { meta };
                    if let Some(ilst) = child(kids, b"ilst") {
                        parse_ilst(ilst, &mut info.tags);
                    }
                }
            }
            _ => {}
        }
    }
    if info.duration_ms > 0 {
        info.bitrate_kbps = (file_len.saturating_mul(8) / info.duration_ms as u64) as u32;
    }
    Some(info)
}

// ---------------- Matroska / WebM ----------------

/// Matroska の最初のほうを見て、映像の道 (TrackType = 1) があるか。分からなければ None
fn matroska_has_video(path: &Path) -> Option<bool> {
    let mut f = File::open(path).ok()?;
    let mut buf = vec![0u8; 1024 * 1024];
    let n = f.read(&mut buf).ok()?;
    let buf = &buf[..n];
    if buf.get(..4) != Some(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return None;
    }
    // Tracks (1654AE6B) を見つけ、その中の TrackType (83, 長さ 1) を見る
    let tracks = buf.windows(4).position(|w| w == [0x16, 0x54, 0xAE, 0x6B])?;
    let rest = &buf[tracks..];
    let mut types = rest.windows(3).filter(|w| w[0] == 0x83 && w[1] == 0x81).map(|w| w[2]);
    Some(types.any(|t| t == 1))
}

// ---------------- MPEG プログラムストリーム (.mpg) ----------------

/// パックの見出しの SCR (90kHz)
fn pack_scr(b: &[u8]) -> Option<u64> {
    let b0 = *b.first()?;
    if b0 >> 6 == 1 {
        // MPEG-2
        let (b1, b2, b3, b4) = (*b.get(1)? as u64, *b.get(2)? as u64, *b.get(3)? as u64, *b.get(4)? as u64);
        Some((((b0 as u64) >> 3) & 7) << 30 | ((b0 as u64) & 3) << 28 | b1 << 20 | (b2 >> 3) << 15 | (b2 & 3) << 13 | b3 << 5 | b4 >> 3)
    } else if b0 >> 4 == 2 {
        // MPEG-1
        let (b1, b2, b3, b4) = (*b.get(1)? as u64, *b.get(2)? as u64, *b.get(3)? as u64, *b.get(4)? as u64);
        Some((((b0 as u64) >> 1) & 7) << 30 | b1 << 22 | (b2 >> 1) << 15 | b3 << 7 | b4 >> 1)
    } else {
        None
    }
}

fn scrs(buf: &[u8]) -> impl Iterator<Item = u64> + '_ {
    buf.windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [0, 0, 1, 0xBA])
        .filter_map(|(i, _)| pack_scr(buf.get(i + 4..)?))
}

/// .mpg の長さ (最初と最後のパックの時刻の差) と大きさ (系列の見出し) を読む
pub fn probe_mpeg_ps(path: &Path) -> Option<VideoInfo> {
    const CHUNK: u64 = 256 * 1024;
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = vec![0u8; CHUNK.min(len) as usize];
    f.read_exact(&mut head).ok()?;
    if head.get(..4) != Some(&[0, 0, 1, 0xBA]) {
        return None;
    }
    let first = scrs(&head).next()?;
    let mut tail = vec![0u8; CHUNK.min(len) as usize];
    f.seek(SeekFrom::Start(len - tail.len() as u64)).ok()?;
    f.read_exact(&mut tail).ok()?;
    let last = scrs(&tail).last().unwrap_or(first);
    let mut info = VideoInfo { codec: VideoCodec::Mpeg2, ..Default::default() };
    info.duration_ms = (last.saturating_sub(first) / 90) as u32;
    // 系列の見出し (000001B3): 幅 12 ビット・高さ 12 ビット
    if let Some(i) = head.windows(4).position(|w| w == [0, 0, 1, 0xB3]) {
        let s = &head[i + 4..];
        if s.len() >= 3 {
            info.width = ((s[0] as u32) << 4) | (s[1] as u32 >> 4);
            info.height = ((s[1] as u32 & 0x0F) << 8) | s[2] as u32;
        }
        // MPEG-2 なら続きに拡張の見出し (000001B5) がある。無ければ MPEG-1
        if !head[i..].windows(4).take(4096).any(|w| w == [0, 0, 1, 0xB5]) {
            info.codec = VideoCodec::Mpeg1;
        }
    }
    if info.duration_ms > 0 {
        info.bitrate_kbps = (len.saturating_mul(8) / info.duration_ms as u64) as u32;
    }
    info.audio = Some(AudioStream { codec: "mp2".into(), ..Default::default() });
    Some(info)
}

// ---------------- ffmpeg に聞く ----------------

fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// `ffmpeg -i` の表示から情報を読む
pub fn probe_ffmpeg(ffmpeg: &Path, path: &Path) -> Result<VideoInfo> {
    let out = no_window(crate::host::command(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(path))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| Error::Convert(t!("{tool} を起動できません: {e}", tool = "ffmpeg", e = e)))?;
    let text = String::from_utf8_lossy(&out.stderr);
    let info = parse_ffmpeg_info(&text);
    if !info.has_video() {
        let last = text.lines().rev().find(|l| !l.trim().is_empty() && !l.contains("output file")).unwrap_or("");
        return Err(Error::Media(if last.contains("Invalid data") || last.contains("No such file") {
            last.trim().to_string()
        } else {
            t!("映像が見つかりません")
        }));
    }
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(VideoInfo { bitrate_kbps: if info.duration_ms > 0 { (len.saturating_mul(8) / info.duration_ms as u64) as u32 } else { 0 }, ..info })
}

fn parse_hms(s: &str) -> Option<u32> {
    let mut parts = s.trim().split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(((h * 3600.0 + m * 60.0 + sec) * 1000.0) as u32)
}

pub(crate) fn parse_ffmpeg_info(text: &str) -> VideoInfo {
    let mut info = VideoInfo::default();
    let mut in_input = false;
    let mut in_meta = false;
    for line in text.lines() {
        let l = line.trim();
        if line.starts_with("Input #0") {
            in_input = true;
            continue;
        }
        if line.starts_with("Input #") || line.starts_with("Output #") {
            in_input = false;
        }
        if !in_input {
            continue;
        }
        // 入れ物全体の Metadata (行頭から 2 文字の字下げ)。道ごとのもの (4 文字) は読まない
        if line.starts_with("  Metadata:") {
            in_meta = true;
            continue;
        }
        if in_meta {
            if line.starts_with("    ") && !line.starts_with("     ") && l.contains(':') {
                let (k, v) = l.split_once(':').unwrap();
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
                let v = Some(v).filter(|s| !s.is_empty());
                let num = || v.as_deref().and_then(|s| s.split('/').next()?.trim().parse().ok()).unwrap_or(0);
                let t = &mut info.tags;
                match k.as_str() {
                    "title" => t.title = v,
                    "artist" => t.artist = v,
                    "album" => t.album = v,
                    "album_artist" => t.album_artist = v,
                    "genre" => t.genre = v,
                    "date" | "year" => t.year = v.as_deref().and_then(|s| s.get(..4)?.parse().ok()).unwrap_or(0),
                    "description" | "comment" | "synopsis" if t.description.is_none() => t.description = v,
                    "show" => t.show = v,
                    "episode_id" => t.episode_id = v,
                    "network" => t.network = v,
                    "season_number" => t.season = num(),
                    "episode_sort" => t.episode = num(),
                    "media_type" => t.media_kind = v.as_deref().and_then(|s| s.parse().ok()),
                    _ => {}
                }
                continue;
            }
            in_meta = false;
        }
        if let Some(rest) = l.strip_prefix("Duration:") {
            info.duration_ms = parse_hms(rest.split(',').next().unwrap_or("")).unwrap_or(0);
            continue;
        }
        if !l.starts_with("Stream #") {
            continue;
        }
        if let Some((_, desc)) = l.split_once("Video: ") {
            if info.codec != VideoCodec::None || desc.contains("(attached pic)") {
                continue;
            }
            let name = desc.split([' ', ',']).next().unwrap_or("");
            let profile = desc.split_once('(').and_then(|(_, p)| p.split_once(')')).map(|(p, _)| p.to_ascii_lowercase()).unwrap_or_default();
            info.codec = match name {
                "h264" => VideoCodec::H264 {
                    profile: match profile.as_str() {
                        p if p.contains("baseline") => 66,
                        "main" => 77,
                        "extended" => 88,
                        _ => 100,
                    },
                    // ffmpeg の表示には段階が無いので、変換するかの判定ではそのまま扱わない
                    level: 0,
                },
                "mpeg4" => VideoCodec::Mpeg4,
                "mpeg1video" => VideoCodec::Mpeg1,
                "mpeg2video" => VideoCodec::Mpeg2,
                other => VideoCodec::Other(other.to_string()),
            };
            for tok in desc.split([',', ' ', '[']) {
                if let Some((w, h)) = tok.split_once('x')
                    && let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>())
                    && w > 0
                    && h > 0
                {
                    info.width = w;
                    info.height = h;
                    break;
                }
            }
            if let Some(i) = desc.find(" fps") {
                info.fps = desc[..i].rsplit([' ', ',']).next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            }
        } else if let Some((_, desc)) = l.split_once("Audio: ") {
            if info.audio.is_some() {
                continue;
            }
            let name = desc.split([' ', ',']).next().unwrap_or("").to_string();
            let rate = desc.split(',').find_map(|p| p.trim().strip_suffix(" Hz")?.parse().ok()).unwrap_or(0);
            let channels = if desc.contains("mono") { 1 } else { 2 };
            info.audio = Some(AudioStream { aac_lc: name == "aac" && desc.contains("(LC)"), codec: name, sample_rate: rate, channels });
        }
    }
    info
}

// ============================================================
// そのまま送れるか・変換
// ============================================================

pub use crate::domain::video::VideoSize;

fn dims(size: VideoSize) -> (u32, u32) {
    match size {
        VideoSize::Vga => (640, 480),
        VideoSize::Qvga => (320, 240),
    }
}

/// 動画を置く先
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTarget {
    Ipod(VideoSize),
    Rockbox,
}

/// iPod OS がそのまま再生できるか (Apple の仕様: H.264 Baseline Level 3.0 か MPEG-4 Simple Profile、640×480・2.5Mbps まで、
/// 音声は AAC-LC の 48kHz・2ch まで)
pub fn ipod_playable(info: &VideoInfo) -> bool {
    let video_ok = match info.codec {
        VideoCodec::H264 { profile, level } => profile == 66 && (1..=30).contains(&level),
        VideoCodec::Mpeg4 => true,
        _ => false,
    };
    let audio_ok = match &info.audio {
        None => true,
        Some(a) => a.codec == "aac" && a.aac_lc && a.sample_rate <= 48000 && a.channels <= 2,
    };
    info.mp4 && video_ok && audio_ok && info.width <= 640 && info.height <= 480 && info.bitrate_kbps <= 2600
}

/// Rockbox がそのまま再生できるか (MPEG-1 / 2 の .mpg)。大きい映像は iPod の処理が追いつかないので変換する
pub fn rockbox_playable(path: &Path, info: &VideoInfo) -> bool {
    ROCKBOX_EXTS.contains(&ext_of(path).as_str())
        && matches!(info.codec, VideoCodec::Mpeg1 | VideoCodec::Mpeg2)
        && (info.width == 0 || (info.width <= 400 && info.height <= 300))
}

pub fn playable(path: &Path, info: &VideoInfo, target: VideoTarget) -> bool {
    match target {
        VideoTarget::Ipod(_) => ipod_playable(info),
        VideoTarget::Rockbox => rockbox_playable(path, info),
    }
}

/// H.264 (iPod OS 用) に使うエンコーダ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Encoder {
    X264,
    VideoToolbox,
    OpenH264,
    /// H.264 のエンコーダが無いときの代わり (MPEG-4 Part 2。ffmpeg に必ず入っている)
    Mpeg4,
}

/// ffmpeg に入っているエンコーダの名前 (ffmpeg ごとに 1 回だけ聞く)
fn encoders(ffmpeg: &Path) -> String {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(s) = cache.lock().unwrap().get(ffmpeg) {
        return s.clone();
    }
    let out = no_window(crate::host::command(ffmpeg).args(["-hide_banner", "-nostdin", "-encoders"]))
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    cache.lock().unwrap().insert(ffmpeg.to_path_buf(), out.clone());
    out
}

fn has_encoder(list: &str, name: &str) -> bool {
    list.lines().any(|l| l.split_whitespace().nth(1) == Some(name))
}

pub fn h264_encoder(ffmpeg: &Path) -> H264Encoder {
    let list = encoders(ffmpeg);
    if has_encoder(&list, "libx264") {
        H264Encoder::X264
    } else if cfg!(target_os = "macos") && has_encoder(&list, "h264_videotoolbox") {
        H264Encoder::VideoToolbox
    } else if has_encoder(&list, "libopenh264") {
        H264Encoder::OpenH264
    } else {
        H264Encoder::Mpeg4
    }
}

/// MPEG-2 の決まったコマ数のうち、元に近く 30 を超えないもの
fn mpeg2_rate(fps: f32) -> &'static str {
    match fps {
        f if f <= 0.0 => "25",
        f if f < 24.5 => "24000/1001",
        f if f < 27.0 => "25",
        f if f < 29.985 => "30000/1001",
        _ => "30",
    }
}

/// 変換の ffmpeg の引数 (入力と出力のパスを除く。`-i` の後に入力、最後に出力を置く)
pub fn convert_args(target: VideoTarget, enc: H264Encoder, info: &VideoInfo) -> Vec<String> {
    let mut a: Vec<String> = ["-map", "0:v:0", "-map", "0:a:0?", "-sn", "-dn", "-map_chapters", "-1"].iter().map(|s| s.to_string()).collect();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match target {
        VideoTarget::Ipod(size) => {
            let (w, h) = dims(size);
            // 縦横比はそのまま、大きさに収める (小さい動画は大きくしない)。画素は正方形に
            let scale = format!(
                "scale=w='min({w},iw)':h='min({h},ih)':force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1,format=yuv420p"
            );
            push(&["-vf", &scale, "-fpsmax", "30"]);
            let (kbps, level) = match size {
                VideoSize::Vga => ("1500", "3.0"),
                VideoSize::Qvga => ("768", "1.3"),
            };
            match enc {
                H264Encoder::X264 => push(&[
                    "-c:v", "libx264", "-profile:v", "baseline", "-level:v", level, "-preset", "medium", "-b:v", &format!("{kbps}k"),
                    "-maxrate", &format!("{kbps}k"), "-bufsize", &format!("{}k", kbps.parse::<u32>().unwrap() * 2),
                ]),
                H264Encoder::VideoToolbox => {
                    push(&["-c:v", "h264_videotoolbox", "-profile:v", "baseline", "-level:v", level, "-b:v", &format!("{kbps}k"), "-allow_sw", "1"])
                }
                H264Encoder::OpenH264 => push(&[
                    "-c:v", "libopenh264", "-profile:v", "constrained_baseline", "-b:v", &format!("{kbps}k"), "-maxrate",
                    &format!("{kbps}k"),
                ]),
                H264Encoder::Mpeg4 => {
                    let kbps = if size == VideoSize::Vga { "2000k" } else { "768k" };
                    push(&["-c:v", "mpeg4", "-vtag", "mp4v", "-bf", "0", "-b:v", kbps, "-maxrate", "2500k", "-bufsize", "2500k"])
                }
            }
            push(&crate::media::convert::IPOD_AAC);
            push(&["-b:a", "128k", "-ac", "2", "-ar", "44100"]);
            push(&["-map_metadata", "0", "-movflags", "+faststart", "-f", "mp4"]);
        }
        VideoTarget::Rockbox => {
            push(&[
                "-vf",
                "scale=w='min(320,iw)':h='min(240,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1,format=yuv420p",
                "-r",
                mpeg2_rate(info.fps),
                "-c:v",
                "mpeg2video",
                "-b:v",
                "600k",
                "-maxrate",
                "1200k",
                "-bufsize",
                "1835k",
                "-c:a",
                "mp2",
                "-b:a",
                "192k",
                "-ac",
                "2",
                "-ar",
                "44100",
                "-f",
                "mpeg",
            ]);
        }
    }
    a
}

/// 変換の進み具合 (0.0〜1.0) を知らせ、中止を聞く
pub struct Progress<'a> {
    pub report: &'a dyn Fn(f32),
    pub cancelled: &'a dyn Fn() -> bool,
}

/// `src` を `target` の形に変換して `dst` に書く。中止されたら `Error::Cancelled` (書きかけは消す)
pub fn convert(ffmpeg: &Path, src: &Path, dst: &Path, target: VideoTarget, info: &VideoInfo, progress: &Progress) -> Result<()> {
    let enc = h264_encoder(ffmpeg);
    let mut cmd = crate::host::command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-i"])
        .arg(src)
        .args(convert_args(target, enc, info))
        .arg(dst);
    let r = run_with_progress(&mut cmd, info.duration_ms, progress);
    if r.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    r
}

fn run_with_progress(cmd: &mut Command, duration_ms: u32, progress: &Progress) -> Result<()> {
    let mut child = no_window(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Convert(t!("{tool} を起動できません: {e}", tool = "ffmpeg", e = e)))?;
    // エラーの表示は別のスレッドで読む (読まないと ffmpeg が書けずに止まる)
    let stderr = child.stderr.take();
    let errs = std::thread::spawn(move || {
        let mut lines: Vec<String> = Vec::new();
        if let Some(s) = stderr {
            for l in BufReader::new(s).lines().map_while(|l| l.ok()) {
                lines.push(l);
                if lines.len() > 20 {
                    lines.remove(0);
                }
            }
        }
        lines
    });
    let mut cancelled = false;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
            if (progress.cancelled)() {
                cancelled = true;
                let _ = child.kill();
                break;
            }
            // out_time_us (古い ffmpeg は out_time_ms という名前で同じくマイクロ秒)
            let us = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")).and_then(|v| v.trim().parse::<i64>().ok());
            if let Some(us) = us
                && duration_ms > 0
            {
                (progress.report)((us as f64 / 1000.0 / duration_ms as f64).clamp(0.0, 1.0) as f32);
            }
        }
    }
    let status = child.wait().map_err(|e| Error::Convert(e.to_string()))?;
    let lines = errs.join().unwrap_or_default();
    if cancelled || (progress.cancelled)() {
        return Err(Error::Cancelled);
    }
    if !status.success() {
        return Err(Error::Convert(
            lines.iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_else(|| t!("{tool} エラー", tool = "ffmpeg")),
        ));
    }
    (progress.report)(1.0);
    Ok(())
}

/// 送る準備ができた動画 (変換・目次の並べ替えを済ませたもの)。落とすと一時ファイルを消す
pub struct Prepared {
    /// 元のファイルの情報 (タグはこちらを使う)
    pub info: VideoInfo,
    /// 送るファイルの情報 (長さ・ビットレート・音声)
    pub props: VideoInfo,
    /// 送るファイル (元のファイルか一時ファイル)
    pub file: PathBuf,
    pub converted: bool,
    /// 一覧の画像 (頼んだときだけ)
    pub thumb: Option<Vec<u8>>,
    tmp: Option<PathBuf>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(t) = &self.tmp {
            let _ = std::fs::remove_file(t);
        }
    }
}

/// 送る準備: `target` で再生できなければ変換し、MP4 の目次が後ろにあれば並べ替える。時間がかかるので、
/// iPod の lock を持たずに呼ぶ (呼ぶ側が `info` を先に読んで、重複などを確かめておく)
pub fn prepare(src: &Path, ffmpeg: Option<&Path>, target: VideoTarget, info: VideoInfo, want_thumb: bool, progress: &Progress) -> Result<Prepared> {
    let dir = std::env::temp_dir().join("zenpod");
    std::fs::create_dir_all(&dir)?;
    let ext = if target == VideoTarget::Rockbox { "mpg" } else { "m4v" };
    let tmp = dir.join(format!("{:016x}.{ext}", rand::random::<u64>()));
    let need = |what: String| {
        Error::Unsupported(t!("{what}変換には ffmpeg が必要です (設定 → 変換 から自動で入れられます)", what = what))
    };
    let mut p = Prepared { props: info.clone(), info, file: src.to_path_buf(), converted: false, thumb: None, tmp: None };
    if !playable(src, &p.info, target) {
        let ff = ffmpeg.ok_or_else(|| {
            need(match target {
                VideoTarget::Ipod(_) => t!("この動画は iPod で再生できる形への変換が必要です。"),
                VideoTarget::Rockbox => t!("この動画は Rockbox で再生できる形 (MPEG-2) への変換が必要です。"),
            })
        })?;
        p.tmp = Some(tmp.clone());
        convert(ff, src, &tmp, target, &p.info, progress)?;
        p.file = tmp;
        p.converted = true;
    } else if target != VideoTarget::Rockbox && crate::media::convert::moov_after_mdat(src) {
        // 目次 (moov) が後ろにある MP4 は、先頭へ並べ替えてから送る (曲と同じ。並べ替えに失敗しても元のまま送る)
        let r = match ffmpeg {
            Some(ff) => crate::media::convert::faststart(ff, src, &tmp),
            None => crate::media::convert::faststart_native(src, &tmp),
        };
        match r {
            Ok(()) => {
                p.tmp = Some(tmp.clone());
                p.file = tmp;
            }
            Err(e) => {
                log::warn!("faststart: {e}");
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    if p.file != src {
        p.props = probe_mp4(&p.file).or_else(|| probe_mpeg_ps(&p.file)).unwrap_or_else(|| p.info.clone());
    }
    if want_thumb {
        p.thumb = thumbnail(ffmpeg, &p.file, &p.info);
    }
    Ok(p)
}

/// 一覧に出す画像。埋め込みの画像 (MP4 の covr) があればそれ、無ければ ffmpeg で 1 コマ (長さの 1 割のところ) を JPEG で
pub fn thumbnail(ffmpeg: Option<&Path>, src: &Path, info: &VideoInfo) -> Option<Vec<u8>> {
    if let Some(c) = &info.tags.cover {
        return Some(c.clone());
    }
    let ff = ffmpeg?;
    let at = (info.duration_ms as f64 / 1000.0 * 0.1).min(60.0);
    let out = no_window(
        crate::host::command(ff)
            .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-ss", &format!("{at:.2}"), "-i"])
            .arg(src)
            .args(["-frames:v", "1", "-vf", "scale=w='min(640,iw)':h=-2", "-f", "image2pipe", "-c:v", "mjpeg", "-q:v", "3", "-"]),
    )
    .stdin(Stdio::null())
    .stderr(Stdio::null())
    .output()
    .ok()?;
    (out.status.success() && out.stdout.len() > 100).then_some(out.stdout)
}

/// 開けない画像 (HEIC など) を ffmpeg で PNG にする
pub fn decode_image_with_ffmpeg(ffmpeg: &Path, src: &Path) -> Option<Vec<u8>> {
    let out = no_window(
        crate::host::command(ffmpeg)
            .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
            .arg(src)
            .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"]),
    )
    .stdin(Stdio::null())
    .stderr(Stdio::null())
    .output()
    .ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
}

// ---------------- アプリの中での再生 ----------------
// WebView がそのまま再生できない動画 (Rockbox の MPEG-1 / 2、MPEG-4 Part 2 など) は、ffmpeg で変換しながら
// 標準出力に流し、画面が Media Source Extensions で少しずつ読む。位置を飛ばしたらその位置から変換し直す

/// WebView でそのまま再生できるか (H.264 と AAC の MP4)。WebKitGTK は GStreamer に H.264 が無いと再生できないので、
/// 画面は失敗したら変換して流す方に切り替える
pub fn webview_playable(info: &VideoInfo) -> bool {
    info.mp4 && matches!(info.codec, VideoCodec::H264 { .. }) && info.audio.as_ref().is_none_or(|a| a.codec == "aac")
}

/// 変換して流す形式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewFormat {
    /// WebM (VP8 + Vorbis / Opus)。H.264 を再生できない WebView (GStreamer に H.264 が無い WebKitGTK など) 向けの予備。
    /// 新しい ffmpeg (Windows に入れる n9.0.2) は塊の中で音声と映像の時刻が前後する WebM を書き、Chromium の MediaSource が受け付けない
    Webm,
    /// 断片化した MP4 (H.264 + AAC)。こちらを先に使う (Windows の LGPL 版の ffmpeg は OpenH264、Mac は VideoToolbox か x264)
    Mp4,
}

impl PreviewFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "webm" => Some(PreviewFormat::Webm),
            "mp4" => Some(PreviewFormat::Mp4),
            _ => None,
        }
    }
}

/// 変換して流す手順: 形式・エンコーダの引数・画面の MediaSource に渡す MIME タイプ (codecs 付き)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPlan {
    pub format: PreviewFormat,
    pub mime: String,
    video: Vec<&'static str>,
    audio: Vec<&'static str>,
}

/// その形式で流すのに要るエンコーダが ffmpeg にあれば、その手順
pub fn preview_plan(ffmpeg: &Path, format: PreviewFormat, has_audio: bool) -> Option<PreviewPlan> {
    let list = encoders(ffmpeg);
    preview_plan_with(format, has_audio, &|name| has_encoder(&list, name), h264_encoder(ffmpeg))
}

fn preview_plan_with(format: PreviewFormat, has_audio: bool, has: &dyn Fn(&str) -> bool, h264: H264Encoder) -> Option<PreviewPlan> {
    let (container, video, vcodec, audio, acodec): (_, Vec<&'static str>, _, Vec<&'static str>, _) = match format {
        PreviewFormat::Webm => {
            if !has("libvpx") {
                return None;
            }
            // 音声は Vorbis を先に使う。Opus は先頭の時刻が負 (エンコーダの遅れの分) になり、
            // Chromium の MediaSource が "Got a block with a timecode before the previous block" で受け付けない
            let (audio, acodec) = if has("libvorbis") {
                (vec!["-c:a", "libvorbis", "-q:a", "5", "-ar", "48000"], "vorbis")
            } else if has("libopus") {
                (vec!["-c:a", "libopus", "-b:a", "128k", "-ar", "48000"], "opus")
            } else {
                return None;
            };
            // 速さを優先 (画面に合わせた大きさの動画なら、元の長さよりずっと早く変換できる)
            let video = vec!["-c:v", "libvpx", "-deadline", "realtime", "-cpu-used", "8", "-b:v", "2500k", "-qmin", "4", "-qmax", "40", "-g", "60"];
            ("webm", video, "vp8", audio, acodec)
        }
        PreviewFormat::Mp4 => {
            let (video, vcodec): (Vec<&'static str>, _) = match h264 {
                H264Encoder::X264 => (vec!["-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-profile:v", "main", "-crf", "21", "-g", "48"], "avc1.4D401F"),
                H264Encoder::VideoToolbox => (vec!["-c:v", "h264_videotoolbox", "-realtime", "1", "-allow_sw", "1", "-profile:v", "main", "-b:v", "4M", "-g", "48"], "avc1.4D401F"),
                H264Encoder::OpenH264 => (vec!["-c:v", "libopenh264", "-b:v", "3M", "-g", "48"], "avc1.42E01F"),
                H264Encoder::Mpeg4 => return None,
            };
            ("mp4", video, vcodec, vec!["-c:a", "aac", "-b:a", "160k", "-ar", "48000"], "mp4a.40.2")
        }
    };
    let codecs = if has_audio { format!("{vcodec}, {acodec}") } else { vcodec.to_string() };
    Some(PreviewPlan { format, mime: format!("video/{container}; codecs=\"{codecs}\""), video, audio: if has_audio { audio } else { Vec::new() } })
}

/// 変換して流す ffmpeg の引数 (ffmpeg の場所を除く。入力は `-i` の後の src)
fn preview_args(plan: &PreviewPlan, src: &Path, start_ms: u32) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = Vec::new();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(std::ffi::OsString::from));
    // 入力の中身を調べるために読む量を減らす (既定の 5 MB / 5 秒は、iPod の HDD からだと流し始めが遅れる)
    push(&["-v", "error", "-nostdin", "-probesize", "2000000", "-analyzeduration", "2000000"]);
    // 入力の前の -ss は速く飛ぶ (変換するので位置はコマ単位で合う)。出力の時刻は 0 から (画面が開始位置をずらす)
    if start_ms > 0 {
        push(&["-ss", &format!("{}.{:03}", start_ms / 1000, start_ms % 1000)]);
    }
    push(&["-i"]);
    a.push(src.as_os_str().to_os_string());
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(std::ffi::OsString::from));
    push(&["-map", "0:v:0", "-map", "0:a:0?", "-sn", "-dn", "-map_chapters", "-1"]);
    // 大きすぎる動画は 1280 の幅まで縮める (幅と高さは偶数に)
    push(&["-vf", "scale='trunc(min(1280,iw)/2)*2':-2", "-pix_fmt", "yuv420p"]);
    push(&plan.video);
    if plan.audio.is_empty() {
        push(&["-an"]);
    } else {
        push(&plan.audio);
        push(&["-ac", "2"]);
    }
    match plan.format {
        PreviewFormat::Webm => push(&["-f", "webm", "-cluster_time_limit", "500"]),
        PreviewFormat::Mp4 => push(&["-f", "mp4", "-movflags", "frag_keyframe+empty_moov+default_base_moof", "-frag_duration", "500000"]),
    }
    push(&["pipe:1"]);
    a
}

/// 変換して流している ffmpeg。止めずに捨てたら終わらせる
pub struct PreviewProcess {
    child: std::process::Child,
    /// 変換した動画 (読むスレッドに渡すときは take する)
    pub stdout: Option<std::process::ChildStdout>,
    /// ffmpeg のエラーの表示 (終わりのほうだけ)
    stderr_tail: std::sync::Arc<Mutex<String>>,
}

impl PreviewProcess {
    /// ffmpeg のエラーの表示 (失敗したとき用)
    pub fn error_text(&self) -> String {
        self.stderr_tail.lock().unwrap().trim().to_string()
    }

    /// 終わっていれば、うまく終わったか
    pub fn finished_ok(&mut self) -> Option<bool> {
        self.child.try_wait().ok().flatten().map(|s| s.success())
    }
}

impl Drop for PreviewProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// start_ms の位置から変換して流し始める
pub fn spawn_preview(ffmpeg: &Path, plan: &PreviewPlan, src: &Path, start_ms: u32) -> Result<PreviewProcess> {
    let mut child = no_window(crate::host::command(ffmpeg).args(preview_args(plan, src, start_ms)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // エラーの表示が多いと (壊れたところのある MPEG など) パイプが詰まって止まるので、別のスレッドで読み続けて終わりだけ残す
    let stderr_tail = std::sync::Arc::new(Mutex::new(String::new()));
    if let Some(err) = child.stderr.take() {
        let tail = stderr_tail.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(|l| l.ok()) {
                let mut t = tail.lock().unwrap();
                t.push_str(&line);
                t.push('\n');
                if t.len() > 4000 {
                    let cut = t.len() - 2000;
                    let cut = (cut..t.len()).find(|&i| t.is_char_boundary(i)).unwrap_or(t.len());
                    t.drain(..cut);
                }
            }
        });
    }
    let stdout = child.stdout.take();
    Ok(PreviewProcess { child, stdout, stderr_tail })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ffmpeg() -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file()))
    }

    fn make(ff: &Path, out: &Path, extra: &[&str]) {
        let mut cmd = Command::new(ff);
        cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=2", "-f", "lavfi", "-i", "sine=duration=2"])
            .args(extra)
            .arg(out);
        assert!(cmd.status().unwrap().success(), "{extra:?}");
    }

    #[test]
    fn parses_ffmpeg_output() {
        let text = "Input #0, matroska,webm, from 'a.mkv':\n  Metadata:\n    title           : 番組 1\n    show            : Show\n    season_number   : 2\n  Duration: 00:01:02.50, start: 0.000000, bitrate: 2000 kb/s\n  Stream #0:0: Video: h264 (High), yuv420p(progressive), 1920x1080 [SAR 1:1 DAR 16:9], 23.98 fps, 23.98 tbr, 1k tbn\n    Metadata:\n      title           : track\n  Stream #0:1(jpn): Audio: aac (LC), 48000 Hz, stereo, fltp\n";
        let i = parse_ffmpeg_info(text);
        assert_eq!(i.duration_ms, 62_500);
        assert_eq!((i.width, i.height), (1920, 1080));
        assert_eq!(i.codec, VideoCodec::H264 { profile: 100, level: 0 });
        assert!((i.fps - 23.98).abs() < 0.01);
        let a = i.audio.unwrap();
        assert!(a.aac_lc && a.sample_rate == 48000 && a.channels == 2);
        assert_eq!(i.tags.title.as_deref(), Some("番組 1"));
        assert_eq!(i.tags.show.as_deref(), Some("Show"));
        assert_eq!(i.tags.season, 2);
    }

    #[test]
    fn placement_overrides_tags() {
        use crate::domain::video::{VideoKind, VideoPlacement};
        let base = VideoTags { show: Some("元の番組".into()), artist: Some("元".into()), media_kind: Some(10), ..Default::default() };
        let mut t = base.clone();
        apply_placement(&mut t, &VideoPlacement { kind: Some(VideoKind::Movie), ..Default::default() });
        assert_eq!((t.media_kind, t.show.as_deref()), (Some(9), None));
        let mut t = base.clone();
        apply_placement(&mut t, &VideoPlacement { kind: Some(VideoKind::TvShow), show: Some("番組".into()), artist: None });
        assert_eq!((t.media_kind, t.show.as_deref()), (Some(10), Some("番組")));
        let mut t = base.clone();
        apply_placement(&mut t, &VideoPlacement { kind: Some(VideoKind::MusicVideo), show: None, artist: Some("歌手".into()) });
        assert_eq!((t.media_kind, t.show.as_deref(), t.artist.as_deref()), (Some(6), None, Some("歌手")));
        assert_eq!(VideoKind::from_tags(t.media_kind, t.show.is_some()), VideoKind::MusicVideo);
        // 指定が無ければそのまま (すべてのビデオに落としたとき)
        let mut t = base.clone();
        apply_placement(&mut t, &VideoPlacement::default());
        assert_eq!((t.media_kind, t.show.as_deref()), (Some(10), Some("元の番組")));
    }

    #[test]
    fn preview_plans() {
        let all = |_: &str| true;
        let p = preview_plan_with(PreviewFormat::Webm, true, &all, H264Encoder::X264).unwrap();
        assert_eq!(p.mime, "video/webm; codecs=\"vp8, vorbis\"");
        let p = preview_plan_with(PreviewFormat::Webm, false, &all, H264Encoder::X264).unwrap();
        assert_eq!(p.mime, "video/webm; codecs=\"vp8\"");
        assert!(p.audio.is_empty());
        let opus = |n: &str| n != "libvorbis";
        assert!(preview_plan_with(PreviewFormat::Webm, true, &opus, H264Encoder::X264).unwrap().mime.contains("opus"));
        assert!(preview_plan_with(PreviewFormat::Webm, true, &|n: &str| n != "libvpx", H264Encoder::X264).is_none());
        let p = preview_plan_with(PreviewFormat::Mp4, true, &all, H264Encoder::OpenH264).unwrap();
        assert_eq!(p.mime, "video/mp4; codecs=\"avc1.42E01F, mp4a.40.2\"");
        assert!(preview_plan_with(PreviewFormat::Mp4, true, &all, H264Encoder::Mpeg4).is_none());
        let a: Vec<String> = preview_args(&p, Path::new("a.mpg"), 61_005).iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert!(a.windows(2).any(|w| w == ["-ss", "61.005"]));
        assert!(a.windows(2).any(|w| w == ["-c:v", "libopenh264"]));
        assert_eq!(a.last().map(String::as_str), Some("pipe:1"));
        let a: Vec<String> = preview_args(&p, Path::new("a.mpg"), 0).iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert!(!a.contains(&"-ss".to_string()));
    }

    /// 本物の ffmpeg で Rockbox の .mpg を変換して流す (WebM と断片化した MP4)
    #[test]
    fn preview_streams() {
        let Some(ff) = ffmpeg() else { return };
        let d = tempfile::tempdir().unwrap();
        let mpg = d.path().join("a.mpg");
        make(&ff, &mpg, &["-s", "320x240", "-c:v", "mpeg2video", "-c:a", "mp2", "-f", "mpeg"]);
        for (format, magic) in [(PreviewFormat::Webm, b"\x1A\x45\xDF\xA3".as_slice()), (PreviewFormat::Mp4, b"ftyp".as_slice())] {
            let Some(plan) = preview_plan(&ff, format, true) else { continue };
            let mut p = spawn_preview(&ff, &plan, &mpg, 1000).unwrap();
            let mut out = Vec::new();
            p.stdout.take().unwrap().read_to_end(&mut out).unwrap();
            assert!(out.len() > 1000 && out.windows(magic.len()).take(16).any(|w| w == magic), "{format:?}: {}", p.error_text());
            if format == PreviewFormat::Mp4 {
                assert!(out.windows(4).any(|w| w == b"moof"));
            }
        }
    }

    #[test]
    fn rates_and_args() {
        assert_eq!(mpeg2_rate(23.976), "24000/1001");
        assert_eq!(mpeg2_rate(25.0), "25");
        assert_eq!(mpeg2_rate(29.97), "30000/1001");
        assert_eq!(mpeg2_rate(60.0), "30");
        let a = convert_args(VideoTarget::Ipod(VideoSize::Qvga), H264Encoder::X264, &VideoInfo::default());
        assert!(a.windows(2).any(|w| w == ["-level:v", "1.3"]));
        assert!(a.windows(2).any(|w| w == ["-f", "mp4"]));
        // 音声は曲の変換と同じく、iPod がうまく扱えない AAC の道具を切る
        assert!(a.windows(2).any(|w| w == ["-aac_pns", "0"]));
        let r = convert_args(VideoTarget::Rockbox, H264Encoder::X264, &VideoInfo { fps: 29.97, ..Default::default() });
        assert!(r.windows(2).any(|w| w == ["-c:v", "mpeg2video"]));
        assert!(r.windows(2).any(|w| w == ["-r", "30000/1001"]));
    }

    /// ffmpeg がある環境だけ: 作った動画を読み、変換して、iPod / Rockbox で再生できる形になる
    #[test]
    fn probe_and_convert() {
        let Some(ff) = ffmpeg() else { return };
        let d = tempfile::tempdir().unwrap();
        // iPod でそのまま再生できない (High・720p)
        let high = d.path().join("high.mp4");
        make(&ff, &high, &["-pix_fmt", "yuv420p", "-c:v", "libx264", "-profile:v", "high", "-c:a", "aac", "-metadata", "title=題名", "-metadata", "show=番組", "-metadata", "season_number=3", "-metadata", "episode_sort=4", "-metadata", "media_type=10"]);
        assert!(is_video_file(&high));
        let i = probe(&high, None).unwrap();
        assert!(i.mp4);
        assert_eq!((i.width, i.height), (1280, 720));
        assert!(matches!(i.codec, VideoCodec::H264 { profile: 100, .. }));
        assert!(i.audio.as_ref().unwrap().aac_lc);
        assert!((1900..2200).contains(&i.duration_ms), "{}", i.duration_ms);
        assert_eq!(i.tags.title.as_deref(), Some("題名"));
        assert_eq!(i.tags.show.as_deref(), Some("番組"));
        assert_eq!((i.tags.season, i.tags.episode, i.tags.media_kind), (3, 4, Some(10)));
        assert!(!ipod_playable(&i));

        let calls = std::cell::Cell::new(0);
        let p = Progress { report: &|f| { assert!((0.0..=1.0).contains(&f)); calls.set(calls.get() + 1) }, cancelled: &|| false };
        let out = d.path().join("out.m4v");
        convert(&ff, &high, &out, VideoTarget::Ipod(VideoSize::Vga), &i, &p).unwrap();
        assert!(calls.get() > 0);
        let o = probe(&out, None).unwrap();
        assert_eq!((o.width, o.height), (640, 360));
        assert!(ipod_playable(&o), "{o:?}");
        assert_eq!(o.tags.show.as_deref(), Some("番組"));
        assert_eq!(o.tags.episode, 4);
        assert!(thumbnail(Some(&ff), &out, &o).is_some_and(|j| j.starts_with(&[0xFF, 0xD8])));

        let mpg = d.path().join("out.mpg");
        convert(&ff, &high, &mpg, VideoTarget::Rockbox, &i, &p).unwrap();
        let m = probe_mpeg_ps(&mpg).unwrap();
        assert_eq!((m.width, m.height), (320, 180));
        assert_eq!(m.codec, VideoCodec::Mpeg2);
        assert!((1500..2600).contains(&m.duration_ms), "{}", m.duration_ms);
        assert!(rockbox_playable(&mpg, &m));

        // 中止
        let stop = d.path().join("stop.m4v");
        let p = Progress { report: &|_| {}, cancelled: &|| true };
        assert!(matches!(convert(&ff, &high, &stop, VideoTarget::Ipod(VideoSize::Vga), &i, &p), Err(Error::Cancelled)));
        assert!(!stop.exists());

        // 音声だけの .mp4 と .webm は動画ではない
        let audio = d.path().join("a.mp4");
        assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "aac"]).arg(&audio).status().unwrap().success());
        assert!(!is_video_file(&audio));
        let webm = d.path().join("a.webm");
        assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "libopus"]).arg(&webm).status().unwrap().success());
        assert!(!is_video_file(&webm));
        let vwebm = d.path().join("v.webm");
        make(&ff, &vwebm, &["-t", "0.5", "-c:v", "libvpx", "-c:a", "libopus"]);
        assert!(is_video_file(&vwebm));
    }
}
