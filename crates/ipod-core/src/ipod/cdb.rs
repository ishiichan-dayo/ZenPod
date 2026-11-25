//! iTunesCDB (iPod nano 5G 以降の、中身を zlib で縮めた iTunesDB)。
//!
//! mhbd のヘッダはそのまま、ヘッダより後ろを 1 つの zlib の流れにする。0xA8 の印を 1 にし、mhbd の 0x08 (全体の大きさ) を
//! 縮めたあとの大きさにする。署名は縮めたあとのバイト列にかける。形は libgpod 0.8.3 の itdb_zlib.c
//! (LGPL-2.1-or-later) に合わせた (圧縮の強さは 1。iTunes と同じ)。
//! iTunesCDB を書いたら iTunesDB は 0 バイトにする (両方あると本体が迷う)。

use std::io::{Read, Write};

use crate::error::{Error, Result};
use crate::t;

/// 0xA8 の印 (1 = 縮めてある) を置けるヘッダの大きさ
const MIN_HEADER: usize = 0xA9;
/// 展開したときの大きさの上限 (壊れたファイルで際限なくメモリを使わない)
const MAX_INFLATED: u64 = 1 << 30;

fn rd32(d: &[u8], at: usize) -> usize {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap()) as usize
}

fn header_len(data: &[u8]) -> Result<usize> {
    if data.len() < 12 || &data[0..4] != b"mhbd" {
        return Err(Error::Corrupt(t!("mhbd ヘッダがありません")));
    }
    let hlen = rd32(data, 4);
    if !(MIN_HEADER..=data.len()).contains(&hlen) {
        return Err(Error::Corrupt(t!("mhbd ヘッダ長が不正です")));
    }
    Ok(hlen)
}

/// 縮めた iTunesCDB を、ふつうの iTunesDB のバイト列に戻す
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let hlen = header_len(data)?;
    let end = rd32(data, 8).clamp(hlen, data.len());
    let mut out = data[..hlen].to_vec();
    flate2::read::ZlibDecoder::new(&data[hlen..end])
        .take(MAX_INFLATED)
        .read_to_end(&mut out)
        .map_err(|e| Error::Corrupt(t!("iTunesCDB を展開できません: {e}", e = e)))?;
    out[0xA8] = 0;
    let total = out.len() as u32;
    out[8..12].copy_from_slice(&total.to_le_bytes());
    Ok(out)
}

/// iTunesDB のバイト列を iTunesCDB の形に縮める (署名はこのあとにかける)
pub fn compress(db: &[u8]) -> Result<Vec<u8>> {
    let hlen = header_len(db)?;
    let end = rd32(db, 8).clamp(hlen, db.len());
    let mut enc = flate2::write::ZlibEncoder::new(db[..hlen].to_vec(), flate2::Compression::new(1));
    enc.write_all(&db[hlen..end])?;
    // ZlibEncoder は最初に渡したバイト列の後ろに書く
    let mut out = enc.finish()?;
    out[0xA8] = 1;
    let total = out.len() as u32;
    out[8..12].copy_from_slice(&total.to_le_bytes());
    Ok(out)
}

/// 0xA8 の印が立っている (縮めてある) か
pub fn is_compressed(data: &[u8]) -> bool {
    data.len() >= MIN_HEADER && &data[0..4] == b"mhbd" && rd32(data, 4) >= MIN_HEADER && data[0xA8] == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipod::itunesdb::{ITunesDb, MHOD_PATH, MHOD_TITLE, Track};

    #[test]
    fn round_trip() {
        let mut db = ITunesDb::new_empty("nano");
        for i in 0..50 {
            let mut t = Track::new(db.new_dbid());
            t.set_string(MHOD_TITLE, Some(&format!("曲 {i}")));
            t.set_string(MHOD_PATH, Some(&format!(":iPod_Control:Music:F00:S{i:03}.m4a")));
            db.playlists[0].items.push(t.dbid());
            db.tracks.push(t);
        }
        let plain = db.serialize();
        let packed = compress(&plain).unwrap();
        assert!(is_compressed(&packed) && !is_compressed(&plain));
        assert!(packed.len() < plain.len());
        assert_eq!(rd32(&packed, 8), packed.len());
        assert_eq!((&packed[..8], &packed[12..0xA8]), (&plain[..8], &plain[12..0xA8]), "大きさのほかはヘッダのまま");
        let back = decompress(&packed).unwrap();
        assert_eq!(back, plain);
        let again = ITunesDb::parse(&back).unwrap();
        assert_eq!(again.tracks.len(), 50);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decompress(b"mhbd").is_err());
        let mut db = ITunesDb::new_empty("nano").serialize();
        db[0xA8] = 1;
        let n = db.len();
        db.extend_from_slice(&[0xFF; 64]);
        let total = (n + 64) as u32;
        db[8..12].copy_from_slice(&total.to_le_bytes());
        assert!(decompress(&db).is_err());
    }
}
