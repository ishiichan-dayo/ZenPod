//! 曲の一覧のためにタグを読むとき、埋め込みの画像 (ジャケット) を読まずに済ませる。
//!
//! lofty は MP3 のタグ (ID3v2) を丸ごと読む。ジャケットはタグの中にあるので、画像を使わない設定にしても
//! 画像の分 (1 曲 0.5〜3MB) を読む。iPod の HDD では、初めて開くときにこれが曲の数だけ積み重なって遅い
//! (1000〜5000 曲で 30 秒以上かかった人がいた)。そこで:
//! - MP3: タグの枠の見出しだけを読みながら、画像の枠 (APIC) は飛ばし (中身は読まない)、残りの枠だけでタグを組み直して、
//!   音声の部分とつなぎ合わせた読み口を lofty に渡す
//! - FLAC: lofty の画像を読まない設定 (画像のブロックは飛ばして読まない)。画像があるかはブロックの見出しで調べる
//! - M4A: 箱 (atom) の見出しをたどって moov/udta/meta/ilst/covr を見つけ、covr を抜いて外側の箱の大きさを書き直した
//!   読み口を渡す (目次が音声の後ろにあるファイルでも、音声の部分は読まない)
//!
//! どちらも、画像があるか (has_picture) は見出しで分かる。画像そのものは表示するときに読む。
//! リリースの種類 (MEDIATYPE) も読んだタグから取り出す (`media::read_release_type` で読み直すと画像ごと読む)。
//! 少しでも普通でない形 (ID3v2.2・非同期化・拡張ヘッダー・壊れた枠) なら、今までの読み方 (`media::read_info`) に任せる。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use lofty::config::ParseOptions;

use crate::error::Result;
use crate::media;
use crate::media::MediaInfo;

/// 一覧のためのタグの読み取り。結果は `media::read_info` と同じ (has_picture も)
pub fn read_info(path: &Path) -> Result<MediaInfo> {
    read_info_release(path, false).map(|(info, _)| info)
}

/// `read_info` と、リリースの種類 (`media::read_release_type` と同じ値)。
/// `release` が false なら、今までの読み方に任せたファイルではリリースの種類を読まない (None)
pub fn read_info_release(path: &Path, release: bool) -> Result<(MediaInfo, Option<String>)> {
    media::lofty_options();
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let fast = match ext.as_str() {
        "mp3" => mp3(path),
        "flac" => flac(path),
        "m4a" | "m4b" | "mp4" => m4a(path),
        _ => None,
    };
    match fast {
        Some(r) => Ok(r),
        None => {
            let info = media::read_info(path)?;
            Ok((info, if release { media::read_release_type(path) } else { None }))
        }
    }
}

fn syncsafe(b: &[u8]) -> u32 {
    b.iter().take(4).fold(0, |a, &x| (a << 7) | (x as u32 & 0x7F))
}

fn to_syncsafe(n: u32) -> [u8; 4] {
    [((n >> 21) & 0x7F) as u8, ((n >> 14) & 0x7F) as u8, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]
}

/// 読み口の一部: メモリの中のバイト (書き直したタグ・箱の大きさ) か、ファイルの一部
pub(crate) enum Seg {
    Mem(Vec<u8>),
    File { start: u64, len: u64 },
}

impl Seg {
    fn len(&self) -> u64 {
        match self {
            Seg::Mem(v) => v.len() as u64,
            Seg::File { len, .. } => *len,
        }
    }
}

/// 部品を並べた読み口 (lofty にはファイルのように見える)
pub(crate) struct Virtual {
    segs: Vec<Seg>,
    file: File,
    pos: u64,
    len: u64,
}

impl Virtual {
    pub(crate) fn new(file: File, segs: Vec<Seg>) -> Self {
        let len = segs.iter().map(Seg::len).sum();
        Virtual { segs, file, pos: 0, len }
    }
}

impl Read for Virtual {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut base = 0u64;
        for seg in &self.segs {
            let l = seg.len();
            if self.pos < base + l {
                let off = self.pos - base;
                let max = (l - off).min(buf.len() as u64) as usize;
                let n = match seg {
                    Seg::Mem(v) => {
                        buf[..max].copy_from_slice(&v[off as usize..off as usize + max]);
                        max
                    }
                    Seg::File { start, .. } => {
                        self.file.seek(SeekFrom::Start(start + off))?;
                        self.file.read(&mut buf[..max])?
                    }
                };
                self.pos += n as u64;
                return Ok(n);
            }
            base += l;
        }
        Ok(0)
    }
}

impl Seek for Virtual {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => self.len as i64 + d,
        };
        if p < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

/// MP3: 画像の枠を抜いたタグで読む。普通でない形なら None (今までの読み方に任せる)
fn mp3(path: &Path) -> Option<(MediaInfo, Option<String>)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut h = [0u8; 10];
    f.read_exact(&mut h).ok()?;
    // ID3v2.3 / 2.4 だけ。非同期化 (0x80)・拡張ヘッダー (0x40)・フッター (0x10) があれば任せる
    if &h[0..3] != b"ID3" || !matches!(h[3], 3 | 4) || h[5] & 0xD0 != 0 {
        return None;
    }
    let major = h[3];
    let size = syncsafe(&h[6..10]) as u64;
    if 10 + size > len {
        return None;
    }
    let mut body = Vec::new();
    let mut ids = Vec::new();
    let mut has_picture = false;
    let mut off = 0u64;
    while off + 10 <= size {
        let mut fh = [0u8; 10];
        f.read_exact(&mut fh).ok()?;
        if fh[0] == 0 {
            break; // 埋め草 (padding)
        }
        if !fh[..4].iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return None;
        }
        let fsize = if major == 4 { syncsafe(&fh[4..8]) } else { u32::from_be_bytes([fh[4], fh[5], fh[6], fh[7]]) } as u64;
        if off + 10 + fsize > size {
            return None;
        }
        if &fh[..4] == b"APIC" {
            has_picture = true;
            f.seek(SeekFrom::Current(fsize as i64)).ok()?;
        } else {
            if fsize > 0 {
                ids.push(String::from_utf8_lossy(&fh[..4]).into_owned());
            }
            body.extend_from_slice(&fh);
            let start = body.len();
            body.resize(start + fsize as usize, 0);
            f.read_exact(&mut body[start..]).ok()?;
        }
        off += 10 + fsize;
    }
    if !has_picture {
        // 画像が無ければ、組み直さずに普通に読む (読む量は同じ)
        return None;
    }
    let mut head = Vec::with_capacity(10 + body.len());
    head.extend_from_slice(&[b'I', b'D', b'3', major, 0, 0]);
    head.extend_from_slice(&to_syncsafe(body.len() as u32));
    head.extend_from_slice(&body);
    let tail_start = 10 + size;
    let v = Virtual::new(f, vec![Seg::Mem(head), Seg::File { start: tail_start, len: len - tail_start }]);
    let mp = <lofty::mpeg::MpegFile as lofty::file::AudioFile>::read_from(&mut BufReader::new(v), ParseOptions::new()).ok()?;
    // lofty が黙って枠を捨てていたら (区切りを見失った)、今までの読み方 (読めない枠を飛ばす) に任せる
    if media::id3salvage::missing(major, &ids, mp.id3v2()) {
        return None;
    }
    let mediatype = media::mediatype_id3(&mp);
    let tf = mp.into();
    Some((media::info_from(&tf, path, has_picture), media::release_type_from(mediatype, &tf)))
}

/// FLAC: 画像のブロックを飛ばして読む。画像があるかはブロックの見出しで調べる
fn flac(path: &Path) -> Option<(MediaInfo, Option<String>)> {
    let mut f = File::open(path).ok()?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    if &magic != b"fLaC" {
        return None;
    }
    let mut has_picture = false;
    loop {
        let mut bh = [0u8; 4];
        f.read_exact(&mut bh).ok()?;
        let last = bh[0] & 0x80 != 0;
        let kind = bh[0] & 0x7F;
        let blen = u32::from_be_bytes([0, bh[1], bh[2], bh[3]]) as i64;
        if kind == 6 {
            has_picture = true;
            break;
        }
        if last {
            break;
        }
        f.seek(SeekFrom::Current(blen)).ok()?;
    }
    f.seek(SeekFrom::Start(0)).ok()?;
    let file = <lofty::flac::FlacFile as lofty::file::AudioFile>::read_from(&mut BufReader::new(f), ParseOptions::new().read_cover_art(false)).ok()?;
    let mediatype = file.vorbis_comments().and_then(|c| c.get("MEDIATYPE")).map(str::to_string);
    let tf = file.into();
    Some((media::info_from(&tf, path, has_picture), media::release_type_from(mediatype, &tf)))
}

/// 箱の見出し: (始まり, 大きさ, 種類)。大きさが 64 ビット・「最後まで」(0) の箱は扱わない (None)
pub(crate) fn atom(f: &mut File, at: u64, end: u64) -> Option<(u64, u64, [u8; 4])> {
    if at + 8 > end {
        return None;
    }
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut h = [0u8; 8];
    f.read_exact(&mut h).ok()?;
    let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
    if size < 8 || at + size > end {
        return None;
    }
    Some((at, size, [h[4], h[5], h[6], h[7]]))
}

/// `start..end` の中の、種類が `kind` の箱 (最初の 1 つ)
pub(crate) fn find_atom(f: &mut File, start: u64, end: u64, kind: &[u8; 4]) -> Option<(u64, u64)> {
    let mut at = start;
    while let Some((a, size, k)) = atom(f, at, end) {
        if &k == kind {
            return Some((a, size));
        }
        at = a + size;
    }
    None
}

/// M4A: ジャケットの箱 (covr) を抜いた読み口で読む。普通でない形なら None
fn m4a(path: &Path) -> Option<(MediaInfo, Option<String>)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let (moov, moov_size) = find_atom(&mut f, 0, len, b"moov")?;
    let (udta, udta_size) = find_atom(&mut f, moov + 8, moov + moov_size, b"udta")?;
    let (meta, meta_size) = find_atom(&mut f, udta + 8, udta + udta_size, b"meta")?;
    // meta は版と印の 4 バイトの後に子の箱が続く (付いていない作りもある)
    let mut vf = [0u8; 4];
    f.seek(SeekFrom::Start(meta + 8)).ok()?;
    f.read_exact(&mut vf).ok()?;
    let kids = if vf == [0, 0, 0, 0] { meta + 12 } else { meta + 8 };
    let (ilst, ilst_size) = find_atom(&mut f, kids, meta + meta_size, b"ilst")?;
    let mut covr = Vec::new();
    let mut at = ilst + 8;
    while let Some((a, size, k)) = atom(&mut f, at, ilst + ilst_size) {
        if &k == b"covr" {
            covr.push((a, size));
        }
        at = a + size;
    }
    if covr.is_empty() {
        return None;
    }
    let removed: u64 = covr.iter().map(|(_, s)| s).sum();
    // 書き直す箱の大きさ (4 バイト) と、抜く範囲を、ファイルの位置の順に並べて部品にする
    let mut cuts: Vec<(u64, u64, Option<Vec<u8>>)> = [(moov, moov_size), (udta, udta_size), (meta, meta_size), (ilst, ilst_size)]
        .iter()
        .map(|&(a, size)| (a, 4, Some(((size - removed) as u32).to_be_bytes().to_vec())))
        .collect();
    cuts.extend(covr.iter().map(|&(a, size)| (a, size, None)));
    cuts.sort_by_key(|c| c.0);
    let mut segs = Vec::new();
    let mut pos = 0u64;
    for (at, size, bytes) in cuts {
        if at > pos {
            segs.push(Seg::File { start: pos, len: at - pos });
        }
        if let Some(b) = bytes {
            segs.push(Seg::Mem(b));
        }
        pos = at + size;
    }
    if pos < len {
        segs.push(Seg::File { start: pos, len: len - pos });
    }
    let v = Virtual::new(f, segs);
    let mp4 = <lofty::mp4::Mp4File as lofty::file::AudioFile>::read_from(&mut BufReader::new(v), ParseOptions::new()).ok()?;
    // lofty が黙って項目を捨てていたら、今までの読み方 (読めない項目を飛ばす) に任せる。ジャケットは抜いてあるので比べない
    if media::mp4_unread_items(&mut File::open(path).ok()?, mp4.ilst(), false) > 0 {
        return None;
    }
    let mediatype = media::mediatype_mp4(&mp4);
    let tf = mp4.into();
    Some((media::info_from(&tf, path, true), media::release_type_from(mediatype, &tf)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ffmpeg がある環境だけ: 画像入りの MP3 / FLAC で、今までの読み方と同じ結果になる
    #[test]
    fn same_as_full_read() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let cover = dir.path().join("c.png");
        image::RgbImage::from_pixel(300, 300, image::Rgb([10, 200, 30])).save(&cover).unwrap();
        for (ext, codec) in [("mp3", "libmp3lame"), ("flac", "flac"), ("m4a", "aac")] {
            for with_art in [true, false] {
                let out = dir.path().join(format!("a{with_art}.{ext}"));
                let mut cmd = std::process::Command::new(&ff);
                cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"]);
                if with_art {
                    cmd.arg("-i").arg(&cover).args(["-map", "0", "-map", "1", "-c:v", "copy", "-disposition:v", "attached_pic"]);
                }
                cmd.args(["-c:a", codec, "-id3v2_version", "3", "-metadata", "title=曲名", "-metadata", "artist=アーティスト", "-metadata", "album=アルバム", "-metadata", "track=7", "-metadata", "date=2024", "-metadata", "RELEASETYPE=EP"]).arg(&out);
                assert!(cmd.status().unwrap().success());
                let a = media::read_info(&out).unwrap();
                let b = read_info(&out).unwrap();
                assert_eq!(
                    (a.title, a.artist, a.album, a.track_nr, a.year, a.duration_ms, a.has_picture, a.bitrate_kbps),
                    (b.title, b.artist, b.album, b.track_nr, b.year, b.duration_ms, b.has_picture, b.bitrate_kbps),
                    "{ext} art={with_art}"
                );
                assert_eq!(b.has_picture, with_art);
                // リリースの種類 (MusicBrainz の RELEASETYPE) も読み直したときと同じ
                assert_eq!(read_info_release(&out, true).unwrap().1, media::read_release_type(&out), "{ext} art={with_art}");
            }
        }
    }
}
