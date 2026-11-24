//! タグの構造 (枠・箱・ブロックの名前と大きさ) を書き出す。「詳細情報」の画面用。
//!
//! タグを読めない曲の報告で、曲のファイルそのものを送ってもらわずに原因を調べるため (配布リポジトリの Issue #7)。
//! 音声・タグの文字・画像の中身は出さない。出すのは名前・大きさ・ID3 の版とフラグ・文字コードの番号
//! (UTF-16 なら BOM)・音声の始まりの 4 バイト (MP3 のフレームの見出し) だけ。
//! 見出しを数バイトずつ読むだけなので、大きなファイルでも速い。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// 書き出す行の上限 (壊れたファイルで延々と続かないように)
const MAX_LINES: usize = 500;
/// 中に箱を持つ MP4 の箱 (ilst は別に読む)
const MP4_CONTAINERS: &[&[u8; 4]] = &[b"moov", b"udta", b"meta", b"trak", b"mdia", b"minf", b"stbl"];
/// ファイルの先頭がこれなら MP4
const MP4_TOP: &[&[u8; 4]] = &[b"ftyp", b"moov", b"mdat", b"wide", b"free", b"skip"];

// tr: 訳さない (この節の文はすべて、報告に貼ってもらう診断用の出力)

pub fn tag_structure(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut out = vec![format!("{len} bytes")];
    let head = read_at(&mut f, 0, 12);
    if head.len() >= 8 && MP4_TOP.iter().any(|t| head[4..8] == t[..]) {
        mp4(&mut f, &mut out, 0, len, 0);
        // 先頭からたどって moov に届かなかったら、ファイルの中を探す (壊れた部分の後ろに残っているか)
        if !out.iter().any(|l| l.starts_with("moov ")) {
            let from = crate::media::moovscan::ftyp_len(&mut f, len).unwrap_or(0);
            out.push(match crate::media::moovscan::find_moov(&mut f, from, len) {
                Some((at, size)) => format!("moov found by searching: at {at}, size {size}"),
                None => "no moov anywhere in the file".into(),
            });
        }
    } else {
        id3_and_rest(&mut f, &mut out, len);
    }
    if out.len() >= MAX_LINES {
        out.truncate(MAX_LINES);
        out.push("...".into());
    }
    Ok(out.join("\n"))
}

/// `pos` から `n` バイト (ファイルの終わりや読み取りの失敗では短くなる)
fn read_at(f: &mut File, pos: u64, n: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(n);
    if f.seek(SeekFrom::Start(pos)).is_ok() {
        let _ = f.by_ref().take(n as u64).read_to_end(&mut buf);
    }
    buf
}

fn be32(b: &[u8]) -> u64 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64
}

fn syncsafe(b: &[u8]) -> u64 {
    b.iter().take(4).fold(0, |a, &x| (a << 7) | (x as u64 & 0x7F))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 箱・枠の名前。読める文字ならそのまま (MP4 の © は "(c)")、それ以外は 16 進
fn name(b: &[u8]) -> String {
    let printable = |x: &u8| (0x20..0x7f).contains(x);
    match b.split_first() {
        Some((0xA9, rest)) if rest.iter().all(printable) => format!("(c){}", String::from_utf8_lossy(rest)),
        _ if b.iter().all(printable) => String::from_utf8_lossy(b).into_owned(),
        _ => format!("0x{}", hex(b)),
    }
}

fn mp4(f: &mut File, out: &mut Vec<String>, mut pos: u64, end: u64, depth: usize) {
    while pos + 8 <= end && out.len() < MAX_LINES {
        let h = read_at(f, pos, 16);
        if h.len() < 8 {
            break;
        }
        let ty = &h[4..8];
        let (size, hdr) = match be32(&h) {
            1 if h.len() >= 16 => (u64::from_be_bytes(h[8..16].try_into().unwrap()), 16),
            0 => (end - pos, 8),
            n => (n, 8),
        };
        let over = if pos.saturating_add(size) > end { "  <-- goes past its parent" } else { "" };
        // 大きさを 64 ビットで書いた箱 (lofty 0.25.4 はこの箱を飛ばすと 8 バイト行き過ぎた。配布リポジトリの Issue #13)
        let wide = if hdr == 16 { " (64-bit size)" } else { "" };
        out.push(format!("{}{} {size}{wide}{over}", "  ".repeat(depth), name(ty)));
        if size < hdr {
            break;
        }
        let inner_end = pos.saturating_add(size).min(end);
        let mut inner = pos + hdr;
        // meta は版とフラグの 4 バイトの後に箱が続く (QuickTime の形は続かない)
        if ty == b"meta" {
            let v = read_at(f, inner, 8);
            if v.len() == 8 && &v[4..8] != b"hdlr" {
                inner += 4;
            }
        }
        if ty == b"ilst" {
            ilst(f, out, inner, inner_end, depth + 1);
        } else if depth < 8 && MP4_CONTAINERS.iter().any(|c| ty == &c[..]) {
            mp4(f, out, inner, inner_end, depth + 1);
        }
        pos = pos.saturating_add(size);
    }
}

/// ilst の項目: 名前と大きさ、中の箱 (data は種類の番号も)
fn ilst(f: &mut File, out: &mut Vec<String>, mut pos: u64, end: u64, depth: usize) {
    while pos + 8 <= end && out.len() < MAX_LINES {
        let h = read_at(f, pos, 8);
        if h.len() < 8 {
            break;
        }
        let size = be32(&h);
        if size < 8 {
            out.push(format!("{}{} {size}  <-- too small", "  ".repeat(depth), name(&h[4..8])));
            break;
        }
        let item_end = pos.saturating_add(size);
        let mut kids = Vec::new();
        let mut p = pos + 8;
        while p + 8 <= item_end.min(end) && kids.len() < 32 {
            let k = read_at(f, p, 12);
            if k.len() < 8 {
                break;
            }
            let ks = be32(&k);
            if ks < 8 {
                kids.push(format!("{} {ks}", name(&k[4..8])));
                break;
            }
            if &k[4..8] == b"data" && k.len() >= 12 {
                kids.push(format!("data(type {}) {ks}", be32(&k[8..12]) & 0xFF_FFFF));
            } else {
                kids.push(format!("{} {ks}", name(&k[4..8])));
            }
            p = p.saturating_add(ks);
        }
        let over = if item_end > end { "  <-- goes past its parent" } else { "" };
        out.push(format!("{}{} {size}: {}{over}", "  ".repeat(depth), name(&h[4..8]), kids.join(", ")));
        pos = item_end;
    }
}

/// 先頭の ID3v2 (重なっていればすべて)、その後ろ (音声の始まり・FLAC のブロック)、末尾の ID3v1 / APE
fn id3_and_rest(f: &mut File, out: &mut Vec<String>, len: u64) {
    let mut pos = 0u64;
    loop {
        let h = read_at(f, pos, 10);
        if h.len() < 10 || &h[..3] != b"ID3" || out.len() >= MAX_LINES {
            break;
        }
        let (major, rev, flags) = (h[3], h[4], h[5]);
        let size = syncsafe(&h[6..10]);
        out.push(format!("ID3v2.{major}.{rev} at {pos}, flags 0x{flags:02x}, size {size}"));
        let end = pos + 10 + size;
        let mut p = pos + 10;
        if major >= 3 && flags & 0x40 != 0 {
            let x = read_at(f, p, 4);
            if x.len() == 4 {
                let ext = if major == 4 { syncsafe(&x) } else { 4 + be32(&x) };
                out.push(format!("  extended header {ext}"));
                p += ext;
            }
        }
        while p < end && out.len() < MAX_LINES {
            let fh = read_at(f, p, 10);
            let (id, fsize, hl) = if major == 2 {
                if fh.len() < 6 {
                    break;
                }
                (&fh[..3], be32(&[0, fh[3], fh[4], fh[5]]), 6)
            } else {
                if fh.len() < 10 {
                    break;
                }
                (&fh[..4], if major == 4 { syncsafe(&fh[4..8]) } else { be32(&fh[4..8]) }, 10)
            };
            if id.iter().all(|&b| b == 0) {
                out.push(format!("  padding {}", end - p));
                break;
            }
            let fl = if major == 2 { String::new() } else { format!(" flags 0x{}", hex(&fh[8..10])) };
            let mut enc = String::new();
            if id[0] == b'T' && fsize > 0 {
                let e = read_at(f, p + hl, 3);
                if let Some(&b) = e.first() {
                    enc = format!(" enc {b}");
                    if b == 1 && e.len() == 3 {
                        enc += &format!(" bom {}", hex(&e[1..3]));
                    }
                }
            }
            let over = if p + hl + fsize > end { "  <-- goes past the tag" } else { "" };
            out.push(format!("  {} {fsize}{fl}{enc}{over}", name(id)));
            let valid = id.iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
            if !valid && fsize == 0 {
                break;
            }
            p += hl + fsize;
        }
        // ID3v2.4 はタグの後ろに 10 バイトのフッターを付けられる
        pos = end + if major == 4 && flags & 0x10 != 0 { 10 } else { 0 };
    }

    let a = read_at(f, pos, 4);
    if a == b"fLaC" {
        out.push(format!("fLaC at {pos}"));
        let mut p = pos + 4;
        while out.len() < MAX_LINES {
            let h = read_at(f, p, 4);
            if h.len() < 4 {
                break;
            }
            let kind = match h[0] & 0x7F {
                0 => "STREAMINFO".to_string(),
                1 => "PADDING".to_string(),
                2 => "APPLICATION".to_string(),
                3 => "SEEKTABLE".to_string(),
                4 => "VORBIS_COMMENT".to_string(),
                5 => "CUESHEET".to_string(),
                6 => "PICTURE".to_string(),
                n => format!("block type {n}"),
            };
            let l = be32(&[0, h[1], h[2], h[3]]);
            let over = if p + 4 + l > len { "  <-- goes past the end of the file" } else { "" };
            out.push(format!("  {kind} {l}{over}"));
            p += 4 + l;
            if h[0] & 0x80 != 0 {
                break;
            }
        }
        out.push(format!("audio starts at {p}"));
    } else {
        out.push(format!("audio starts at {pos}: {}", hex(&a)));
    }
    if len >= 128 && read_at(f, len - 128, 3) == b"TAG" {
        out.push("ID3v1 at end".into());
    }
    for back in [32, 160] {
        if len >= back && read_at(f, len - back, 8) == b"APETAGEX" {
            ape(f, out, len - back + 32);
        }
    }
}

/// 末尾の APE タグ (`end` はフッターの終わり): 版・項目の数と、項目ごとのキー・大きさ・種類。
/// 文字の項目は、UTF-8 として読めないときだけ印を付ける (中身は出さない)
fn ape(f: &mut File, out: &mut Vec<String>, end: u64) {
    let foot = read_at(f, end - 32, 32);
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let (version, size, count, flags) = (u32_at(&foot, 8), u32_at(&foot, 12) as u64, u32_at(&foot, 16), u32_at(&foot, 20));
    out.push(format!("APE tag at end: version {version}, size {size}, {count} items, flags 0x{flags:08x}"));
    if size < 32 || size > end {
        out.push("  <-- size goes past the file".into());
        return;
    }
    let mut p = end - size;
    for _ in 0..count {
        if p + 8 > end - 32 || out.len() >= MAX_LINES {
            break;
        }
        let h = read_at(f, p, 8 + 256);
        if h.len() < 9 {
            break;
        }
        let vsize = u32_at(&h, 0) as u64;
        let ty = (u32_at(&h, 4) >> 1) & 3;
        let Some(k) = h[8..].iter().position(|&b| b == 0) else {
            out.push("  <-- item key has no end".into());
            break;
        };
        let key = &h[8..8 + k];
        let key = if key.iter().all(|c| (0x20..0x7F).contains(c)) { String::from_utf8_lossy(key).into_owned() } else { format!("0x{}", hex(key)) };
        let vstart = p + 8 + k as u64 + 1;
        let mut line = format!("  {key} {vsize} type {ty}");
        if vstart + vsize > end - 32 {
            line.push_str("  <-- goes past the tag");
            out.push(line);
            break;
        }
        // 文字 (0) と場所 (2) は UTF-8 のはず
        if ty != 1 && vsize <= 1024 * 1024 && std::str::from_utf8(&read_at(f, vstart, vsize as usize)).is_err() {
            line.push_str(" not UTF-8");
        }
        out.push(line);
        p = vstart + vsize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zenpod-structure-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn atom(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = ((data.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend(ty);
        v.extend(data);
        v
    }

    /// MP4: 箱の入れ子と ilst の項目 (文字は出さない)、64 bit の大きさの mdat
    #[test]
    fn mp4_tree() {
        let data = atom(b"data", b"\0\0\0\x01\0\0\0\0Secret title");
        let mut meta = vec![0; 4];
        meta.extend(atom(b"hdlr", &[0; 25]));
        meta.extend(atom(b"ilst", &atom(b"\xA9nam", &data)));
        let mut file = atom(b"ftyp", b"M4A \0\0\0\0");
        file.extend(1u32.to_be_bytes());
        file.extend(b"mdat");
        file.extend(24u64.to_be_bytes());
        file.extend([0; 8]);
        file.extend(atom(b"moov", &atom(b"udta", &atom(b"meta", &meta))));
        let s = tag_structure(&tmp("a.m4a", &file)).unwrap();
        assert!(s.contains("\nmdat 24 (64-bit size)\n"), "{s}");
        assert!(s.contains("        (c)nam 36: data(type 1) 28"), "{s}");
        assert!(!s.contains("Secret"), "{s}");
    }

    /// MP3: ID3v2.3 の枠・文字コード・余白・音声の始まり・末尾の ID3v1 (文字は出さない)
    #[test]
    fn id3_frames() {
        let mut body = b"TIT2".to_vec();
        body.extend(15u32.to_be_bytes());
        body.extend([0, 0]);
        body.extend(b"\x01\xff\xfeS\0e\0c\0r\0e\0t\0");
        body.extend([0; 20]);
        let mut file = b"ID3\x03\x00\x00".to_vec();
        file.extend([0, 0, 0, body.len() as u8]);
        file.extend(&body);
        file.extend([0xFF, 0xFB, 0x90, 0xC0]);
        file.extend([0; 200]);
        let n = file.len();
        file[n - 128..n - 125].copy_from_slice(b"TAG");
        let s = tag_structure(&tmp("a.mp3", &file)).unwrap();
        assert!(s.contains("ID3v2.3.0 at 0, flags 0x00, size 45"), "{s}");
        assert!(s.contains("  TIT2 15 flags 0x0000 enc 1 bom fffe"), "{s}");
        assert!(s.contains("  padding 20"), "{s}");
        assert!(s.contains("audio starts at 55: fffb90c0"), "{s}");
        assert!(s.contains("ID3v1 at end"), "{s}");
        assert!(!s.contains("S\0e") && !s.contains("Secret"), "{s}");
    }

    /// MP3 の末尾の APE タグ: 項目のキー・大きさ・種類と、UTF-8 でない文字の印 (文字は出さない)
    #[test]
    fn ape_items() {
        let mut file = vec![0xFF, 0xFB, 0x90, 0xC0];
        file.extend([0; 200]);
        file.extend(crate::media::apesalvage::tests::tag(&[
            crate::media::apesalvage::tests::text_item("Album", b"Secr\xe9t"),
            crate::media::apesalvage::tests::text_item("Year", b"2001"),
        ]));
        let s = tag_structure(&tmp("c.mp3", &file)).unwrap();
        assert!(s.contains("APE tag at end: version 2000, size 69, 2 items, flags 0x80000000"), "{s}");
        assert!(s.contains("  Album 6 type 0 not UTF-8\n  Year 4 type 0"), "{s}");
        assert!(!s.contains("Secr"), "{s}");
    }

    /// 壊れた大きさ (親をはみ出す箱) でも止まり、印を付ける
    #[test]
    fn broken_sizes() {
        let mut file = atom(b"ftyp", b"M4A \0\0\0\0");
        let mut moov = atom(b"moov", &atom(b"udta", &[]));
        moov[0..4].copy_from_slice(&1000u32.to_be_bytes());
        file.extend(moov);
        let s = tag_structure(&tmp("b.m4a", &file)).unwrap();
        assert!(s.contains("moov 1000  <-- goes past its parent"), "{s}");
    }
}
