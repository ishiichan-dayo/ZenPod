//! M4A のタグの後ろに残った大きな余白 (`free` の箱) を詰める。
//!
//! lofty はタグ (ilst) が縮むと、音声の位置の表 (stco / co64) を直さずに済むよう、縮んだ分を必ず `free` の箱で埋める。
//! 文字の編集なら数バイトだが、埋め込みのジャケットを縮める・外すと、元の画像の大きさの分 (数 MB) が残り、
//! 容量が減らない (配布リポジトリの Issue #24)。余白が `MAX_SLACK` を超えたら、moov.udta.meta (と udta) の中の
//! `free` / `skip` を外し、親の箱の大きさと、moov の後ろにある音声の位置を直して書き直す。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::media::MAX_SLACK;

/// 読み込む moov の上限 (これより大きいのは普通の曲ではないので触らない)
const MOOV_MAX: u64 = 64 * 1024 * 1024;

/// 余白が大きければ詰める。詰めたら true。扱えない形 (64 ビットの大きさ・断片化した MP4 など) はそのままにする
pub(crate) fn compact(f: &mut File) -> std::io::Result<bool> {
    let len = f.metadata()?.len();
    // 先頭の階層の箱
    let mut moov = None;
    let mut off = 0u64;
    while off + 8 <= len {
        let mut h = [0u8; 8];
        f.seek(SeekFrom::Start(off))?;
        f.read_exact(&mut h)?;
        let size = u64::from(u32::from_be_bytes(h[..4].try_into().unwrap()));
        let size = match size {
            0 => len - off,
            // 64 ビットの大きさの箱 (大きい mdat) は飛ばすだけ
            1 => {
                let mut b = [0u8; 8];
                f.read_exact(&mut b)?;
                u64::from_be_bytes(b)
            }
            s => s,
        };
        if size < 8 || off + size > len {
            return Ok(false);
        }
        match &h[4..8] {
            b"moov" if size <= MOOV_MAX && h[..4] != [0, 0, 0, 1] => moov = Some((off, size)),
            // 断片化した MP4 は音声の位置を moof が持つので触らない
            b"moof" => return Ok(false),
            _ => {}
        }
        off += size;
    }
    let Some((moov_off, moov_size)) = moov else { return Ok(false) };
    let mut buf = vec![0u8; moov_size as usize];
    f.seek(SeekFrom::Start(moov_off))?;
    f.read_exact(&mut buf)?;
    let Some(new) = strip(&buf) else { return Ok(false) };
    let removed = moov_size - new.len() as u64;
    if removed <= MAX_SLACK {
        return Ok(false);
    }
    let mut new = new;
    // moov の後ろにある音声は、外した分だけ前へずれる
    let moov_end = moov_off + moov_size;
    if crate::media::convert::patch_chunk_offsets(&mut new, &|o| if o >= moov_end { o - removed } else { o }).is_err() {
        return Ok(false);
    }
    let mut rest = Vec::new();
    f.seek(SeekFrom::Start(moov_end))?;
    f.read_to_end(&mut rest)?;
    f.seek(SeekFrom::Start(moov_off))?;
    f.write_all(&new)?;
    f.write_all(&rest)?;
    let end = f.stream_position()?;
    f.set_len(end)?;
    Ok(true)
}

/// moov から udta と udta.meta の中の `free` / `skip` を外した moov。外すものが無い・扱えない形なら None
fn strip(moov: &[u8]) -> Option<Vec<u8>> {
    let kids = children(moov, 8)?;
    let mut out = moov[..8].to_vec();
    let mut changed = false;
    for (start, end) in kids {
        let b = &moov[start..end];
        if &b[4..8] == b"udta" {
            let udta = strip_udta(b)?;
            changed |= udta.len() != b.len();
            out.extend(udta);
        } else {
            out.extend_from_slice(b);
        }
    }
    if !changed {
        return None;
    }
    set_size(&mut out)?;
    Some(out)
}

fn strip_udta(udta: &[u8]) -> Option<Vec<u8>> {
    let mut out = udta[..8].to_vec();
    for (start, end) in children(udta, 8)? {
        let b = &udta[start..end];
        match &b[4..8] {
            b"free" | b"skip" => {}
            b"meta" => {
                // ISO の形は見出しの後ろに版と印の 4 バイトがある。QuickTime の形は無い (見出しのすぐ後ろが hdlr)
                let head = if b.get(12..16) == Some(b"hdlr") { 8 } else { 12 };
                let mut meta = b[..head].to_vec();
                for (s, e) in children(b, head)? {
                    if !matches!(&b[s + 4..s + 8], b"free" | b"skip") {
                        meta.extend_from_slice(&b[s..e]);
                    }
                }
                set_size(&mut meta)?;
                out.extend(meta);
            }
            _ => out.extend_from_slice(b),
        }
    }
    set_size(&mut out)?;
    Some(out)
}

/// 箱 `b` の `head` バイト目から並ぶ子の箱の範囲。64 ビットの大きさ・大きさ 0・はみ出しがあれば None
fn children(b: &[u8], head: usize) -> Option<Vec<(usize, usize)>> {
    if b.len() < head || b.get(..4)? == [0, 0, 0, 1] {
        return None;
    }
    let mut v = Vec::new();
    let mut off = head;
    while off < b.len() {
        let size = u32::from_be_bytes(b.get(off..off + 4)?.try_into().ok()?) as usize;
        if size < 8 || off + size > b.len() {
            return None;
        }
        v.push((off, off + size));
        off += size;
    }
    Some(v)
}

fn set_size(b: &mut [u8]) -> Option<()> {
    let n = u32::try_from(b.len()).ok()?;
    b[..4].copy_from_slice(&n.to_be_bytes());
    Some(())
}
