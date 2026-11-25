//! アートワーク (iPod_Control/Artwork/ArtworkDB と F<format>_<n>.ithmb) の読み書き。
//!
//! - ArtworkDB は mhfd → mhsd(1: mhli/mhii, 2: mhla, 3: mhlf/mhif) の構造。
//! - 画像本体は RGB565 (little endian) の生データを .ithmb に追記する。
//! - 同じ画像は 1 つの mhii にまとめ、各トラックは mhit の mhii_link で参照する
//!   (iPod classic / nano 3G 以降の "sparse artwork" 方式)。
//! - 写真の DB (iPod_Control/Photos/Photo Database) も同じ形。こちらは mhsd 2 に写真のアルバム (mhla/mhba/mhia) があり、
//!   画像は Photos/Thumbs/ に機種ごとの形式 (RGB565 の LE / BE・UYVY・I420) で、縦横比を保って余白を付けて置く
//!   (libgpod 0.8.3 の db-artwork-writer.c / ithumb-writer.c と同じ)

use crate::t;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, RgbImage};

use crate::error::{Error, Result};
use crate::ipod::itunesdb::{rd16, rd32, rd64, wr16, wr32, wr64};

/// 1 ファイルあたりの ithmb 上限 (libgpod と同じ 256MB)
const ITHMB_MAX: u64 = 256 * 1000 * 1000;
const FIRST_IMAGE_ID: u32 = 0x64;
/// DB に書いてあるサムネイルの幅・高さの上限 (壊れた DB で巨大な画像を作らないように。一番大きい形式は 720×480)
pub(crate) const MAX_THUMB_DIM: u32 = 2048;

/// 画素の並べ方 (libgpod の THUMB_FORMAT_*)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pix {
    Rgb565Le,
    Rgb565Be,
    /// テレビ出力用 (iPod photo / video)。偶数の行を前半、奇数の行を後半に置く
    Uyvy,
    /// テレビ出力用 (iPod classic / nano 3G)
    I420,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtFormat {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub pix: Pix,
    /// 1 行のバイト数をこの倍数に切り上げる (0 はそろえない)。SysInfoExtended の RowBytesAlignment
    /// (AlignRowBytes だけのときは 4。libgpod と同じ)。そろえた分は右端の余白 (黒) になる
    pub row_align: u32,
}

impl ArtFormat {
    /// 1 行のバイト数 (右端の余白を含む)
    pub fn row_bytes(&self) -> u32 {
        let b = self.width * 2;
        if self.row_align > 1 { b.div_ceil(self.row_align) * self.row_align } else { b }
    }
}

const fn f(id: u32, w: u32, h: u32) -> ArtFormat {
    fp(id, w, h, Pix::Rgb565Le)
}

const fn fp(id: u32, w: u32, h: u32, pix: Pix) -> ArtFormat {
    ArtFormat {
        id,
        width: w,
        height: h,
        pix,
        row_align: 0,
    }
}

/// 行のバイト数をそろえる RGB565 (LE) の形式
pub(crate) const fn fa(id: u32, w: u32, h: u32, row_align: u32) -> ArtFormat {
    ArtFormat {
        id,
        width: w,
        height: h,
        pix: Pix::Rgb565Le,
        row_align,
    }
}

/// iPod classic 全世代 / nano 3G。1061 (アルバムの一覧・歌詞の画面の小さいジャケット) は 55×55 を 4 バイトそろえの行
/// (56 画素 × 55 行 = 6160 バイト) で書く。iPod 本体の SysInfoExtended の値で、libgpod も本体ではこれで書く
/// (libgpod の機種の既定値の 56×56 には「公式には 55×55」とある)。56×56 (6272 バイト) で書くと
/// 本体に出なかった (配布リポジトリの Issue #12、2026-10-25)
pub const CLASSIC: &[ArtFormat] = &[
    fa(1061, 55, 55, 4),
    f(1055, 128, 128),
    f(1068, 128, 128),
    f(1060, 320, 320),
];
/// nano 4G
pub const NANO4: &[ArtFormat] = &[f(1055, 128, 128), f(1068, 128, 128)];
/// iPod video 5G / 5.5G
pub const VIDEO: &[ArtFormat] = &[f(1028, 100, 100), f(1029, 200, 200)];
/// iPod photo (カラー液晶の 4G を含む)。libgpod の ipod_photo_cover_art_info
pub const PHOTO: &[ArtFormat] = &[f(1017, 56, 56), f(1016, 140, 140)];
/// iPod nano 1G / 2G。libgpod の ipod_nano_cover_art_info
pub const NANO: &[ArtFormat] = &[f(1031, 42, 42), f(1027, 100, 100)];

/// iPod nano 5G。libgpod の ipod_nano5g_cover_art_info
pub const NANO5: &[ArtFormat] = &[f(1056, 128, 128), f(1078, 80, 80), f(1073, 240, 240), f(1074, 50, 50)];
/// iPod nano 6G。libgpod には無く、iOpenPod が実機のデータから作った表 (本体の SysInfoExtended があればそちらを使う)
pub const NANO6: &[ArtFormat] = &[f(1073, 240, 240), f(1085, 88, 88), f(1089, 58, 58), f(1074, 50, 50)];
/// iPod nano 7G。NANO6 と同じく iOpenPod の表。1016 は 57 画素を 4 バイトそろえの行 (116 バイト)
pub const NANO7: &[ArtFormat] = &[f(1010, 240, 240), f(1013, 50, 50), f(1015, 58, 58), fa(1016, 57, 57, 4)];

// ---- 写真の形式 (libgpod 0.8.3 の itdb_device.c の *_photo_info) ----
/// iPod photo (カラー液晶の 4G)
pub const PHOTO_PHOTOS: &[ArtFormat] = &[
    f(1009, 42, 30),
    f(1015, 130, 88),
    fp(1013, 220, 176, Pix::Rgb565Be),
    fp(1019, 720, 480, Pix::Uyvy),
];
/// iPod nano 1G / 2G
pub const NANO_PHOTOS: &[ArtFormat] = &[f(1032, 42, 37), fp(1023, 176, 132, Pix::Rgb565Be)];
/// iPod video 5G / 5.5G
pub const VIDEO_PHOTOS: &[ArtFormat] = &[f(1036, 50, 41), f(1015, 130, 88), f(1024, 320, 240), fp(1019, 720, 480, Pix::Uyvy)];
/// iPod classic 全世代 / nano 3G
pub const CLASSIC_PHOTOS: &[ArtFormat] = &[fp(1067, 720, 480, Pix::I420), f(1024, 320, 240), f(1066, 64, 64)];
/// iPod nano 4G (縦にも回すので 240×320 がある。JPEG の 1081 は libgpod も書かない)
pub const NANO4_PHOTOS: &[ArtFormat] = &[f(1024, 320, 240), f(1066, 64, 64), f(1079, 80, 80), f(1083, 240, 320)];
/// iPod nano 5G (JPEG の 1081 は libgpod も書かない)
pub const NANO5_PHOTOS: &[ArtFormat] = &[f(1087, 384, 384), f(1079, 80, 80), f(1066, 64, 64)];

fn known_format(id: u32) -> Option<ArtFormat> {
    CLASSIC
        .iter()
        .chain(NANO4)
        .chain(VIDEO)
        .chain(PHOTO)
        .chain(NANO)
        .chain(PHOTO_PHOTOS)
        .chain(NANO_PHOTOS)
        .chain(VIDEO_PHOTOS)
        .chain(CLASSIC_PHOTOS)
        .chain(NANO4_PHOTOS)
        // 番号がほかの機種と重なるもの (nano 7G の 1015 など) は前の表を優先する
        .chain(NANO5)
        .chain(NANO6)
        .chain(NANO7)
        .chain(NANO5_PHOTOS)
        .copied()
        .find(|f| f.id == id)
}

/// 1 枚の画素データの大きさ (I420 も libgpod は 2 バイト × 画素で書く。後ろの 1/4 は使わない)
pub(crate) fn frame_size(f: &ArtFormat) -> u32 {
    f.row_bytes() * f.height
}

// ============================================================
// ArtworkDB
// ============================================================

#[derive(Debug, Clone)]
pub struct Thumb {
    pub format_id: u32,
    /// ":F1055_1.ithmb"
    pub file: String,
    pub offset: u32,
    pub size: u32,
    pub vpad: u16,
    pub hpad: u16,
    pub height: u16,
    pub width: u16,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub(crate) hdr: Vec<u8>,
    pub id: u32,
    /// 最初にこの画像を使ったトラックの dbid (写真の DB では id + 2)
    pub song_id: u64,
    /// 元画像のバイト数
    pub orig_size: u32,
    pub thumbs: Vec<Thumb>,
    /// サムネイル以外の mhod (写真の DB の原寸の画像の場所など)。そのまま書き戻す
    pub extra: Vec<Vec<u8>>,
}

impl Image {
    /// 写真の撮った日時 (Mac の秒。0 = なし)
    pub fn orig_date(&self) -> u32 {
        rd32(&self.hdr, 0x28)
    }
    pub fn set_dates(&mut self, mac: u32) {
        wr32(&mut self.hdr, 0x28, mac);
        wr32(&mut self.hdr, 0x2C, mac);
    }
}

/// 写真のアルバム (mhba)。1 つ目 (album_type 1) は全部の写真が入る「フォトライブラリ」
#[derive(Debug, Clone)]
pub struct PhotoAlbum {
    pub(crate) hdr: Vec<u8>,
    pub id: u32,
    pub name: String,
    /// 写真 (mhii) の id
    pub items: Vec<u32>,
    /// 名前以外の mhod。そのまま書き戻す
    extra: Vec<Vec<u8>>,
}

impl PhotoAlbum {
    pub fn new(name: &str, master: bool) -> Self {
        let mut hdr = vec![0u8; 0x94];
        hdr[0..4].copy_from_slice(b"mhba");
        wr32(&mut hdr, 4, 0x94);
        hdr[0x1E] = if master { 1 } else { 2 };
        PhotoAlbum { hdr, id: 0, name: name.to_string(), items: Vec::new(), extra: Vec::new() }
    }
    pub fn is_master(&self) -> bool {
        self.hdr[0x1E] == 1
    }
}

#[derive(Debug, Clone)]
pub struct ArtworkDb {
    hdr: Vec<u8>,
    pub images: Vec<Image>,
    /// 写真のアルバム (ArtworkDB では空)
    pub albums: Vec<PhotoAlbum>,
    /// mhlf に載せる (format id, 1 枚あたりのバイト数)
    pub files: Vec<(u32, u32)>,
}

impl ArtworkDb {
    pub fn new_empty() -> Self {
        let mut hdr = vec![0u8; 0x84];
        hdr[0..4].copy_from_slice(b"mhfd");
        wr32(&mut hdr, 4, 0x84);
        wr32(&mut hdr, 0x10, 2);
        hdr[0x30] = 2;
        ArtworkDb {
            hdr,
            images: Vec::new(),
            albums: Vec::new(),
            files: Vec::new(),
        }
    }

    pub fn next_id(&self) -> u32 {
        self.images
            .iter()
            .map(|i| i.id.saturating_add(1))
            .max()
            .unwrap_or(FIRST_IMAGE_ID)
            .max(FIRST_IMAGE_ID)
    }

    pub fn image(&self, id: u32) -> Option<&Image> {
        self.images.iter().find(|i| i.id == id)
    }

    /// 書き込みに使うフォーマット。本体の形式 (`device`: SysInfoExtended か機種の既定値) を正とし、
    /// 既存 DB の mhlf にしか無い形式は、寸法が分かるものだけ足す。
    /// 本体と同じ ID が mhlf で違う大きさでも本体の値を使う (以前の版は mhlf を優先し、iTunes が書いた
    /// 1061 の 6160 バイトを 56×56 と合わないとして黙って外していた。Issue #12)
    pub fn formats(&self, device: &[ArtFormat]) -> Vec<ArtFormat> {
        let mut out: Vec<ArtFormat> = Vec::new();
        for (id, size) in &self.files {
            if out.iter().any(|f| f.id == *id) {
                continue;
            }
            let fmt = device
                .iter()
                .copied()
                .find(|f| f.id == *id)
                .or_else(|| known_format(*id).filter(|f| frame_size(f) == *size))
                .or_else(|| self.format_from_thumbs(*id, *size));
            match fmt {
                Some(f) => out.push(f),
                None => log::warn!("ArtworkDB: 寸法の分からない形式 {id} ({size} バイト) には書かない"),
            }
        }
        for f in device {
            if !out.iter().any(|o| o.id == f.id) {
                out.push(*f);
            }
        }
        out
    }

    /// 未知の ID の形式を既存のサムネイルの寸法から推定する (行のバイト数をそろえた形も)
    fn format_from_thumbs(&self, id: u32, size: u32) -> Option<ArtFormat> {
        self.images.iter().flat_map(|i| &i.thumbs).find_map(|t| {
            let (w, h) = (t.width as u32, t.height as u32);
            if t.format_id != id || !(1..=MAX_THUMB_DIM).contains(&w) || !(1..=MAX_THUMB_DIM).contains(&h) {
                return None;
            }
            if w * h * 2 == size {
                return Some(f(id, w, h));
            }
            // 1 行が幅より少し長い (右端に余白がある) 形。その行のバイト数をそろえの単位にする
            let rb = size / h;
            (size.is_multiple_of(h) && rb >= w * 2 && rb < w * 2 + 16).then(|| fa(id, w, h, rb))
        })
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let corrupt = |m: &str| Error::Corrupt(format!("ArtworkDB: {m}"));
        if data.len() < 0x20 || &data[0..4] != b"mhfd" {
            return Err(corrupt(&t!("mhfd がありません")));
        }
        let hlen = rd32(data, 4) as usize;
        let n = rd32(data, 0x14);
        let mut db = ArtworkDb {
            hdr: data[..hlen.min(data.len())].to_vec(),
            images: Vec::new(),
            albums: Vec::new(),
            files: Vec::new(),
        };
        if db.hdr.len() < 0x84 {
            db.hdr.resize(0x84, 0);
        }
        let mut pos = hlen;
        for _ in 0..n {
            let sd = sub(data, pos, b"mhsd").ok_or_else(|| corrupt(&t!("mhsd が不正です")))?;
            let body_off = rd32(sd, 4) as usize;
            let body = sd.get(body_off..).unwrap_or(&[]);
            match rd16(sd, 0x0C) {
                1 => db.images = parse_mhli(body).ok_or_else(|| corrupt(&t!("mhli が不正です")))?,
                2 => db.albums = parse_mhla(body),
                3 => db.files = parse_mhlf(body),
                _ => {}
            }
            pos += sd.len();
        }
        Ok(db)
    }

    pub fn serialize(&self) -> Vec<u8> {
        self.serialize_with_next_id(self.next_id().saturating_sub(1).max(FIRST_IMAGE_ID - 1))
    }

    /// mhfd の next_id を決めて書く (写真の DB は「一番大きい id + 1」、ArtworkDB は「一番大きい id」。libgpod と同じ)
    pub fn serialize_with_next_id(&self, next_id: u32) -> Vec<u8> {
        // mhsd 1: 画像一覧
        let mut mhli = list_header(b"mhli", self.images.len() as u32);
        for img in &self.images {
            mhli.extend_from_slice(&write_mhii(img));
        }
        // mhsd 2: 写真のアルバム (音楽用 DB では空)
        let mut mhla = list_header(b"mhla", self.albums.len() as u32);
        for a in &self.albums {
            mhla.extend_from_slice(&write_mhba(a));
        }
        // mhsd 3: ithmb ファイル一覧
        let mut mhlf = list_header(b"mhlf", self.files.len() as u32);
        for (id, size) in &self.files {
            let mut h = vec![0u8; 0x7C];
            h[0..4].copy_from_slice(b"mhif");
            wr32(&mut h, 4, 0x7C);
            wr32(&mut h, 8, 0x7C);
            wr32(&mut h, 0x10, *id);
            wr32(&mut h, 0x14, *size);
            mhlf.extend_from_slice(&h);
        }

        let mut out = self.hdr.clone();
        let hl = out.len() as u32;
        wr32(&mut out, 4, hl);
        wr32(&mut out, 0x10, 2);
        wr32(&mut out, 0x14, 3);
        wr32(&mut out, 0x1C, next_id);
        for (i, body) in [mhli, mhla, mhlf].into_iter().enumerate() {
            let mut h = vec![0u8; 0x60];
            h[0..4].copy_from_slice(b"mhsd");
            wr32(&mut h, 4, 0x60);
            wr32(&mut h, 8, (0x60 + body.len()) as u32);
            wr16(&mut h, 0x0C, i as u16 + 1);
            out.extend_from_slice(&h);
            out.extend_from_slice(&body);
        }
        let total = out.len() as u32;
        wr32(&mut out, 8, total);
        out
    }
}

fn sub<'a>(data: &'a [u8], pos: usize, tag: &[u8; 4]) -> Option<&'a [u8]> {
    let h = data.get(pos..pos + 12)?;
    if &h[0..4] != tag {
        return None;
    }
    let total = rd32(data, pos + 8) as usize;
    if total < 12 {
        return None;
    }
    data.get(pos..pos + total)
}

fn list_header(tag: &[u8; 4], n: u32) -> Vec<u8> {
    let mut h = vec![0u8; 0x5C];
    h[0..4].copy_from_slice(tag);
    wr32(&mut h, 4, 0x5C);
    wr32(&mut h, 8, n);
    h
}

fn parse_mhli(body: &[u8]) -> Option<Vec<Image>> {
    if body.len() < 12 || &body[0..4] != b"mhli" {
        return None;
    }
    let n = rd32(body, 8);
    let mut pos = rd32(body, 4) as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let ii = sub(body, pos, b"mhii")?;
        let hlen = rd32(ii, 4) as usize;
        let nchild = rd32(ii, 0x0C);
        let mut img = Image {
            hdr: ii.get(..hlen)?.to_vec(),
            id: rd32(ii, 0x10),
            song_id: rd64(ii, 0x14),
            orig_size: rd32(ii, 0x30),
            thumbs: Vec::new(),
            extra: Vec::new(),
        };
        if img.hdr.len() < 0x98 {
            img.hdr.resize(0x98, 0);
        }
        let mut cp = hlen;
        for _ in 0..nchild {
            let od = sub(ii, cp, b"mhod")?;
            // type 2 = サムネイル (mhni を内包)。ほか (5 = 原寸の画像など) はそのまま持つ
            if rd16(od, 0x0C) == 2 {
                if let Some(t) = parse_mhni(od.get(rd32(od, 4) as usize..)?) {
                    img.thumbs.push(t);
                }
            } else {
                img.extra.push(od.to_vec());
            }
            cp += od.len();
        }
        pos += ii.len();
        out.push(img);
    }
    Some(out)
}

fn parse_mhni(b: &[u8]) -> Option<Thumb> {
    let ni = sub(b, 0, b"mhni")?;
    let hlen = rd32(ni, 4) as usize;
    // 子 mhod type 3 = ファイル名
    let od = sub(ni, hlen, b"mhod")?;
    let file = mhod_string(od)?;
    Some(Thumb {
        format_id: rd32(ni, 0x10),
        file,
        offset: rd32(ni, 0x14),
        size: rd32(ni, 0x18),
        vpad: rd16(ni, 0x1C),
        hpad: rd16(ni, 0x1E),
        height: rd16(ni, 0x20),
        width: rd16(ni, 0x22),
    })
}

/// 文字列の mhod (type 1 / 3) の中身。encoding 2 は UTF-16LE、ほかは UTF-8
fn mhod_string(od: &[u8]) -> Option<String> {
    let len = rd32(od, 0x18) as usize;
    let raw = od.get(0x24..0x24 + len)?;
    Some(if od[0x1C] == 2 {
        let u: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(raw).into_owned()
    })
}

fn parse_mhla(body: &[u8]) -> Vec<PhotoAlbum> {
    if body.len() < 12 || &body[0..4] != b"mhla" {
        return Vec::new();
    }
    let n = rd32(body, 8);
    let mut pos = rd32(body, 4) as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let Some(ba) = sub(body, pos, b"mhba") else { break };
        let hlen = (rd32(ba, 4) as usize).min(ba.len());
        let mut hdr = ba[..hlen].to_vec();
        if hdr.len() < 0x94 {
            hdr.resize(0x94, 0);
        }
        let mut album = PhotoAlbum { id: rd32(ba, 0x14), hdr, name: String::new(), items: Vec::new(), extra: Vec::new() };
        let (nmhod, nmhia) = (rd32(ba, 0x0C), rd32(ba, 0x10));
        let mut cp = hlen;
        for _ in 0..nmhod {
            let Some(od) = sub(ba, cp, b"mhod") else { break };
            if rd16(od, 0x0C) == 1 && album.name.is_empty() {
                album.name = mhod_string(od).unwrap_or_default();
            } else {
                album.extra.push(od.to_vec());
            }
            cp += od.len();
        }
        for _ in 0..nmhia {
            let Some(ia) = sub(ba, cp, b"mhia") else { break };
            album.items.push(rd32(ia, 0x10));
            cp += ia.len();
        }
        pos += ba.len();
        out.push(album);
    }
    out
}

/// 文字列の mhod (ArtworkDB の形。`encoding` 1 = UTF-8 (アルバム名)、2 = UTF-16LE (ファイル名))
fn write_string_mhod(kind: u16, text: &str, encoding: u8) -> Vec<u8> {
    let bytes: Vec<u8> = if encoding == 2 { text.encode_utf16().flat_map(|c| c.to_le_bytes()).collect() } else { text.as_bytes().to_vec() };
    let slen = bytes.len();
    let pad = (4 - (0x24 + slen) % 4) % 4;
    let mut od = vec![0u8; 0x24];
    od[0..4].copy_from_slice(b"mhod");
    wr32(&mut od, 4, 0x18);
    wr32(&mut od, 8, (0x24 + slen + pad) as u32);
    wr16(&mut od, 0x0C, kind);
    od[0x0F] = pad as u8;
    wr32(&mut od, 0x18, slen as u32);
    od[0x1C] = encoding;
    od.extend_from_slice(&bytes);
    od.extend(std::iter::repeat_n(0u8, pad));
    od
}

fn write_mhba(a: &PhotoAlbum) -> Vec<u8> {
    let mut out = a.hdr.clone();
    let hl = out.len() as u32;
    wr32(&mut out, 4, hl);
    wr32(&mut out, 0x0C, 1 + a.extra.len() as u32);
    wr32(&mut out, 0x10, a.items.len() as u32);
    wr32(&mut out, 0x14, a.id);
    out.extend_from_slice(&write_string_mhod(1, &a.name, 1));
    for e in &a.extra {
        out.extend_from_slice(e);
    }
    for id in &a.items {
        let mut ia = vec![0u8; 0x28];
        ia[0..4].copy_from_slice(b"mhia");
        wr32(&mut ia, 4, 0x28);
        wr32(&mut ia, 8, 0x28);
        wr32(&mut ia, 0x10, *id);
        out.extend_from_slice(&ia);
    }
    let total = out.len() as u32;
    wr32(&mut out, 8, total);
    out
}

fn parse_mhlf(body: &[u8]) -> Vec<(u32, u32)> {
    if body.len() < 12 || &body[0..4] != b"mhlf" {
        return Vec::new();
    }
    let n = rd32(body, 8);
    let mut pos = rd32(body, 4) as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let Some(f) = sub(body, pos, b"mhif") else {
            break;
        };
        out.push((rd32(f, 0x10), rd32(f, 0x14)));
        pos += f.len();
    }
    out
}

fn write_mhii(img: &Image) -> Vec<u8> {
    let mut out = img.hdr.clone();
    let hl = out.len() as u32;
    wr32(&mut out, 4, hl);
    wr32(&mut out, 0x0C, (img.thumbs.len() + img.extra.len()) as u32);
    wr32(&mut out, 0x10, img.id);
    wr64(&mut out, 0x14, img.song_id);
    wr32(&mut out, 0x30, img.orig_size);
    for t in &img.thumbs {
        // mhod(type 2) → mhni → mhod(type 3, ファイル名、UTF-16LE)
        let fname = write_string_mhod(3, &t.file, 2);

        let mut ni = vec![0u8; 0x4C];
        ni[0..4].copy_from_slice(b"mhni");
        wr32(&mut ni, 4, 0x4C);
        wr32(&mut ni, 8, (0x4C + fname.len()) as u32);
        wr32(&mut ni, 0x0C, 1);
        wr32(&mut ni, 0x10, t.format_id);
        wr32(&mut ni, 0x14, t.offset);
        wr32(&mut ni, 0x18, t.size);
        wr16(&mut ni, 0x1C, t.vpad);
        wr16(&mut ni, 0x1E, t.hpad);
        wr16(&mut ni, 0x20, t.height);
        wr16(&mut ni, 0x22, t.width);
        ni.extend_from_slice(&fname);

        let mut od = vec![0u8; 0x18];
        od[0..4].copy_from_slice(b"mhod");
        wr32(&mut od, 4, 0x18);
        wr32(&mut od, 8, (0x18 + ni.len()) as u32);
        wr16(&mut od, 0x0C, 2);
        od.extend_from_slice(&ni);
        out.extend_from_slice(&od);
    }
    for e in &img.extra {
        out.extend_from_slice(e);
    }
    let total = out.len() as u32;
    wr32(&mut out, 8, total);
    out
}

// ============================================================
// 画像変換 / ithmb 入出力
// ============================================================

/// 中央を正方形に切り抜いてリサイズし、RGB565LE に変換
pub fn to_rgb565(img: &DynamicImage, fmt: ArtFormat) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let target_ratio = fmt.width as f64 / fmt.height as f64;
    let (cw, ch) = if (w as f64 / h as f64) > target_ratio {
        (((h as f64) * target_ratio).round() as u32, h)
    } else {
        (w, ((w as f64) / target_ratio).round() as u32)
    };
    let cropped = img.crop_imm((w - cw) / 2, (h - ch) / 2, cw.max(1), ch.max(1));
    let resized = cropped
        .resize_exact(fmt.width, fmt.height, FilterType::CatmullRom)
        .to_rgb8();
    pack_rgb565(&resized, fmt.width, fmt.height, 0, 0, false, fmt.row_bytes())
}

fn rgb565(p: [u8; 3]) -> u16 {
    let [r, g, b] = p;
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// `img` を (hpad, vpad) の位置に置いた fw×fh の RGB565 (余白は黒)。1 行は `row_bytes` バイト (右端の余りは 0)
fn pack_rgb565(img: &RgbImage, fw: u32, fh: u32, hpad: u32, vpad: u32, big_endian: bool, row_bytes: u32) -> Vec<u8> {
    let row_bytes = row_bytes.max(fw * 2);
    let mut out = Vec::with_capacity((row_bytes * fh) as usize);
    for y in 0..fh {
        for x in 0..fw {
            let v = match (x.checked_sub(hpad), y.checked_sub(vpad)) {
                (Some(ix), Some(iy)) if ix < img.width() && iy < img.height() => rgb565(img.get_pixel(ix, iy).0),
                _ => 0,
            };
            out.extend_from_slice(&if big_endian { v.to_be_bytes() } else { v.to_le_bytes() });
        }
        out.resize(out.len() + (row_bytes - fw * 2) as usize, 0);
    }
    out
}

fn yuv(p: [u8; 3]) -> (u8, u8, u8) {
    let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (y as u8, u as u8, v as u8)
}

/// 余白を付けた全体 (fw×fh、余白は黒)
fn canvas(img: &RgbImage, fw: u32, fh: u32, hpad: u32, vpad: u32) -> RgbImage {
    let mut c = RgbImage::new(fw, fh);
    image::imageops::replace(&mut c, img, hpad as i64, vpad as i64);
    c
}

/// UYVY (libgpod の pack_UYVY と同じ。偶数の行を前半、奇数の行を後半に置き、色は 2 画素の左を使う)
fn pack_uyvy(c: &RgbImage) -> Vec<u8> {
    let (w, h) = c.dimensions();
    let mut out = vec![0u8; (w * h * 2) as usize];
    let half = out.len() / 2;
    let (mut even, mut odd) = (0usize, half);
    for y in 0..h {
        let at = if y % 2 == 0 { &mut even } else { &mut odd };
        for x in (0..w).step_by(2) {
            let (y0, u0, v0) = yuv(c.get_pixel(x, y).0);
            let (y1, _, _) = yuv(c.get_pixel((x + 1).min(w - 1), y).0);
            out[*at..*at + 4].copy_from_slice(&[u0, y0, v0, y1]);
            *at += 4;
        }
    }
    out
}

/// I420 (Y の面・U・V)。libgpod と同じく 2 バイト × 画素の大きさで書く (後ろは 0)
fn pack_i420(c: &RgbImage) -> Vec<u8> {
    let (w, h) = c.dimensions();
    let n = (w * h) as usize;
    let mut out = vec![0u8; n * 2];
    let (us, vs) = (n, n + n / 4);
    for (i, p) in c.pixels().enumerate() {
        let (y, u, v) = yuv(p.0);
        let (row, col) = (i / w as usize, i % w as usize);
        out[i] = y;
        let ci = (row / 2) * (w as usize / 2) + col / 2;
        out[us + ci] = u;
        out[vs + ci] = v;
    }
    out
}

fn rgb_from_yuv(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (c, d, e) = (y as i32 - 16, u as i32 - 128, v as i32 - 128);
    let clip = |x: i32| x.clamp(0, 255) as u8;
    [clip((298 * c + 409 * e + 128) >> 8), clip((298 * c - 100 * d - 208 * e + 128) >> 8), clip((298 * c + 516 * d + 128) >> 8)]
}

/// 縮めて詰めたサムネイル 1 枚分
pub struct Packed {
    pub data: Vec<u8>,
    /// 余白を含めた幅・高さ (mhni に書く値。libgpod と同じく「余白 + 画像」)
    pub width: u16,
    pub height: u16,
    pub hpad: u16,
    pub vpad: u16,
}

/// 写真を形式の大きさに収めて (縦横比はそのまま、余白は黒で中央に) 詰める
pub fn pack_fit(img: &DynamicImage, fmt: ArtFormat) -> Packed {
    let (w, h) = img.dimensions();
    let scale = (fmt.width as f64 / w as f64).min(fmt.height as f64 / h as f64);
    let sw = ((w as f64 * scale).ceil() as u32).clamp(1, fmt.width);
    let sh = ((h as f64 * scale).ceil() as u32).clamp(1, fmt.height);
    let scaled = img.resize_exact(sw, sh, FilterType::CatmullRom).to_rgb8();
    let (hpad, vpad) = ((fmt.width - sw) / 2, (fmt.height - sh) / 2);
    let data = match fmt.pix {
        Pix::Rgb565Le => pack_rgb565(&scaled, fmt.width, fmt.height, hpad, vpad, false, fmt.row_bytes()),
        Pix::Rgb565Be => pack_rgb565(&scaled, fmt.width, fmt.height, hpad, vpad, true, fmt.row_bytes()),
        Pix::Uyvy => pack_uyvy(&canvas(&scaled, fmt.width, fmt.height, hpad, vpad)),
        Pix::I420 => pack_i420(&canvas(&scaled, fmt.width, fmt.height, hpad, vpad)),
    };
    Packed { data, width: (hpad + sw) as u16, height: (vpad + sh) as u16, hpad: hpad as u16, vpad: vpad as u16 }
}

pub fn from_rgb565(data: &[u8], w: u32, h: u32) -> RgbImage {
    from_rgb565_endian(data, w, h, false, w * 2)
}

/// 1 行 `row_bytes` バイトの RGB565 を読む (右端の余りは読み飛ばす)
fn from_rgb565_endian(data: &[u8], w: u32, h: u32, big_endian: bool, row_bytes: u32) -> RgbImage {
    let mut img = RgbImage::new(w, h);
    let row_bytes = row_bytes.max(w * 2) as usize;
    for (i, px) in img.pixels_mut().enumerate() {
        let at = (i / w as usize) * row_bytes + (i % w as usize) * 2;
        let b2 = [data.get(at).copied().unwrap_or(0), data.get(at + 1).copied().unwrap_or(0)];
        let v = if big_endian { u16::from_be_bytes(b2) } else { u16::from_le_bytes(b2) };
        let r = ((v >> 11) & 0x1F) as u8;
        let g = ((v >> 5) & 0x3F) as u8;
        let b = (v & 0x1F) as u8;
        px.0 = [
            (r << 3) | (r >> 2),
            (g << 2) | (g >> 4),
            (b << 3) | (b >> 2),
        ];
    }
    img
}

fn from_uyvy(data: &[u8], w: u32, h: u32) -> RgbImage {
    let mut img = RgbImage::new(w, h);
    let half = data.len() / 2;
    let (mut even, mut odd) = (0usize, half);
    for y in 0..h {
        let at = if y % 2 == 0 { &mut even } else { &mut odd };
        for x in (0..w).step_by(2) {
            let q = data.get(*at..*at + 4).unwrap_or(&[128, 16, 128, 16]);
            img.put_pixel(x, y, image::Rgb(rgb_from_yuv(q[1], q[0], q[2])));
            if x + 1 < w {
                img.put_pixel(x + 1, y, image::Rgb(rgb_from_yuv(q[3], q[0], q[2])));
            }
            *at += 4;
        }
    }
    img
}

fn from_i420(data: &[u8], w: u32, h: u32) -> RgbImage {
    let n = (w * h) as usize;
    let (us, vs) = (n, n + n / 4);
    let mut img = RgbImage::new(w, h);
    for (i, px) in img.pixels_mut().enumerate() {
        let (row, col) = (i / w as usize, i % w as usize);
        let ci = (row / 2) * (w as usize / 2) + col / 2;
        let g = |o: usize| data.get(o).copied().unwrap_or(128);
        px.0 = rgb_from_yuv(data.get(i).copied().unwrap_or(16), g(us + ci), g(vs + ci));
    }
    img
}

/// ithmb ファイルに追記して (":F1055_1.ithmb", offset) を返す。
/// `prefix` は DB に書く場所の前置き (ArtworkDB は ":"、写真の DB は ":Thumbs:")
pub fn append_ithmb(dir: &Path, prefix: &str, format_id: u32, pixels: &[u8]) -> Result<(String, u32)> {
    fs::create_dir_all(dir)?;
    let mut idx = 1;
    while dir.join(format!("F{format_id}_{}.ithmb", idx + 1)).exists() {
        idx += 1;
    }
    let mut path = dir.join(format!("F{format_id}_{idx}.ithmb"));
    let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if len + pixels.len() as u64 > ITHMB_MAX {
        idx += 1;
        path = dir.join(format!("F{format_id}_{idx}.ithmb"));
    }
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    let offset = f.seek(SeekFrom::End(0))?;
    f.write_all(pixels)?;
    crate::device::sync_file(&f)?;
    Ok((format!("{prefix}F{format_id}_{idx}.ithmb"), offset as u32))
}

/// 1 つの形式の画像を、まだ無い番号の新しい ithmb (今ある番号の次から) に順に書く。
/// 大きさの違う画像が同じファイルに混ざらないように、形式の画像をまとめて書き直すときに使う
pub struct IthmbWriter<'a> {
    dir: &'a Path,
    prefix: &'a str,
    format_id: u32,
    idx: u32,
    file: Option<(fs::File, u64)>,
}

impl<'a> IthmbWriter<'a> {
    pub fn new(dir: &'a Path, prefix: &'a str, format_id: u32) -> Self {
        let mut idx = 1;
        while dir.join(format!("F{format_id}_{idx}.ithmb")).exists() {
            idx += 1;
        }
        IthmbWriter { dir, prefix, format_id, idx, file: None }
    }

    /// 1 枚書いて (":F1061_2.ithmb", offset) を返す
    pub fn push(&mut self, pixels: &[u8]) -> Result<(String, u32)> {
        if self.file.as_ref().is_some_and(|(_, len)| len + pixels.len() as u64 > ITHMB_MAX) {
            self.close()?;
            self.idx += 1;
        }
        if self.file.is_none() {
            fs::create_dir_all(self.dir)?;
            let path = self.dir.join(format!("F{}_{}.ithmb", self.format_id, self.idx));
            self.file = Some((OpenOptions::new().create(true).write(true).truncate(true).open(path)?, 0));
        }
        let (f, len) = self.file.as_mut().expect("直前で開いた");
        let offset = *len;
        f.write_all(pixels)?;
        *len += pixels.len() as u64;
        Ok((format!("{}F{}_{}.ithmb", self.prefix, self.format_id, self.idx), offset as u32))
    }

    /// 書いたファイルを確定する
    pub fn close(&mut self) -> Result<()> {
        if let Some((f, _)) = self.file.take() {
            crate::device::sync_file(&f)?;
        }
        Ok(())
    }
}

/// サムネイルの .ithmb の場所 (":F1055_1.ithmb" → <dir>/F1055_1.ithmb)。`dir` の外を指していれば None
pub(crate) fn thumb_path(dir: &Path, file: &str) -> Option<std::path::PathBuf> {
    crate::domain::devpath::join_inside(dir, file, crate::domain::devpath::IPOD_SEPS).filter(|p| p != dir)
}

/// サムネイルの位置と大きさがファイルに収まっているか (壊れた DB で何 GB も確保しないように)
pub(crate) fn thumb_fits(t: &Thumb, file_len: u64) -> bool {
    t.size <= MAX_THUMB_DIM * MAX_THUMB_DIM * 2 && t.offset as u64 + t.size as u64 <= file_len
}

/// サムネイルを読む。`dir` は DB の場所の基準 (ArtworkDB は Artwork、写真の DB は Photos)。余白は除く
pub fn read_thumb(dir: &Path, t: &Thumb) -> Result<RgbImage> {
    let path = thumb_path(dir, &t.file).ok_or_else(|| Error::Corrupt(t!("mhli が不正です")))?;
    let mut f = fs::File::open(path)?;
    if !thumb_fits(t, f.metadata()?.len()) {
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
    }
    f.seek(SeekFrom::Start(t.offset as u64))?;
    let mut buf = vec![0u8; t.size as usize];
    f.read_exact(&mut buf)?;
    // 形式の大きさで書いてあれば、その大きさで読んで余白を除く
    if let Some(fmt) = known_format(t.format_id).filter(|f| frame_size(f) == t.size || f.width * f.height * 2 == t.size) {
        let rb = if frame_size(&fmt) == t.size { fmt.row_bytes() } else { fmt.width * 2 };
        let full = match fmt.pix {
            Pix::Rgb565Le => from_rgb565_endian(&buf, fmt.width, fmt.height, false, rb),
            Pix::Rgb565Be => from_rgb565_endian(&buf, fmt.width, fmt.height, true, rb),
            Pix::Uyvy => from_uyvy(&buf, fmt.width, fmt.height),
            Pix::I420 => from_i420(&buf, fmt.width, fmt.height),
        };
        let (x0, y0) = (t.hpad as u32, t.vpad as u32);
        let (x1, y1) = ((t.width as u32).min(fmt.width), (t.height as u32).min(fmt.height));
        if x1 > x0 && y1 > y0 && (x0, y0, x1, y1) != (0, 0, fmt.width, fmt.height) {
            return Ok(image::imageops::crop_imm(&full, x0, y0, x1 - x0, y1 - y0).to_image());
        }
        return Ok(full);
    }
    // パディング付きで保存されている場合は実寸 (width+hpad) で読む
    let w = t.width as u32 + t.hpad as u32;
    let h = t.height as u32 + t.vpad as u32;
    let (w, h) = if w as u64 * h as u64 * 2 == t.size as u64 {
        (w, h)
    } else {
        (t.width as u32, t.height as u32)
    };
    if w > MAX_THUMB_DIM || h > MAX_THUMB_DIM {
        return Err(Error::Corrupt(t!("mhli が不正です")));
    }
    // 1 行が幅より長い (右端に余白がある) ときは、その行の長さで読む
    let rb = if h > 0 && t.size.is_multiple_of(h) && t.size / h > w * 2 { t.size / h } else { w * 2 };
    Ok(from_rgb565_endian(&buf, w, h, false, rb))
}

/// 画像バイト列から全フォーマットのサムネイルを作り ithmb に書き込む
pub fn build_image(
    dir: &Path,
    bytes: &[u8],
    formats: &[ArtFormat],
    id: u32,
    song_id: u64,
) -> Result<Image> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| Error::Media(t!("画像を読めません: {e}", e = e)))?;
    // 3000px 級の画像はフォーマットごとに縮小すると遅いので、先に 1000px 程度まで縮める
    let img = if img.width().max(img.height()) > 1000 {
        img.resize(1000, 1000, FilterType::Triangle)
    } else {
        img
    };
    let mut thumbs = Vec::new();
    for fmt in formats {
        let px = to_rgb565(&img, *fmt);
        let (file, offset) = append_ithmb(dir, ":", fmt.id, &px)?;
        thumbs.push(Thumb {
            format_id: fmt.id,
            file,
            offset,
            size: px.len() as u32,
            vpad: 0,
            hpad: 0,
            height: fmt.height as u16,
            width: fmt.width as u16,
        });
    }
    Ok(Image {
        hdr: new_mhii_header(),
        id,
        song_id,
        orig_size: bytes.len() as u32,
        thumbs,
        extra: Vec::new(),
    })
}

pub(crate) fn new_mhii_header() -> Vec<u8> {
    let mut hdr = vec![0u8; 0x98];
    hdr[0..4].copy_from_slice(b"mhii");
    wr32(&mut hdr, 4, 0x98);
    hdr
}

/// 画像内容ハッシュ → 登録済み画像。他のツール (iTunes 等) が ArtworkDB を書き直すと
/// id が振り直されるため、元画像サイズと song_id も一致したときだけ再利用する
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct IndexEntry {
    pub id: u32,
    pub size: u32,
    pub song: u64,
}

impl IndexEntry {
    pub fn valid_in(&self, db: &ArtworkDb) -> bool {
        db.image(self.id)
            .is_some_and(|i| i.orig_size == self.size && i.song_id == self.song)
    }
}

pub type HashIndex = HashMap<String, IndexEntry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_roundtrip() {
        let mut src = RgbImage::new(4, 2);
        for (i, p) in src.pixels_mut().enumerate() {
            p.0 = [(i * 30) as u8, 255 - (i * 20) as u8, 128];
        }
        let dynimg = DynamicImage::ImageRgb8(src.clone());
        let px = to_rgb565(&dynimg, f(1, 4, 2));
        assert_eq!(px.len(), 16);
        let back = from_rgb565(&px, 4, 2);
        for (a, b) in src.pixels().zip(back.pixels()) {
            for c in 0..3 {
                assert!((a.0[c] as i32 - b.0[c] as i32).abs() <= 8);
            }
        }
    }

    #[test]
    fn db_roundtrip() {
        let mut db = ArtworkDb::new_empty();
        db.files = CLASSIC.iter().map(|f| (f.id, frame_size(f))).collect();
        let mut hdr = vec![0u8; 0x98];
        hdr[0..4].copy_from_slice(b"mhii");
        db.images.push(Image {
            hdr,
            id: 100,
            song_id: 0x1122334455667788,
            orig_size: 12345,
            extra: Vec::new(),
            thumbs: vec![Thumb {
                format_id: 1055,
                file: ":F1055_1.ithmb".into(),
                offset: 32768,
                size: 32768,
                vpad: 0,
                hpad: 0,
                height: 128,
                width: 128,
            }],
        });
        let bytes = db.serialize();
        let back = ArtworkDb::parse(&bytes).unwrap();
        assert_eq!(back.images.len(), 1);
        assert_eq!(back.images[0].id, 100);
        assert_eq!(back.images[0].song_id, 0x1122334455667788);
        assert_eq!(back.images[0].thumbs[0].file, ":F1055_1.ithmb");
        assert_eq!(back.images[0].thumbs[0].offset, 32768);
        assert_eq!(back.files.len(), 4);
        assert_eq!(back.formats(&[]), CLASSIC.to_vec());
    }

    #[test]
    fn aligned_rows_roundtrip() {
        // iPod classic の 1061: 55 画素 (110 バイト) の行を 112 バイトにそろえる
        let fmt = CLASSIC[0];
        assert_eq!((fmt.id, fmt.row_bytes(), frame_size(&fmt)), (1061, 112, 6160));
        let src = RgbImage::from_pixel(80, 80, image::Rgb([200, 100, 50]));
        let px = to_rgb565(&DynamicImage::ImageRgb8(src), fmt);
        assert_eq!(px.len(), 6160);
        // 各行の最後の 2 バイトが余白
        assert!(px.chunks(112).all(|row| row[110..] == [0, 0] && row[..2] != [0, 0]));

        let dir = std::env::temp_dir().join(format!("zenpod-aligned-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (file, offset) = append_ithmb(&dir, ":", 1061, &px).unwrap();
        let t = Thumb { format_id: 1061, file, offset, size: 6160, vpad: 0, hpad: 0, height: 55, width: 55 };
        let back = read_thumb(&dir, &t).unwrap();
        assert_eq!(back.dimensions(), (55, 55));
        assert!(back.pixels().all(|p| (p.0[0] as i32 - 200).abs() <= 8 && (p.0[2] as i32 - 50).abs() <= 8));
        // 形式の分からない ID でも、行の長さから読む (右端の余白を画素にしない)
        let back = read_thumb(&dir, &Thumb { format_id: 9999, ..t }).unwrap();
        assert_eq!(back.dimensions(), (55, 55));
        assert!(back.pixels().all(|p| (p.0[0] as i32 - 200).abs() <= 8));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn device_formats_win_over_db() {
        let mut db = ArtworkDb::new_empty();
        // 以前の版が書いた 56×56 (6272 バイト) の 1061 は、本体の形式 (55×55・6160 バイト) に置き換える
        db.files = vec![(1061, 56 * 56 * 2), (1055, 32768)];
        assert_eq!(db.formats(CLASSIC), CLASSIC.to_vec());
        // iTunes が書いた 6160 バイトの 1061 は、本体の形式が分からなくても外さない (以前は外していた)
        db.files = vec![(1061, 6160), (1055, 32768)];
        assert_eq!(db.formats(&[]), vec![CLASSIC[0], CLASSIC[1]]);
        // 形式の分からない ID は、既存のサムネイルの寸法と行の長さから推定する
        db.files = vec![(2000, 112 * 55)];
        db.images.push(Image {
            hdr: new_mhii_header(),
            id: 100,
            song_id: 1,
            orig_size: 1,
            extra: Vec::new(),
            thumbs: vec![Thumb { format_id: 2000, file: ":F2000_1.ithmb".into(), offset: 0, size: 112 * 55, vpad: 0, hpad: 0, height: 55, width: 55 }],
        });
        let f = db.formats(&[]);
        assert_eq!((f[0].id, f[0].width, f[0].height, frame_size(&f[0])), (2000, 55, 55, 6160));
        // 寸法の分からない形式は書かない
        db.files = vec![(2001, 12345)];
        assert!(db.formats(&[]).is_empty());
    }
}
