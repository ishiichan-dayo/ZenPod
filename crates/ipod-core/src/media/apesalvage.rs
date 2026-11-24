//! MP3 の末尾の APE タグに lofty が読めない項目がある曲を読む。
//!
//! lofty 0.25 は、APE タグの文字の項目が UTF-8 でない (Latin-1 などで書いたアルバム名) と、
//! ParsingMode に関係なくファイルごと読めない扱いにする。Music.app は APE タグを読まないので、
//! ほかのアプリでは普通に見える (配布リポジトリの Issue #7)。
//! ここでは項目を 1 つずつ lofty に読ませて読めない項目を見つけ、それを除いて組み直した APE タグに
//! 差し替えた読み口で読む。ファイルの APE タグは、編集で空にした項目を消す (`clear`) ほかは触らない (編集では ID3v2 だけを書く)。

use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};

use lofty::config::ParseOptions;
use lofty::file::AudioFile;

use crate::media::fastread::Seg;

/// これより大きい APE タグは扱わない (普通でない)
const MAX_TAG: u64 = 64 * 1024 * 1024;

/// 末尾の APE タグを、読める項目だけで組み直したもの
pub(crate) struct ApeFix {
    /// 元の APE タグの場所 (ヘッダー込み)。ファイルの [start, end)
    start: u64,
    end: u64,
    /// 読める項目だけで組み直した APE タグ (読める項目が無ければ空)
    tag: Vec<u8>,
    /// 読めない項目 (キー, 読めない理由)
    pub bad: Vec<(String, String)>,
}

/// 項目を並べた APE タグ (ヘッダーとフッター付き、版 2.0)
fn ape_tag(items: &[u8], count: u32) -> Vec<u8> {
    let size = (items.len() + 32) as u32;
    let part = |flags: u32| {
        let mut v = b"APETAGEX".to_vec();
        for n in [2000, size, count, flags] {
            v.extend_from_slice(&n.to_le_bytes());
        }
        v.extend_from_slice(&[0; 8]);
        v
    };
    // ヘッダー: 「ヘッダーあり」「これはヘッダー」、フッター: 「ヘッダーあり」
    let mut out = part(0xA000_0000);
    out.extend_from_slice(items);
    out.extend_from_slice(&part(0x8000_0000));
    out
}

/// 項目 1 つを lofty が読めるか。読めなければ理由 (原因の連なり)
fn item_error(item: &[u8]) -> Option<String> {
    // MPEG のフレームを 1 つ (MPEG-1 Layer III・128kbps・44.1kHz、417 バイト) + その項目だけの APE タグ
    let mut bytes = vec![0xFF, 0xFB, 0x90, 0x00];
    bytes.resize(417, 0);
    bytes.extend_from_slice(&ape_tag(item, 1));
    let opts = ParseOptions::new().read_properties(false);
    match lofty::mpeg::MpegFile::read_from(&mut Cursor::new(bytes), opts) {
        Ok(_) => None,
        Err(e) => Some(crate::media::error_chain(&e)),
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// 末尾 (ID3v1 があればその前) の APE タグを調べ、読めない項目があれば除いて組み直す。
/// APE タグが無い・読めない項目が無い・普通でない形なら None
pub(crate) fn find(f: &mut File, len: u64) -> Option<ApeFix> {
    // 終わりだけで本体が無い (前は音声)。lofty はファイルごと読めないので、読み口から終わりの 32 バイトを外す
    if let Some(at) = stray_footer(f, len) {
        let why = crate::t!("APE タグの終わりだけがあり、中身がありません");
        return Some(ApeFix { start: at, end: at + 32, tag: Vec::new(), bad: vec![("APETAGEX".into(), why)] });
    }
    let (start, end, body, count) = locate(f, len)?;
    let mut good = Vec::new();
    let mut n_good = 0u32;
    let mut bad = Vec::new();
    let mut p = 0usize;
    for _ in 0..count {
        // 値の大きさ (4)・フラグ (4)・キー (\0 で終わる)・値
        if p + 8 > body.len() {
            return None;
        }
        let vsize = u32_at(&body, p) as usize;
        let key_end = p + 8 + body[p + 8..].iter().position(|&b| b == 0)?;
        let item_end = (key_end + 1).checked_add(vsize).filter(|&e| e <= body.len())?;
        let item = &body[p..item_end];
        match item_error(item) {
            None => {
                good.extend_from_slice(item);
                n_good += 1;
            }
            Some(why) => bad.push((String::from_utf8_lossy(&body[p + 8..key_end]).into_owned(), why)),
        }
        p = item_end;
    }
    if bad.is_empty() {
        return None;
    }
    let tag = if n_good > 0 { ape_tag(&good, n_good) } else { Vec::new() };
    Some(ApeFix { start, end, tag, bad })
}

/// 末尾の APE タグの終わりが「ヘッダーあり」と言うのに、その場所にヘッダーが無い (終わりの 32 バイトだけが付いている。
/// 上流の lofty の Issue #685 の報告者のファイル)。そうなら終わりの位置
fn stray_footer(f: &mut File, len: u64) -> Option<u64> {
    let end = tag_end(f, len)?;
    let mut foot = [0u8; 32];
    f.seek(SeekFrom::Start(end.checked_sub(32)?)).ok()?;
    f.read_exact(&mut foot).ok()?;
    if &foot[..8] != b"APETAGEX" || u32_at(&foot, 20) & 0x8000_0000 == 0 {
        return None;
    }
    let whole = u64::from(u32_at(&foot, 12)) + 32;
    let mut head = [0u8; 8];
    let ok = end >= whole && f.seek(SeekFrom::Start(end - whole)).is_ok() && f.read_exact(&mut head).is_ok() && &head == b"APETAGEX";
    (!ok).then_some(end - 32)
}

/// APE タグが終わるはずの位置 (ID3v1 があればその前、無ければファイルの終わり)
fn tag_end(f: &mut File, len: u64) -> Option<u64> {
    if len >= 128 {
        let mut t = [0u8; 3];
        f.seek(SeekFrom::Start(len - 128)).ok()?;
        f.read_exact(&mut t).ok()?;
        if &t == b"TAG" {
            return Some(len - 128);
        }
    }
    Some(len)
}

/// 末尾 (ID3v1 があればその前) の APE タグの場所 [start, end) と、項目の並び (ヘッダー・フッターを除く) と項目の数。
/// 終わりだけで本体が無い形 (`stray_footer`) は None
fn locate(f: &mut File, len: u64) -> Option<(u64, u64, Vec<u8>, u32)> {
    if stray_footer(f, len).is_some() {
        return None;
    }
    let mut end = len;
    if len >= 128 {
        let mut t = [0u8; 3];
        f.seek(SeekFrom::Start(len - 128)).ok()?;
        f.read_exact(&mut t).ok()?;
        if &t == b"TAG" {
            end = len - 128;
        }
    }
    if end < 32 {
        return None;
    }
    let mut foot = [0u8; 32];
    f.seek(SeekFrom::Start(end - 32)).ok()?;
    f.read_exact(&mut foot).ok()?;
    if &foot[..8] != b"APETAGEX" {
        return None;
    }
    // size はフッターを含み、ヘッダーを含まない
    let size = u32_at(&foot, 12) as u64;
    let count = u32_at(&foot, 16);
    let has_header = u32_at(&foot, 20) & 0x8000_0000 != 0;
    let whole = size + if has_header { 32 } else { 0 };
    if !(32..=MAX_TAG).contains(&size) || whole > end {
        return None;
    }
    let start = end - whole;
    let mut body = vec![0u8; (size - 32) as usize];
    f.seek(SeekFrom::Start(end - size)).ok()?;
    f.read_exact(&mut body).ok()?;
    Some((start, end, body, count))
}

/// MP3 の末尾 (ID3v1 があればその前) の APE タグから、キー (小文字で比べる) の文字の項目を消す。
/// 読むときは ID3v2 に無い項目を APE タグから補うので、ID3v2 から消しただけでは元の値に戻って見える。
/// 項目が残らなければ APE タグごと消す。APE タグが無い・普通でない形なら何もしない
pub(crate) fn clear(path: &std::path::Path, keys: &[&str]) -> std::io::Result<()> {
    use std::io::Write;
    if keys.is_empty() {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let len = f.metadata()?.len();
    let Some((start, end, body, count)) = locate(&mut f, len) else {
        return Ok(());
    };
    let mut kept = Vec::new();
    let mut n = 0u32;
    let mut p = 0usize;
    for _ in 0..count {
        if p + 8 > body.len() {
            return Ok(());
        }
        let vsize = u32_at(&body, p) as usize;
        let flags = u32_at(&body, p + 4);
        let Some(key_end) = body[p + 8..].iter().position(|&b| b == 0).map(|k| p + 8 + k) else { return Ok(()) };
        let Some(item_end) = (key_end + 1).checked_add(vsize).filter(|&e| e <= body.len()) else { return Ok(()) };
        let key = String::from_utf8_lossy(&body[p + 8..key_end]).to_lowercase();
        // 文字の項目 (種類の 2 ビットが 0) だけ消す
        if !(flags & 0b110 == 0 && keys.contains(&key.as_str())) {
            kept.extend_from_slice(&body[p..item_end]);
            n += 1;
        }
        p = item_end;
    }
    if n == count {
        return Ok(());
    }
    let new = if n > 0 { ape_tag(&kept, n) } else { Vec::new() };
    // 後ろ (ID3v1) はそのまま、APE タグの部分を差し替えて詰める
    let mut tail = Vec::new();
    f.seek(SeekFrom::Start(end))?;
    f.read_to_end(&mut tail)?;
    f.seek(SeekFrom::Start(start))?;
    f.write_all(&new)?;
    f.write_all(&tail)?;
    f.set_len(start + (new.len() + tail.len()) as u64)
}

impl ApeFix {
    /// 読み口の部品のうち、元の APE タグの範囲を組み直したタグに差し替える
    pub(crate) fn splice(&self, segs: Vec<Seg>) -> Vec<Seg> {
        let mut out = Vec::with_capacity(segs.len() + 2);
        for seg in segs {
            match seg {
                Seg::File { start, len } if start <= self.start && self.end <= start + len => {
                    out.push(Seg::File { start, len: self.start - start });
                    out.push(Seg::Mem(self.tag.clone()));
                    out.push(Seg::File { start: self.end, len: start + len - self.end });
                }
                s => out.push(s),
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// APE タグの項目 1 つ (文字)
    pub(crate) fn text_item(key: &str, value: &[u8]) -> Vec<u8> {
        let mut v = (value.len() as u32).to_le_bytes().to_vec();
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(key.as_bytes());
        v.push(0);
        v.extend_from_slice(value);
        v
    }

    /// 項目を並べた APE タグ (テスト用)
    pub(crate) fn tag(items: &[Vec<u8>]) -> Vec<u8> {
        ape_tag(&items.concat(), items.len() as u32)
    }

    #[test]
    fn finds_unreadable_items() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.mp3");
        let mut bytes = Vec::new();
        for _ in 0..20 {
            bytes.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
            bytes.resize(bytes.len() + 413, 0);
        }
        let audio = bytes.len() as u64;
        // "Mamá" の á が Latin-1 (0xE1) で、UTF-8 としては読めない
        bytes.extend_from_slice(&tag(&[text_item("Title", b"Title"), text_item("Album", b"Mam\xe1"), text_item("Artist", b"Artist")]));
        bytes.extend_from_slice(b"TAG");
        bytes.resize(bytes.len() + 125, 0);
        std::fs::write(&p, &bytes).unwrap();
        let len = bytes.len() as u64;
        let fix = find(&mut File::open(&p).unwrap(), len).unwrap();
        assert_eq!(fix.bad.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["Album"]);
        assert_eq!((fix.start, fix.end), (audio, len - 128));
        // 差し替えた読み口なら lofty で読める
        let segs = fix.splice(vec![Seg::File { start: 0, len }]);
        let v = crate::media::fastread::Virtual::new(File::open(&p).unwrap(), segs);
        let mp = lofty::mpeg::MpegFile::read_from(&mut std::io::BufReader::new(v), ParseOptions::new()).unwrap();
        let ape = mp.ape().unwrap();
        assert_eq!(ape.get("Artist").and_then(|i| i.value().text()), Some("Artist"));
        assert!(ape.get("Album").is_none());
    }
}
