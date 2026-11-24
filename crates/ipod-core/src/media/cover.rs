//! ジャケットの画像: 曲のファイルや同じフォルダから探す・一覧用に縮小する (iPod の ArtworkDB とは関係ない)

use std::fs;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;

/// 一覧表示用の小さな正方形の JPEG (中央を切り抜き)。読めない画像は None
pub fn thumbnail_jpeg(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let thumb = img.resize_to_fill(size, size, FilterType::Triangle).to_rgb8();
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&thumb)
        .ok()?;
    Some(out.into_inner())
}

/// 1 回の読み込みで複数の大きさの正方形 JPEG を作る (中央を切り抜き)。読めない画像は None。
/// 元画像のデコードと縮小が重いので、同じ画像から何通りも作るときはこちらを使う
pub fn thumbnails_jpeg(bytes: &[u8], sizes: &[u32]) -> Option<Vec<Vec<u8>>> {
    let img = image::load_from_memory(bytes).ok()?;
    let largest = sizes.iter().copied().max()?;
    // 3000px 級の画像をそのまま何度も縮小すると遅いので、先に必要な大きさの 2 倍程度まで縮める
    let img = if img.width().min(img.height()) > largest * 2 {
        img.resize(largest * 2, largest * 2, FilterType::Triangle)
    } else {
        img
    };
    sizes
        .iter()
        .map(|&size| {
            let thumb = img.resize_to_fill(size, size, FilterType::Triangle).to_rgb8();
            let mut out = std::io::Cursor::new(Vec::new());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
                .encode_image(&thumb)
                .ok()?;
            Some(out.into_inner())
        })
        .collect()
}

/// 音声ファイル埋め込みのジャケット → 同じフォルダの cover.jpg 等の順で探す
pub fn find_cover(src: &Path) -> Option<Vec<u8>> {
    find_cover_src(src).map(|(bytes, _)| bytes)
}

/// `find_cover` と同じ。2 つ目は曲のファイルに埋め込まれた画像か (false = 同じフォルダの画像。
/// 曲のファイルが変わらなくても差し替わることがある)
pub fn find_cover_src(src: &Path) -> Option<(Vec<u8>, bool)> {
    use lofty::picture::PictureType;
    use lofty::prelude::*;
    // 曲の情報と同じ読み方で読む (読めない枠・項目を飛ばす、拡張子と中身が違えば緩めて読む)
    if let Ok(tf) = super::read_any(src) {
        let pics: Vec<_> = tf.tags().iter().flat_map(|t| t.pictures()).collect();
        let pick = pics
            .iter()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| pics.first());
        if let Some(p) = pick {
            return Some((p.data().to_vec(), true));
        }
    }
    let dir = src.parent()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            continue;
        }
        let stem = p
            .file_stem()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let rank = [
            "cover",
            "folder",
            "front",
            "album",
            "albumart",
            "albumartlarge",
        ]
        .iter()
        .position(|n| stem == *n);
        if let Some(r) = rank
            && best.as_ref().is_none_or(|(br, _)| r < *br)
        {
            best = Some((r, p));
        }
    }
    fs::read(best?.1).ok().map(|b| (b, false))
}

/// 埋め込みのジャケットを縮める大きさ (長い辺) の既定。Rockbox の cover.jpg と同じ。
/// iPod classic の画面 (320×240) より大きく、テーマが大きく出しても足りる
pub const EMBED_MAX: u32 = 500;

/// 画面で選んだ縮める大きさを、使える範囲に収める (無ければ既定)
pub fn embed_size(px: Option<u32>) -> u32 {
    px.unwrap_or(EMBED_MAX).clamp(64, 3000)
}

/// 長い辺を `max` までに縮め (切り抜かない)、ベースラインの JPEG にする。読めない画像は None。
/// Rockbox はプログレッシブ JPEG を読めないので、Rockbox に置く画像はこれで作る
pub fn fit_jpeg(bytes: &[u8], max: u32) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width().max(img.height()) > max {
        img.resize(max, max, FilterType::CatmullRom)
    } else {
        img
    };
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&img.to_rgb8())
        .ok()?;
    Some(out.into_inner())
}

/// 画像の幅と高さ (見出しだけを読む。読めなければ None)
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    imagesize::blob_size(bytes).ok().map(|s| (s.width as u32, s.height as u32))
}

/// 作り直さなくてよい画像か: ベースラインの JPEG で、長い辺が `max` 以下
pub fn fits(bytes: &[u8], max: u32) -> bool {
    matches!(jpeg_frame(bytes), Some(0xC0 | 0xC1))
        && imagesize::blob_size(bytes).is_ok_and(|s| s.width.max(s.height) <= max as usize)
}

/// JPEG のフレームの種類 (SOF のマーカー。0xC0 / 0xC1 = ベースライン、0xC2 = プログレッシブなど)。JPEG でなければ None
fn jpeg_frame(b: &[u8]) -> Option<u8> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let m = b[i + 1];
        match m {
            // 埋め草
            0xFF => i += 1,
            // 長さの無いマーカー
            0x01 | 0xD0..=0xD7 => i += 2,
            // SOF (0xC4 = DHT・0xC8 = JPG・0xCC = DAC は除く)
            0xC0..=0xCF if !matches!(m, 0xC4 | 0xC8 | 0xCC) => return Some(m),
            // 画像の始まり・終わりより前に SOF が無い
            0xD9 | 0xDA => return None,
            _ => i += 2 + u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize,
        }
    }
    None
}

/// 画像内容の SHA-1 (同一画像の共有判定用)
pub fn content_key(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32, progressive: bool) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30]));
        if progressive {
            // image のエンコーダはプログレッシブを書けないので、見出しの SOF0 を SOF2 に書き換えたものを使う
            let mut b = jpeg(w, h, false);
            let at = b.windows(2).position(|x| x == [0xFF, 0xC0]).unwrap();
            b[at + 1] = 0xC2;
            return b;
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img).unwrap();
        out.into_inner()
    }

    #[test]
    fn fits_only_small_baseline_jpeg() {
        assert!(fits(&jpeg(500, 400, false), 500));
        assert!(!fits(&jpeg(501, 400, false), 500));
        assert!(!fits(&jpeg(300, 300, true), 500), "プログレッシブ");
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(10, 10, image::Rgb([1, 2, 3])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        assert!(!fits(png.get_ref(), 500), "JPEG でない");
        // 縮めた画像は条件に合う
        let small = fit_jpeg(&jpeg(1200, 900, false), 500).unwrap();
        assert!(fits(&small, 500));
        assert_eq!(imagesize::blob_size(&small).map(|s| (s.width, s.height)).unwrap(), (500, 375));
    }
}
