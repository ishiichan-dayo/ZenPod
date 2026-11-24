//! ID3v2 の中に lofty が読めない枠がある MP3 を読み書きする。
//!
//! lofty 0.25 は、枠 1 つの中身を読めないと (UTF-8 と書いてあるのに中身が Latin-1 の歌詞・コメント、
//! BOM の無い UTF-16 など)、ParsingMode に関係なくファイルごと読めない扱いにする。Music.app などは
//! その枠だけ飛ばして読むので、ほかのアプリでは普通に見える (配布リポジトリの Issue #7)。
//! ここでは枠を 1 つずつ lofty に読ませて読めない枠を見つけ、それを除いたタグで読む。
//! 書くときは、読めない枠を元のバイトのまま残す (消さない)。

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFile};
use lofty::probe::Probe;
use lofty::tag::{Accessor, TagExt};

use crate::media::apesalvage::ApeFix;
use crate::media::fastread::{Seg, Virtual};

/// これより大きいタグは扱わない (普通でない)
const MAX_TAG: u64 = 64 * 1024 * 1024;

/// 読めない枠を除いて組み直したタグ
pub(crate) struct Salvaged {
    /// 読める枠だけで組み直したタグ (見出し込み)
    tag: Vec<u8>,
    /// 元のタグの版 (3 か 4)
    major: u8,
    /// 読めない枠 (枠の ID, 読めない理由, 見出し込みの元のバイト)
    pub bad: Vec<(String, String, Vec<u8>)>,
    /// 元のタグの終わり
    tag_end: u64,
    /// 続けて重なったタグも含めた終わり (= 音声の始まり。`stacked_end`)
    stack_end: u64,
    len: u64,
    /// 末尾の APE タグに読めない項目があれば、それを除いて組み直したもの (ファイルの APE タグには触らない)
    ape: Option<ApeFix>,
}

fn syncsafe(b: &[u8]) -> u64 {
    b.iter().take(4).fold(0, |a, &x| (a << 7) | (x as u64 & 0x7F))
}

fn to_syncsafe(n: u64) -> [u8; 4] {
    [((n >> 21) & 0x7F) as u8, ((n >> 14) & 0x7F) as u8, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]
}

fn header(major: u8, body: u64) -> Vec<u8> {
    let mut h = vec![b'I', b'D', b'3', major, 0, 0];
    h.extend_from_slice(&to_syncsafe(body));
    h
}

/// 並べ替え用の読みの枠のうち、lofty 0.25 が ID3v2.3 で書くときに「2.4 の枠」として捨てるもの。
/// iTunes も Mp3tag も 2.3 にこれらを書き、iPod も読む。捨てると、読みを入れても保存されず、
/// ほかの項目を編集しただけでも元の読みが消える (配布リポジトリの Issue #23)。TSO2・TSOC は捨てられない
const V24_SORT: &[&str] = &["TSOP", "TSOA", "TSOT"];

/// 2.3 で書くときに lofty が捨てる読みの枠があるか
pub(crate) fn has_v24_sort(tag: &lofty::id3::v2::Id3v2Tag) -> bool {
    tag.into_iter().any(|fr| V24_SORT.contains(&fr.id_str()))
}

/// タグを余白なしで書き出す。2.3 で書くときは、lofty が捨てる読みの枠 (`V24_SORT`) を 2.3 の形で後ろに足す
pub(crate) fn dump(tag: &lofty::id3::v2::Id3v2Tag, w: WriteOptions, v23: bool) -> std::result::Result<Vec<u8>, lofty::error::TagEncodingError> {
    use lofty::id3::v2::Frame;
    let mut new = Vec::new();
    tag.dump_to(&mut new, w.preferred_padding(0).use_id3v23(v23))?;
    if !v23 {
        return Ok(new);
    }
    let mut extra = Vec::new();
    for fr in tag {
        if let Frame::Text(t) = fr
            && V24_SORT.contains(&fr.id_str())
        {
            extra.extend(v23_text_frame(fr.id_str(), &t.value));
        }
    }
    if extra.is_empty() {
        return Ok(new);
    }
    // ほかに枠が無いと lofty は何も書かない
    if new.is_empty() {
        new = header(3, 0);
    }
    // 非同期化やフッターが付いた形には足さない (余白なしの 2.3 なら付かない)
    if new.len() < 10 || new[..3] != *b"ID3" || new[3] != 3 || new[5] != 0 {
        return Ok(new);
    }
    new.extend(extra);
    let body = new.len() as u64 - 10;
    new[6..10].copy_from_slice(&to_syncsafe(body));
    Ok(new)
}

/// ID3v2.3 の文字の枠。Latin-1 に収まれば Latin-1、収まらなければ BOM 付きの UTF-16 (2.3 に UTF-8 は無い)。
/// 2.4 の複数の値 (\0 区切り) は、2.3 の慣例の「/」でつなぐ
fn v23_text_frame(id: &str, value: &str) -> Vec<u8> {
    let value = value.replace('\0', "/");
    let mut content = Vec::new();
    if value.chars().all(|c| (c as u32) <= 0xFF) {
        content.push(0);
        content.extend(value.chars().map(|c| c as u8));
    } else {
        content.extend_from_slice(&[1, 0xFF, 0xFE]);
        content.extend(value.encode_utf16().flat_map(u16::to_le_bytes));
    }
    let mut f = id.as_bytes().to_vec();
    f.extend_from_slice(&(content.len() as u32).to_be_bytes());
    f.extend_from_slice(&[0, 0]);
    f.extend(content);
    f
}

/// 枠が始まる位置 (見出しの後ろ)。拡張ヘッダーがあれば飛ばす。読めない形なら None。
/// lofty 0.25 は ID3v2.3 の拡張ヘッダー (大きさの欄は自分を含まず 6 か 10) を 2.4 の形で読み、
/// 4 バイトずれて枠を 1 つも読めない (エラーにならない)。ここで飛ばして組み直す
fn frames_start(f: &mut File, h: &[u8; 10]) -> Option<u64> {
    if h[5] & 0x40 == 0 {
        return Some(10);
    }
    let mut e = [0u8; 4];
    f.seek(SeekFrom::Start(10)).ok()?;
    f.read_exact(&mut e).ok()?;
    match h[3] {
        3 => {
            let n = u32::from_be_bytes(e);
            matches!(n, 6 | 10).then_some(14 + n as u64)
        }
        // 2.4 は大きさが自分を含む
        _ => Some(10 + syncsafe(&e)).filter(|&n| n >= 16),
    }
}

/// 目印の枠の中身 (後ろに付けて、ここまで正しく読めたかを見る)
const SENTINEL: &str = "zenpod-sentinel";

/// 枠 1 つを lofty が読めるか。読めなければ理由 (原因の連なり)。
/// 後ろに目印の枠を付けて、目印まで正しく読めたかも見る。lofty は中身を途中までしか読まない枠
/// (頭に \0 がある URL の枠など) の残りを次の枠の見出しとして読み、そこから区切りがずれる
fn frame_error(major: u8, frame: &[u8]) -> Option<String> {
    let mut sentinel = b"TIT2".to_vec();
    let text = [&[0u8][..], SENTINEL.as_bytes()].concat();
    let n = text.len() as u64;
    sentinel.extend_from_slice(&if major == 4 { to_syncsafe(n) } else { (n as u32).to_be_bytes() });
    sentinel.extend_from_slice(&[0, 0]);
    sentinel.extend_from_slice(&text);
    let mut bytes = header(major, (frame.len() + sentinel.len()) as u64);
    bytes.extend_from_slice(frame);
    bytes.extend_from_slice(&sentinel);
    // MPEG のフレームを 1 つ付ける (MPEG-1 Layer III・128kbps・44.1kHz、417 バイト)
    bytes.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
    bytes.resize(bytes.len() + 413, 0);
    let opts = ParseOptions::new().read_properties(false);
    match lofty::mpeg::MpegFile::read_from(&mut Cursor::new(bytes), opts) {
        Ok(f) if f.id3v2().and_then(|t| t.title()).as_deref() == Some(SENTINEL) => None,
        // tr: 訳さない (診断用。英語の lofty のエラーと並ぶ)
        Ok(_) => Some("the frames after this one were misread".to_string()),
        Err(e) => Some(crate::media::error_chain(&e)),
    }
}

/// 先頭の ID3v2 の枠と末尾の APE タグの項目を調べ、読めないものがあれば除いて組み直す。
/// 読めないものが無い・普通でない形なら None。
/// ID3v2 は 2.3 / 2.4 だけ (非同期化・拡張ヘッダー・フッターがあるものは扱わない)。ID3v2 が無くてもよい (APE タグだけの曲)
pub(crate) fn salvage(f: &mut File) -> Option<Salvaged> {
    let len = f.metadata().ok()?.len();
    let ape = crate::media::apesalvage::find(f, len);
    f.seek(SeekFrom::Start(0)).ok()?;
    let mut h = [0u8; 10];
    f.read_exact(&mut h).ok()?;
    if &h[..3] != b"ID3" {
        // ID3v2 が無い: APE タグに読めない項目があるときだけ (編集では ID3v2 を先頭に足す)
        return ape.map(|ape| Salvaged { tag: Vec::new(), major: 4, bad: Vec::new(), tag_end: 0, stack_end: 0, len, ape: Some(ape) });
    }
    if !matches!(h[3], 3 | 4) || h[5] & 0x90 != 0 || h[6..].iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    let major = h[3];
    let size = syncsafe(&h[6..10]);
    if size > MAX_TAG || 10 + size > len {
        return None;
    }
    let start = frames_start(f, &h).filter(|&s| s <= 10 + size)?;
    f.seek(SeekFrom::Start(10)).ok()?;
    let mut body = vec![0u8; size as usize];
    f.read_exact(&mut body).ok()?;

    let mut good = Vec::new();
    let mut bad = Vec::new();
    // 拡張ヘッダーは組み直したタグには入れない (2.3 は lofty が読めないため。CRC は編集すると合わなくなる)
    let mut p = (start - 10) as usize;
    while p + 10 <= body.len() {
        let fh = &body[p..p + 10];
        if fh[0] == 0 {
            break; // 余白
        }
        // 枠の ID が壊れていたら、そこから先の区切りが分からないので扱わない
        if !fh[..4].iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return None;
        }
        let fsize = if major == 4 { syncsafe(&fh[4..8]) } else { u32::from_be_bytes([fh[4], fh[5], fh[6], fh[7]]) as u64 } as usize;
        let end = p.checked_add(10 + fsize).filter(|&e| e <= body.len())?;
        let frame = &body[p..end];
        match frame_error(major, frame) {
            None => good.extend_from_slice(frame),
            Some(why) => bad.push((String::from_utf8_lossy(&fh[..4]).into_owned(), why, frame.to_vec())),
        }
        p = end;
    }
    if bad.is_empty() && ape.is_none() && !(major == 3 && start > 10) {
        return None;
    }
    let mut tag = header(major, good.len() as u64);
    tag.extend_from_slice(&good);
    let stack_end = stacked_end(f, 10 + size);
    Some(Salvaged { tag, major, bad, tag_end: 10 + size, stack_end, len, ape })
}

/// 先頭に続けて並んだ ID3v2 タグの終わり (`first_end` は 1 つ目のタグの終わり)。
/// タグを 2 つ続けて書いたファイルがあり (配布リポジトリの Issue #14)、lofty は後ろのタグの枠で前の枠を
/// 上書きして 1 つのタグとして読む。1 つ目だけを書き換えると、読み直したときに後ろのタグの古い値に戻るので、
/// 書くときはここまでをまとめて 1 つのタグにする
pub(crate) fn stacked_end(f: &mut File, first_end: u64) -> u64 {
    let Ok(len) = f.metadata().map(|m| m.len()) else { return first_end };
    let mut end = first_end;
    let mut h = [0u8; 10];
    while end + 10 <= len
        && f.seek(SeekFrom::Start(end)).is_ok()
        && f.read_exact(&mut h).is_ok()
        && &h[..3] == b"ID3"
        && h[6..].iter().all(|b| b & 0x80 == 0)
    {
        let footer = if h[5] & 0x10 != 0 { 10 } else { 0 };
        let next = end + 10 + syncsafe(&h[6..10]) + footer;
        if next > len {
            break;
        }
        end = next;
    }
    end
}

/// ファイルの先頭 `old` バイトを `head` に置き換え、後ろ (音声) をずらして書き直す (lofty と同じく後ろはメモリに読む)
pub(crate) fn rewrite_head(f: &mut File, head: &[u8], old: u64) -> std::io::Result<()> {
    let mut rest = Vec::new();
    f.seek(SeekFrom::Start(old))?;
    f.read_to_end(&mut rest)?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(head)?;
    f.write_all(&rest)?;
    let end = f.stream_position()?;
    f.set_len(end)
}

impl Salvaged {
    /// 読める枠だけのタグ + 元の音声 (ファイルのように読める)
    pub(crate) fn reader(&self, path: &Path) -> std::io::Result<BufReader<Virtual>> {
        let f = File::open(path)?;
        let mut segs = vec![Seg::Mem(self.tag.clone()), Seg::File { start: self.tag_end, len: self.len - self.tag_end }];
        if let Some(ape) = &self.ape {
            segs = ape.splice(segs);
        }
        Ok(BufReader::new(Virtual::new(f, segs)))
    }

    /// 読めなかった枠・項目の一覧 ("USLT: 理由" / "APE Album: 理由" を改行でつなぐ)
    pub(crate) fn describe(&self) -> String {
        let frames = self.bad.iter().map(|(id, why, _)| format!("{id}: {why}"));
        let items = self.ape.iter().flat_map(|a| &a.bad).map(|(key, why)| format!("APE {key}: {why}"));
        frames.chain(items).collect::<Vec<_>>().join("\n")
    }

    /// 編集したタグ (`tag`) を書く。読めなかった枠は元のバイトのまま後ろに付ける。
    /// 版は元のタグと同じ (2.3 は 2.3 のまま。iPod は 2.4 を読めないことがある) なので、元の枠をそのまま付けられる。
    /// 元の場所に収まればその場で書き、収まらなければファイルを書き直す
    pub(crate) fn write(&self, f: &mut File, tag: &lofty::id3::v2::Id3v2Tag) -> Result<(), String> {
        let mut new = dump(tag, WriteOptions::default(), self.major == 3).map_err(|e| crate::media::error_chain(&e))?;
        // 非同期化やフッターが付くなら扱わない
        if new.len() < 10 || new[..3] != *b"ID3" || new[3] != self.major || new[5] != 0 {
            return Err(crate::t!("タグを書けない形です"));
        }
        let mut body = new.split_off(10);
        let written: std::collections::HashSet<&str> = tag.into_iter().map(|fr| fr.id().as_str()).collect();
        for (id, _, frame) in &self.bad {
            // 1 つのタグに 1 つだけの文字の枠 (TXXX 以外の T…) を編集で書いたなら、読めなかった元の枠は置き換わった
            if id.starts_with('T') && id != "TXXX" && written.contains(id.as_str()) {
                continue;
            }
            body.extend_from_slice(frame);
        }
        let io = |e: std::io::Error| e.to_string();
        // 後ろに重なったタグも、まとめて置き換える
        let old = self.stack_end;
        // 大きく縮んだ (ジャケットを縮めた・外した) ときは、余白にせず書き直して詰める
        if 10 + body.len() as u64 <= old && old - 10 - body.len() as u64 <= crate::media::MAX_SLACK {
            // 収まる: 余白 (0) で埋めて、タグの部分だけ書く
            body.resize((old - 10) as usize, 0);
            f.seek(SeekFrom::Start(0)).map_err(io)?;
            f.write_all(&header(self.major, body.len() as u64)).map_err(io)?;
            f.write_all(&body).map_err(io)?;
            return Ok(());
        }
        // 収まらない: 余白を足したタグ + 音声で書き直す
        body.resize(body.len() + 1024, 0);
        let mut head = header(self.major, body.len() as u64);
        head.extend_from_slice(&body);
        rewrite_head(f, &head, old).map_err(io)
    }
}

/// 言語の欄が英字 3 文字でないコメント・歌詞の枠 (\0\x13\0 など、配布リポジトリの Issue #8) の言語を「不明」(XXX) にする。
/// lofty はそうした枠を読めるが書けず、タグ全体を書けなくなる
pub(crate) fn fix_languages(tag: &mut lofty::id3::v2::Id3v2Tag) {
    use lofty::id3::v2::Frame;
    tag.retain_mut(|fr| {
        let lang = match fr {
            Frame::Comment(c) => &mut c.language,
            Frame::UnsynchronizedText(u) => &mut u.language,
            _ => return true,
        };
        if !lang.iter().all(u8::is_ascii_alphabetic) {
            *lang = *b"XXX";
        }
        true
    });
}

/// ID3v2.3 から 2.4 へ直すとき、lofty が年月日の TDRC にまとめる枠 (読めた枠と比べるときは除く)
const MERGED_V3: &[&str] = &["TYER", "TDAT", "TIME", "TRDA"];

/// 先頭の ID3v2 の枠の ID (版, ID の並び)。見出しだけを読む (中身は飛ばす)。
/// 大きさ 0 の枠は lofty も読み飛ばすので入れない。普通でない形 (非同期化など) なら None
pub(crate) fn raw_frame_ids(f: &mut File) -> Option<(u8, Vec<String>)> {
    f.seek(SeekFrom::Start(0)).ok()?;
    let mut h = [0u8; 10];
    f.read_exact(&mut h).ok()?;
    if &h[..3] != b"ID3" || !matches!(h[3], 3 | 4) || h[5] & 0x90 != 0 || h[6..].iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    let major = h[3];
    let end = 10 + syncsafe(&h[6..10]);
    let mut ids = Vec::new();
    let mut p = frames_start(f, &h)?;
    while p + 10 <= end {
        let mut fh = [0u8; 10];
        f.seek(SeekFrom::Start(p)).ok()?;
        f.read_exact(&mut fh).ok()?;
        if !fh[..4].iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            break; // 余白か、壊れた見出し (lofty もそこで止まる)
        }
        let fsize = if major == 4 { syncsafe(&fh[4..8]) } else { u32::from_be_bytes([fh[4], fh[5], fh[6], fh[7]]) as u64 };
        if fsize > 0 {
            ids.push(String::from_utf8_lossy(&fh[..4]).into_owned());
        }
        p += 10 + fsize;
    }
    Some((major, ids))
}

/// 元のタグにある枠の ID が、lofty が読めた枠に無いか。lofty は中身を途中までしか読まない枠
/// (頭に \0 がある URL の枠など) で区切りを見失うと、エラーを出さずに後ろの枠を捨てることがある。
/// そのまま書き戻すと、捨てた枠がファイルから消える
pub(crate) fn lost_frames(f: &mut File, tag: Option<&lofty::id3::v2::Id3v2Tag>) -> bool {
    raw_frame_ids(f).is_some_and(|(major, ids)| missing(major, &ids, tag))
}

/// 元の枠の ID (`ids`、版 `major`) のうち、lofty が読めた枠に無いものがあるか
pub(crate) fn missing(major: u8, ids: &[String], tag: Option<&lofty::id3::v2::Id3v2Tag>) -> bool {
    let have: std::collections::HashSet<&str> = tag.into_iter().flatten().map(|fr| fr.id().as_str()).collect();
    ids.iter().any(|id| {
        let id = id.as_str();
        if major == 3 && MERGED_V3.contains(&id) {
            return false;
        }
        let id = if major == 3 { lofty::id3::v2::upgrade_v3(id).unwrap_or(id) } else { id };
        !have.contains(id)
    })
}

/// lofty は読めたが黙って枠を捨てていた MP3 を、読めない枠を除いて読み直す。そうでなければ None
pub(crate) fn read_if_lost(path: &Path) -> Option<(TaggedFile, Salvaged)> {
    // MP3 だけ (ほかの形式を MPEG として読むと、音声の頭を探してファイル全体を読む)
    if !path.extension()?.to_str()?.eq_ignore_ascii_case("mp3") {
        return None;
    }
    let mut r = BufReader::new(File::open(path).ok()?);
    let mp = lofty::mpeg::MpegFile::read_from(&mut r, ParseOptions::new()).ok()?;
    if !lost_frames(r.get_mut(), mp.id3v2()) {
        return None;
    }
    read(path)
}

/// 普通に読めない MP3 を、読めない枠を除いて読む。MP3 でない・読めない枠が無いなら None
pub(crate) fn read(path: &Path) -> Option<(TaggedFile, Salvaged)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ext != "mp3" {
        return None;
    }
    let s = salvage(&mut File::open(path).ok()?)?;
    let tf = Probe::with_file_type(s.reader(path).ok()?, FileType::Mpeg).options(ParseOptions::new()).read().ok()?;
    Some((tf, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 枠を並べた ID3v2 (版 `major`) + 無音の MPEG フレーム 20 個 + 末尾の ID3v1
    fn mp3(major: u8, frames: &[(&str, Vec<u8>)], padding: usize) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, content) in frames {
            body.extend_from_slice(id.as_bytes());
            let n = content.len() as u64;
            body.extend_from_slice(&if major == 4 { to_syncsafe(n) } else { (n as u32).to_be_bytes() });
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(content);
        }
        body.resize(body.len() + padding, 0);
        let mut out = header(major, body.len() as u64);
        out.extend_from_slice(&body);
        for _ in 0..20 {
            out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
            out.resize(out.len() + 413, 0);
        }
        out.extend_from_slice(b"TAG");
        out.resize(out.len() + 125, 0);
        out
    }

    fn text(s: &str) -> Vec<u8> {
        let mut v = vec![0];
        v.extend_from_slice(s.as_bytes());
        v
    }

    /// 歌詞の枠が「UTF-8」なのに中身が Latin-1 の MP3 (配布リポジトリの Issue #7):
    /// 読める・編集できる・歌詞の枠は元のバイトのまま残る・音声と ID3v1 は変わらない
    #[test]
    fn reads_and_writes_around_unreadable_frames() {
        // "Mamá" の á が Latin-1 (0xE1) で、UTF-8 としては読めない
        let lyrics = b"\x03eng\0Mam\xe1, se\xf1or".to_vec();
        for (major, padding) in [(3, 0), (3, 2000), (4, 0), (4, 2000)] {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("a.mp3");
            let frames = [("TIT2", text("Title")), ("TPE1", text("Artist")), ("USLT", lyrics.clone()), ("TALB", text("Album"))];
            let orig = mp3(major, &frames, padding);
            std::fs::write(&p, &orig).unwrap();
            // lofty だけでは読めない
            assert!(lofty::read_from_path(&p).is_err());

            let info = crate::media::read_info(&p).unwrap();
            assert_eq!((info.title.as_deref(), info.artist.as_deref(), info.album.as_deref()), (Some("Title"), Some("Artist"), Some("Album")));

            let edit = crate::domain::track::TagEdit { artist: Some("New Artist ".repeat(if padding == 0 { 50 } else { 1 })), ..Default::default() };
            crate::media::write_tags(&p, &edit).unwrap();
            let after = std::fs::read(&p).unwrap();
            let info = crate::media::read_info(&p).unwrap();
            assert_eq!(info.artist.as_deref(), Some(edit.artist.as_deref().unwrap().trim()), "v{major} pad {padding}");
            assert_eq!(info.album.as_deref(), Some("Album"));
            // 版は元のまま、歌詞の枠は元のバイトのまま
            assert_eq!(after[3], major, "v{major} pad {padding}");
            let mut needle = b"USLT".to_vec();
            needle.extend_from_slice(&(lyrics.len() as u32).to_be_bytes());
            needle.extend_from_slice(&[0, 0]);
            needle.extend_from_slice(&lyrics);
            assert!(after.windows(needle.len()).any(|w| w == needle.as_slice()), "v{major} pad {padding}");
            // 音声と ID3v1 は変わらない
            let audio = 20 * 417 + 128;
            assert_eq!(after[after.len() - audio..], orig[orig.len() - audio..]);
        }
    }

    /// ID3v2 タグが 2 つ続けて重なった MP3 (配布リポジトリの Issue #14)。lofty は後ろのタグの枠で前の枠を上書きして読むので、
    /// 1 つ目だけを書き換えると、読み直したときに古い値に戻っていた。まとめて 1 つのタグにする。
    /// 収まる / 収まらない (書き直す) / 1 つ目に読めない枠がある (salvage)
    #[test]
    fn writes_over_stacked_tags() {
        let lyrics = b"\x03eng\0Mam\xe1".to_vec();
        for (bad, padding, title) in [(false, 500, "New Title".to_string()), (false, 0, "長".repeat(400)), (true, 500, "New Title".to_string())] {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("a.mp3");
            let mut first = vec![("TIT2", text("Old Title 1")), ("TPE2", text("Album Artist")), ("TCON", text("Jungle"))];
            if bad {
                first.push(("USLT", lyrics.clone()));
            }
            let a = mp3(4, &first, padding);
            let first_len = 10 + syncsafe(&a[6..10]) as usize;
            let b = mp3(4, &[("TIT2", text("Old Title 2")), ("TCON", text("Jungle"))], 100);
            let second_len = 10 + syncsafe(&b[6..10]) as usize;
            let mut orig = a[..first_len].to_vec();
            orig.extend_from_slice(&b[..second_len]);
            orig.extend_from_slice(&a[first_len..]);
            std::fs::write(&p, &orig).unwrap();
            assert_eq!(crate::media::read_info(&p).unwrap().title.as_deref(), Some("Old Title 2"));

            let edit = crate::domain::track::TagEdit { title: Some(title.clone()), genre: Some("Drum & Bass".into()), ..Default::default() };
            crate::media::write_tags(&p, &edit).unwrap();
            let after = std::fs::read(&p).unwrap();
            let info = crate::media::read_info(&p).unwrap();
            assert_eq!(info.title.as_deref(), Some(title.as_str()), "bad {bad} pad {padding}");
            assert_eq!(info.genre.as_deref(), Some("Drum & Bass"), "bad {bad} pad {padding}");
            assert_eq!(info.album_artist.as_deref(), Some("Album Artist"), "bad {bad} pad {padding}");
            // タグは 1 つだけ
            let end = 10 + syncsafe(&after[6..10]) as usize;
            assert_ne!(&after[end..end + 3], b"ID3", "bad {bad} pad {padding}");
            if padding > 0 {
                assert_eq!(after.len(), orig.len(), "bad {bad} pad {padding}");
            }
            // 音声と ID3v1 は変わらない
            let audio = 20 * 417 + 128;
            assert_eq!(after[after.len() - audio..], orig[orig.len() - audio..]);
            assert_eq!(after.len() - end, audio, "bad {bad} pad {padding}");
        }
    }

    /// 末尾に ID3v1 がある MP3 で項目を空にして消すと、読むときに ID3v1 から補われて元に戻っていた。
    /// 空にした項目は ID3v1 からも消す (値を変えた項目は ID3v2 が先に読まれるので ID3v1 はそのまま)
    #[test]
    fn clearing_fields_also_clears_id3v1() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.mp3");
        let mut orig = mp3(3, &[("TIT2", text("Title")), ("TALB", text("Album")), ("TCON", text("Jungle")), ("TYER", text("2005")), ("TRCK", text("2"))], 200);
        // ID3v1: 曲名・アーティスト・アルバム・年・トラック (1.1)・ジャンル
        let v1 = orig.len() - 128;
        orig[v1 + 3..v1 + 8].copy_from_slice(b"V1 Ti");
        orig[v1 + 33..v1 + 38].copy_from_slice(b"V1 Ar");
        orig[v1 + 63..v1 + 68].copy_from_slice(b"V1 Al");
        orig[v1 + 93..v1 + 97].copy_from_slice(b"2001");
        orig[v1 + 126] = 7;
        orig[v1 + 127] = 5;
        std::fs::write(&p, &orig).unwrap();
        let info = crate::media::read_info(&p).unwrap();
        assert_eq!(info.artist.as_deref(), Some("V1 Ar"), "ID3v2 に無い項目は ID3v1 から補う");

        let edit = crate::domain::track::TagEdit {
            title: Some("New".into()),
            album: Some(String::new()),
            genre: Some(" ".into()),
            year: Some(0),
            track_nr: Some(0),
            ..Default::default()
        };
        crate::media::write_tags(&p, &edit).unwrap();
        let info = crate::media::read_info(&p).unwrap();
        assert_eq!(info.title.as_deref(), Some("New"));
        assert_eq!(info.album, None);
        assert_eq!(info.genre, None);
        assert_eq!(info.year, 0);
        assert_eq!(info.track_nr, 0);
        // 触っていない項目は ID3v1 に残る
        assert_eq!(info.artist.as_deref(), Some("V1 Ar"));
        let after = std::fs::read(&p).unwrap();
        assert_eq!(&after[after.len() - 128 + 3..after.len() - 128 + 8], b"V1 Ti", "値を変えた項目の ID3v1 はそのまま");
        // 音声は変わらない
        let audio = 20 * 417;
        assert_eq!(after[after.len() - 128 - audio..after.len() - 128], orig[orig.len() - 128 - audio..orig.len() - 128]);

        // タイトルを空にすると、読むときはファイル名になる (保存できていない扱いにしない)。ID3v1 からも消える
        let clear_title = crate::domain::track::TagEdit { title: Some(String::new()), ..Default::default() };
        crate::media::write_tags(&p, &clear_title).unwrap();
        assert!(!crate::media::read_info(&p).unwrap().title_tagged);
    }

    /// 末尾の APE タグのアルバム名が Latin-1 の MP3 (配布リポジトリの Issue #7)。ID3v2 あり / なし:
    /// 読める・ジャケットも出る・編集できる・APE タグは元のバイトのまま残る
    #[test]
    fn reads_and_writes_around_unreadable_ape_items() {
        use crate::media::apesalvage::tests::{tag, text_item};
        let ape = tag(&[text_item("Album", b"Mam\xe1"), text_item("Year", b"2001")]);
        for with_id3 in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("a.mp3");
            let frames = [("TIT2", text("Title")), ("TPE1", text("Artist")), ("APIC", [&b"\0image/jpeg\0\x03\0\xff\xd8"[..], &[0u8; 200]].concat())];
            let mut orig = if with_id3 { mp3(4, &frames, 100) } else { mp3(4, &[], 0)[20..].to_vec() };
            // ID3v1 の前に APE タグを入れる
            let v1 = orig.split_off(orig.len() - 128);
            orig.extend_from_slice(&ape);
            orig.extend_from_slice(&v1);
            std::fs::write(&p, &orig).unwrap();
            assert!(lofty::read_from_path(&p).is_err());

            for info in [crate::media::read_info(&p).unwrap(), crate::media::fastread::read_info(&p).unwrap()] {
                let want = if with_id3 { (Some("Title"), Some("Artist"), true) } else { (Some("a"), None, false) };
                assert_eq!((info.title.as_deref(), info.artist.as_deref(), info.has_picture), want, "id3={with_id3}");
                assert_eq!(info.year, 2001, "id3={with_id3}");
            }
            assert_eq!(crate::media::cover::find_cover(&p).is_some(), with_id3);
            let d = crate::media::details::file_details(&p).unwrap();
            assert!(d.tag_error.is_none() && d.tag_note.as_deref().unwrap_or("").contains("APE Album"), "{:?} {:?}", d.tag_error, d.tag_note);

            let edit = crate::domain::track::TagEdit { album: Some("New Album".into()), ..Default::default() };
            crate::media::write_tags(&p, &edit).unwrap();
            let after = std::fs::read(&p).unwrap();
            let info = crate::media::read_info(&p).unwrap();
            assert_eq!(info.album.as_deref(), Some("New Album"), "id3={with_id3}");
            // APE タグ・ID3v1 は元のバイトのまま、末尾に残る
            assert_eq!(after[after.len() - 128 - ape.len()..], orig[orig.len() - 128 - ape.len()..]);
        }
    }

    /// 年の欄が日付でない枠 (TDRC) と、頭に \0 がある URL の枠 (WORS)。
    /// lofty は WORS の残りを次の枠の見出しとして読み、区切りがずれる。単独で試すと読めてしまうので、目印の枠で見つける
    #[test]
    fn finds_frames_that_shift_the_rest() {
        // 残りが小文字だと lofty はエラーを出さずに後ろの枠を捨て、大文字だとエラーになる。どちらも読めて、残る
        // 年の欄が日付でない (普通の読み方ではエラー) / 正常 (lofty はエラーを出さずに読み違える)、
        // ジャケットあり (一覧の近道 fastread を通る) / なし
        for (url, bad_year, picture) in [
            (&b"\0example"[..], true, false),
            (&b"\0EXAMPLE"[..], true, false),
            (&b"\0example"[..], false, false),
            (&b"\0example"[..], false, true),
            (&b"\0EXAMPLE"[..], false, true),
        ] {
            let case = format!("{} year={bad_year} picture={picture}", String::from_utf8_lossy(&url[1..]));
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("a.mp3");
            let mut frames = vec![
                ("TDRC", text(if bad_year { "Abcd" } else { "2005" })),
                ("TENC", text("Encoder")),
                ("WORS", url.to_vec()),
                ("TEXT", text("Writer")),
                ("TIT2", text("Title")),
                ("TPE1", text("Artist")),
            ];
            if picture {
                frames.push(("APIC", [&b"\0image/jpeg\0\x03\0\xff\xd8"[..], &[0u8; 200]].concat()));
            }
            std::fs::write(&p, mp3(4, &frames, 100)).unwrap();
            let s = salvage(&mut File::open(&p).unwrap()).unwrap();
            let ids: Vec<&str> = s.bad.iter().map(|(id, _, _)| id.as_str()).collect();
            assert_eq!(ids, if bad_year { vec!["TDRC", "WORS"] } else { vec!["WORS"] }, "{case}");
            for info in [crate::media::read_info(&p).unwrap(), crate::media::fastread::read_info(&p).unwrap()] {
                assert_eq!((info.title.as_deref(), info.artist.as_deref(), info.has_picture), (Some("Title"), Some("Artist"), picture), "{case}");
            }
            // ジャケットも読める (一覧に出す画像)
            assert_eq!(crate::media::cover::find_cover(&p).is_some(), picture, "{case}");
            // 年も入れ直す: 読めなかった元の年の枠は残さない (2 つ並ばない)
            let edit = crate::domain::track::TagEdit { album: Some("Album".into()), year: Some(2010), ..Default::default() };
            crate::media::write_tags(&p, &edit).unwrap();
            let after = std::fs::read(&p).unwrap();
            assert!(after.windows(url.len()).any(|w| w == url), "WORS は残る: {case}");
            assert_eq!(after.windows(4).filter(|w| *w == b"TDRC").count(), 1, "{case}");
            assert_eq!(crate::media::read_info(&p).unwrap().year, 2010, "{case}");
            let info = crate::media::read_info(&p).unwrap();
            assert_eq!((info.album.as_deref(), info.artist.as_deref(), info.has_picture), (Some("Album"), Some("Artist"), picture), "{case}");
        }
    }

    /// 中身が見出しの大きさより短い枠 (ピークの長さが足りない音量調整の RVA2、配布リポジトリの Issue #7)。
    /// lofty は I/O のエラー (UnexpectedEof) を返すが、ファイルを読めなかったのではないので、枠を飛ばして読む。
    /// 一覧の曲名・ジャケット・編集・ジャケットの差し替えができる
    #[test]
    fn reads_and_writes_around_truncated_frames() {
        // 識別子 "trk" + チャンネル + 調整 (2 バイト) + ピークのビット数 32 → ピークは 4 バイト要るが 2 バイトしかない
        let rva2 = b"trk\0\x01\x00\x10\x20\0\0".to_vec();
        let frames = [
            ("RVA2", rva2.clone()),
            ("TIT2", text("Title")),
            ("TALB", text("Album")),
            ("APIC", [&b"\0image/jpeg\0\x00\0\xff\xd8"[..], &[0u8; 200]].concat()),
        ];
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.mp3");
        std::fs::write(&p, mp3(4, &frames, 100)).unwrap();
        assert!(lofty::read_from_path(&p).is_err());

        for info in [crate::media::read_info(&p).unwrap(), crate::media::fastread::read_info(&p).unwrap()] {
            assert_eq!((info.title.as_deref(), info.album.as_deref(), info.has_picture), (Some("Title"), Some("Album"), true));
        }
        let edit = crate::domain::track::TagEdit { album: Some("New Album".into()), ..Default::default() };
        crate::media::write_tags(&p, &edit).unwrap();
        let mut jpg = Vec::new();
        image::DynamicImage::new_rgb8(8, 8).write_to(&mut Cursor::new(&mut jpg), image::ImageFormat::Jpeg).unwrap();
        crate::media::write_cover(&p, Some(&jpg)).unwrap();

        let info = crate::media::fastread::read_info(&p).unwrap();
        assert_eq!((info.title.as_deref(), info.album.as_deref(), info.has_picture), (Some("Title"), Some("New Album"), true));
        assert_eq!(crate::media::cover::find_cover(&p).unwrap(), jpg);
        // RVA2 は元のバイトのまま残る
        let after = std::fs::read(&p).unwrap();
        assert!(after.windows(rva2.len()).any(|w| w == rva2.as_slice()));
    }

    /// 言語の欄が英字でないコメント・歌詞の枠 (配布リポジトリの Issue #8)。lofty は読めるが書けない:
    /// 編集・ジャケットの差し替えができる・枠は言語を「不明」(XXX) にして中身を残す
    #[test]
    fn writes_frames_with_invalid_language() {
        let frames = [
            ("TIT2", text("Title")),
            ("COMM", b"\0\0\x13\0\0".to_vec()),
            ("USLT", b"\0\0\0\0\0Lyrics".to_vec()),
            ("TALB", text("Album")),
        ];
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.mp3");
        std::fs::write(&p, mp3(4, &frames, 100)).unwrap();
        assert!(lofty::read_from_path(&p).is_ok());

        let edit = crate::domain::track::TagEdit { album: Some("New Album".into()), ..Default::default() };
        crate::media::write_tags(&p, &edit).unwrap();
        let mut jpg = Vec::new();
        image::DynamicImage::new_rgb8(8, 8).write_to(&mut Cursor::new(&mut jpg), image::ImageFormat::Jpeg).unwrap();
        crate::media::write_cover(&p, Some(&jpg)).unwrap();

        let info = crate::media::read_info(&p).unwrap();
        assert_eq!((info.title.as_deref(), info.album.as_deref(), info.has_picture), (Some("Title"), Some("New Album"), true));
        let mut r = BufReader::new(File::open(&p).unwrap());
        let file = lofty::mpeg::MpegFile::read_from(&mut r, ParseOptions::new()).unwrap();
        let tag = file.id3v2().unwrap();
        assert!(tag.comments().all(|c| &c.language == b"XXX"));
        let lyrics: Vec<_> = tag.unsync_text().map(|u| (u.language, u.content.to_string())).collect();
        assert_eq!(lyrics, [(*b"XXX", "Lyrics".to_string())]);
    }
}
