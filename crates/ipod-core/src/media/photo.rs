//! 写真のファイル: 読み込み (EXIF の向きに合わせて回す)・撮った日時・同じ写真かの印・JPEG への書き出し

use crate::t;
use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageReader};

use crate::error::{Error, Result};

/// 読む写真の縦横の上限。横長のパノラマも通るよう広めにし、全体の大きさは下で抑える
const MAX_SIDE: u32 = 32768;
/// 読み込みに使ってよいメモリ (2 億画素のスマホの写真 (RGB で約 600MB) が入る程度)
const MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// 写真として扱う拡張子 (HEIC などは ffmpeg があれば読める)
const PHOTO_EXTS: &[&str] = &["jpg", "jpeg", "png", "bmp", "gif", "webp", "heic", "heif", "tif", "tiff"];

pub fn is_photo_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| PHOTO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

pub struct LoadedPhoto {
    /// 向きを直した画像
    pub image: DynamicImage,
    /// 撮った日時 (EXIF の DateTimeOriginal を UTC として読んだ UNIX 秒。無ければファイルの更新日時)
    pub date: i64,
    /// 元のファイルの大きさ
    pub size: u64,
    /// 中身の印 (同じ写真を 2 回送らないため)
    pub key: String,
}

/// 写真を読む。image で読めない形式 (HEIC など) は ffmpeg で PNG にしてから読む
pub fn load(path: &Path, ffmpeg: Option<&Path>) -> Result<LoadedPhoto> {
    let bytes = std::fs::read(path)?;
    let key = super::cover::content_key(&bytes);
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (image, exif) = match decode(&bytes)? {
        Some(x) => x,
        None => {
            let png = ffmpeg
                .and_then(|ff| super::video::decode_image_with_ffmpeg(ff, path))
                .ok_or_else(|| Error::Media(t!("画像を読めません: {e}", e = path.extension().and_then(|e| e.to_str()).unwrap_or("?"))))?;
            let (img, _) = decode(&png)?.ok_or_else(|| Error::Media(t!("画像を読めません: {e}", e = "png")))?;
            (img, find_exif(&bytes))
        }
    };
    let date = exif.as_deref().and_then(exif_date).unwrap_or(mtime);
    Ok(LoadedPhoto { image, date, size: bytes.len() as u64, key })
}

/// 画像を読んで EXIF の向きに回す。(画像, EXIF)。読めない形式は None、大きすぎる画像はエラー
fn decode(bytes: &[u8]) -> Result<Option<(DynamicImage, Option<Vec<u8>>)>> {
    let too_large = |e: String| Error::Media(t!("画像を読めません: {e}", e = e));
    let Ok(mut reader) = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format() else { return Ok(None) };
    // into_decoder + from_decoder は image の既定のメモリの上限を通らないので、ここで決める。
    // 縦横が見出しだけ巨大な画像 (数百バイトで数十 GB を確保させる) を読む前に止める
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_BYTES);
    reader.limits(limits);
    let mut dec = match reader.into_decoder() {
        Ok(d) => d,
        Err(e @ image::ImageError::Limits(_)) => return Err(too_large(e.to_string())),
        Err(_) => return Ok(None),
    };
    let (w, h) = dec.dimensions();
    if w > MAX_SIDE || h > MAX_SIDE || dec.total_bytes() > MAX_BYTES {
        return Err(too_large(format!("{w}×{h}")));
    }
    let orientation = dec.orientation().ok();
    let exif = dec.exif_metadata().ok().flatten();
    let mut img = match DynamicImage::from_decoder(dec) {
        Ok(i) => i,
        Err(e @ image::ImageError::Limits(_)) => return Err(too_large(e.to_string())),
        Err(_) => return Ok(None),
    };
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(Some((img, exif)))
}

/// image が読めない形式のファイルから EXIF を探す ("Exif\0\0" の後ろ)
fn find_exif(bytes: &[u8]) -> Option<Vec<u8>> {
    let at = bytes.windows(6).take(1 << 20).position(|w| w == b"Exif\0\0")?;
    Some(bytes[at + 6..].iter().take(64 * 1024).copied().collect())
}

/// EXIF (TIFF の形) から撮った日時 (DateTimeOriginal、無ければ DateTime) を読む
pub fn exif_date(exif: &[u8]) -> Option<i64> {
    let e = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let le = match e.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| e.get(o..o + 2).map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |o: usize| e.get(o..o + 4).map(|b| if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) });
    // IFD の中の項目 (tag) の値の位置 (4 バイトに入らない値は場所を指す)
    let entry = |ifd: usize, tag: u16| -> Option<(u16, u32, usize)> {
        let n = u16_at(ifd)? as usize;
        (0..n.min(512)).find_map(|i| {
            let at = ifd + 2 + i * 12;
            (u16_at(at)? == tag).then(|| Some((u16_at(at + 2)?, u32_at(at + 4)?, at + 8))).flatten()
        })
    };
    let ascii = |(ty, count, at): (u16, u32, usize)| -> Option<String> {
        if ty != 2 {
            return None;
        }
        let off = if count > 4 { u32_at(at)? as usize } else { at };
        let raw = e.get(off..off + count as usize)?;
        Some(String::from_utf8_lossy(raw).trim_end_matches('\0').to_string())
    };
    let ifd0 = u32_at(4)? as usize;
    let original = entry(ifd0, 0x8769)
        .and_then(|(_, _, at)| u32_at(at))
        .and_then(|sub| entry(sub as usize, 0x9003))
        .and_then(ascii);
    let s = original.or_else(|| entry(ifd0, 0x0132).and_then(ascii))?;
    let dt = chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y:%m:%d %H:%M:%S").ok()?;
    Some(dt.and_utc().timestamp())
}

/// JPEG にする (品質は 0〜100)
pub fn to_jpeg(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&img.to_rgb8()).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DateTimeOriginal を持つ小さな EXIF (TIFF, little endian)
    fn exif_with_date(date: &str) -> Vec<u8> {
        let mut v = b"II*\0".to_vec();
        v.extend_from_slice(&8u32.to_le_bytes()); // IFD0
        // IFD0: 1 項目 (ExifIFD の場所)
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&0x8769u16.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&26u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        // ExifIFD (26): 1 項目
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&0x9003u16.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&20u32.to_le_bytes());
        v.extend_from_slice(&44u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(date.as_bytes());
        v.push(0);
        v
    }

    #[test]
    fn reads_exif_date() {
        let e = exif_with_date("2024:05:06 07:08:09");
        assert_eq!(exif_date(&e), Some(1714979289));
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend_from_slice(&e);
        assert_eq!(exif_date(&prefixed), Some(1714979289));
        assert_eq!(exif_date(b"nope"), None);
    }

    #[test]
    fn loads_and_keys() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        image::RgbImage::from_pixel(30, 20, image::Rgb([200, 10, 10])).save(&p).unwrap();
        let l = load(&p, None).unwrap();
        assert_eq!((l.image.width(), l.image.height()), (30, 20));
        assert!(l.date > 0);
        let q = d.path().join("b.png");
        std::fs::copy(&p, &q).unwrap();
        assert_eq!(load(&q, None).unwrap().key, l.key);
        assert!(is_photo_file(&p) && !is_photo_file(&d.path().join("a.mp3")));
    }
}
