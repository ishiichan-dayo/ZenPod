//! iPod shuffle の曲の一覧 (iPod_Control/iTunes/iTunesSD) と再生回数 (iTunesStats)。
//!
//! shuffle は iTunesDB ではなく iTunesSD を読んで再生する (iTunesDB も libgpod などが読むので書く)。
//! 署名は要らない。形式は 2 つ:
//! - 1G / 2G (`SdFormat::V1`): 18 バイトのヘッダ + 1 曲 558 バイトの固定長。整数は 24 ビットのビッグエンディアン、
//!   場所は UTF-16LE (`/` 区切り)
//! - 3G / 4G (`SdFormat::V2`): bdhs (全体) → hths (曲の目次) → rths (曲) → hphs (プレイリストの目次) → lphs (プレイリスト)。
//!   整数はリトルエンディアン、場所は UTF-8
//!
//! 形は libgpod 0.8.3 の itdb_itunesdb.c (itdb_shuffle_write_file・write_bdhs など、LGPL-2.1-or-later) に合わせた。
//! ただし 1G / 2G の「シャッフルに含める」の 1 バイトは、libgpod は iTunesDB の「シャッフル時にスキップ」をそのまま
//! 書いていて逆になっている (gnupod と libgpod 自身の説明では 1 = 含める) ので、ここでは反転して書く。
//!
//! 本体が書く iTunesStats は iTunesSD と同じ曲の順に並ぶ。曲の順は保存のたびに変わりうるので、開いたときに
//! 前の iTunesSD の場所で曲を突き合わせて取り込み、保存したら消す。

use std::collections::HashMap;

use serde::Serialize;

use super::itunesdb::{ITunesDb, Track};
use crate::domain::audio::MediaKind;

/// iTunesSD の形式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SdFormat {
    /// shuffle 1G / 2G
    V1,
    /// shuffle 3G / 4G
    V2,
}

const V1_HEADER: usize = 0x12;
const V1_ENTRY: usize = 0x22E;
/// V1 の場所の欄 (UTF-16 で 261 文字)
const V1_PATH_CHARS: usize = 261;
const RTHS_LEN: usize = 0x174;
const BDHS_LEN: usize = 64;
/// V2 の場所の欄 (終わりの 0 を含めて 256 バイト)
const V2_PATH_BYTES: usize = 256;

/// iTunesDB の場所 (`:iPod_Control:Music:F00:ABCD.mp3`) を iTunesSD の形 (`/iPod_Control/Music/F00/ABCD.mp3`) に
fn sd_path(t: &Track) -> String {
    t.ipod_path().unwrap_or_default().replace(':', "/")
}

/// 曲のファイルの種類 (1 = MP3、2 = AAC / ALAC (m4a)、4 = WAV)。
/// libgpod は iTunesDB の種類の文字列で見ているが、ALAC は "Apple Lossless audio file" で MP3 扱いになるので、拡張子で見る
fn file_type(t: &Track) -> u32 {
    let path = t.ipod_path().unwrap_or_default().to_ascii_lowercase();
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "m4a" | "m4b" | "m4p" | "mp4" | "aac" => 2,
        "wav" => 4,
        _ => 1,
    }
}

fn is_special(t: &Track) -> bool {
    matches!(t.kind(), MediaKind::Podcast | MediaKind::Audiobook)
}

/// 保存する iTunesSD の中身。`db` は `serialize` 済み (アルバム・アーティストの番号が入っている) であること
pub fn write_itunes_sd(db: &ITunesDb, format: SdFormat) -> Vec<u8> {
    match format {
        SdFormat::V1 => write_v1(db),
        SdFormat::V2 => write_v2(db),
    }
}

fn put24be(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes()[1..]);
}

fn write_v1(db: &ITunesDb) -> Vec<u8> {
    let mut out = Vec::with_capacity(V1_HEADER + db.tracks.len() * V1_ENTRY);
    put24be(&mut out, db.tracks.len() as u32);
    put24be(&mut out, 0x010600);
    put24be(&mut out, V1_HEADER as u32);
    out.resize(V1_HEADER, 0);
    for t in &db.tracks {
        let start = out.len();
        put24be(&mut out, V1_ENTRY as u32);
        put24be(&mut out, 0x5AA501);
        // 始め・終わりの位置は 256 ミリ秒単位
        put24be(&mut out, t.start_ms() / 256);
        put24be(&mut out, 0);
        put24be(&mut out, 0);
        put24be(&mut out, t.stop_ms() / 256);
        put24be(&mut out, 0);
        put24be(&mut out, 0);
        // 音量 -255〜255 → 0〜200 (100 が ±0)
        put24be(&mut out, ((t.volume().clamp(-255, 255) + 255) * 201 / 511) as u32);
        put24be(&mut out, file_type(t));
        put24be(&mut out, 0x200);
        let units: Vec<u16> = sd_path(t).encode_utf16().take(V1_PATH_CHARS).collect();
        for u in &units {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out.resize(start + 0x21 + V1_PATH_CHARS * 2, 0);
        out.push(!t.skip_shuffle() as u8);
        out.push(t.remember_position() as u8);
        out.push(0);
        debug_assert_eq!(out.len() - start, V1_ENTRY);
    }
    out
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn set32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// 場所を 255 バイトまでに切る (文字の途中では切らない。終わりの 0 を残す)
fn v2_path(t: &Track) -> Vec<u8> {
    let p = sd_path(t);
    let mut end = p.len().min(V2_PATH_BYTES - 1);
    while !p.is_char_boundary(end) {
        end -= 1;
    }
    p.as_bytes()[..end].to_vec()
}

fn write_rths(out: &mut Vec<u8>, t: &Track) {
    let start = out.len();
    out.extend_from_slice(b"rths");
    put32(out, RTHS_LEN as u32);
    put32(out, t.start_ms());
    put32(out, t.stop_ms());
    put32(out, t.volume().clamp(-255, 255) as u32);
    put32(out, file_type(t));
    let path = v2_path(t);
    out.extend_from_slice(&path);
    out.resize(start + 24 + V2_PATH_BYTES, 0);
    put32(out, t.bookmark_ms());
    // iTunesDB とは逆で 1 = シャッフルに含める
    out.push(!t.skip_shuffle() as u8);
    out.push(t.remember_position() as u8);
    // ギャップレスのアルバム・不明
    out.extend_from_slice(&[0, 0]);
    put32(out, t.pregap());
    put32(out, t.postgap());
    put32(out, t.samplecount() as u32);
    put32(out, 0);
    put32(out, t.gapless_data());
    put32(out, 0);
    put32(out, t.album_id());
    put16(out, t.track_nr().min(u16::MAX as u32) as u16);
    put16(out, t.cd_nr().min(u16::MAX as u32) as u16);
    out.extend_from_slice(&[0; 8]);
    // VoiceOver の読み上げのファイル名にも使われる
    out.extend_from_slice(&t.dbid().to_le_bytes());
    put32(out, t.artist_id());
    out.resize(start + RTHS_LEN, 0);
}

fn write_v2(db: &ITunesDb) -> Vec<u8> {
    let n = db.tracks.len();
    let index: HashMap<u64, u32> = db.tracks.iter().enumerate().map(|(i, t)| (t.dbid(), i as u32)).collect();
    let special: HashMap<u64, bool> = db.tracks.iter().map(|t| (t.dbid(), is_special(t))).collect();
    // 空のプレイリストは書かない。マスターは先頭 (playlists[0])
    let lists: Vec<_> = db
        .playlists
        .iter()
        .map(|p| (p, p.items.iter().filter_map(|id| index.get(id).map(|&i| (i, special[id]))).collect::<Vec<_>>()))
        .filter(|(_, items)| !items.is_empty())
        .collect();

    let mut out = Vec::with_capacity(BDHS_LEN + 20 + n * (4 + RTHS_LEN) + 4096);
    // bdhs (位置の欄は後で埋める)
    out.extend_from_slice(b"bdhs");
    put32(&mut out, 0x02000003);
    put32(&mut out, BDHS_LEN as u32);
    put32(&mut out, n as u32);
    put32(&mut out, lists.len() as u32);
    out.extend_from_slice(&[0; 8]);
    // 音量の制限なし・VoiceOver あり (libgpod と同じ)
    out.push(0);
    out.push(1);
    put16(&mut out, 0);
    put32(&mut out, db.tracks.iter().filter(|t| !is_special(t)).count() as u32);
    put32(&mut out, 0); // hths の位置
    put32(&mut out, 0); // hphs の位置
    out.resize(BDHS_LEN, 0);

    // hths
    let hths = out.len();
    set32(&mut out, 36, hths as u32);
    out.extend_from_slice(b"hths");
    put32(&mut out, (20 + 4 * n) as u32);
    put32(&mut out, n as u32);
    out.extend_from_slice(&[0; 8]);
    let table = out.len();
    out.resize(table + 4 * n, 0);
    for (i, t) in db.tracks.iter().enumerate() {
        let at = out.len() as u32;
        set32(&mut out, table + 4 * i, at);
        write_rths(&mut out, t);
    }

    // hphs
    let hphs = out.len();
    set32(&mut out, 40, hphs as u32);
    let podcasts = lists.iter().filter(|(p, _)| !p.is_master() && p.is_podcast()).count();
    let masters = lists.iter().filter(|(p, _)| p.is_master()).count();
    out.extend_from_slice(b"hphs");
    put32(&mut out, (20 + 4 * lists.len()) as u32);
    put16(&mut out, lists.len() as u16);
    put16(&mut out, 0);
    put16(&mut out, (lists.len() - podcasts) as u16);
    put16(&mut out, masters as u16);
    // オーディオブックのプレイリストは作らない
    put16(&mut out, lists.len() as u16);
    put16(&mut out, 0);
    let table = out.len();
    out.resize(table + 4 * lists.len(), 0);
    for (k, (p, items)) in lists.iter().enumerate() {
        let at = out.len() as u32;
        set32(&mut out, table + 4 * k, at);
        let kind = if p.is_master() {
            1
        } else if p.is_podcast() {
            3
        } else {
            2
        };
        out.extend_from_slice(b"lphs");
        put32(&mut out, (44 + 4 * items.len()) as u32);
        put32(&mut out, items.len() as u32);
        put32(&mut out, items.iter().filter(|(_, s)| !s).count() as u32);
        // VoiceOver の読み上げのファイル名にも使われる (マスターは 0)
        out.extend_from_slice(&if p.is_master() { 0 } else { p.id() }.to_le_bytes());
        put32(&mut out, kind);
        out.extend_from_slice(&[0; 16]);
        for (i, _) in items {
            put32(&mut out, *i);
        }
    }
    out
}

/// `sd_format_of` で見分けるのに要る先頭のバイト数
pub const SD_SNIFF_LEN: usize = V1_HEADER;

/// iTunesSD の形式を中身から見分ける
pub fn sd_format_of(data: &[u8]) -> Option<SdFormat> {
    if data.starts_with(b"bdhs") {
        Some(SdFormat::V2)
    } else if data.len() >= V1_HEADER && be24(data, 6) == V1_HEADER as u32 {
        Some(SdFormat::V1)
    } else {
        None
    }
}

fn be24(d: &[u8], at: usize) -> u32 {
    d.get(at..at + 3).map_or(0, |b| u32::from_be_bytes([0, b[0], b[1], b[2]]))
}
fn le24(d: &[u8], at: usize) -> u32 {
    d.get(at..at + 3).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], 0]))
}
fn le32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// iTunesSD に書かれた曲の場所 (順番どおり)。読めなければ None
pub fn sd_paths(data: &[u8]) -> Option<(SdFormat, Vec<String>)> {
    let format = sd_format_of(data)?;
    let mut paths = Vec::new();
    match format {
        SdFormat::V1 => {
            let n = be24(data, 0) as usize;
            for i in 0..n {
                let o = V1_HEADER + i * V1_ENTRY;
                let raw = data.get(o + 0x21..o + 0x21 + V1_PATH_CHARS * 2)?;
                let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
                paths.push(String::from_utf16_lossy(&units));
            }
        }
        SdFormat::V2 => {
            let hths = le32(data, 36)? as usize;
            if data.get(hths..hths + 4)? != b"hths" {
                return None;
            }
            let n = le32(data, hths + 8)? as usize;
            for i in 0..n {
                let o = le32(data, hths + 20 + 4 * i)? as usize;
                if data.get(o..o + 4)? != b"rths" {
                    return None;
                }
                let raw = data.get(o + 24..o + 24 + V2_PATH_BYTES)?;
                let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                paths.push(String::from_utf8_lossy(&raw[..end]).into_owned());
            }
        }
    }
    Some((format, paths))
}

/// 本体が書いた iTunesStats の 1 曲分
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Stat {
    bookmark_ms: u32,
    plays: u32,
    skips: u32,
    /// 最後に再生した日時 (Mac 時刻、V2 だけ)
    last_played: u32,
    /// 最後に飛ばした日時 (Mac 時刻、V2 だけ)
    last_skipped: u32,
}

fn parse_stats(format: SdFormat, data: &[u8]) -> Option<Vec<Stat>> {
    let mut out = Vec::new();
    match format {
        SdFormat::V1 => {
            // 件数 (24 ビット) のあと 6 バイト目から。1 件は長さ (18 以上)・しおり・?・?・再生回数・飛ばした回数 (各 24 ビット)
            let n = le24(data, 0) as usize;
            let mut o = 6;
            for _ in 0..n {
                let len = le24(data, o) as usize;
                if len < 18 || o + 18 > data.len() {
                    return None;
                }
                out.push(Stat { bookmark_ms: le24(data, o + 3), plays: le24(data, o + 12), skips: le24(data, o + 15), ..Stat::default() });
                o += len;
            }
        }
        SdFormat::V2 => {
            // 件数 (32 ビット) のあと 8 バイト目から。1 件は長さ (32 以上)・しおり・再生回数・最後の再生・飛ばした回数・最後に飛ばした日時
            let n = le32(data, 0)? as usize;
            let mut o = 8;
            for _ in 0..n {
                let len = le32(data, o)? as usize;
                if len < 32 || o + 24 > data.len() {
                    return None;
                }
                out.push(Stat {
                    bookmark_ms: le32(data, o + 4)?,
                    plays: le32(data, o + 8)?,
                    last_played: le32(data, o + 12)?,
                    skips: le32(data, o + 16)?,
                    last_skipped: le32(data, o + 20)?,
                });
                o += len;
            }
        }
    }
    Some(out)
}

/// 前回の iTunesSD と本体が書いた iTunesStats から、再生回数・飛ばした回数・しおりを iTunesDB の曲に足す。
/// 曲は iTunesSD の場所で突き合わせる (保存のたびに曲の順が変わるため)。数が合わないなど読めなければ何もしない。
/// 戻り値は反映した曲の数
pub fn merge_stats(db: &mut ITunesDb, itunes_sd: &[u8], stats: &[u8]) -> usize {
    let Some((format, paths)) = sd_paths(itunes_sd) else { return 0 };
    let Some(stats) = parse_stats(format, stats) else { return 0 };
    if stats.len() != paths.len() {
        log::warn!("iTunesStats: {} entries but iTunesSD has {} tracks, not merging", stats.len(), paths.len());
        return 0;
    }
    let by_path: HashMap<String, usize> = db.tracks.iter().enumerate().map(|(i, t)| (sd_path(t).to_ascii_lowercase(), i)).collect();
    let mut merged = 0;
    for (path, s) in paths.iter().zip(&stats) {
        let Some(&i) = by_path.get(&path.to_ascii_lowercase()) else { continue };
        let t = &mut db.tracks[i];
        let mut changed = false;
        if s.plays != 0 {
            t.set_playcount(t.playcount().saturating_add(s.plays));
            t.set_mark_unplayed(1);
            if s.last_played != 0 {
                t.set_last_played_mac(s.last_played);
            }
            changed = true;
        }
        if s.skips != 0 {
            t.set_skipcount(t.skipcount().saturating_add(s.skips));
            if s.last_skipped != 0 {
                t.set_last_skipped_mac(s.last_skipped);
            }
            changed = true;
        }
        if s.bookmark_ms != 0 && t.remember_position() {
            t.set_bookmark_ms(s.bookmark_ms);
            changed = true;
        }
        merged += changed as usize;
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipod::itunesdb::{MHOD_PATH, MHOD_TITLE, Playlist};

    fn track(db: &mut ITunesDb, path: &str) -> u64 {
        let mut t = Track::new(db.new_dbid());
        t.set_string(MHOD_TITLE, Some("曲"));
        t.set_string(MHOD_PATH, Some(path));
        let id = t.dbid();
        db.tracks.push(t);
        db.playlists[0].items.push(id);
        id
    }

    fn sample() -> (ITunesDb, [u64; 3]) {
        let mut db = ITunesDb::new_empty("shuffle");
        let a = track(&mut db, ":iPod_Control:Music:F00:AAAA.mp3");
        let b = track(&mut db, ":iPod_Control:Music:F01:BBBB.m4a");
        let c = track(&mut db, ":iPod_Control:Music:F02:日本語.wav");
        db.tracks[1].set_skip_shuffle(true);
        db.tracks[2].set_remember_position(true);
        let mut pl = Playlist::new("好きな曲", 0x1122334455667788, false);
        pl.items = vec![c, a];
        db.playlists.push(pl);
        // 空のプレイリストは書かない
        db.playlists.push(Playlist::new("空", 7, false));
        db.serialize();
        (db, [a, b, c])
    }

    #[test]
    fn v1_layout() {
        let (db, _) = sample();
        let sd = write_itunes_sd(&db, SdFormat::V1);
        assert_eq!(sd.len(), V1_HEADER + 3 * V1_ENTRY);
        assert_eq!(&sd[..9], &[0, 0, 3, 0x01, 0x06, 0x00, 0, 0, 0x12]);
        let e = &sd[V1_HEADER..V1_HEADER + V1_ENTRY];
        assert_eq!(&e[..6], &[0x00, 0x02, 0x2E, 0x5A, 0xA5, 0x01]);
        // 音量 ±0 は 100、種類は MP3
        assert_eq!(be24(e, 0x18), 100);
        assert_eq!(be24(e, 0x1B), 1);
        // 含める・覚えない
        assert_eq!(&e[0x22B..], &[1, 0, 0]);
        let e2 = &sd[V1_HEADER + V1_ENTRY..];
        assert_eq!(be24(e2, 0x1B), 2);
        assert_eq!(e2[0x22B], 0, "シャッフル時にスキップの曲は含めない");
        let e3 = &sd[V1_HEADER + 2 * V1_ENTRY..];
        assert_eq!((be24(e3, 0x1B), e3[0x22C]), (4, 1));
        assert_eq!(sd_format_of(&sd), Some(SdFormat::V1));
        let (_, paths) = sd_paths(&sd).unwrap();
        assert_eq!(paths, ["/iPod_Control/Music/F00/AAAA.mp3", "/iPod_Control/Music/F01/BBBB.m4a", "/iPod_Control/Music/F02/日本語.wav"]);
    }

    #[test]
    fn v2_layout() {
        let (db, _) = sample();
        let sd = write_itunes_sd(&db, SdFormat::V2);
        assert_eq!(&sd[..4], b"bdhs");
        assert_eq!(le32(&sd, 8), Some(64));
        assert_eq!(le32(&sd, 12), Some(3));
        // マスターと「好きな曲」の 2 つ (空は書かない)
        assert_eq!(le32(&sd, 16), Some(2));
        let hths = le32(&sd, 36).unwrap() as usize;
        assert_eq!(hths, 64);
        assert_eq!(&sd[hths..hths + 4], b"hths");
        assert_eq!(le32(&sd, hths + 4), Some(20 + 12));
        let r0 = le32(&sd, hths + 20).unwrap() as usize;
        assert_eq!(r0, hths + 32);
        assert_eq!(&sd[r0..r0 + 4], b"rths");
        assert_eq!(le32(&sd, r0 + 4), Some(RTHS_LEN as u32));
        let r1 = le32(&sd, hths + 24).unwrap() as usize;
        assert_eq!(r1 - r0, RTHS_LEN);
        assert_eq!((le32(&sd, r1 + 20), sd[r1 + 284]), (Some(2), 0));
        assert_eq!(le32(&sd, r0 + 328).unwrap() as u64 | (le32(&sd, r0 + 332).unwrap() as u64) << 32, db.tracks[0].dbid());
        let hphs = le32(&sd, 40).unwrap() as usize;
        assert_eq!(hphs, r0 + 3 * RTHS_LEN);
        assert_eq!(&sd[hphs..hphs + 4], b"hphs");
        assert_eq!(le32(&sd, hphs + 4), Some(20 + 8));
        let l1 = le32(&sd, hphs + 24).unwrap() as usize;
        assert_eq!(&sd[l1..l1 + 4], b"lphs");
        // 種類 2、曲は hths の中の番号 (日本語.wav = 2、AAAA = 0)
        assert_eq!(le32(&sd, l1 + 24), Some(2));
        assert_eq!(le32(&sd, l1 + 16), Some(0x55667788));
        assert_eq!((le32(&sd, l1 + 44), le32(&sd, l1 + 48)), (Some(2), Some(0)));
        assert_eq!(sd.len(), l1 + 52);
        let l0 = le32(&sd, hphs + 20).unwrap() as usize;
        assert_eq!((le32(&sd, l0 + 8), le32(&sd, l0 + 16), le32(&sd, l0 + 24)), (Some(3), Some(0), Some(1)));
        let (f, paths) = sd_paths(&sd).unwrap();
        assert_eq!((f, paths.len()), (SdFormat::V2, 3));
        assert_eq!(paths[2], "/iPod_Control/Music/F02/日本語.wav");
    }

    #[test]
    fn long_paths_are_cut_at_char_boundary() {
        let mut db = ITunesDb::new_empty("shuffle");
        track(&mut db, &format!(":iPod_Control:Music:F00:{}.mp3", "あ".repeat(100)));
        db.serialize();
        let sd = write_itunes_sd(&db, SdFormat::V2);
        let (_, paths) = sd_paths(&sd).unwrap();
        assert!(paths[0].len() <= 255 && paths[0].starts_with("/iPod_Control/Music/F00/あ"));
    }

    #[test]
    fn merges_stats_by_path() {
        let (mut db, [a, _, c]) = sample();
        // 本体の iTunesStats は前回の iTunesSD の順。曲の順を入れ替えてから取り込む
        let sd = write_itunes_sd(&db, SdFormat::V2);
        db.tracks.reverse();
        let mut stats = Vec::new();
        stats.extend_from_slice(&3u32.to_le_bytes());
        stats.extend_from_slice(&[0; 4]);
        for (plays, skips, bookmark) in [(2u32, 1u32, 0u32), (0, 0, 0), (1, 0, 5000)] {
            let mut e = [0u8; 32];
            e[0..4].copy_from_slice(&32u32.to_le_bytes());
            e[4..8].copy_from_slice(&bookmark.to_le_bytes());
            e[8..12].copy_from_slice(&plays.to_le_bytes());
            e[12..16].copy_from_slice(&1234u32.to_le_bytes());
            e[16..20].copy_from_slice(&skips.to_le_bytes());
            stats.extend_from_slice(&e);
        }
        assert_eq!(merge_stats(&mut db, &sd, &stats), 2);
        let get = |id: u64| db.tracks.iter().find(|t| t.dbid() == id).unwrap();
        assert_eq!((get(a).playcount(), get(a).skipcount(), get(a).last_played_mac()), (2, 1, 1234));
        assert_eq!((get(c).playcount(), get(c).bookmark_ms()), (1, 5000));
        // 件数が合わなければ取り込まない
        stats[0] = 2;
        assert_eq!(merge_stats(&mut db, &sd, &stats), 0);
    }

    #[test]
    fn merges_v1_stats() {
        let (mut db, [a, ..]) = sample();
        let sd = write_itunes_sd(&db, SdFormat::V1);
        let mut stats = vec![3, 0, 0, 0, 0, 0];
        for plays in [4u8, 0, 0] {
            let mut e = [0u8; 18];
            e[0] = 18;
            e[12] = plays;
            stats.extend_from_slice(&e);
        }
        assert_eq!(merge_stats(&mut db, &sd, &stats), 1);
        assert_eq!(db.tracks.iter().find(|t| t.dbid() == a).unwrap().playcount(), 4);
    }
}
