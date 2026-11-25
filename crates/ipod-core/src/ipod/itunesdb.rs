//! iTunesDB (iPod_Control/iTunes/iTunesDB) のパーサ / ライタ。
//!
//! 方針:
//! - 既存レコードのヘッダは生バイトのまま保持し、必要なフィールドだけ書き換える
//!   (iTunes が書いた未知フィールドを壊さないため)。
//! - 新規レコードのレイアウトは libgpod (itdb_itunesdb.c) の書き出し形式に合わせる。
//! - 書き出し時の mhsd 順序も libgpod と同じ: 1, 3, 2, 4, 8, 6, 10, 5, (9)。

use crate::t;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::error::{Error, Result};
use crate::domain::audio::MediaKind;
use crate::domain::video::VideoKind;
use crate::smartpl::*;

pub const MHBD_HLEN: usize = 0xF4;
pub const MHIT_HLEN: usize = 0x248;
pub const MHYP_HLEN: usize = 108;
const FIRST_TRACK_ID: u32 = 52;
/// 1904-01-01 から 1970-01-01 までの秒数
const MAC_EPOCH_OFFSET: i64 = 2_082_844_800;

// ---- MHOD 種別 ----
pub const MHOD_TITLE: u32 = 1;
pub const MHOD_PATH: u32 = 2;
pub const MHOD_ALBUM: u32 = 3;
pub const MHOD_ARTIST: u32 = 4;
pub const MHOD_GENRE: u32 = 5;
pub const MHOD_FILETYPE: u32 = 6;
/// イコライザの設定 ("#!#番号#!#")。文字列だが、libgpod 以外の書き方に備えて生のまま持つ (eq_preset で読み書き)
pub const MHOD_EQ: u32 = 7;
pub const MHOD_COMMENT: u32 = 8;
pub const MHOD_GROUPING: u32 = 13;
pub const MHOD_COMPOSER: u32 = 12;
pub const MHOD_ALBUM_ARTIST: u32 = 22;
pub const MHOD_SORT_ARTIST: u32 = 23;
pub const MHOD_SORT_ALBUM_ARTIST: u32 = 29;
pub const MHOD_SORT_TITLE: u32 = 27;
pub const MHOD_SORT_ALBUM: u32 = 28;
pub const MHOD_SORT_COMPOSER: u32 = 30;
const MHOD_SPLPREF: u32 = 50;
const MHOD_SPLRULES: u32 = 51;
const MHOD_LIBINDEX: u32 = 52;
const MHOD_LIBJUMP: u32 = 53;
const MHOD_PLAYLIST: u32 = 100;

// ---- mediatype (ビット。libgpod の ITDB_MEDIATYPE_*) ----
const MEDIA_AUDIO: u32 = 1;
const MEDIA_PODCAST: u32 = 1 << 2;
const MEDIA_AUDIOBOOK: u32 = 1 << 3;
const MEDIA_MOVIE: u32 = 1 << 1;
const MEDIA_MUSICVIDEO: u32 = 1 << 5;
const MEDIA_TVSHOW: u32 = 1 << 6;
/// 動画系 (映画・ミュージックビデオ・テレビ番組)。種類を変えない
const MEDIA_VIDEO_BITS: u32 = MEDIA_MOVIE | MEDIA_MUSICVIDEO | MEDIA_TVSHOW;
// 動画の文字列の MHOD (libgpod の MHOD_ID_*)
pub const MHOD_DESCRIPTION: u32 = 14;
pub const MHOD_TV_SHOW: u32 = 19;
pub const MHOD_TV_EPISODE: u32 = 20;
pub const MHOD_TV_NETWORK: u32 = 21;
pub const MHOD_SORT_TV_SHOW: u32 = 31;

/// UTF-16 文字列 MHOD として扱う種別 (15/16 は UTF-8 生文字列、17 はバイナリなので除外)
fn is_string_mhod(t: u32) -> bool {
    matches!(t, 1..=14 | 18..=31 | 200..=202 | 300) && !matches!(t, 7 | 10 | 11)
}

/// mhit 内の文字列 MHOD の書き出し順 (libgpod と同じ)
const TRACK_STRING_ORDER: &[u32] = &[
    1, 4, 3, 6, 8, 2, 5, 9, 12, 13, 14, 18, 19, 20, 21, 22, 24, 23, 27, 28, 29, 30, 31,
];

// ---- バイト操作 ----
pub(crate) fn rd16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2)
        .map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}
pub(crate) fn rd32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
pub(crate) fn rd64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8)
        .map_or(0, |s| u64::from_le_bytes(s.try_into().unwrap()))
}
pub(crate) fn wr8(b: &mut [u8], o: usize, v: u8) {
    b[o] = v;
}
pub(crate) fn wr16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn wr32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn wr64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

pub fn unix_to_mac(t: i64) -> u32 {
    if t <= 0 {
        0
    } else {
        (t + MAC_EPOCH_OFFSET) as u32
    }
}

pub fn mac_to_unix(t: u32) -> i64 {
    if t == 0 {
        0
    } else {
        t as i64 - MAC_EPOCH_OFFSET
    }
}

// ============================================================
// Track
// ============================================================

#[derive(Debug, Clone)]
pub struct Track {
    /// mhit ヘッダ (常に MHIT_HLEN 以上)
    pub(crate) hdr: Vec<u8>,
    pub(crate) strings: BTreeMap<u32, String>,
    /// 解釈しない MHOD (ポッドキャスト URL・チャプタ等) をそのまま保持
    pub(crate) raw_mhods: Vec<Vec<u8>>,
}

macro_rules! field32 {
    ($get:ident, $set:ident, $off:expr) => {
        pub fn $get(&self) -> u32 {
            rd32(&self.hdr, $off)
        }
        pub fn $set(&mut self, v: u32) {
            wr32(&mut self.hdr, $off, v)
        }
    };
}

impl Track {
    /// libgpod の mk_mhit 相当の既定値で新規トラックを作る
    pub fn new(dbid: u64) -> Self {
        let mut hdr = vec![0u8; MHIT_HLEN];
        hdr[0..4].copy_from_slice(b"mhit");
        wr32(&mut hdr, 0x04, MHIT_HLEN as u32);
        wr32(&mut hdr, 0x14, 1); // visible
        wr64(&mut hdr, 0x70, dbid);
        wr8(&mut hdr, 0x78, 0); // checked (0 = チェックあり)
        wr8(&mut hdr, 0xA4, 2); // has_artwork: 2 = なし
        wr8(&mut hdr, 0xB2, 1); // mark_unplayed: 1 = 未再生マークなし
        wr64(&mut hdr, 0xA8, dbid); // dbid2
        wr32(&mut hdr, 0xD0, 1); // mediatype: audio
        wr64(&mut hdr, 0x134, 0x8080_8080_8080);
        wr32(&mut hdr, 0x168, 1);
        Track {
            hdr,
            strings: BTreeMap::new(),
            raw_mhods: Vec::new(),
        }
    }

    pub fn dbid(&self) -> u64 {
        rd64(&self.hdr, 0x70)
    }
    field32!(id, set_id, 0x10);
    field32!(filetype_marker, set_filetype_marker, 0x18);
    field32!(time_modified_mac, set_time_modified_mac, 0x20);
    field32!(size, set_size, 0x24);
    field32!(length_ms, set_length_ms, 0x28);
    field32!(track_nr, set_track_nr, 0x2C);
    field32!(track_total, set_track_total, 0x30);
    field32!(year, set_year, 0x34);
    field32!(bitrate, set_bitrate, 0x38);
    field32!(playcount, set_playcount, 0x50);
    field32!(last_played_mac, set_last_played_mac, 0x58);
    field32!(cd_nr, set_cd_nr, 0x5C);
    field32!(cd_total, set_cd_total, 0x60);
    field32!(time_added_mac, set_time_added_mac, 0x68);
    field32!(bookmark_ms, set_bookmark_ms, 0x6C);
    // 公開日 (libgpod の time_released。iTunes はポッドキャストに入れる)
    field32!(time_released_mac, set_time_released_mac, 0x8C);
    field32!(skipcount, set_skipcount, 0x9C);
    field32!(last_skipped_mac, set_last_skipped_mac, 0xA0);
    field32!(mediatype, set_mediatype, 0xD0);

    pub fn rating(&self) -> u8 {
        self.hdr[0x1F]
    }
    pub fn set_rating(&mut self, v: u8) {
        self.hdr[0x1F] = v;
    }
    pub fn set_types(&mut self, type1: u8, type2: u8) {
        self.hdr[0x1C] = type1;
        self.hdr[0x1D] = type2;
    }
    pub fn compilation(&self) -> bool {
        self.hdr[0x1E] != 0
    }
    pub fn set_compilation(&mut self, v: bool) {
        self.hdr[0x1E] = v as u8;
    }
    pub fn samplerate(&self) -> u32 {
        rd32(&self.hdr, 0x3C) >> 16
    }
    pub fn set_samplerate(&mut self, hz: u32) {
        wr32(&mut self.hdr, 0x3C, hz.min(0xFFFF) << 16);
        self.hdr[0x88..0x8C].copy_from_slice(&(hz as f32).to_le_bytes());
    }
    pub fn set_unk126(&mut self, v: u16) {
        wr16(&mut self.hdr, 0x7E, v);
    }
    pub fn set_unk144(&mut self, v: u16) {
        wr16(&mut self.hdr, 0x90, v);
    }
    pub fn set_mark_unplayed(&mut self, v: u8) {
        self.hdr[0xB2] = v;
    }
    /// 曲のファイルに歌詞 (MP3 の USLT・M4A の ©lyr) があるか。iPod は印がある曲でしか歌詞を出さない
    pub fn lyrics_flag(&self) -> bool {
        self.hdr[0xB0] != 0
    }
    pub fn set_lyrics_flag(&mut self, v: bool) {
        self.hdr[0xB0] = v as u8;
    }
    /// 曲の種類 (mediatype のビットから)
    pub fn kind(&self) -> MediaKind {
        let m = self.mediatype();
        if m & MEDIA_AUDIOBOOK != 0 {
            MediaKind::Audiobook
        } else if m & MEDIA_PODCAST != 0 {
            MediaKind::Podcast
        } else {
            MediaKind::Music
        }
    }
    /// 動画 (iTunes で入れた映画など) か。種類は変えられない
    pub fn is_video(&self) -> bool {
        self.mediatype() & MEDIA_VIDEO_BITS != 0
    }
    /// 「ビデオ」に出す動画の種類。ビデオポッドキャストは曲 (ポッドキャスト) として扱うので None
    pub fn video_kind(&self) -> Option<VideoKind> {
        let m = self.mediatype();
        if m & MEDIA_VIDEO_BITS == 0 || m & MEDIA_PODCAST != 0 {
            None
        } else if m & MEDIA_TVSHOW != 0 {
            Some(VideoKind::TvShow)
        } else if m & MEDIA_MUSICVIDEO != 0 {
            Some(VideoKind::MusicVideo)
        } else {
            Some(VideoKind::Movie)
        }
    }
    /// 動画にする・動画の種類を変える。ムービーとテレビ番組は「シャッフルで飛ばす」「再生位置を覚える」を付ける
    /// (iTunes と同じ)。変わったら true
    pub fn set_video_kind(&mut self, k: VideoKind) -> bool {
        if self.video_kind() == Some(k) && self.hdr[0xB1] == 1 {
            return false;
        }
        self.set_mediatype(match k {
            VideoKind::Movie => MEDIA_MOVIE,
            VideoKind::MusicVideo => MEDIA_MUSICVIDEO,
            VideoKind::TvShow => MEDIA_TVSHOW,
        });
        let long = (k != VideoKind::MusicVideo) as u8;
        self.hdr[0xA5] = long;
        self.hdr[0xA6] = long;
        self.hdr[0xB1] = 1; // movie_flag
        true
    }
    /// シャッフルで飛ばす (iTunes の「シャッフル時にスキップ」)
    pub fn skip_shuffle(&self) -> bool {
        self.hdr[0xA5] != 0
    }
    pub fn set_skip_shuffle(&mut self, v: bool) {
        self.hdr[0xA5] = v as u8;
    }
    /// 再生した位置を覚える (iTunes の「再生位置を記憶」)
    pub fn remember_position(&self) -> bool {
        self.hdr[0xA6] != 0
    }
    pub fn set_remember_position(&mut self, v: bool) {
        self.hdr[0xA6] = v as u8;
    }
    /// 音量の調整 (-255〜255。iTunes の -100%〜+100%)
    pub fn volume(&self) -> i32 {
        rd32(&self.hdr, 0x40) as i32
    }
    pub fn set_volume(&mut self, v: i32) {
        wr32(&mut self.hdr, 0x40, v.clamp(-255, 255) as u32)
    }
    // 再生を始める・止める位置 (ミリ秒。止める位置の 0 は最後まで)
    field32!(start_ms, set_start_ms, 0x44);
    field32!(stop_ms, set_stop_ms, 0x48);
    field32!(playcount2, set_playcount2, 0x54);
    /// イコライザのプリセットの番号 (iTunes の 100〜)。None = なし
    pub fn eq_preset(&self) -> Option<u32> {
        let s = self.raw_mhods.iter().filter(|m| rd32(m, 0x0C) == MHOD_EQ).find_map(|m| decode_string_mhod(m))?;
        s.strip_prefix("#!#")?.strip_suffix("#!#")?.parse().ok()
    }
    /// 変わったら true
    pub fn set_eq_preset(&mut self, v: Option<u32>) -> bool {
        if self.eq_preset() == v {
            return false;
        }
        self.raw_mhods.retain(|m| rd32(m, 0x0C) != MHOD_EQ);
        if let Some(n) = v {
            self.raw_mhods.push(string_mhod(MHOD_EQ, &format!("#!#{n}#!#")));
        }
        true
    }
    field32!(season, set_season, 0xD4);
    field32!(episode, set_episode, 0xD8);
    /// 種類を変える。ポッドキャストとオーディオブックは「シャッフルで飛ばす」「再生位置を覚える」を付ける
    /// (libgpod の itdb.h の説明どおり)。動画は変えない。変わったら true
    pub fn set_kind(&mut self, k: MediaKind) -> bool {
        if self.is_video() || self.kind() == k {
            return false;
        }
        let (media, skip_shuffle, remember, flag4) = match k {
            MediaKind::Music => (MEDIA_AUDIO, 0, 0, 0),
            // flag4 = 1: 再生中の画面に曲名とアルバム (番組名) を出す
            MediaKind::Podcast => (MEDIA_PODCAST, 1, 1, 1),
            MediaKind::Audiobook => (MEDIA_AUDIOBOOK, 1, 1, 0),
        };
        self.set_mediatype(media);
        self.hdr[0xA5] = skip_shuffle;
        self.hdr[0xA6] = remember;
        self.hdr[0xA7] = flag4;
        // ポッドキャストはまだ聞いていない回に印 (2) を付ける。ほかは 1
        self.hdr[0xB2] = if k == MediaKind::Podcast && self.playcount() == 0 { 2 } else { 1 };
        true
    }
    pub fn set_samplecount(&mut self, v: u64) {
        wr64(&mut self.hdr, 0xBC, v);
    }
    pub fn samplecount(&self) -> u64 {
        rd64(&self.hdr, 0xBC)
    }
    // ギャップレス再生の情報 (iTunes が書く。shuffle 3G / 4G の iTunesSD に写す)
    field32!(pregap, set_pregap, 0xB8);
    field32!(postgap, set_postgap, 0xC8);
    field32!(gapless_data, set_gapless_data, 0xF8);
    /// アルバム・アーティストの番号 (`ITunesDb::serialize` で振る)
    pub fn album_id(&self) -> u32 {
        rd32(&self.hdr, 0x120)
    }
    pub fn artist_id(&self) -> u32 {
        rd32(&self.hdr, 0x1E0)
    }
    /// ArtworkDB の mhii id (0 = なし)
    pub fn mhii_link(&self) -> u32 {
        rd32(&self.hdr, 0x160)
    }
    pub fn has_artwork(&self) -> bool {
        self.hdr[0xA4] == 1 && rd16(&self.hdr, 0x7C) > 0
    }
    pub fn set_artwork(&mut self, image_id: u32, orig_size: u32) {
        wr16(&mut self.hdr, 0x7C, 1);
        wr32(&mut self.hdr, 0x80, orig_size);
        self.hdr[0xA4] = 1;
        wr32(&mut self.hdr, 0x160, image_id);
    }
    pub fn clear_artwork(&mut self) {
        wr16(&mut self.hdr, 0x7C, 0);
        wr32(&mut self.hdr, 0x80, 0);
        self.hdr[0xA4] = 2;
        wr32(&mut self.hdr, 0x160, 0);
    }

    pub fn string(&self, t: u32) -> Option<&str> {
        self.strings
            .get(&t)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }
    pub fn set_string(&mut self, t: u32, v: Option<&str>) {
        match v.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => {
                self.strings.insert(t, s.to_string());
            }
            None => {
                self.strings.remove(&t);
            }
        }
    }
    pub fn title(&self) -> Option<&str> {
        self.string(MHOD_TITLE)
    }
    pub fn artist(&self) -> Option<&str> {
        self.string(MHOD_ARTIST)
    }
    pub fn album(&self) -> Option<&str> {
        self.string(MHOD_ALBUM)
    }
    pub fn album_artist(&self) -> Option<&str> {
        self.string(MHOD_ALBUM_ARTIST)
    }
    pub fn genre(&self) -> Option<&str> {
        self.string(MHOD_GENRE)
    }
    pub fn composer(&self) -> Option<&str> {
        self.string(MHOD_COMPOSER)
    }
    /// ":iPod_Control:Music:F00:ABCD.mp3" 形式
    pub fn ipod_path(&self) -> Option<&str> {
        self.string(MHOD_PATH)
    }
}

// ============================================================
// Playlist
// ============================================================

#[derive(Debug, Clone)]
pub struct Playlist {
    pub(crate) hdr: Vec<u8>,
    pub name: String,
    /// メンバーのトラック dbid
    pub items: Vec<u64>,
    /// スマートプレイリストの MHOD 50/51 (生バイト)
    pub(crate) spl_mhods: Vec<Vec<u8>>,
}

impl Playlist {
    pub fn new(name: &str, id: u64, master: bool) -> Self {
        let mut hdr = vec![0u8; MHYP_HLEN];
        hdr[0..4].copy_from_slice(b"mhyp");
        wr32(&mut hdr, 0x04, MHYP_HLEN as u32);
        hdr[0x14] = master as u8;
        wr32(&mut hdr, 0x18, unix_to_mac(now_unix()));
        wr64(&mut hdr, 0x1C, id);
        wr16(&mut hdr, 0x28, 1);
        Playlist {
            hdr,
            name: name.to_string(),
            items: Vec::new(),
            spl_mhods: Vec::new(),
        }
    }
    pub fn id(&self) -> u64 {
        rd64(&self.hdr, 0x1C)
    }
    pub fn is_master(&self) -> bool {
        self.hdr[0x14] == 1
    }
    pub fn is_podcast(&self) -> bool {
        rd16(&self.hdr, 0x2A) == 1
    }
    pub fn is_smart(&self) -> bool {
        !self.spl_mhods.is_empty()
    }
    fn spl_mhod(&self, kind: u32) -> Option<&[u8]> {
        self.spl_mhods
            .iter()
            .find(|m| rd32(m, 0x0C) == kind)
            .map(|m| m.as_slice())
    }
    /// スマートプレイリストの設定と条件。条件の MHOD が無ければ空の条件
    pub fn smart(&self) -> Option<(SmartPrefs, Rules)> {
        let prefs = SmartPrefs::from_mhod(self.spl_mhod(MHOD_SPLPREF)?).ok()?;
        let rules = match self.spl_mhod(MHOD_SPLRULES) {
            Some(m) => Rules::from_mhod(m).ok()?,
            None => Rules {
                any: false,
                items: Vec::new(),
            },
        };
        Some((prefs, rules))
    }
    /// スマートプレイリストにする・条件を変える (MHOD 50 の知らない場所は今の値を残す)
    pub fn set_smart(&mut self, prefs: &SmartPrefs, rules: &Rules) {
        let base = self.spl_mhod(MHOD_SPLPREF).map(|m| m.to_vec());
        self.spl_mhods = vec![prefs.to_mhod(base.as_deref()), rules.to_mhod()];
    }
}

impl SplTrack for Track {
    fn spl_id(&self) -> u64 {
        self.dbid()
    }
    fn spl_text(&self, field: u32) -> Option<&str> {
        let mhod = match field {
            F_NAME => MHOD_TITLE,
            F_ALBUM => MHOD_ALBUM,
            F_ARTIST => MHOD_ARTIST,
            F_GENRE => MHOD_GENRE,
            F_KIND => MHOD_FILETYPE,
            F_COMMENT => MHOD_COMMENT,
            F_COMPOSER => MHOD_COMPOSER,
            F_CATEGORY => 9,
            F_GROUPING => 13,
            F_DESCRIPTION => 14,
            F_TV_SHOW => 19,
            F_ALBUM_ARTIST => MHOD_ALBUM_ARTIST,
            F_SORT_NAME => MHOD_SORT_TITLE,
            F_SORT_ALBUM => MHOD_SORT_ALBUM,
            F_SORT_ARTIST => MHOD_SORT_ARTIST,
            F_SORT_ALBUM_ARTIST => 29,
            F_SORT_COMPOSER => MHOD_SORT_COMPOSER,
            F_SORT_TV_SHOW => 31,
            _ => return None,
        };
        self.string(mhod)
    }
    fn spl_num(&self, field: u32) -> Option<i64> {
        let v = match field {
            F_BITRATE => self.bitrate() as i64,
            F_SAMPLE_RATE => self.samplerate() as i64,
            F_YEAR => self.year() as i64,
            F_DATE_MODIFIED => self.time_modified_mac() as i64,
            F_TRACK_NUMBER => self.track_nr() as i64,
            F_SIZE => self.size() as i64,
            F_TIME => self.length_ms() as i64,
            F_DATE_ADDED => self.time_added_mac() as i64,
            F_PLAY_COUNT => self.playcount() as i64,
            F_LAST_PLAYED => self.last_played_mac() as i64,
            F_DISC_NUMBER => self.cd_nr() as i64,
            F_RATING => self.rating() as i64,
            F_CHECKED => self.spl_checked() as i64,
            F_COMPILATION => self.compilation() as i64,
            F_BPM => rd16(&self.hdr, 0x7A) as i64,
            F_ARTWORK => self.has_artwork() as i64,
            // 古い DB の 0 は音楽として扱う (libgpod と同じ)
            F_MEDIA_KIND => self.mediatype().max(1) as i64,
            F_SEASON => rd32(&self.hdr, 0xD4) as i64,
            F_SKIP_COUNT => self.skipcount() as i64,
            F_LAST_SKIPPED => self.last_skipped_mac() as i64,
            // iPod の DB に無い項目。無し・0 として扱う
            F_PURCHASED | F_ALBUM_RATING | F_LOVE | F_ALBUM_LOVE | F_MOVEMENT_NUMBER => 0,
            _ => return None,
        };
        Some(v)
    }
    fn spl_checked(&self) -> bool {
        self.hdr[0x78] == 0
    }
    fn spl_length_ms(&self) -> u64 {
        self.length_ms() as u64
    }
    fn spl_size(&self) -> u64 {
        self.size() as u64
    }
}

// ============================================================
// Database
// ============================================================

#[derive(Debug, Clone)]
pub struct ITunesDb {
    pub(crate) header: Vec<u8>,
    pub tracks: Vec<Track>,
    /// playlists[0] は常にマスタープレイリスト
    pub playlists: Vec<Playlist>,
    /// そのまま書き戻す mhsd (type 5 / 9)
    preserved_mhsd: Vec<(u32, Vec<u8>)>,
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn random_id() -> u64 {
    loop {
        let v: u64 = rand::random();
        if v != 0 {
            return v;
        }
    }
}

impl ITunesDb {
    /// 空の DB を作る (iTunesDB が存在しない初期化直後の iPod 用)
    pub fn new_empty(ipod_name: &str) -> Self {
        let mut h = vec![0u8; MHBD_HLEN];
        h[0..4].copy_from_slice(b"mhbd");
        wr32(&mut h, 0x04, MHBD_HLEN as u32);
        wr32(&mut h, 0x0C, 1);
        wr32(&mut h, 0x10, 0x30);
        wr64(&mut h, 0x18, random_id());
        wr16(&mut h, 0x20, 2); // platform: Windows (FAT32 前提)
        wr16(&mut h, 0x22, 0x0263); // iTunes 9 の既定値
        wr64(&mut h, 0x24, random_id());
        h[0x46] = b'e';
        h[0x47] = b'n';
        wr64(&mut h, 0x48, random_id());
        ITunesDb {
            header: h,
            tracks: Vec::new(),
            playlists: vec![Playlist::new(ipod_name, random_id(), true)],
            preserved_mhsd: Vec::new(),
        }
    }

    /// hashing_scheme (0: なし, 1: hash58, 2: hash72, 4: hashAB)
    pub fn hashing_scheme(&self) -> u16 {
        rd16(&self.header, 0x30)
    }
    fn id_0x24(&self) -> u64 {
        rd64(&self.header, 0x24)
    }

    pub fn master(&self) -> &Playlist {
        &self.playlists[0]
    }
    pub fn master_mut(&mut self) -> &mut Playlist {
        &mut self.playlists[0]
    }

    pub fn track_by_dbid(&self, dbid: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.dbid() == dbid)
    }

    /// プレイリストの id → 入っている曲 (スマートプレイリストの「プレイリストが…」の条件用)
    pub fn playlist_members(&self) -> HashMap<u64, HashSet<u64>> {
        self.playlists
            .iter()
            .map(|p| (p.id(), p.items.iter().copied().collect()))
            .collect()
    }

    /// 条件と上限で入る曲を決める (`previous` は今入っている曲)
    pub fn evaluate_smart(&self, prefs: &SmartPrefs, rules: &Rules, previous: &[u64], now_mac: i64) -> Vec<u64> {
        let members = self.playlist_members();
        let cx = EvalContext {
            now_mac,
            playlists: &members,
        };
        evaluate(rules, prefs, &self.tracks, &cx, previous)
    }

    /// ライブアップデートのスマートプレイリストの中身を条件から作り直す。変わったら true。
    /// iPod の DB に値が無い項目 (購入した項目・ラブなど) を使う条件は、作り直すと iTunes が入れた曲が
    /// 消えてしまうので触らない
    pub fn refresh_smart_playlists(&mut self, now_mac: i64) -> bool {
        let mut changed = false;
        // 「プレイリストが…」の条件はほかのプレイリストの中身で変わるので、変わらなくなるまで繰り返す
        // (iTunes は循環する条件を作らせない。念のため回数に上限)
        for _ in 0..4 {
            let members = self.playlist_members();
            let mut round = false;
            for i in 1..self.playlists.len() {
                let Some((prefs, rules)) = self.playlists[i].smart() else {
                    continue;
                };
                if !prefs.live_update || rules.uses_any_field(FIELDS_NOT_ON_IPOD) {
                    continue;
                }
                let cx = EvalContext {
                    now_mac,
                    playlists: &members,
                };
                let items = evaluate(&rules, &prefs, &self.tracks, &cx, &self.playlists[i].items);
                if items != self.playlists[i].items {
                    self.playlists[i].items = items;
                    round = true;
                }
            }
            if !round {
                break;
            }
            changed = true;
        }
        changed
    }

    pub fn new_dbid(&self) -> u64 {
        loop {
            let id = random_id();
            if self.track_by_dbid(id).is_none() {
                return id;
            }
        }
    }

    // ---------------- parse ----------------

    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 0x20 || &data[0..4] != b"mhbd" {
            return Err(Error::Corrupt(t!("mhbd ヘッダがありません")));
        }
        if rd32(data, 0x0C) > 2 {
            // byte order 反転 DB (iPod mobile 等) は非対応
            return Err(Error::Corrupt(t!("未対応の iTunesDB 形式です")));
        }
        let hlen = rd32(data, 4) as usize;
        let nchild = rd32(data, 0x14);
        if hlen < 0x68 || hlen > data.len() {
            return Err(Error::Corrupt(t!("mhbd ヘッダ長が不正です")));
        }
        let mut header = data[..hlen].to_vec();
        if header.len() < MHBD_HLEN {
            header.resize(MHBD_HLEN, 0);
        }

        let mut tracks = Vec::new();
        let mut pl2 = None;
        let mut pl3 = None;
        let mut preserved = Vec::new();
        let mut pos = hlen;
        for _ in 0..nchild {
            let c = chunk(data, pos, b"mhsd")?;
            let mhsd_hlen = rd32(c, 4) as usize;
            let body = c.get(mhsd_hlen..).unwrap_or(&[]);
            match rd32(c, 0x0C) {
                1 => tracks = parse_tracks(body)?,
                2 => pl2 = Some(body),
                3 => pl3 = Some(body),
                t @ (5 | 9) => preserved.push((t, c.to_vec())),
                _ => {}
            }
            pos += c.len();
        }

        let id_map: HashMap<u32, u64> = tracks.iter().map(|t| (t.id(), t.dbid())).collect();
        let mut playlists = match pl2.or(pl3) {
            Some(b) => parse_playlists(b, &id_map)?,
            None => Vec::new(),
        };
        // マスターを先頭に
        match playlists.iter().position(|p| p.is_master()) {
            Some(0) => {}
            Some(i) => {
                let m = playlists.remove(i);
                playlists.insert(0, m);
            }
            None => playlists.insert(0, Playlist::new("iPod", random_id(), true)),
        }

        Ok(ITunesDb {
            header,
            tracks,
            playlists,
            preserved_mhsd: preserved,
        })
    }

    // ---------------- write ----------------

    /// 書き出し用バイト列を生成する。署名 (hash58) は呼び出し側で付与する。
    pub fn serialize(&mut self) -> Vec<u8> {
        self.prepare();

        let mut out = self.header.clone();
        wr32(&mut out, 0x04, self.header.len() as u32);
        if rd32(&out, 0x10) < 0x30 {
            wr32(&mut out, 0x10, 0x30);
        }
        // 署名関連は毎回クリア (必要なら後で hash58 を付与)
        wr16(&mut out, 0x30, 0);
        out[0x58..0x6C].fill(0);
        wr16(&mut out, 0x70, 0);
        out[0x72..0xA0].fill(0);
        if out.len() >= 0xAB + 57 {
            out[0xAB..0xAB + 57].fill(0);
        }

        let mut n = 0u32;
        let mut push = |out: &mut Vec<u8>, b: Vec<u8>| {
            out.extend_from_slice(&b);
            n += 1;
        };
        push(&mut out, mhsd(1, self.write_tracks()));
        // 3 番 (podcast 用) と 2 番のプレイリストは、ポッドキャストのプレイリストの書き方だけが違う
        // (3 番は番組ごとにまとめる)。全曲の並べ替えの索引は重いので 1 回だけ作る
        let (grouped, flat) = self.write_playlists();
        push(&mut out, mhsd(3, grouped));
        push(&mut out, mhsd(2, flat));
        let (mhla, mhli) = self.write_albums_artists();
        push(&mut out, mhsd(4, mhla));
        push(&mut out, mhsd(8, mhli));
        push(&mut out, mhsd(6, list_header(b"mhlt", 0)));
        push(&mut out, mhsd(10, list_header(b"mhlt", 0)));
        match self.preserved_mhsd.iter().find(|(t, _)| *t == 5) {
            Some((_, raw)) => push(&mut out, raw.clone()),
            None => push(&mut out, mhsd(5, list_header(b"mhlp", 0))),
        }
        if let Some((_, raw)) = self.preserved_mhsd.iter().find(|(t, _)| *t == 9) {
            push(&mut out, raw.clone());
        }
        let total = out.len() as u32;
        wr32(&mut out, 0x08, total);
        wr32(&mut out, 0x14, n);
        out
    }

    /// トラック ID 振り直し・マスタープレイリスト同期・アルバム/アーティスト ID 付与
    fn prepare(&mut self) {
        // マスターは全トラックをトラック順で保持
        let all: Vec<u64> = self.tracks.iter().map(|t| t.dbid()).collect();
        let valid: std::collections::HashSet<u64> = all.iter().copied().collect();
        self.playlists[0].items = all;
        for pl in self.playlists.iter_mut().skip(1) {
            pl.items.retain(|d| valid.contains(d));
        }
        self.sync_podcast_playlist();

        let id24 = self.id_0x24();
        let mut albums: HashMap<(String, String), u32> = HashMap::new();
        let mut artists: HashMap<String, u32> = HashMap::new();
        let mut composers: HashMap<String, u32> = HashMap::new();
        for (i, t) in self.tracks.iter_mut().enumerate() {
            t.set_id(FIRST_TRACK_ID + i as u32);
            let album_id = match t.album() {
                Some(a) => {
                    let key = (a.to_string(), album_key_artist(t));
                    let next = albums.len() as u32 + 1;
                    *albums.entry(key).or_insert(next)
                }
                None => 0,
            };
            let artist_id = match t.artist() {
                Some(a) => {
                    let next = artists.len() as u32 + 1;
                    *artists.entry(a.to_string()).or_insert(next)
                }
                None => 0,
            };
            let composer_id = match t.composer() {
                Some(c) => {
                    let next = composers.len() as u32 + 1;
                    *composers.entry(c.to_string()).or_insert(next)
                }
                None => 0,
            };
            wr32(&mut t.hdr, 0x120, album_id);
            wr64(&mut t.hdr, 0x124, id24);
            let size = t.size();
            wr32(&mut t.hdr, 0x12C, size);
            wr32(&mut t.hdr, 0x1E0, artist_id);
            wr32(&mut t.hdr, 0x1F4, composer_id);
        }
    }

    /// iPod の「ポッドキャスト」に出す専用のプレイリスト (podcastflag = 1) を、ポッドキャストの曲に合わせる。
    /// 今の順番は残し、足りない回を後ろに足す。ポッドキャストが無く、プレイリストも無ければ作らない
    fn sync_podcast_playlist(&mut self) {
        let pods: Vec<u64> = self.tracks.iter().filter(|t| t.kind() == MediaKind::Podcast).map(|t| t.dbid()).collect();
        let pos = self.playlists.iter().skip(1).position(|p| p.is_podcast()).map(|i| i + 1);
        let i = match pos {
            Some(i) => i,
            None if pods.is_empty() => return,
            None => {
                let mut pl = Playlist::new("Podcasts", random_id(), false);
                wr16(&mut pl.hdr, 0x2A, 1);
                self.playlists.insert(1, pl);
                1
            }
        };
        let set: std::collections::HashSet<u64> = pods.iter().copied().collect();
        let pl = &mut self.playlists[i];
        pl.items.retain(|d| set.contains(d));
        let have: std::collections::HashSet<u64> = pl.items.iter().copied().collect();
        pl.items.extend(pods.into_iter().filter(|d| !have.contains(d)));
    }

    fn write_tracks(&self) -> Vec<u8> {
        let mut out = list_header(b"mhlt", self.tracks.len() as u32);
        for t in &self.tracks {
            let start = out.len();
            let mut hdr = t.hdr.clone();
            let hl = hdr.len() as u32;
            wr32(&mut hdr, 0x04, hl);
            out.extend_from_slice(&hdr);
            let mut nmhod = 0u32;
            for ty in TRACK_STRING_ORDER {
                if let Some(s) = t.string(*ty) {
                    out.extend_from_slice(&string_mhod(*ty, s));
                    nmhod += 1;
                }
            }
            for raw in &t.raw_mhods {
                out.extend_from_slice(raw);
                nmhod += 1;
            }
            let total = (out.len() - start) as u32;
            wr32(&mut out, start + 0x08, total);
            wr32(&mut out, start + 0x0C, nmhod);
        }
        out
    }

    /// (3 番用, 2 番用)。違うのはポッドキャストのプレイリストだけ (3 番は番組ごとの見出しの下に回を並べる。libgpod の write_podcast_mhips と同じ)
    fn write_playlists(&self) -> (Vec<u8>, Vec<u8>) {
        let id_of: HashMap<u64, u32> = self.tracks.iter().map(|t| (t.dbid(), t.id())).collect();
        let by_dbid: HashMap<u64, &Track> = self.tracks.iter().map(|t| (t.dbid(), t)).collect();
        let mut grouped = list_header(b"mhlp", self.playlists.len() as u32);
        let mut flat = grouped.clone();
        for pl in &self.playlists {
            let mut hdr = pl.hdr.clone();
            let hl = hdr.len() as u32;
            wr32(&mut hdr, 0x04, hl);
            wr16(&mut hdr, 0x28, 1);
            let mut body = hdr;
            let mut nmhod = 2u32;
            body.extend_from_slice(&string_mhod(MHOD_TITLE, &pl.name));
            body.extend_from_slice(&long_playlist_mhod());
            if pl.is_master() && !pl.items.is_empty() {
                let idx = self.library_indices();
                for (sort_type, order, letters) in idx {
                    body.extend_from_slice(&mhod52(sort_type, &order));
                    body.extend_from_slice(&mhod53(sort_type, &letters));
                    nmhod += 2;
                }
            }
            for raw in &pl.spl_mhods {
                body.extend_from_slice(raw);
                nmhod += 1;
            }
            wr32(&mut body, 0x0C, nmhod);
            let head_len = body.len();

            // 並べたままの曲 (2 番。ポッドキャスト以外は 3 番も同じ)
            let mut items = Vec::new();
            let mut nitems = 0u32;
            for (pos, dbid) in pl.items.iter().enumerate() {
                let Some(tid) = id_of.get(dbid) else { continue };
                items.extend_from_slice(&mhip(*tid, pos as u32));
                nitems += 1;
            }
            let finish = |mut b: Vec<u8>, extra: &[u8], n: u32| {
                b.extend_from_slice(extra);
                let total = b.len() as u32;
                wr32(&mut b, 0x08, total);
                wr32(&mut b, 0x10, n);
                b
            };
            flat.extend_from_slice(&finish(body.clone(), &items, nitems));
            if pl.is_podcast() {
                let (g, n) = self.podcast_groups(&pl.items, &id_of, &by_dbid);
                grouped.extend_from_slice(&finish(body[..head_len].to_vec(), &g, n));
            } else {
                grouped.extend_from_slice(&finish(body, &items, nitems));
            }
        }
        (grouped, flat)
    }

    /// ポッドキャストの回を番組 (アルバム名) ごとにまとめた mhip の並び と その数 (見出しを含む)。
    /// 番組の順は最初に出てきた順。見出しと回の mhip の id は曲の id と重ならない番号にする
    fn podcast_groups(&self, items: &[u64], id_of: &HashMap<u64, u32>, by_dbid: &HashMap<u64, &Track>) -> (Vec<u8>, u32) {
        let mut groups: Vec<(String, Vec<u32>)> = Vec::new();
        for d in items {
            let (Some(tid), Some(t)) = (id_of.get(d), by_dbid.get(d)) else { continue };
            let album = t.album().unwrap_or("").to_string();
            match groups.iter_mut().find(|(a, _)| *a == album) {
                Some((_, v)) => v.push(*tid),
                None => groups.push((album, vec![*tid])),
            }
        }
        let mut next_id = FIRST_TRACK_ID + self.tracks.len() as u32 + 1;
        let mut out = Vec::new();
        let mut n = 0u32;
        for (album, tids) in groups {
            let group_id = next_id;
            next_id += 1;
            out.extend_from_slice(&podcast_group_mhip(group_id, &album));
            n += 1;
            for tid in tids {
                let id = next_id;
                next_id += 1;
                let mut m = mhip(tid, id);
                wr32(&mut m, 0x14, id);
                wr32(&mut m, 0x20, group_id);
                out.extend_from_slice(&m);
                n += 1;
            }
        }
        (out, n)
    }

    /// マスタープレイリスト用の MHOD52 (ソート済みインデックス) と MHOD53 (ジャンプテーブル)
    fn library_indices(&self) -> Vec<LibraryIndex> {
        struct Key {
            title: String,
            artist: String,
            album: String,
            genre: String,
            composer: String,
            cd: u32,
            nr: u32,
        }
        let pick = |t: &Track, sort: u32, main: u32| -> String {
            t.string(sort).or(t.string(main)).unwrap_or("").to_string()
        };
        let keys: Vec<Key> = self
            .tracks
            .iter()
            .map(|t| Key {
                title: pick(t, MHOD_SORT_TITLE, MHOD_TITLE),
                artist: pick(t, MHOD_SORT_ARTIST, MHOD_ARTIST),
                album: pick(t, MHOD_SORT_ALBUM, MHOD_ALBUM),
                genre: t.genre().unwrap_or("").to_string(),
                composer: pick(t, MHOD_SORT_COMPOSER, MHOD_COMPOSER),
                cd: t.cd_nr(),
                nr: t.track_nr(),
            })
            .collect();
        let lc: Vec<[String; 5]> = keys
            .iter()
            .map(|k| {
                [
                    collate(&k.title),
                    collate(&k.artist),
                    collate(&k.album),
                    collate(&k.genre),
                    collate(&k.composer),
                ]
            })
            .collect();

        // (sort_type, 比較フィールド順)  0:title 1:artist 2:album 3:genre 4:composer
        let specs: [(u32, &[usize], usize); 5] = [
            (0x03, &[0], 0),
            (0x05, &[1, 2], 1),
            (0x04, &[2], 2),
            (0x07, &[3, 1, 2], 3),
            (0x12, &[4, 2], 4),
        ];
        let mut res = Vec::new();
        for (sort_type, fields, letter_field) in specs {
            let mut order: Vec<usize> = (0..keys.len()).collect();
            order.sort_by(|&a, &b| {
                for &f in fields {
                    let c = lc[a][f].cmp(&lc[b][f]);
                    if c != std::cmp::Ordering::Equal {
                        return c;
                    }
                }
                if sort_type != 0x03 {
                    let c = keys[a]
                        .cd
                        .cmp(&keys[b].cd)
                        .then(keys[a].nr.cmp(&keys[b].nr));
                    if c != std::cmp::Ordering::Equal {
                        return c;
                    }
                }
                lc[a][0].cmp(&lc[b][0])
            });
            let mut letters: Vec<(u16, u32, u32)> = Vec::new();
            for (i, &ti) in order.iter().enumerate() {
                let src = match letter_field {
                    0 => &keys[ti].title,
                    1 => &keys[ti].artist,
                    2 => &keys[ti].album,
                    3 => &keys[ti].genre,
                    _ => &keys[ti].composer,
                };
                let l = jump_letter(src);
                match letters.last_mut() {
                    Some(last) if last.0 == l => last.2 += 1,
                    _ => letters.push((l, i as u32, 1)),
                }
            }
            res.push((
                sort_type,
                order.iter().map(|&i| i as u32).collect(),
                letters,
            ));
        }
        res
    }

    fn write_albums_artists(&self) -> (Vec<u8>, Vec<u8>) {
        // prepare() 済みの album_id / artist_id を使う
        let mut albums: BTreeMap<u32, &Track> = BTreeMap::new();
        let mut artists: BTreeMap<u32, &Track> = BTreeMap::new();
        for t in &self.tracks {
            let aid = rd32(&t.hdr, 0x120);
            if aid != 0 {
                albums.entry(aid).or_insert(t);
            }
            let arid = rd32(&t.hdr, 0x1E0);
            if arid != 0 {
                artists.entry(arid).or_insert(t);
            }
        }

        let mut mhla = list_header(b"mhla", albums.len() as u32);
        for (id, t) in &albums {
            let start = mhla.len();
            let mut h = vec![0u8; 88];
            h[0..4].copy_from_slice(b"mhia");
            wr32(&mut h, 4, 88);
            wr32(&mut h, 0x10, *id);
            wr64(&mut h, 0x14, random_id());
            wr32(&mut h, 0x1C, 2);
            mhla.extend_from_slice(&h);
            let mut n = 0;
            if let Some(a) = t.album() {
                mhla.extend_from_slice(&string_mhod(200, a));
                n += 1;
            }
            if let Some(a) = t.album_artist().or(t.artist()) {
                mhla.extend_from_slice(&string_mhod(201, a));
                n += 1;
            }
            if let Some(a) = t.string(29).or(t.string(MHOD_SORT_ARTIST)) {
                mhla.extend_from_slice(&string_mhod(202, a));
                n += 1;
            }
            let total = (mhla.len() - start) as u32;
            wr32(&mut mhla, start + 8, total);
            wr32(&mut mhla, start + 12, n);
        }

        let mut mhli = list_header(b"mhli", artists.len() as u32);
        for (id, t) in &artists {
            let start = mhli.len();
            let mut h = vec![0u8; 80];
            h[0..4].copy_from_slice(b"mhii");
            wr32(&mut h, 4, 80);
            wr32(&mut h, 0x10, *id);
            wr64(&mut h, 0x14, random_id());
            wr32(&mut h, 0x1C, 2);
            mhli.extend_from_slice(&h);
            let mut n = 0;
            if let Some(a) = t.artist() {
                mhli.extend_from_slice(&string_mhod(300, a));
                n += 1;
            }
            let total = (mhli.len() - start) as u32;
            wr32(&mut mhli, start + 8, total);
            wr32(&mut mhli, start + 12, n);
        }
        (mhla, mhli)
    }
}

/// (ソート種別, ソート済みトラック位置, ジャンプテーブル[(頭文字, 開始, 件数)])
type LibraryIndex = (u32, Vec<u32>, Vec<(u16, u32, u32)>);

fn album_key_artist(t: &Track) -> String {
    t.album_artist().or(t.artist()).unwrap_or("").to_string()
}

/// 大文字小文字を無視した比較キー
fn collate(s: &str) -> String {
    s.to_lowercase()
}

/// ジャンプテーブル用の頭文字 (英字は大文字、数字・記号のみは '0')
fn jump_letter(s: &str) -> u16 {
    for c in s.chars() {
        if c.is_alphanumeric() {
            if c.is_numeric() {
                return b'0' as u16;
            }
            let up = crate::domain::yomi::base_kana(c.to_uppercase().next().unwrap_or(c));
            let mut buf = [0u16; 2];
            return up.encode_utf16(&mut buf)[0];
        }
    }
    b'0' as u16
}

// ---------------- chunk helpers ----------------

fn chunk<'a>(data: &'a [u8], pos: usize, tag: &[u8; 4]) -> Result<&'a [u8]> {
    let c = data
        .get(pos..pos + 12)
        .ok_or_else(|| Error::Corrupt(t!("{tag} が途中で切れています", tag = tag_str(tag))))?;
    if &c[0..4] != tag {
        return Err(Error::Corrupt(t!(
            "{tag} を期待しましたが {got} でした (offset {pos})",
            tag = tag_str(tag),
            got = format!("{:?}", String::from_utf8_lossy(&c[0..4])),
            pos = pos
        )));
    }
    let total = rd32(data, pos + 8) as usize;
    if total < 12 || pos + total > data.len() {
        return Err(Error::Corrupt(t!("{tag} の長さが不正です", tag = tag_str(tag))));
    }
    Ok(&data[pos..pos + total])
}

fn tag_str(t: &[u8; 4]) -> String {
    String::from_utf8_lossy(t).into_owned()
}

fn parse_tracks(body: &[u8]) -> Result<Vec<Track>> {
    if body.len() < 12 || &body[0..4] != b"mhlt" {
        return Err(Error::Corrupt(t!("mhlt がありません")));
    }
    let n = rd32(body, 8);
    let mut pos = rd32(body, 4) as usize;
    // 数は壊れていることがあるので、入りきる数 (mhit は 12 バイト以上) までしか先に確保しない
    let mut tracks = Vec::with_capacity((n as usize).min(body.len() / 12));
    for _ in 0..n {
        let c = chunk(body, pos, b"mhit")?;
        let hlen = rd32(c, 4) as usize;
        let nmhod = rd32(c, 0x0C);
        let mut hdr = c.get(..hlen).unwrap_or(c).to_vec();
        if hdr.len() < MHIT_HLEN {
            hdr.resize(MHIT_HLEN, 0);
        }
        let mut t = Track {
            hdr,
            strings: BTreeMap::new(),
            raw_mhods: Vec::new(),
        };
        let mut mp = hlen;
        for _ in 0..nmhod {
            let m = chunk(c, mp, b"mhod")?;
            let ty = rd32(m, 0x0C);
            if is_string_mhod(ty) {
                if let Some(s) = decode_string_mhod(m) {
                    t.strings.insert(ty, s);
                }
            } else {
                t.raw_mhods.push(m.to_vec());
            }
            mp += m.len();
        }
        pos += c.len();
        tracks.push(t);
    }
    Ok(tracks)
}

fn parse_playlists(body: &[u8], id_map: &HashMap<u32, u64>) -> Result<Vec<Playlist>> {
    if body.len() < 12 || &body[0..4] != b"mhlp" {
        return Err(Error::Corrupt(t!("mhlp がありません")));
    }
    let n = rd32(body, 8);
    let mut pos = rd32(body, 4) as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let c = chunk(body, pos, b"mhyp")?;
        let hlen = rd32(c, 4) as usize;
        let mut hdr = c.get(..hlen).unwrap_or(c).to_vec();
        if hdr.len() < MHYP_HLEN {
            hdr.resize(MHYP_HLEN, 0);
        }
        let mut pl = Playlist {
            hdr,
            name: String::new(),
            items: Vec::new(),
            spl_mhods: Vec::new(),
        };
        let mut cp = hlen;
        while cp + 12 <= c.len() {
            let tag = &c[cp..cp + 4];
            let len = rd32(c, cp + 8) as usize;
            if len < 12 || cp + len > c.len() {
                break;
            }
            let sub = &c[cp..cp + len];
            if tag == b"mhod" {
                match rd32(sub, 0x0C) {
                    MHOD_TITLE => pl.name = decode_string_mhod(sub).unwrap_or_default(),
                    MHOD_SPLPREF | MHOD_SPLRULES => pl.spl_mhods.push(sub.to_vec()),
                    _ => {}
                }
            } else if tag == b"mhip" {
                // podcast グループヘッダ (flag 0x100) は無視
                if rd32(sub, 0x10) != 0x100
                    && let Some(d) = id_map.get(&rd32(sub, 0x18))
                {
                    pl.items.push(*d);
                }
            }
            cp += len;
        }
        pos += c.len();
        out.push(pl);
    }
    Ok(out)
}

fn decode_string_mhod(m: &[u8]) -> Option<String> {
    let hlen = rd32(m, 4) as usize;
    let enc = rd32(m, hlen);
    let len = rd32(m, hlen + 4) as usize;
    let s = m.get(hlen + 16..hlen + 16 + len)?;
    if enc == 2 {
        Some(String::from_utf8_lossy(s).into_owned())
    } else {
        let u: Vec<u16> = s
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Some(String::from_utf16_lossy(&u))
    }
}

// ---------------- writers ----------------

fn list_header(tag: &[u8; 4], n: u32) -> Vec<u8> {
    let mut h = vec![0u8; 92];
    h[0..4].copy_from_slice(tag);
    wr32(&mut h, 4, 92);
    wr32(&mut h, 8, n);
    h
}

fn mhsd(ty: u32, body: Vec<u8>) -> Vec<u8> {
    let mut h = vec![0u8; 96];
    h[0..4].copy_from_slice(b"mhsd");
    wr32(&mut h, 4, 96);
    wr32(&mut h, 8, (96 + body.len()) as u32);
    wr32(&mut h, 12, ty);
    h.extend_from_slice(&body);
    h
}

pub(crate) fn string_mhod(ty: u32, s: &str) -> Vec<u8> {
    let u: Vec<u16> = s.encode_utf16().collect();
    let blen = u.len() * 2;
    let mut m = vec![0u8; 40];
    m[0..4].copy_from_slice(b"mhod");
    wr32(&mut m, 4, 24);
    wr32(&mut m, 8, (40 + blen) as u32);
    wr32(&mut m, 12, ty);
    wr32(&mut m, 24, 1); // position / encoding (UTF-16)
    wr32(&mut m, 28, blen as u32);
    wr32(&mut m, 32, 1);
    for c in u {
        m.extend_from_slice(&c.to_le_bytes());
    }
    m
}

/// libgpod の mk_long_mhod_id_playlist (iTunes 表示設定用の固定 MHOD100)
fn long_playlist_mhod() -> Vec<u8> {
    let mut m = Vec::with_capacity(0x288);
    let p = |m: &mut Vec<u8>, v: u32| m.extend_from_slice(&v.to_le_bytes());
    let z = |m: &mut Vec<u8>, n: usize| m.extend(std::iter::repeat_n(0u8, n * 4));
    m.extend_from_slice(b"mhod");
    p(&mut m, 0x18);
    p(&mut m, 0x288);
    p(&mut m, MHOD_PLAYLIST);
    z(&mut m, 6);
    for v in [0x010084, 0x05, 0x09, 0x03, 0x120001] {
        p(&mut m, v);
    }
    for v in [0xc80002, 0x3c000d, 0x7d0004, 0x7d0003, 0x640008] {
        z(&mut m, 3);
        p(&mut m, v);
    }
    z(&mut m, 3);
    p(&mut m, 0x640017);
    p(&mut m, 0x01);
    z(&mut m, 2);
    p(&mut m, 0x500014);
    p(&mut m, 0x01);
    z(&mut m, 2);
    p(&mut m, 0x7d0015);
    p(&mut m, 0x01);
    z(&mut m, 114);
    debug_assert_eq!(m.len(), 0x288);
    m
}

fn mhod52(sort_type: u32, order: &[u32]) -> Vec<u8> {
    let mut m = vec![0u8; 72];
    m[0..4].copy_from_slice(b"mhod");
    wr32(&mut m, 4, 24);
    wr32(&mut m, 8, (72 + order.len() * 4) as u32);
    wr32(&mut m, 12, MHOD_LIBINDEX);
    wr32(&mut m, 24, sort_type);
    wr32(&mut m, 28, order.len() as u32);
    for i in order {
        m.extend_from_slice(&i.to_le_bytes());
    }
    m
}

fn mhod53(sort_type: u32, letters: &[(u16, u32, u32)]) -> Vec<u8> {
    let mut m = vec![0u8; 40];
    m[0..4].copy_from_slice(b"mhod");
    wr32(&mut m, 4, 24);
    wr32(&mut m, 8, (40 + letters.len() * 12) as u32);
    wr32(&mut m, 12, MHOD_LIBJUMP);
    wr32(&mut m, 24, sort_type);
    wr32(&mut m, 28, letters.len() as u32);
    for (l, start, count) in letters {
        m.extend_from_slice(&l.to_le_bytes());
        m.extend_from_slice(&0u16.to_le_bytes());
        m.extend_from_slice(&start.to_le_bytes());
        m.extend_from_slice(&count.to_le_bytes());
    }
    m
}

fn mhip(track_id: u32, pos: u32) -> Vec<u8> {
    let mut m = vec![0u8; 76 + 44];
    m[0..4].copy_from_slice(b"mhip");
    wr32(&mut m, 4, 76);
    wr32(&mut m, 8, 76 + 44);
    wr32(&mut m, 12, 1); // child count
    wr32(&mut m, 24, track_id);
    let o = 76;
    m[o..o + 4].copy_from_slice(b"mhod");
    wr32(&mut m, o + 4, 24);
    wr32(&mut m, o + 8, 44);
    wr32(&mut m, o + 12, MHOD_PLAYLIST);
    wr32(&mut m, o + 24, pos);
    m
}

/// ポッドキャストの番組の見出し (libgpod の mk_mhip(fexp, 1, 256, groupid, 0, 0, 0) + 番組名の mhod)
fn podcast_group_mhip(group_id: u32, album: &str) -> Vec<u8> {
    let title = string_mhod(MHOD_TITLE, album);
    let mut m = vec![0u8; 76];
    m[0..4].copy_from_slice(b"mhip");
    wr32(&mut m, 4, 76);
    wr32(&mut m, 12, 1); // child count
    wr32(&mut m, 16, 0x100); // podcast group flag
    wr32(&mut m, 20, group_id);
    m.extend_from_slice(&title);
    let total = m.len() as u32;
    wr32(&mut m, 8, total);
    m
}

// ============================================================
// Play Counts / OTG
// ============================================================

/// iPod 本体で再生された回数などを記録した "Play Counts" を DB に取り込む
pub fn merge_play_counts(db: &mut ITunesDb, data: &[u8]) {
    if data.len() < 16 || &data[0..4] != b"mhdp" {
        return;
    }
    let hlen = rd32(data, 4) as usize;
    let elen = rd32(data, 8) as usize;
    let n = rd32(data, 12) as usize;
    if elen < 0x0C {
        return;
    }
    for (i, t) in db.tracks.iter_mut().enumerate().take(n) {
        let o = hlen + i * elen;
        if o + elen > data.len() {
            break;
        }
        let plays = rd32(data, o);
        let last = rd32(data, o + 4);
        let bookmark = rd32(data, o + 8);
        if elen >= 0x10 {
            let rating = rd32(data, o + 12);
            if rating != u32::MAX && rating <= 100 {
                t.set_rating(rating as u8);
            }
        }
        if last != 0 {
            t.set_last_played_mac(last);
        }
        if bookmark != 0 {
            t.set_bookmark_ms(bookmark);
        }
        if plays != 0 {
            t.set_playcount(t.playcount().saturating_add(plays));
            t.set_mark_unplayed(1);
        }
        if elen >= 0x1C {
            t.set_skipcount(t.skipcount().saturating_add(rd32(data, o + 20)));
            let ls = rd32(data, o + 24);
            if ls != 0 {
                t.set_last_skipped_mac(ls);
            }
        }
    }
}

/// On-The-Go プレイリスト (iPod 本体で作成) をトラック dbid 列として読む
pub fn parse_otg(db: &ITunesDb, data: &[u8]) -> Vec<u64> {
    if data.len() < 16 || &data[0..4] != b"mhpo" {
        return Vec::new();
    }
    let hlen = rd32(data, 4) as usize;
    let elen = rd32(data, 8) as usize;
    let n = rd32(data, 12) as usize;
    if elen < 4 {
        return Vec::new();
    }
    // 数が壊れていても、ファイルに入っている分だけ読む
    let n = n.min(data.len().saturating_sub(hlen) / elen);
    (0..n)
        .filter_map(|i| {
            let idx = rd32(data, hlen + i * elen) as usize;
            db.tracks.get(idx).map(|t| t.dbid())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track(db: &ITunesDb, title: &str, artist: &str, album: &str) -> Track {
        let mut t = Track::new(db.new_dbid());
        t.set_string(MHOD_TITLE, Some(title));
        t.set_string(MHOD_ARTIST, Some(artist));
        t.set_string(MHOD_ALBUM, Some(album));
        t.set_string(MHOD_PATH, Some(":iPod_Control:Music:F00:ABCD.mp3"));
        t.set_length_ms(180_000);
        t.set_size(4_000_000);
        t
    }

    #[test]
    fn long_mhod_len() {
        assert_eq!(long_playlist_mhod().len(), 0x288);
    }

    #[test]
    fn roundtrip() {
        let mut db = ITunesDb::new_empty("テストiPod");
        for (ti, ar, al) in [
            ("夜に駆ける", "YOASOBI", "THE BOOK"),
            ("Bohemian Rhapsody", "Queen", "A Night at the Opera"),
            ("Love of My Life", "Queen", "A Night at the Opera"),
        ] {
            let t = sample_track(&db, ti, ar, al);
            db.tracks.push(t);
        }
        let mut pl = Playlist::new("お気に入り", random_id(), false);
        pl.items = vec![db.tracks[2].dbid(), db.tracks[0].dbid()];
        db.playlists.push(pl);

        let bytes = db.serialize();
        assert_eq!(rd32(&bytes, 8) as usize, bytes.len());

        let mut db2 = ITunesDb::parse(&bytes).unwrap();
        assert_eq!(db2.tracks.len(), 3);
        assert_eq!(db2.tracks[0].title(), Some("夜に駆ける"));
        assert_eq!(db2.master().name, "テストiPod");
        assert_eq!(db2.master().items.len(), 3);
        assert_eq!(db2.playlists.len(), 2);
        assert_eq!(db2.playlists[1].name, "お気に入り");
        assert_eq!(
            db2.playlists[1].items,
            vec![db.tracks[2].dbid(), db.tracks[0].dbid()]
        );
        // 再シリアライズでバイト一致 (ランダムな sql_id を除けば安定)
        let b2 = db2.serialize();
        assert_eq!(bytes.len(), b2.len());
    }

    /// mhsd の中身 (種別 ty)
    fn mhsd_body(bytes: &[u8], ty: u32) -> Vec<u8> {
        let mut pos = rd32(bytes, 4) as usize;
        for _ in 0..rd32(bytes, 0x14) {
            let len = rd32(bytes, pos + 8) as usize;
            if rd32(bytes, pos + 0x0C) == ty {
                let hl = rd32(bytes, pos + 4) as usize;
                return bytes[pos + hl..pos + len].to_vec();
            }
            pos += len;
        }
        panic!("mhsd {ty} が無い");
    }

    /// mhlp の中のプレイリストごとの mhip: (podcast flag, mhip の [flag, id, track id, group ref] の並び)
    fn playlists_mhips(body: &[u8]) -> Vec<(bool, Vec<[u32; 4]>)> {
        let mut out = Vec::new();
        let mut pos = rd32(body, 4) as usize;
        for _ in 0..rd32(body, 8) {
            let len = rd32(body, pos + 8) as usize;
            let pl = &body[pos..pos + len];
            let mut cp = rd32(pl, 4) as usize;
            let mut items = Vec::new();
            while cp + 12 <= pl.len() {
                let l = rd32(pl, cp + 8) as usize;
                if &pl[cp..cp + 4] == b"mhip" {
                    items.push([rd32(pl, cp + 0x10), rd32(pl, cp + 0x14), rd32(pl, cp + 0x18), rd32(pl, cp + 0x20)]);
                }
                cp += l;
            }
            out.push((rd16(pl, 0x2A) == 1, items));
            pos += len;
        }
        out
    }

    #[test]
    fn kinds_and_podcast_playlist() {
        let mut db = ITunesDb::new_empty("iPod");
        for (ti, ar, al, k) in [
            ("曲", "A", "X", MediaKind::Music),
            ("第 1 回", "番組の人", "ラジオ A", MediaKind::Podcast),
            ("第 1 章", "作者", "本", MediaKind::Audiobook),
            ("Episode 1", "Host", "Show B", MediaKind::Podcast),
            ("第 2 回", "番組の人", "ラジオ A", MediaKind::Podcast),
        ] {
            let mut t = sample_track(&db, ti, ar, al);
            t.set_kind(k);
            db.tracks.push(t);
        }
        // 種類ごとの印 (libgpod の itdb.h の説明どおり)
        let pod = &db.tracks[1];
        assert_eq!((pod.mediatype(), pod.hdr[0xA5], pod.hdr[0xA6], pod.hdr[0xA7], pod.hdr[0xB2]), (4, 1, 1, 1, 2));
        let book = &db.tracks[2];
        assert_eq!((book.mediatype(), book.hdr[0xA5], book.hdr[0xA6], book.hdr[0xB2]), (8, 1, 1, 1));
        assert_eq!(db.tracks[0].mediatype(), 1);

        let bytes = db.serialize();
        // ポッドキャストのプレイリストが 2 番目にでき、2 番は並べたまま、3 番は番組ごとの見出しの下に回
        let flat = playlists_mhips(&mhsd_body(&bytes, 2));
        let grouped = playlists_mhips(&mhsd_body(&bytes, 3));
        assert!(flat[1].0 && grouped[1].0);
        assert_eq!(flat[1].1.len(), 3);
        assert!(flat[1].1.iter().all(|m| m[0] == 0));
        let g = &grouped[1].1;
        assert_eq!(g.len(), 3 + 2);
        let heads: Vec<&[u32; 4]> = g.iter().filter(|m| m[0] == 0x100).collect();
        assert_eq!(heads.len(), 2);
        // 回は直前の見出しを指す。見出しの id は曲の id と重ならない
        let track_ids: std::collections::HashSet<u32> = db.tracks.iter().map(|t| t.id()).collect();
        let mut cur = 0;
        for m in g {
            if m[0] == 0x100 {
                cur = m[1];
                assert!(!track_ids.contains(&cur));
            } else {
                assert_eq!(m[3], cur);
                assert!(track_ids.contains(&m[2]));
            }
        }
        // ラジオ A の 2 回は同じ見出しの下
        assert_eq!(g[0][0], 0x100);
        assert_eq!((g[1][2], g[2][2]), (db.tracks[1].id(), db.tracks[4].id()));
        // 通常のプレイリストは 3 番でも同じ
        assert_eq!(flat[0].1.len(), grouped[0].1.len());

        // 読み直すと種類はそのまま、ポッドキャストのプレイリストも 1 つだけ
        let mut db2 = ITunesDb::parse(&bytes).unwrap();
        assert_eq!(db2.tracks[1].kind(), MediaKind::Podcast);
        assert_eq!(db2.tracks[2].kind(), MediaKind::Audiobook);
        assert_eq!(db2.playlists.iter().filter(|p| p.is_podcast()).count(), 1);
        assert_eq!(db2.playlists[1].items.len(), 3);
        // 曲に戻すとポッドキャストのプレイリストから外れる。印も戻る
        assert!(db2.tracks[3].set_kind(MediaKind::Music));
        assert_eq!((db2.tracks[3].mediatype(), db2.tracks[3].hdr[0xA5], db2.tracks[3].hdr[0xA6]), (1, 0, 0));
        let b3 = db2.serialize();
        let flat3 = playlists_mhips(&mhsd_body(&b3, 2));
        assert_eq!(flat3[1].1.len(), 2);
    }

    #[test]
    fn video_kind_is_kept() {
        let db = ITunesDb::new_empty("iPod");
        let mut t = sample_track(&db, "Movie", "", "");
        t.set_mediatype(2);
        assert!(!t.set_kind(MediaKind::Podcast));
        assert_eq!(t.mediatype(), 2);
    }
}
