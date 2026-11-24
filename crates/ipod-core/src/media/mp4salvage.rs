//! lofty が読めない項目がある M4A を読み書きする。
//!
//! lofty 0.25 は、ilst の中の壊れた項目 (`----` の名前が UTF-8 でない・種類の分からないジャケットなど) に当たると、
//! エラーを出さずにその項目、またはそこから後ろの項目を捨てる。読めた分だけで書き戻すと、捨てた項目が
//! ファイルから消える (配布リポジトリの Issue #7)。ここでは項目を 1 つずつ lofty に読ませて読めない項目を見つけ、
//! それを除いた ilst で読む。書くときは、読めない項目を元のバイトのまま ilst の末尾に戻す (消さない)。
//! `keys` の箱を使う形 (項目を番号で指す) は、普通の項目を足してもほかのアプリで読めないので扱わない。

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType, TaggedFile};
use lofty::mp4::{AtomData, AtomIdent, Ilst, Mp4File};
use lofty::probe::Probe;

use crate::media::fastread::{Seg, Virtual, atom, find_atom};

/// これより大きい ilst は扱わない (普通でない)
const MAX_ILST: u64 = 256 * 1024 * 1024;
/// 目印の項目 (後ろに付けて、ここまで正しく読めたかを見る)
const SENTINEL: &[u8; 4] = b"zsnt";
const SENTINEL_TEXT: &str = "zenpod-sentinel";

/// 箱の位置と大きさ (見出しを含む)
#[derive(Clone, Copy)]
struct Span {
    at: u64,
    size: u64,
}

impl Span {
    fn end(&self) -> u64 {
        self.at + self.size
    }
}

/// moov/udta/meta/ilst の位置と、ilst の中の項目
struct Layout {
    moov: Span,
    udta: Span,
    meta: Span,
    ilst: Span,
    items: Vec<Span>,
}

fn layout(f: &mut File) -> Option<Layout> {
    let len = f.metadata().ok()?.len();
    let (at, size) = find_atom(f, 0, len, b"moov")?;
    let moov = Span { at, size };
    let (at, size) = find_atom(f, moov.at + 8, moov.end(), b"udta")?;
    let udta = Span { at, size };
    let (at, size) = find_atom(f, udta.at + 8, udta.end(), b"meta")?;
    let meta = Span { at, size };
    // meta は版と印の 4 バイトの後に箱が続く (付いていない作りもある)
    let mut vf = [0u8; 4];
    f.seek(SeekFrom::Start(meta.at + 8)).ok()?;
    f.read_exact(&mut vf).ok()?;
    let kids = if vf == [0, 0, 0, 0] { meta.at + 12 } else { meta.at + 8 };
    // keys の箱を使う形は扱わない
    if find_atom(f, kids, meta.end(), b"keys").is_some() {
        return None;
    }
    let (at, size) = find_atom(f, kids, meta.end(), b"ilst")?;
    let ilst = Span { at, size };
    if ilst.size > MAX_ILST {
        return None;
    }
    let mut items = Vec::new();
    let mut p = ilst.at + 8;
    while let Some((at, size, _)) = atom(f, p, ilst.end()) {
        items.push(Span { at, size });
        p = at + size;
    }
    // 項目の並びが ilst の終わりまで届かない (大きさの壊れた項目がある) なら扱わない
    if p != ilst.end() {
        return None;
    }
    Some(Layout { moov, udta, meta, ilst, items })
}

fn mk_atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// 項目の鍵 (`media::mp4_key` と同じ形): 名前の 4 バイトか "----:mean:name"
fn raw_key(item: &[u8]) -> Vec<u8> {
    let kind = &item[4..8];
    if kind != b"----" {
        return kind.to_vec();
    }
    let child = |name: &[u8]| -> String {
        let mut p = 8;
        while p + 12 <= item.len() {
            let size = u32::from_be_bytes([item[p], item[p + 1], item[p + 2], item[p + 3]]) as usize;
            if size < 8 || p + size > item.len() {
                break;
            }
            if &item[p + 4..p + 8] == name && size >= 12 {
                return String::from_utf8_lossy(&item[p + 12..p + size]).into_owned();
            }
            p += size;
        }
        String::new()
    };
    format!("----:{}:{}", child(b"mean"), child(b"name")).into_bytes()
}

/// 項目 1 つを lofty が読めるか。読めなければ理由。後ろに目印の項目を付け、目印まで正しく読めたかも見る
fn item_error(item: &[u8]) -> Option<String> {
    let sentinel = mk_atom(SENTINEL, &mk_atom(b"data", &[&1u32.to_be_bytes()[..], &[0; 4], SENTINEL_TEXT.as_bytes()].concat()));
    let ilst = mk_atom(b"ilst", &[item, &sentinel].concat());
    let hdlr = mk_atom(b"hdlr", &[&[0u8; 8][..], b"mdirappl", &[0; 9]].concat());
    let meta = mk_atom(b"meta", &[&[0u8; 4][..], &hdlr, &ilst].concat());
    let file = [mk_atom(b"ftyp", b"M4A \0\0\0\0M4A "), mk_atom(b"moov", &mk_atom(b"udta", &meta))].concat();
    let parsed = match Mp4File::read_from(&mut Cursor::new(file), ParseOptions::new().read_properties(false)) {
        Ok(f) => f,
        Err(e) => return Some(crate::media::error_chain(&e)),
    };
    let ilst = parsed.ilst();
    let sentinel_ok = ilst
        .and_then(|i| i.get(&AtomIdent::Fourcc(*SENTINEL)))
        .and_then(|a| a.data().next())
        .is_some_and(|d| matches!(d, AtomData::UTF8(s) if s == SENTINEL_TEXT));
    // tr: 訳さない (診断用。英語の lofty のエラーと並ぶ)
    if !sentinel_ok {
        return Some("the items after this one were misread".to_string());
    }
    let key = raw_key(item);
    let have: Vec<Vec<u8>> = ilst.into_iter().flatten().map(|a| crate::media::mp4_key(a.ident())).collect();
    // gnre は lofty が ©gen に直して読む
    let ok = if key == b"gnre" { have.iter().any(|k| k == b"\xa9gen" || k == b"gnre") } else { have.contains(&key) };
    // tr: 訳さない (診断用)
    (!ok).then(|| "this item was dropped".to_string())
}

/// 読めない項目を除いて組み直した ilst
pub(crate) struct Salvaged {
    layout: Layout,
    /// 読める項目だけの ilst の中身 (見出しを除く)
    good: Vec<u8>,
    /// 読めない項目 (鍵, 理由, 元のバイト)
    pub bad: Vec<(String, String, Vec<u8>)>,
    len: u64,
}

/// ilst の項目を調べ、読めない項目があれば除いて組み直す。読めない項目が無い・普通でない形なら None
pub(crate) fn salvage(f: &mut File) -> Option<Salvaged> {
    let len = f.metadata().ok()?.len();
    let layout = layout(f)?;
    let mut good = Vec::new();
    let mut bad = Vec::new();
    for item in &layout.items {
        let mut bytes = vec![0u8; item.size as usize];
        f.seek(SeekFrom::Start(item.at)).ok()?;
        f.read_exact(&mut bytes).ok()?;
        match item_error(&bytes) {
            None => good.extend_from_slice(&bytes),
            Some(why) => bad.push((String::from_utf8_lossy(&raw_key(&bytes)).into_owned(), why, bytes)),
        }
    }
    if bad.is_empty() {
        return None;
    }
    Some(Salvaged { layout, good, bad, len })
}

impl Salvaged {
    /// 読める項目だけの ilst に差し替えたファイル (ファイルのように読める)
    pub(crate) fn reader(&self, path: &Path) -> std::io::Result<BufReader<Virtual>> {
        let l = &self.layout;
        let removed = (l.ilst.size - 8) - self.good.len() as u64;
        // 大きさを書き直す箱 (4 バイト) と、差し替える ilst の中身を、ファイルの位置の順に並べる
        let size_of = |s: Span| Some(((s.size - removed) as u32).to_be_bytes().to_vec());
        let cuts: Vec<(u64, u64, Option<Vec<u8>>)> = vec![
            (l.moov.at, 4, size_of(l.moov)),
            (l.udta.at, 4, size_of(l.udta)),
            (l.meta.at, 4, size_of(l.meta)),
            (l.ilst.at, 4, size_of(l.ilst)),
            (l.ilst.at + 8, l.ilst.size - 8, Some(self.good.clone())),
        ];
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
        if pos < self.len {
            segs.push(Seg::File { start: pos, len: self.len - pos });
        }
        Ok(BufReader::new(Virtual::new(File::open(path)?, segs)))
    }

    /// 読めなかった項目の一覧 ("鍵: 理由" を改行でつなぐ)
    pub(crate) fn describe(&self) -> String {
        self.bad.iter().map(|(k, why, _)| format!("{k}: {why}")).collect::<Vec<_>>().join("\n")
    }

    /// 編集した ilst を書く。lofty に書かせたあと、読めなかった項目を元のバイトのまま ilst の末尾に戻す。
    /// すぐ後ろに余白 (free) があればそこに収め、無ければ親の箱の大きさと音声の位置の表 (stco / co64) を直す
    pub(crate) fn write(&self, f: &mut File, ilst: Ilst) -> Result<(), String> {
        let io = |e: std::io::Error| e.to_string();
        // 編集で同じ鍵の項目を書いたなら、読めなかった元の項目は置き換わった
        let written: Vec<Vec<u8>> = (&ilst).into_iter().map(|a| crate::media::mp4_key(a.ident())).collect();
        let extra: Vec<u8> = self
            .bad
            .iter()
            .filter(|(_, _, raw)| !written.contains(&raw_key(raw)))
            .flat_map(|(_, _, raw)| raw.iter().copied())
            .collect();
        crate::media::save_ilst(f, &ilst)?;
        if extra.is_empty() {
            return Ok(());
        }
        let n = extra.len() as u64;
        let l = layout(f).ok_or_else(|| crate::t!("タグを書けない形です"))?;
        let pos = l.ilst.end();
        let write_size = |f: &mut File, at: u64, size: u64| -> Result<(), String> {
            let size = u32::try_from(size).map_err(|_| crate::t!("タグを書けない形です"))?;
            f.seek(SeekFrom::Start(at)).map_err(io)?;
            f.write_all(&size.to_be_bytes()).map_err(io)
        };
        // ilst のすぐ後ろの余白に収まるなら、その場で書く (ほかの箱の大きさも音声の位置も変わらない)
        if let Some((at, size, kind)) = atom(f, pos, l.meta.end())
            && at == pos
            && matches!(&kind, b"free" | b"skip")
            && size >= n + 8
        {
            f.seek(SeekFrom::Start(pos)).map_err(io)?;
            f.write_all(&extra).map_err(io)?;
            f.write_all(&((size - n) as u32).to_be_bytes()).map_err(io)?;
            f.write_all(b"free").map_err(io)?;
            return write_size(f, l.ilst.at, l.ilst.size + n);
        }
        // 収まらない: 後ろをずらして差し込み、親の箱の大きさと、後ろにずれた音声を指す位置を直す
        let mut rest = Vec::new();
        f.seek(SeekFrom::Start(pos)).map_err(io)?;
        f.read_to_end(&mut rest).map_err(io)?;
        f.seek(SeekFrom::Start(pos)).map_err(io)?;
        f.write_all(&extra).map_err(io)?;
        f.write_all(&rest).map_err(io)?;
        for s in [l.ilst, l.meta, l.udta, l.moov] {
            write_size(f, s.at, s.size + n)?;
        }
        let mut moov = vec![0u8; (l.moov.size + n) as usize];
        f.seek(SeekFrom::Start(l.moov.at)).map_err(io)?;
        f.read_exact(&mut moov).map_err(io)?;
        crate::media::convert::patch_chunk_offsets(&mut moov, &|o| if o >= pos { o + n } else { o }).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(l.moov.at)).map_err(io)?;
        f.write_all(&moov).map_err(io)
    }
}

/// lofty が黙って項目を捨てていた M4A を、読めない項目を除いて読む。そうでない・扱えない形なら None
pub(crate) fn read(path: &Path) -> Option<(TaggedFile, Salvaged)> {
    let s = salvage(&mut File::open(path).ok()?)?;
    let tf = Probe::with_file_type(s.reader(path).ok()?, FileType::Mp4).options(ParseOptions::new()).read().ok()?;
    Some((tf, s))
}

/// lofty は読めたが黙って項目を捨てていた M4A を、読めない項目を除いて読み直す。そうでなければ None
pub(crate) fn read_if_lost(path: &Path) -> Option<(TaggedFile, Salvaged)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "m4a" | "m4b" | "mp4") {
        return None;
    }
    let mut r = BufReader::new(File::open(path).ok()?);
    let mp4 = Mp4File::read_from(&mut r, ParseOptions::new()).ok()?;
    if crate::media::mp4_unread_items(r.get_mut(), mp4.ilst(), true) == 0 {
        return None;
    }
    read(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::config::WriteOptions;
    use std::path::PathBuf;

    fn ffmpeg() -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file()))
    }

    /// ffmpeg で 2 秒の M4A を作り、lofty で独自項目 (先頭) とジャケットを足す。`faststart` なら目次 (moov) を音声の前に置く
    fn make(ff: &Path, dir: &Path, faststart: bool, cover: usize) -> PathBuf {
        let p = dir.join(format!("a{faststart}{cover}.m4a"));
        let mut cmd = std::process::Command::new(ff);
        cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=2", "-c:a", "aac"]);
        if faststart {
            cmd.args(["-movflags", "+faststart"]);
        }
        assert!(cmd.args(["-metadata", "album=Album", "-metadata", "artist=Artist"]).arg(&p).status().unwrap().success());
        let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
        let mut file = Mp4File::read_from(&mut f, ParseOptions::new()).unwrap();
        let old = file.remove_ilst().unwrap();
        let mut ilst = Ilst::default();
        let ident = AtomIdent::Freeform { mean: "com.apple.iTunes".into(), name: "zzname".into() };
        ilst.insert(lofty::mp4::Atom::new(ident, AtomData::UTF8("value".into())));
        for a in old {
            ilst.insert(a);
        }
        // 本物の JPEG (ffmpeg が入力を調べるときに画像も読むため)。大きいほうは模様を入れて大きくする
        let side = if cover > 1000 { 200 } else { 16 };
        let mut seed = 1u32;
        let img = image::RgbImage::from_fn(side, side, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
        });
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90).encode_image(&img).unwrap();
        let jpeg = jpeg.into_inner();
        assert!(jpeg.len() >= cover.min(20000), "{}", jpeg.len());
        ilst.insert_picture(lofty::picture::Picture::unchecked(jpeg).mime_type(lofty::picture::MimeType::Jpeg).build());
        file.set_ilst(ilst);
        file.save_to(&mut f, WriteOptions::default()).unwrap();
        p
    }

    /// ffmpeg で最後まで読めるか (音声の位置の表が正しいか)
    fn decodes(ff: &Path, p: &Path) -> bool {
        let out = std::process::Command::new(ff).args(["-v", "error", "-i"]).arg(p).args(["-map", "0:a", "-f", "null", "-"]).output().unwrap();
        if !out.stderr.is_empty() {
            eprintln!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr));
        }
        out.status.success() && out.stderr.is_empty()
    }

    /// lofty が読めない項目がある M4A (配布リポジトリの Issue #7): 読める・編集できる・読めない項目は元のバイトのまま残る。
    /// 余白に収まる場合と、収まらずに後ろをずらす場合 (目次が音声の前 / 後) の両方
    #[test]
    fn reads_and_writes_around_unreadable_items() {
        let Some(ff) = ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        // (目次が前か, ジャケットの大きさ, ジャケットも読めなくするか)
        for (faststart, cover, bad_cover) in [(false, 500, false), (true, 500, false), (false, 30000, true), (true, 30000, true)] {
            let case = format!("faststart={faststart} cover={cover} bad_cover={bad_cover}");
            let p = make(&ff, dir.path(), faststart, cover);
            let mut bytes = std::fs::read(&p).unwrap();
            // 独自項目の名前を UTF-8 でない字にする (長さは同じ)。lofty はここから後ろの項目を黙って捨てる
            let at = bytes.windows(6).position(|w| w == b"zzname").unwrap();
            bytes[at] = 0xE9;
            // ジャケットの種類を知らない番号にする (lofty はジャケットを捨てる。大きいので余白に収まらない)
            if bad_cover {
                let at = bytes.windows(4).position(|w| w == b"covr").unwrap();
                bytes[at + 12 + 3] = 99;
            }
            std::fs::write(&p, &bytes).unwrap();
            let s = salvage(&mut File::open(&p).unwrap()).unwrap();
            assert_eq!(s.bad.len(), if bad_cover { 2 } else { 1 }, "{case}");

            let info = crate::media::read_info(&p).unwrap();
            assert_eq!((info.album.as_deref(), info.artist.as_deref()), (Some("Album"), Some("Artist")), "{case}");
            assert_eq!(info.has_picture, !bad_cover, "{case}");
            // ジャケットも読める (一覧に出す画像)
            assert_eq!(crate::media::cover::find_cover(&p).is_some(), !bad_cover, "{case}");
            let fast = crate::media::fastread::read_info(&p).unwrap();
            assert_eq!(fast.artist.as_deref(), Some("Artist"), "{case}");

            let edit = crate::domain::track::TagEdit { title: Some("New".into()), ..Default::default() };
            crate::media::write_tags(&p, &edit).unwrap();
            let after = std::fs::read(&p).unwrap();
            for (_, _, raw) in &s.bad {
                assert!(after.windows(raw.len()).any(|w| w == raw.as_slice()), "読めない項目は残る: {case}");
            }
            let info = crate::media::read_info(&p).unwrap();
            assert_eq!(
                (info.title.as_deref(), info.album.as_deref(), info.artist.as_deref()),
                (Some("New"), Some("Album"), Some("Artist")),
                "{case}"
            );
            assert!(info.duration_ms > 1900, "{case}");
            assert!(decodes(&ff, &p), "音声を最後まで読める: {case}");
            // もう一度編集しても同じ (読めない項目が 2 つに増えない)
            crate::media::write_tags(&p, &crate::domain::track::TagEdit { title: Some("Again".into()), ..Default::default() }).unwrap();
            assert_eq!(salvage(&mut File::open(&p).unwrap()).unwrap().bad.len(), s.bad.len(), "{case}");
            assert_eq!(crate::media::read_info(&p).unwrap().title.as_deref(), Some("Again"), "{case}");
            assert!(decodes(&ff, &p), "{case}");
        }
    }

    /// 16MB (lofty の既定の上限) を超えるジャケットが入った M4A も、読めて編集できる (`lofty_options`)
    #[test]
    fn handles_huge_cover() {
        let Some(ff) = ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let p = make(&ff, dir.path(), false, 500);
        {
            let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
            let mut file = Mp4File::read_from(&mut f, ParseOptions::new()).unwrap();
            let mut ilst = file.remove_ilst().unwrap();
            ilst.remove_pictures();
            let mut big = vec![0xFF, 0xD8, 0xFF];
            big.resize(20 * 1024 * 1024, 7);
            ilst.insert_picture(lofty::picture::Picture::unchecked(big).mime_type(lofty::picture::MimeType::Jpeg).build());
            file.set_ilst(ilst);
            file.save_to(&mut f, WriteOptions::default()).unwrap();
        }
        let info = crate::media::read_info(&p).unwrap();
        assert!(info.has_picture);
        assert_eq!(info.artist.as_deref(), Some("Artist"));
        let edit = crate::domain::track::TagEdit { title: Some("New".into()), ..Default::default() };
        crate::media::write_tags(&p, &edit).unwrap();
        let info = crate::media::read_info(&p).unwrap();
        assert_eq!((info.title.as_deref(), info.has_picture), (Some("New"), true));
        assert!(std::fs::metadata(&p).unwrap().len() > 20 * 1024 * 1024, "ジャケットは残る");
    }

    /// keys の箱を使う形 (項目を番号で指す) は、普通の項目を足してもほかのアプリで読めないので書かない
    #[test]
    fn leaves_keys_style_alone() {
        let Some(ff) = ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k.m4a");
        let ok = std::process::Command::new(&ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "aac", "-movflags", "use_metadata_tags"])
            .args(["-metadata", "album=Album", "-metadata", "zzname=value"])
            .arg(&p)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let before = std::fs::read(&p).unwrap();
        assert!(salvage(&mut File::open(&p).unwrap()).is_none());
        let edit = crate::domain::track::TagEdit { title: Some("New".into()), ..Default::default() };
        assert!(crate::media::write_tags(&p, &edit).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), before, "ファイルを変えない");
    }
}
