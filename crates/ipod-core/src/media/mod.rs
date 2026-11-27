//! 音声ファイル: 形式の判定とタグの読み書き (lofty)・ジャケット・変換

pub mod convert;
pub mod cover;
pub mod details;
pub mod fastread;
pub mod ffinstall;
pub mod preconvert;
pub(crate) mod apesalvage;
pub(crate) mod id3salvage;
pub(crate) mod moovscan;
pub(crate) mod mp4compact;
pub(crate) mod mp4salvage;
pub mod photo;
pub mod structure;
pub mod video;

use crate::t;
use std::fs::File;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::mp4::{Mp4Codec, Mp4File};
use lofty::prelude::*;

use crate::domain::audio::{Format, MediaKind};
use crate::error::{Error, Result};

const NATIVE_MP4: &[&str] = &["m4a", "m4b", "mp4"];
const LOSSLESS: &[&str] = &["flac", "ape", "wv", "tta"];
const LOSSY: &[&str] = &[
    "ogg", "oga", "opus", "wma", "aac", "mka", "webm", "ac3", "mpc",
];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// フォルダ走査時に拾う拡張子か
pub fn is_audio_file(path: &Path) -> bool {
    let e = ext_of(path);
    matches!(e.as_str(), "mp3" | "wav" | "aif" | "aiff" | "aifc")
        || NATIVE_MP4.contains(&e.as_str())
        || LOSSLESS.contains(&e.as_str())
        || LOSSY.contains(&e.as_str())
}

pub fn classify(path: &Path) -> Result<Format> {
    lofty_options();
    let e = ext_of(path);
    Ok(match e.as_str() {
        "mp3" => Format::Mp3,
        "wav" => Format::Wav,
        "aif" | "aiff" | "aifc" => Format::Aiff,
        x if NATIVE_MP4.contains(&x) => {
            let mut f = File::open(path)?;
            let mp4 = Mp4File::read_from(&mut f, ParseOptions::new().read_tags(false))
                .map_err(|e| Error::Media(error_chain(&e)))?;
            match mp4.properties().codec() {
                Some(Mp4Codec::AAC) => Format::Aac,
                Some(Mp4Codec::ALAC) => Format::Alac,
                Some(Mp4Codec::FLAC) => Format::ConvertLossless,
                _ => Format::ConvertLossy,
            }
        }
        x if LOSSLESS.contains(&x) => Format::ConvertLossless,
        x if LOSSY.contains(&x) => Format::ConvertLossy,
        _ => return Err(Error::Unsupported(t!("未対応の形式です: .{e}", e = e))),
    })
}

#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub comment: Option<String>,
    /// グループ (ID3 の TIT1 / MP4 の ©grp / GROUPING)。Rockbox のデータベースに入れる
    pub grouping: Option<String>,
    /// 曲名がタグにあったか (無ければ title はファイル名)
    pub title_tagged: bool,
    /// タグに画像 (ジャケット) が埋め込まれているか
    pub has_picture: bool,
    /// タグに歌詞 (同期しない歌詞) があるか
    pub has_lyrics: bool,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
    pub sort_composer: Option<String>,
    pub year: u32,
    /// タグの日付 (年月日まであるとき)。その日の正午 (UTC) の UNIX 秒、無ければ 0。ポッドキャストの公開日に使う
    pub date: i64,
    pub track_nr: u32,
    pub track_total: u32,
    pub disc_nr: u32,
    pub disc_total: u32,
    pub compilation: bool,
    pub duration_ms: u32,
    pub bitrate_kbps: u32,
    pub sample_rate: u32,
    /// タグにポッドキャストの印 (ID3 の PCST・MP4 の pcst) か、ポッドキャストの URL がある
    pub podcast_tagged: bool,
}

/// lofty でファイルを読む。MP3 は、lofty が黙って枠を捨てていたら (区切りを見失った)、
/// 読めない枠を飛ばす読み方に切り替える (`id3salvage`)
fn read_tagged(path: &Path) -> std::result::Result<lofty::file::TaggedFile, lofty::error::FileParseError> {
    lofty_options();
    // M4A も、lofty が黙って項目を捨てていたら、読めない項目を飛ばす読み方にする (`mp4salvage`)
    // 読み口はバッファ付き (lofty は小さな読み取りを何度もする)
    if NATIVE_MP4.contains(&ext_of(path).as_str())
        && let Ok(f) = File::open(path)
        && let mut r = std::io::BufReader::new(f)
        && let Ok(mp4) = Mp4File::read_from(&mut r, ParseOptions::new())
    {
        if mp4_unread_items(r.get_mut(), mp4.ilst(), true) > 0
            && let Some((tf, _)) = mp4salvage::read(path)
        {
            return Ok(tf);
        }
        return Ok(mp4.into());
    }
    if ext_of(path) == "mp3"
        && let Ok(f) = File::open(path)
        && let mut r = std::io::BufReader::new(f)
        && let Ok(mp) = lofty::mpeg::MpegFile::read_from(&mut r, ParseOptions::new())
    {
        if id3salvage::lost_frames(r.get_mut(), mp.id3v2())
            && let Some((tf, _)) = id3salvage::read(path)
        {
            return Ok(tf);
        }
        return Ok(mp.into());
    }
    lofty::read_from_path(path)
}

/// 曲のファイルをタグごと読む。曲の情報・ジャケットはこれを通す
pub(crate) fn read_any(path: &Path) -> Result<lofty::file::TaggedFile> {
    // lofty が panic しても、読めない曲として扱う (`edit_tag` と同じ)
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_any_inner(path)))
        .unwrap_or_else(|_| Err(Error::Media(t!("タグの読み書きの途中で内部エラーが起きました"))))
}

fn read_any_inner(path: &Path) -> Result<lofty::file::TaggedFile> {
    match read_tagged(path) {
        Ok(tf) => Ok(tf),
        // ファイルを読めなかった (ほかのアプリが開いている・iPod の HDD の読み取りの失敗など) は、形式の問題ではないので
        // 読み方を変えずに I/O エラーで返す (呼び出し側が少し待って読み直せるように)
        Err(e) if is_io_error(&e) => Err(Error::Io(std::io::Error::other(error_chain(&e)))),
        // 読めない枠がある MP3 は、その枠を飛ばして読む (Music.app などと同じ)。緩めた読み方より先に試す
        // (緩めた読み方は、区切りを見失って後ろの枠を黙って捨てたまま「読めた」ことにする場合がある)
        // 箱の並びが途中で壊れた M4A は、壊れた部分の後ろの moov で読む
        Err(e) => id3salvage::read(path)
            .map(|(tf, _)| tf)
            .or_else(|| moovscan::read(path))
            .or_else(|| read_leniently(path))
            .ok_or_else(|| Error::Media(error_chain(&e))),
    }
}

pub fn read_info(path: &Path) -> Result<MediaInfo> {
    let tf = read_any(path)?;
    let has_picture = tf.tags().iter().any(|t| !t.pictures().is_empty());
    Ok(info_from(&tf, path, has_picture))
}

/// 読んだタグと音声の情報から MediaInfo を作る。`has_picture` は呼ぶ側が決める
/// (画像を読まずにタグを読んだときは、画像の枠があるかだけを別に調べる。fastread)
pub(crate) fn info_from(tf: &lofty::file::TaggedFile, path: &Path, has_picture: bool) -> MediaInfo {
    let p = tf.properties();
    let mut info = MediaInfo {
        has_picture,
        duration_ms: p.duration().as_millis() as u32,
        bitrate_kbps: p.audio_bitrate().or(p.overall_bitrate()).unwrap_or(0),
        sample_rate: p.sample_rate().unwrap_or(44100),
        ..Default::default()
    };

    // タグが複数あるファイル (MP3 の ID3v2 と ID3v1・APE など) は、代表のタグを先に見て、
    // 空の項目はほかのタグから補う (代表のタグにアルバム名が無いだけで「アルバムなし」にならないように)
    let mut tags: Vec<&lofty::tag::Tag> = tf.primary_tag().into_iter().collect();
    for t in tf.tags() {
        if !tags.iter().any(|x| std::ptr::eq(*x, t)) {
            tags.push(t);
        }
    }
    let text = |f: &dyn Fn(&lofty::tag::Tag) -> Option<String>| {
        tags.iter().find_map(|t| f(t).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()))
    };
    let s = |k: ItemKey| text(&|t| t.get_string(k).map(str::to_string));
    let num = |f: &dyn Fn(&lofty::tag::Tag) -> Option<u32>| tags.iter().find_map(|t| f(t).filter(|&n| n > 0)).unwrap_or(0);
    info.title = text(&|t| t.title().map(|v| v.into_owned()));
    info.artist = text(&|t| t.artist().map(|v| v.into_owned()));
    info.album = text(&|t| t.album().map(|v| v.into_owned()));
    info.genre = text(&|t| t.genre().map(|v| v.into_owned()));
    info.comment = text(&|t| t.comment().map(|v| v.into_owned()));
    info.album_artist = s(ItemKey::AlbumArtist);
    info.grouping = s(ItemKey::ContentGroup);
    info.composer = s(ItemKey::Composer);
    info.sort_title = s(ItemKey::TrackTitleSortOrder);
    info.sort_artist = s(ItemKey::TrackArtistSortOrder);
    info.sort_album = s(ItemKey::AlbumTitleSortOrder);
    info.sort_album_artist = s(ItemKey::AlbumArtistSortOrder);
    info.sort_composer = s(ItemKey::ComposerSortOrder);
    info.track_nr = num(&|t| t.track());
    info.track_total = num(&|t| t.track_total());
    info.disc_nr = num(&|t| t.disk());
    info.disc_total = num(&|t| t.disk_total());
    info.year = num(&|t| t.date().map(|d| d.year as u32));
    // 正午にしておけば、時差があっても日付がずれない
    info.date = tags
        .iter()
        .find_map(|t| t.date())
        .and_then(|d| chrono::NaiveDate::from_ymd_opt(d.year as i32, d.month? as u32, d.day? as u32))
        .and_then(|d| d.and_hms_opt(12, 0, 0))
        .map(|d| d.and_utc().timestamp())
        .unwrap_or(0);
    info.has_lyrics = tags.iter().any(|t| lyrics_of(t).is_some());
    info.compilation = s(ItemKey::FlagCompilation).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    info.podcast_tagged = s(ItemKey::FlagPodcast).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        // 番組の URL は ID3v2 では URL の枠 (WFED) なので、文字ではなく有無で見る
        || tags.iter().any(|t| t.get(ItemKey::PodcastUrl).is_some());
    info.title_tagged = info.title.is_some();
    if info.title.is_none() {
        info.title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    }
    info
}

/// エラーの文に原因 (source) の連なりを付ける。lofty の外側の文は「failed to parse Mp4 file」だけで、
/// 本当の理由 (確保の上限を超えた・moov が無いなど) は内側にある
pub(crate) fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(x) = cur {
        let m = x.to_string();
        if !m.is_empty() && !s.ends_with(&m) {
            s.push_str(": ");
            s.push_str(&m);
        }
        cur = x.source();
    }
    s
}

/// ファイルの読み取りそのものの失敗 (形式の問題ではない) か。
/// 中身が壊れている (圧縮した枠を展開できないなど) ときも lofty は I/O エラーの形で返すので、それは除く
fn is_io_error(e: &lofty::error::FileParseError) -> bool {
    use std::io::ErrorKind;
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(x) = cur {
        if let Some(io) = x.downcast_ref::<std::io::Error>() {
            // UnexpectedEof は枠の中身が見出しの大きさより短いとき (RVA2 の欠けなど、配布リポジトリの Issue #7)。形式の問題
            return !matches!(io.kind(), ErrorKind::InvalidData | ErrorKind::InvalidInput | ErrorKind::UnexpectedEof);
        }
        cur = x.source();
    }
    false
}

/// M4A の項目の鍵: 名前の 4 バイトか "----:mean:name" (読めなかった項目を見つけるときに比べる)
pub(crate) fn mp4_key(ident: &lofty::mp4::AtomIdent) -> Vec<u8> {
    use lofty::mp4::AtomIdent;
    match ident {
        AtomIdent::Fourcc(b) => b.to_vec(),
        AtomIdent::Freeform { mean, name } => format!("----:{mean}:{name}").into_bytes(),
    }
}

/// lofty の設定 (スレッドごとに効くので、lofty を呼ぶ入口で毎回通す)。
/// 埋め込みの画像が 16MB (lofty の既定の上限) を超える曲も読めるように、一度に確保してよい大きさを上げる
pub(crate) fn lofty_options() {
    use std::cell::Cell;
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    DONE.with(|done| {
        if !done.get() {
            lofty::config::apply_global_options(lofty::config::GlobalOptions::new().allocation_limit(256 * 1024 * 1024));
            done.set(true);
        }
    });
}

/// M4A のタグ (ilst) の項目のうち、lofty が読めなかったものの数。
/// lofty は壊れた項目 (`----` の名前が UTF-8 でないなど) に当たると、エラーを出さずにそこから後ろを捨てる。
/// 読めた分だけで書き戻すと、捨てた項目 (ジャケット・アルバムなど) がファイルから消える (配布リポジトリの Issue #7)。
/// 箱をたどれない普通でない形 (64 ビットの大きさなど) は確かめられないので 0。
/// `covers_read` が false (ジャケットを読まない設定で読んだ) なら covr は比べない
pub(crate) fn mp4_unread_items(f: &mut File, ilst: Option<&lofty::mp4::Ilst>, covers_read: bool) -> usize {
    use crate::media::fastread::{atom, find_atom};
    use std::collections::HashMap;
    use std::io::{Read, Seek, SeekFrom};

    // `----` の中の mean / name の文字 (版と印の 4 バイトの後)
    fn text_of(f: &mut File, at: u64, size: u64) -> Option<String> {
        let len = size.checked_sub(12)?;
        if len > 4096 {
            return None;
        }
        let mut b = vec![0u8; len as usize];
        f.seek(SeekFrom::Start(at + 12)).ok()?;
        f.read_exact(&mut b).ok()?;
        Some(String::from_utf8_lossy(&b).into_owned())
    }
    let mut read_raw = || -> Option<Vec<Vec<u8>>> {
        let len = f.metadata().ok()?.len();
        let (moov, moov_size) = find_atom(f, 0, len, b"moov")?;
        let (udta, udta_size) = find_atom(f, moov + 8, moov + moov_size, b"udta")?;
        let (meta, meta_size) = find_atom(f, udta + 8, udta + udta_size, b"meta")?;
        let mut vf = [0u8; 4];
        f.seek(SeekFrom::Start(meta + 8)).ok()?;
        f.read_exact(&mut vf).ok()?;
        let kids = if vf == [0, 0, 0, 0] { meta + 12 } else { meta + 8 };
        let (ilst, ilst_size) = find_atom(f, kids, meta + meta_size, b"ilst")?;
        let mut keys = Vec::new();
        let mut at = ilst + 8;
        while let Some((a, size, k)) = atom(f, at, ilst + ilst_size) {
            at = a + size;
            // free / skip は項目ではない。gnre は lofty が ©gen に直して読む
            if matches!(&k, b"free" | b"skip" | b"gnre") || (!covers_read && &k == b"covr") {
                continue;
            }
            if &k == b"----" {
                let mean = find_atom(f, a + 8, a + size, b"mean").and_then(|(m, ms)| text_of(f, m, ms));
                let name = find_atom(f, a + 8, a + size, b"name").and_then(|(n, ns)| text_of(f, n, ns));
                keys.push(format!("----:{}:{}", mean.unwrap_or_default(), name.unwrap_or_default()).into_bytes());
            } else {
                keys.push(k.to_vec());
            }
        }
        Some(keys)
    };
    let Some(raw) = read_raw() else { return 0 };
    let mut have: HashMap<Vec<u8>, usize> = HashMap::new();
    for a in ilst.into_iter().flatten() {
        *have.entry(mp4_key(a.ident())).or_default() += 1;
    }
    raw.into_iter()
        .filter(|k| match have.get_mut(k) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .count()
}

/// 普通に読めなかったファイルを、順に緩めた読み方で読む:
/// 1. 拡張子ではなく中身から形式を決める (中身は M4A なのに .mp3 など、拡張子と中身が違うファイル)
/// 2. 多少壊れたタグも読み飛ばす
/// 3. 長さなど (音声の部分) は諦めて、タグだけ読む (音声の先頭が見つからない MP3 など)
pub(crate) fn read_leniently(path: &Path) -> Option<lofty::file::TaggedFile> {
    lofty_options();
    use lofty::config::ParsingMode;
    use lofty::probe::Probe;
    let relaxed = || ParseOptions::new().parsing_mode(ParsingMode::Relaxed);
    let by_content = |opts: ParseOptions| Probe::open(path).ok()?.options(opts).guess_file_type().ok()?.read().ok();
    by_content(ParseOptions::new())
        .or_else(|| Probe::open(path).ok()?.options(relaxed()).read().ok())
        .or_else(|| by_content(relaxed()))
        .or_else(|| by_content(relaxed().read_properties(false)))
}

/// 転送するときの曲の種類を決める。拡張子 (.m4b など)・ポッドキャストの印・MP4 の種類 (stik)・ジャンルの順に見る。
/// MP4 の種類はタグをもう一度読むので、フォルダの読み込みではなく転送のときだけ呼ぶ
pub fn detect_kind(path: &Path, info: &MediaInfo) -> MediaKind {
    lofty_options();
    use lofty::mp4::{AtomData, AtomIdent};
    let ext = ext_of(path);
    if matches!(ext.as_str(), "m4b" | "aa" | "aax") {
        return MediaKind::Audiobook;
    }
    if info.podcast_tagged {
        return MediaKind::Podcast;
    }
    if NATIVE_MP4.contains(&ext.as_str()) {
        // stik: 2 = オーディオブック、21 = ポッドキャスト (iTunes が付ける)
        let stik = File::open(path).ok().and_then(|mut f| {
            let opts = ParseOptions::new().read_properties(false).read_cover_art(false);
            let mp4 = Mp4File::read_from(&mut f, opts).ok()?;
            mp4.ilst()?.get(&AtomIdent::Fourcc(*b"stik"))?.data().find_map(|d| match d {
                AtomData::SignedInteger(v) => Some(*v as i64),
                AtomData::UnsignedInteger(v) => Some(*v as i64),
                AtomData::Unknown { data, .. } => data.first().map(|&b| b as i64),
                _ => None,
            })
        });
        match stik {
            Some(2) => return MediaKind::Audiobook,
            Some(21) => return MediaKind::Podcast,
            _ => {}
        }
    }
    kind_from_genre(info.genre.as_deref().unwrap_or(""))
}

/// ジャンルが「Podcast」「Audiobook」などなら、その種類
fn kind_from_genre(genre: &str) -> MediaKind {
    let g = genre.trim().to_lowercase();
    match g.as_str() {
        // tr: 訳さない (タグのジャンルとの比較)
        "podcast" | "podcasts" | "ポッドキャスト" => MediaKind::Podcast,
        // tr: 訳さない (タグのジャンルとの比較)
        "audiobook" | "audiobooks" | "audio book" | "audio books" | "オーディオブック" => MediaKind::Audiobook,
        _ => MediaKind::Music,
    }
}

/// リリースの種類 ("album" / "single" / "ep" など、小文字)。配信サイトから落とした曲の
/// MEDIATYPE (独自タグ) か、MusicBrainz の RELEASETYPE。どちらも無ければ None
pub fn read_release_type(path: &Path) -> Option<String> {
    lofty_options();
    use lofty::flac::FlacFile;
    use lofty::mpeg::MpegFile;
    use lofty::ogg::{OpusFile, VorbisFile};

    // MEDIATYPE は汎用のタグにすると消えるので、形式ごとのタグから取り出してから汎用のタグにする (ファイルは 1 回だけ読む)
    let custom = |f: &mut File| -> Option<(Option<String>, lofty::file::TaggedFile)> {
        let opts = ParseOptions::new().read_properties(false).read_cover_art(false);
        Some(match ext_of(path).as_str() {
            "flac" => {
                let file = FlacFile::read_from(f, opts).ok()?;
                (file.vorbis_comments().and_then(|c| c.get("MEDIATYPE")).map(str::to_string), file.into())
            }
            "ogg" | "oga" => {
                let file = VorbisFile::read_from(f, opts).ok()?;
                (file.vorbis_comments().get("MEDIATYPE").map(str::to_string), file.into())
            }
            "opus" => {
                let file = OpusFile::read_from(f, opts).ok()?;
                (file.vorbis_comments().get("MEDIATYPE").map(str::to_string), file.into())
            }
            "mp3" => {
                let file = MpegFile::read_from(f, opts).ok()?;
                (mediatype_id3(&file), file.into())
            }
            x if NATIVE_MP4.contains(&x) => {
                let file = Mp4File::read_from(f, opts).ok()?;
                (mediatype_mp4(&file), file.into())
            }
            _ => return None,
        })
    };
    match File::open(path).ok().and_then(|mut f| custom(&mut f)) {
        Some((mediatype, tf)) => release_type_from(mediatype, &tf),
        None => release_type_from(None, &lofty::read_from_path(path).ok()?),
    }
}

/// MP3 の ID3v2 の MEDIATYPE (TXXX)
pub(crate) fn mediatype_id3(file: &lofty::mpeg::MpegFile) -> Option<String> {
    file.id3v2()?.get_user_text("MEDIATYPE").map(str::to_string)
}

/// M4A の MEDIATYPE (----:com.apple.iTunes:MEDIATYPE)
pub(crate) fn mediatype_mp4(file: &Mp4File) -> Option<String> {
    let ident = lofty::mp4::AtomIdent::Freeform { mean: "com.apple.iTunes".into(), name: "MEDIATYPE".into() };
    file.ilst()?.get(&ident)?.data().find_map(|d| match d {
        lofty::mp4::AtomData::UTF8(s) => Some(s.clone()),
        _ => None,
    })
}

/// リリースの種類を決める: 形式ごとのタグの MEDIATYPE、無ければ代表のタグの MusicBrainz の RELEASETYPE (小文字)
pub(crate) fn release_type_from(mediatype: Option<String>, tf: &lofty::file::TaggedFile) -> Option<String> {
    let v = mediatype.or_else(|| {
        let tag = tf.primary_tag().or_else(|| tf.first_tag())?;
        tag.get_string(ItemKey::MusicBrainzReleaseType).map(str::to_string)
    })?;
    let v = v.trim().to_lowercase();
    (!v.is_empty()).then_some(v)
}

/// PC のファイルのタグを書き換える (曲情報の編集用)。`None` の項目は変えない、空文字は削除。
/// 書いたあとに読み直し、編集した項目がそのとおりになっていなければエラーにする
/// (ファイルの作りによって、エラーにならずに保存されないことがあった。配布リポジトリの Issue #14)
pub fn write_tags(path: &Path, e: &crate::domain::track::TagEdit) -> Result<()> {
    edit_tag(path, |tag| edit_text(tag, e)).map_err(|e| Error::Other(t!("タグを書き込めませんでした: {e}", e = e)))?;
    if ext_of(path) == "mp3" {
        clear_id3v1(path, e).map_err(|e| Error::Other(t!("タグを書き込めませんでした: {e}", e = e)))?;
        apesalvage::clear(path, &cleared_ape_keys(e)).map_err(|e| Error::Other(t!("タグを書き込めませんでした: {e}", e = e)))?;
    }
    let bad = unsaved_fields(path, e)?;
    if !bad.is_empty() {
        return Err(Error::Other(t!("保存しましたが、読み直すと次の項目が変わっていませんでした: {f}", f = bad.join(" / "))));
    }
    Ok(())
}

/// 編集で空にした項目の APE タグのキー (小文字)。APE タグも ID3v1 と同じく、ID3v2 に無い項目を補うのに読む
fn cleared_ape_keys(e: &crate::domain::track::TagEdit) -> Vec<&'static str> {
    let cleared = |v: &Option<String>| v.as_deref().is_some_and(|s| s.trim().is_empty());
    let mut keys = Vec::new();
    for (v, k) in [
        (&e.title, &["title"][..]),
        (&e.artist, &["artist"]),
        (&e.album, &["album"]),
        (&e.album_artist, &["album artist", "albumartist"]),
        (&e.genre, &["genre"]),
        (&e.composer, &["composer"]),
    ] {
        if cleared(v) {
            keys.extend_from_slice(k);
        }
    }
    if e.year == Some(0) {
        keys.push("year");
    }
    if e.track_nr == Some(0) {
        keys.push("track");
    }
    if e.disc_nr == Some(0) {
        keys.push("disc");
    }
    if e.lyrics.as_deref().is_some_and(|s| normalize_lyrics(s).is_empty()) {
        keys.push("lyrics");
    }
    keys
}

/// MP3 の末尾の ID3v1 から、編集で空にした項目を消す。読むときは ID3v2 に無い項目を ID3v1 から補うので、
/// ID3v2 から消しただけでは元の値に戻って見える。値を変えた項目は ID3v2 が先に読まれるので ID3v1 はそのまま
fn clear_id3v1(path: &Path, e: &crate::domain::track::TagEdit) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let cleared = |v: &Option<String>| v.as_deref().is_some_and(|s| s.trim().is_empty());
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    if f.metadata()?.len() < 128 {
        return Ok(());
    }
    f.seek(SeekFrom::End(-128))?;
    let mut b = [0u8; 128];
    f.read_exact(&mut b)?;
    if &b[..3] != b"TAG" {
        return Ok(());
    }
    let old = b;
    for (v, r) in [(&e.title, 3..33), (&e.artist, 33..63), (&e.album, 63..93)] {
        if cleared(v) {
            b[r].fill(0);
        }
    }
    if e.year == Some(0) {
        b[93..97].fill(0);
    }
    // ID3v1.1 のトラック番号 (コメントの最後の 2 バイトが 0 と番号)
    if e.track_nr == Some(0) && b[125] == 0 {
        b[126] = 0;
    }
    if cleared(&e.genre) {
        b[127] = 255;
    }
    if b != old {
        f.seek(SeekFrom::End(-128))?;
        f.write_all(&b)?;
    }
    Ok(())
}

/// 書いたあとに読み直して、編集した項目がそのとおりになっているか。違う項目の名前を返す
fn unsaved_fields(path: &Path, e: &crate::domain::track::TagEdit) -> Result<Vec<String>> {
    let info = read_info(path)?;
    let mut bad = Vec::new();
    let mut text = |name: String, want: Option<Option<&str>>, got: &Option<String>| {
        if let Some(want) = want
            && got.as_deref() != want
        {
            bad.push(name);
        }
    };
    // 空文字は削除
    fn want(v: &Option<String>) -> Option<Option<&str>> {
        v.as_deref().map(|s| Some(s.trim()).filter(|s| !s.is_empty()))
    }
    // 読みは、指定が無くても表記を変えたら消す
    fn sort<'a>(v: &'a Option<String>, shown: &Option<String>) -> Option<Option<&'a str>> {
        want(v).or_else(|| shown.as_ref().map(|_| None))
    }
    // タイトルが無いと、読むときはファイル名になる
    let title = if info.title_tagged { info.title.clone() } else { None };
    text(t!("タイトル"), want(&e.title), &title);
    text(t!("アーティスト"), want(&e.artist), &info.artist);
    text(t!("アルバム"), want(&e.album), &info.album);
    text(t!("アルバムアーティスト"), want(&e.album_artist), &info.album_artist);
    text(t!("ジャンル"), want(&e.genre), &info.genre);
    text(t!("作曲者"), want(&e.composer), &info.composer);
    text(t!("タイトルの読み"), sort(&e.sort_title, &e.title), &info.sort_title);
    text(t!("アーティストの読み"), sort(&e.sort_artist, &e.artist), &info.sort_artist);
    text(t!("アルバムの読み"), sort(&e.sort_album, &e.album), &info.sort_album);
    text(t!("アルバムアーティストの読み"), sort(&e.sort_album_artist, &e.album_artist), &info.sort_album_artist);
    for (name, want, got) in [(t!("年"), e.year, info.year), (t!("トラック番号"), e.track_nr, info.track_nr), (t!("ディスク番号"), e.disc_nr, info.disc_nr)] {
        if want.is_some_and(|n| n != got) {
            bad.push(name);
        }
    }
    if let Some(v) = &e.lyrics {
        let v = normalize_lyrics(v);
        if read_lyrics(path)? != Some(v).filter(|v| !v.is_empty()) {
            bad.push(t!("歌詞"));
        }
    }
    Ok(bad)
}

fn edit_text(tag: &mut lofty::tag::Tag, e: &crate::domain::track::TagEdit) {
    use lofty::tag::{ItemKey, Tag, items::Timestamp};

    let text = |tag: &mut Tag, key: ItemKey, v: &Option<String>| {
        if let Some(v) = v.as_deref().map(str::trim) {
            if v.is_empty() {
                tag.remove_key(key);
            } else {
                tag.insert_text(key, v.to_string());
            }
        }
    };
    text(tag, ItemKey::TrackTitle, &e.title);
    text(tag, ItemKey::TrackArtist, &e.artist);
    text(tag, ItemKey::AlbumTitle, &e.album);
    text(tag, ItemKey::AlbumArtist, &e.album_artist);
    text(tag, ItemKey::Genre, &e.genre);
    text(tag, ItemKey::Composer, &e.composer);
    // 表記を変えたら、元のファイルに入っていたソート用の読みは合わなくなるので消す
    for (v, key) in [
        (&e.title, ItemKey::TrackTitleSortOrder),
        (&e.artist, ItemKey::TrackArtistSortOrder),
        (&e.album, ItemKey::AlbumTitleSortOrder),
        (&e.album_artist, ItemKey::AlbumArtistSortOrder),
        (&e.composer, ItemKey::ComposerSortOrder),
    ] {
        if v.is_some() {
            tag.remove_key(key);
        }
    }
    text(tag, ItemKey::TrackTitleSortOrder, &e.sort_title);
    text(tag, ItemKey::TrackArtistSortOrder, &e.sort_artist);
    text(tag, ItemKey::AlbumTitleSortOrder, &e.sort_album);
    text(tag, ItemKey::AlbumArtistSortOrder, &e.sort_album_artist);
    match e.year {
        Some(0) => tag.remove_date(),
        // 年が同じなら月日は残す
        Some(y) if tag.date().map(|d| d.year as u32) != Some(y) => tag.set_date(Timestamp {
            year: y as u16,
            ..Default::default()
        }),
        _ => {}
    }
    match e.track_nr {
        Some(0) => tag.remove_track(),
        Some(n) => tag.set_track(n),
        None => {}
    }
    match e.disc_nr {
        Some(0) => tag.remove_disk(),
        Some(n) => tag.set_disk(n),
        None => {}
    }
    if let Some(v) = &e.lyrics {
        tag.remove_key(ItemKey::Lyrics);
        tag.remove_key(ItemKey::UnsyncLyrics);
        let v = normalize_lyrics(v);
        if !v.is_empty() {
            // ID3v2 の歌詞の枠 (USLT) は UnsyncLyrics にしか書けない。Vorbis コメントと APE は LYRICS が広く使われている
            let key = match tag.tag_type() {
                lofty::tag::TagType::Id3v2 | lofty::tag::TagType::Mp4Ilst => ItemKey::UnsyncLyrics,
                _ => ItemKey::Lyrics,
            };
            tag.insert_text(key, v);
        }
    }
}

/// 歌詞の改行を \n にそろえ、前後の空行と行末の空白を除く
fn normalize_lyrics(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n").lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim_matches('\n').to_string()
}

/// 曲のファイルの歌詞 (無ければ None)。改行は \n にそろえる
pub fn read_lyrics(path: &Path) -> Result<Option<String>> {
    let tf = read_any(path)?;
    Ok(tf.tags().iter().find_map(lyrics_of).map(|s| normalize_lyrics(&s)).filter(|s| !s.is_empty()))
}

/// 曲に埋め込まれたジャケットが長い辺 `max` より大きいか、ベースラインの JPEG でなければ、
/// `max` までに縮めたベースラインの JPEG に差し替える。差し替えたら true。
/// Rockbox は埋め込みの JPEG を先に読み (cover.jpg より先)、表示のたびに縮小するので、大きいと表示が遅い。
/// プログレッシブの JPEG は読めない。iPod OS は埋め込みの画像を使わない (容量を使うだけ)
pub fn shrink_embedded_cover(path: &Path, max: u32) -> Result<bool> {
    let Some((bytes, true)) = cover::find_cover_src(path) else {
        return Ok(false);
    };
    // 読めない画像は、そのままにしておく
    let Some(jpg) = (!cover::fits(&bytes, max)).then(|| cover::fit_jpeg(&bytes, max)).flatten() else {
        return Ok(false);
    };
    write_cover(path, Some(&jpg))?;
    Ok(true)
}

/// 転送の前に、`src` を一時フォルダに写して埋め込みのジャケットを縮める (`shrink_embedded_cover`)。
/// 縮めなくてよい・縮められなければ None (元のファイルを送る)。iPod の HDD の上で書き直すより、PC で縮めてから送るほうが速い。
/// 使い終わった一時ファイルは呼び出し側が消す
pub fn shrunk_copy(src: &Path, max: u32) -> Option<std::path::PathBuf> {
    let Some((bytes, true)) = cover::find_cover_src(src) else {
        return None;
    };
    if cover::fits(&bytes, max) {
        return None;
    }
    let dir = std::env::temp_dir().join("zenpod");
    std::fs::create_dir_all(&dir).ok()?;
    let ext = src.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!("{:016x}.{ext}", crate::ipod::itunesdb::random_id()));
    std::fs::copy(src, &tmp).ok()?;
    match shrink_embedded_cover(&tmp, max) {
        Ok(true) => Some(tmp),
        r => {
            if let Err(e) = r {
                // tr: 訳さない (ログ)
                log::warn!("アートワークを縮められませんでした: {}: {e}", src.display());
            }
            let _ = std::fs::remove_file(&tmp);
            None
        }
    }
}

/// 曲に埋め込まれたジャケットを差し替える (元のファイルを変更する)。None なら削除。
/// 埋め込み画像はすべて外してから表紙を 1 枚だけ入れる (古い画像が先に見つかって表示されないように)
pub fn write_cover(path: &Path, image: Option<&[u8]>) -> Result<()> {
    use lofty::picture::{MimeType, Picture, PictureType};

    let picture = match image {
        Some(bytes) => {
            let img = image::load_from_memory(bytes)
                .map_err(|e| Error::Media(t!("画像を読めません: {e}", e = e)))?;
            // JPEG / PNG はそのまま、それ以外 (WebP など) は多くのプレーヤーが読める JPEG にする
            let (data, mime) = match image::guess_format(bytes) {
                Ok(image::ImageFormat::Jpeg) => (bytes.to_vec(), MimeType::Jpeg),
                Ok(image::ImageFormat::Png) => (bytes.to_vec(), MimeType::Png),
                _ => {
                    let mut out = std::io::Cursor::new(Vec::new());
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92)
                        .encode_image(&img.to_rgb8())
                        .map_err(|e| Error::Media(t!("画像を変換できません: {e}", e = e)))?;
                    (out.into_inner(), MimeType::Jpeg)
                }
            };
            Some(
                Picture::unchecked(data)
                    .pic_type(PictureType::CoverFront)
                    .mime_type(mime)
                    .build(),
            )
        }
        None => None,
    };

    let want = picture.as_ref().map(|p| p.data().to_vec());
    edit_tag(path, |tag| {
        while !tag.pictures().is_empty() {
            tag.remove_picture(0);
        }
        if let Some(pic) = picture {
            tag.push_picture(pic);
        }
    })
    .map_err(|e| Error::Other(t!("画像を書き込めませんでした: {e}", e = e)))?;
    // 書いたあとに読み直して確かめる (`write_tags` と同じ)
    let tf = read_any(path)?;
    let mut pics = tf.tags().iter().flat_map(|t| t.pictures()).map(|p| p.data());
    let saved = match &want {
        Some(d) => pics.any(|p| p == &d[..]),
        None => pics.next().is_none(),
    };
    if !saved {
        return Err(Error::Other(t!("保存しましたが、読み直すとアートワークが変わっていませんでした")));
    }
    Ok(())
}

/// ポッドキャストの回として書き込む内容 (落としたファイルに。PC の曲には使わない)
pub struct PodcastTags<'a> {
    pub title: &'a str,
    /// 番組名 (アルバムにする。iPod は番組ごとにアルバム名でまとめる)
    pub show: &'a str,
    /// 番組の作者 (ファイルにアーティストが無いときだけ使う)
    pub author: &'a str,
    /// 公開日時 (UNIX 秒、0 = 不明)
    pub published: i64,
    pub description: &'a str,
    pub feed_url: &'a str,
    pub guid: &'a str,
}

/// 落としたポッドキャストの回のタグを書き換える。曲名・アルバム (番組名)・ポッドキャストの印・番組の URL・日付。
/// 読みのタグは表記と合わなくなるので消す
pub fn tag_podcast_episode(path: &Path, p: &PodcastTags) -> Result<()> {
    use lofty::tag::{Accessor, ItemKey, items::Timestamp};
    edit_tag(path, |tag| {
        tag.set_title(p.title.to_string());
        tag.set_album(p.show.to_string());
        if tag.artist().is_none_or(|a| a.trim().is_empty()) && !p.author.is_empty() {
            tag.set_artist(p.author.to_string());
        }
        if tag.genre().is_none_or(|g| g.trim().is_empty()) {
            tag.set_genre("Podcast".to_string());
        }
        tag.remove_key(ItemKey::TrackTitleSortOrder);
        tag.remove_key(ItemKey::AlbumTitleSortOrder);
        // ID3v2 の PCST は文字の枠ではないので lofty が書けない (MP3 は番組の URL の WFED で印になる)
        if tag.tag_type() != lofty::tag::TagType::Id3v2 {
            tag.insert_text(ItemKey::FlagPodcast, "1".to_string());
        }
        if !p.feed_url.is_empty() {
            tag.insert_text(ItemKey::PodcastUrl, p.feed_url.to_string());
        }
        if !p.guid.is_empty() {
            tag.insert_text(ItemKey::PodcastGlobalUniqueId, p.guid.to_string());
        }
        if !p.description.is_empty() {
            tag.insert_text(ItemKey::PodcastDescription, p.description.to_string());
        }
        if p.published > 0
            && let Some(d) = chrono::DateTime::from_timestamp(p.published, 0)
        {
            use chrono::Datelike;
            // 日付は PC の時刻で (日本の朝 8 時の回が前の日にならないように)
            let d = d.with_timezone(&chrono::Local);
            tag.set_date(Timestamp {
                year: d.year() as u16,
                month: Some(d.month() as u8),
                day: Some(d.day() as u8),
                ..Default::default()
            });
        }
    })
    .map_err(|e| Error::Other(t!("タグを書き込めませんでした: {e}", e = e)))
}

/// Vorbis コメント (FLAC / Ogg) は、汎用の Tag で変わった項目だけを書き換える。
/// split → merge だと、同じ意味の別名 (TOTALTRACKS と TRACKTOTAL、ORGANIZATION と LABEL など) が
/// 1 つの名前にまとめ直されて値が重複するため
fn edit_vorbis(
    vc: &mut lofty::ogg::tag::VorbisComments,
    edit: impl FnOnce(&mut lofty::tag::Tag),
) -> std::result::Result<(), String> {
    use lofty::ogg::OggPictureStorage;
    use lofty::tag::{Tag, TagType};

    let before: Tag = vc.clone().into();
    let mut after = before.clone();
    edit(&mut after);

    let mut keys: Vec<ItemKey> = Vec::new();
    for item in before.items().chain(after.items()) {
        if !keys.contains(&item.key()) {
            keys.push(item.key());
        }
    }
    let values = |t: &Tag, k: ItemKey| t.get_strings(k).map(str::to_string).collect::<Vec<_>>();
    for k in keys {
        let new = values(&after, k);
        if values(&before, k) == new {
            continue;
        }
        // この項目にあたる名前 (別名を含む) をすべて消してから、新しい値を入れる
        let old: Vec<String> = vc
            .items()
            .filter(|(key, _)| ItemKey::from_key(TagType::VorbisComments, key).as_ref() == Some(&k))
            .map(|(key, _)| key.to_string())
            .collect();
        for key in old {
            vc.remove(&key).for_each(drop);
        }
        if let Some(name) = k.map_key(TagType::VorbisComments) {
            for v in new {
                vc.push(name.to_string(), v);
            }
        }
    }
    if before.pictures() != after.pictures() {
        vc.remove_pictures();
        for pic in after.pictures() {
            vc.insert_picture(pic.clone(), None).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// ファイルのタグを汎用の Tag として書き換える。
/// lofty の汎用 Tag で保存すると、汎用の項目に無いタグ (配信サイトが入れる MEDIATYPE や UPC など) が
/// 消えてしまうので、形式ごとのタグを split → 編集 → merge して、それ以外の項目を残したまま保存する
fn edit_tag(path: &Path, edit: impl FnOnce(&mut lofty::tag::Tag)) -> std::result::Result<(), String> {
    // lofty が panic しても (変わった構造のファイルで起きることがある)、エラーにして返す。
    // 呼び出し側が持っている lock を壊したり、画面の保存中の表示が終わらなくなったりしないように
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| edit_tag_inner(path, edit)))
        .unwrap_or_else(|_| Err(t!("タグの読み書きの途中で内部エラーが起きました")))
}

/// ilst を書く。lofty 0.25.5 は ilst がちょうど 8 バイト縮むと、余白の箱を書くところの assert で panic する
/// (上流の Issue #758。ファイルを書き換える前に止まる)。そのときは一時の項目を足して大きさを変えて書いてから、外して書き直す
pub(crate) fn save_ilst(f: &mut File, ilst: &lofty::mp4::Ilst) -> std::result::Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::mp4::{Atom, AtomData, AtomIdent};
    use lofty::tag::TagExt;
    use std::io::Seek;
    let save = |f: &mut File, ilst: &lofty::mp4::Ilst| -> std::result::Result<(), String> {
        f.rewind().map_err(|e| e.to_string())?;
        ilst.save_to(f, WriteOptions::default()).map_err(|e| error_chain(&e))
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| save(f, ilst))) {
        Ok(r) => r,
        Err(_) => {
            let mut padded = ilst.clone();
            let ident = AtomIdent::Freeform { mean: "com.zenpod".into(), name: "pad".into() };
            padded.insert(Atom::new(ident, AtomData::UTF8("x".into())));
            save(f, &padded)?;
            save(f, ilst)
        }
    }
}

fn edit_tag_inner(path: &Path, edit: impl FnOnce(&mut lofty::tag::Tag)) -> std::result::Result<(), String> {
    lofty_options();
    use lofty::config::WriteOptions;
    use lofty::ogg::OggPictureStorage;
    use lofty::tag::{MergeTag, SplitTag};

    fn apply<T>(tag: T, edit: impl FnOnce(&mut lofty::tag::Tag)) -> T
    where
        T: SplitTag,
        T::Remainder: MergeTag<Merged = T>,
    {
        let (rest, mut generic) = tag.split_tag();
        edit(&mut generic);
        rest.merge_tag(generic)
    }
    fn apply_id3(tag: lofty::id3::v2::Id3v2Tag, edit: impl FnOnce(&mut lofty::tag::Tag)) -> lofty::id3::v2::Id3v2Tag {
        let mut tag = apply(tag, edit);
        id3salvage::fix_languages(&mut tag);
        tag
    }
    fn save(f: &mut File, file: &impl AudioFile) -> std::result::Result<(), String> {
        save_with(f, file, WriteOptions::default())
    }
    fn save_with(f: &mut File, file: &impl AudioFile, w: WriteOptions) -> std::result::Result<(), String> {
        use std::io::Seek;
        f.rewind().map_err(|e| e.to_string())?;
        file.save_to(f, w).map_err(|e| error_chain(&e))
    }

    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| e.to_string())?;
    let opts = ParseOptions::new();
    fn err<E: std::error::Error + 'static>(e: E) -> String {
        error_chain(&e)
    }
    match ext_of(path).as_str() {
        "mp3" => {
            let mut file = match lofty::mpeg::MpegFile::read_from(&mut f, opts) {
                // lofty が黙って枠を捨てていたら、読めない枠を飛ばして読み、書くときは元のバイトのまま残す
                Ok(file) if id3salvage::lost_frames(&mut f, file.id3v2()) => match id3salvage::salvage(&mut f) {
                    Some(s) => {
                        let mut r = s.reader(path).map_err(|e| e.to_string())?;
                        let mut file = lofty::mpeg::MpegFile::read_from(&mut r, opts).map_err(err)?;
                        let tag = apply_id3(file.remove_id3v2().unwrap_or_default(), edit);
                        return s.write(&mut f, &tag);
                    }
                    None => file,
                },
                Ok(file) => file,
                // 読めない枠がある: その枠を飛ばして読み、書くときは元のバイトのまま残す
                Err(e) => {
                    let Some(s) = id3salvage::salvage(&mut f) else { return Err(err(e)) };
                    let mut r = s.reader(path).map_err(|e| e.to_string())?;
                    let mut file = lofty::mpeg::MpegFile::read_from(&mut r, opts).map_err(err)?;
                    let tag = apply_id3(file.remove_id3v2().unwrap_or_default(), edit);
                    return s.write(&mut f, &tag);
                }
            };
            // ID3v2 の版は元のまま (lofty は既定で 2.4 に変える)。iPod は 2.4 を読めないことがあり、
            // iPod の中の曲 (歌詞の編集) と変換した MP3 は 2.3 なので。タグが無ければ 2.3 で作る
            let v23 = file.id3v2().is_none_or(|t| t.original_version() != lofty::id3::v2::Id3v2Version::V4);
            let w = WriteOptions::default().use_id3v23(v23);
            let tag = apply_id3(file.remove_id3v2().unwrap_or_default(), edit);
            if id3v2_in_place(&mut f, &tag, w, v23).map_err(err)? {
                return Ok(());
            }
            // lofty は 2.3 で書くと読みの枠 (TSOP など) を捨てるので、書き直したタグ (余白付き) にあとで入れ直す
            let resort = (v23 && id3salvage::has_v24_sort(&tag)).then(|| tag.clone());
            file.set_id3v2(tag);
            // 書くのは ID3v2 だけ。APE タグ・ID3v1 は lofty に書き直させない (ファイルのまま残る)。
            // lofty 0.25.5 はヘッダーの無い APE タグを書き直すと、その前の 32 バイト (音声) を消す (上流の Issue #759)
            file.remove_ape();
            file.remove_id3v1();
            save_with(&mut f, &file, w)?;
            if let Some(tag) = resort {
                id3v2_in_place(&mut f, &tag, w, v23).map_err(err)?;
            }
            Ok(())
        }
        x if NATIVE_MP4.contains(&x) => {
            let mut file = match Mp4File::read_from(&mut f, opts) {
                Ok(file) => file,
                // 壊れた部分の後ろの moov で読めた曲 (`moovscan`) は、音声の場所が分からないので書かない
                Err(_) if moovscan::read(path).is_some() => {
                    return Err(t!("ファイルの構造の一部が壊れているため、書き換えませんでした"));
                }
                Err(e) => return Err(err(e)),
            };
            // 読めなかった項目があれば書かない (読めた分だけで書き戻すと、残りの項目が消える)
            // 読めなかった項目があれば、それを飛ばして読み、書くときは元のバイトのまま残す
            // (読めた分だけで書き戻すと、残りの項目が消える)。残せない形 (keys の箱など) なら書かない
            if mp4_unread_items(&mut f, file.ilst(), true) > 0 {
                let Some(s) = mp4salvage::salvage(&mut f) else {
                    return Err(t!("読めないタグの項目があるため、書き換えませんでした (書き換えると、それらの情報が消えます)"));
                };
                let mut r = s.reader(path).map_err(|e| e.to_string())?;
                let mut file = Mp4File::read_from(&mut r, opts).map_err(err)?;
                let tag = file.remove_ilst().unwrap_or_default();
                s.write(&mut f, apply(tag, edit))?;
            } else {
                let tag = file.remove_ilst().unwrap_or_default();
                save_ilst(&mut f, &apply(tag, edit))?;
            }
            // 縮んだ分は lofty が free の箱で埋めるので、ジャケットを縮めた・外したときの大きな余白は詰める (Issue #24)
            mp4compact::compact(&mut f).map_err(|e| e.to_string())?;
            Ok(())
        }
        "flac" => {
            let mut file = lofty::flac::FlacFile::read_from(&mut f, opts).map_err(err)?;
            // 先頭に ID3v2 が付いた FLAC (古いソフトが書く。FLAC の決まりには無く、lofty は書けない) は、
            // Vorbis コメントに無い項目と画像を写してから、保存したあとに ID3v2 を外す
            let id3: Option<lofty::tag::Tag> = file.remove_id3v2().map(Into::into);
            let mut tag = file.remove_vorbis_comments().unwrap_or_default();
            // FLAC の画像はタグの外 (PICTURE ブロック) にあるので、いったんタグに入れて一緒に編集する
            for (pic, info) in file.remove_pictures() {
                tag.insert_picture(pic, Some(info)).map_err(err)?;
            }
            edit_vorbis(&mut tag, |t| {
                if let Some(id3) = &id3 {
                    for item in id3.items() {
                        if t.get(item.key()).is_none() {
                            t.insert(item.clone());
                        }
                    }
                    if t.pictures().is_empty() {
                        id3.pictures().iter().for_each(|p| t.push_picture(p.clone()));
                    }
                }
                edit(t)
            })?;
            file.set_vorbis_comments(tag);
            if id3.is_none() && flac_in_place(&mut f, &file)? {
                return Ok(());
            }
            save(&mut f, &file)?;
            if id3.is_some() {
                strip_id3v2_prefix(&mut f).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        "ogg" | "oga" => {
            let mut file = lofty::ogg::VorbisFile::read_from(&mut f, opts).map_err(err)?;
            edit_vorbis(file.vorbis_comments_mut(), edit)?;
            save(&mut f, &file)
        }
        "opus" => {
            let mut file = lofty::ogg::OpusFile::read_from(&mut f, opts).map_err(err)?;
            edit_vorbis(file.vorbis_comments_mut(), edit)?;
            save(&mut f, &file)
        }
        "wav" => {
            let mut file = lofty::iff::wav::WavFile::read_from(&mut f, opts).map_err(err)?;
            let tag = file.remove_id3v2().unwrap_or_default();
            file.set_id3v2(apply_id3(tag, edit));
            save(&mut f, &file)
        }
        "aif" | "aiff" | "aifc" => {
            let mut file = lofty::iff::aiff::AiffFile::read_from(&mut f, opts).map_err(err)?;
            let tag = file.remove_id3v2().unwrap_or_default();
            file.set_id3v2(apply_id3(tag, edit));
            save(&mut f, &file)
        }
        "ape" => {
            let mut file = lofty::ape::ApeFile::read_from(&mut f, opts).map_err(err)?;
            let tag = file.remove_ape().unwrap_or_default();
            file.set_ape(apply(tag, edit));
            save(&mut f, &file)
        }
        "wv" => {
            let mut file = lofty::wavpack::WavPackFile::read_from(&mut f, opts).map_err(err)?;
            let tag = file.remove_ape().unwrap_or_default();
            file.set_ape(apply(tag, edit));
            save(&mut f, &file)
        }
        _ => {
            // 上以外 (まれな形式) は汎用のタグで保存する (汎用の項目に無いタグは残らない)
            drop(f);
            let mut tf = lofty::read_from_path(path).map_err(err)?;
            if tf.primary_tag().is_none() {
                let ty = tf.primary_tag_type();
                tf.insert_tag(lofty::tag::Tag::new(ty));
            }
            // tr: 訳さない (起きない前提の panic の文言)
            edit(tf.primary_tag_mut().expect("直前に作成済み"));
            tf.save_to_path(path, WriteOptions::default()).map_err(err)
        }
    }
}

// lofty はタグの長さが変わると、その後ろ (音声) をすべてずらしてファイルを丸ごと書き直す (FLAC は PADDING を使わない、
// ID3v2 は毎回 1024 バイトの余白を足すので長さがほぼ必ず変わる)。iPod の HDD では 1 曲 2〜4 秒かかり、
// アルバム 1 枚の名前を変えるのに 1 分近くかかっていた。新しいタグが今の場所 (余白を含む) に収まるなら、
// 余白の長さで差を吸収して、タグの部分だけをその場で書く。収まらないときは今までどおり lofty に任せる

/// 読み込むメタデータの上限 (これより大きいのは普通でないので lofty に任せる)
const IN_PLACE_MAX: u64 = 32 * 1024 * 1024;

/// その場で書いたときに残してよい余白。ジャケットを縮めた・外したときは元の画像の分 (数 MB) が余白として残り、
/// 容量が減らないので、これを超えるときはファイルを書き直して詰める (配布リポジトリの Issue #24)
pub(crate) const MAX_SLACK: u64 = 64 * 1024;

/// FLAC: メタデータのブロック (先頭の "fLaC" から最後のブロックまで) をその場で書き直す。書いたら true
fn flac_in_place(f: &mut File, file: &lofty::flac::FlacFile) -> std::result::Result<bool, String> {
    use lofty::config::WriteOptions;
    use std::io::{Cursor, Read, Seek, SeekFrom, Write};
    let io = |e: std::io::Error| e.to_string();
    // 今のメタデータの長さ (ID3v2 が前に付いた FLAC は扱わない)
    f.seek(SeekFrom::Start(0)).map_err(io)?;
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() || &magic != b"fLaC" {
        return Ok(false);
    }
    let mut end = 4u64;
    loop {
        let mut h = [0u8; 4];
        f.read_exact(&mut h).map_err(io)?;
        let len = u64::from(u32::from_be_bytes([0, h[1], h[2], h[3]]));
        end += 4 + len;
        if end > IN_PLACE_MAX {
            return Ok(false);
        }
        if h[0] & 0x80 != 0 {
            break;
        }
        f.seek(SeekFrom::Current(len as i64)).map_err(io)?;
    }
    // メタデータの部分だけをメモリに写し、そこへ lofty に書かせる (音声が無いので速い)。
    // 後ろに音声の代わりの数バイトを付ける (lofty は書き換える範囲がファイルの末尾までだと、縮めるときに長さを誤る)
    const TAIL: usize = 16;
    let mut head = vec![0u8; end as usize + TAIL];
    f.seek(SeekFrom::Start(0)).map_err(io)?;
    f.read_exact(&mut head[..end as usize]).map_err(io)?;
    let mut cur = Cursor::new(head);
    file.save_to(&mut cur, WriteOptions::default()).map_err(|e| error_chain(&e))?;
    let mut new = cur.into_inner();
    if new.len() < TAIL + 4 || new[new.len() - TAIL..].iter().any(|&b| b != 0) {
        return Ok(false);
    }
    new.truncate(new.len() - TAIL);
    // 書いたブロックから PADDING を外し、元の長さとの差を PADDING にする
    let mut blocks: Vec<(u8, &[u8])> = Vec::new();
    let mut at = 4usize;
    while at + 4 <= new.len() {
        let ty = new[at] & 0x7f;
        let len = u32::from_be_bytes([0, new[at + 1], new[at + 2], new[at + 3]]) as usize;
        let Some(body) = new.get(at + 4..at + 4 + len) else { return Ok(false) };
        if ty != 1 {
            blocks.push((ty, body));
        }
        at += 4 + len;
        if new[at - 4 - len] & 0x80 != 0 {
            break;
        }
    }
    if &new[..4] != b"fLaC" || blocks.first().map(|b| b.0) != Some(0) {
        return Ok(false);
    }
    let used: u64 = 4 + blocks.iter().map(|b| 4 + b.1.len() as u64).sum::<u64>();
    let pad = match end.checked_sub(used) {
        Some(0) => None,
        // 大きく縮んだ (ジャケットを縮めた・外した) ときは、余白にせず書き直して詰める
        Some(d) if d > MAX_SLACK => return Ok(false),
        // PADDING のブロックは見出しの 4 バイトが要る。長さは 24 ビットまで
        Some(d) if d >= 4 && d - 4 < 1 << 24 => Some((d - 4) as usize),
        _ => return Ok(false),
    };
    let mut out = Vec::with_capacity(end as usize);
    out.extend_from_slice(b"fLaC");
    let n = blocks.len();
    for (k, (ty, body)) in blocks.iter().enumerate() {
        let last = pad.is_none() && k + 1 == n;
        let len = body.len() as u32;
        out.push(ty | if last { 0x80 } else { 0 });
        out.extend_from_slice(&len.to_be_bytes()[1..]);
        out.extend_from_slice(body);
    }
    if let Some(p) = pad {
        out.push(0x80 | 1);
        out.extend_from_slice(&(p as u32).to_be_bytes()[1..]);
        out.resize(out.len() + p, 0);
    }
    debug_assert_eq!(out.len() as u64, end);
    f.seek(SeekFrom::Start(0)).map_err(io)?;
    f.write_all(&out).map_err(io)?;
    Ok(true)
}

/// FLAC の先頭の ID3v2 を外す (すぐ後ろが fLaC のときだけ)
fn strip_id3v2_prefix(f: &mut File) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::Start(0))?;
    let mut h = [0u8; 10];
    if f.read_exact(&mut h).is_err() || &h[..3] != b"ID3" || h[6..].iter().any(|b| b & 0x80 != 0) {
        return Ok(());
    }
    let end = 10 + h[6..].iter().fold(0u64, |a, &b| (a << 7) | u64::from(b)) + if h[5] & 0x10 != 0 { 10 } else { 0 };
    let mut magic = [0u8; 4];
    f.seek(SeekFrom::Start(end))?;
    if f.read_exact(&mut magic).is_err() || &magic != b"fLaC" {
        return Ok(());
    }
    id3salvage::rewrite_head(f, &[], end)
}

/// MP3: 先頭の ID3v2 を、今の長さ (余白を含む) のまま書き直す。続けて重なったタグがあれば、それも含めて 1 つにする。書いたら true
fn id3v2_in_place(f: &mut File, tag: &lofty::id3::v2::Id3v2Tag, w: lofty::config::WriteOptions, v23: bool) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom, Write};
    f.seek(SeekFrom::Start(0))?;
    let mut h = [0u8; 10];
    if f.read_exact(&mut h).is_err() || &h[..3] != b"ID3" || h[5] & 0x10 != 0 || h[6..].iter().any(|b| b & 0x80 != 0) {
        return Ok(false);
    }
    let size = h[6..].iter().fold(0u64, |a, &b| (a << 7) | u64::from(b));
    let first = 10 + size;
    // 後ろに続けて重なったタグも、まとめて 1 つのタグに置き換える (lofty は 1 つ目のタグだけを置き換える)
    let old = id3salvage::stacked_end(f, first);
    // 余白なしで書いて、足りない分を 0 で埋める (ID3v2 の余白は 0 の並び)
    let Ok(mut new) = id3salvage::dump(tag, w, v23) else {
        return Ok(false);
    };
    if new.len() < 10 || &new[..3] != b"ID3" || new[5] & 0x10 != 0 {
        return Ok(false);
    }
    // 大きく縮んだ (ジャケットを縮めた・外した) ときは、余白にせず書き直して詰める
    let shrunk = new.len() as u64 + MAX_SLACK < old;
    let fits = new.len() as u64 <= old && !shrunk;
    if !fits && !shrunk && old == first {
        return Ok(false);
    }
    // 収まらない (重なったタグがあるとき)・大きく縮んだときは、余白 1024 バイトで書き直す
    new.resize(if fits { old as usize } else { new.len() + 1024 }, 0);
    let body = new.len() as u64 - 10;
    for (k, b) in new[6..10].iter_mut().enumerate() {
        *b = ((body >> (7 * (3 - k))) & 0x7f) as u8;
    }
    if !fits {
        id3salvage::rewrite_head(f, &new, old)?;
        return Ok(true);
    }
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&new)?;
    Ok(true)
}

/// 元のファイルのタグとジャケットを、変換してできた m4a に写す (afconvert はタグを書かないため)。
/// 汎用のタグを経由するので、汎用の項目に無い独自のタグは写らない
pub fn copy_tags_to_m4a(src: &Path, dst: &Path) -> std::result::Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::tag::TagType;
    let Some(mut tag) = read_source_tag(src)? else {
        return Ok(());
    };
    tag.re_map(TagType::Mp4Ilst);
    tag.save_to_path(dst, WriteOptions::default()).map_err(|e| error_chain(&e))
}

/// 元のファイルの埋め込み画像と歌詞を、ffmpeg で変換してできたファイルに写す。変換は -vn で画像を落とし、
/// 歌詞も FLAC などの UNSYNCEDLYRICS は落とす・MP3 には歌詞の枠 (USLT) でなく TXXX で書くため。
/// 変換先にすでにあるものは写さない
pub fn copy_pictures_and_lyrics(src: &Path, dst: &Path) -> std::result::Result<(), String> {
    let Some(tag) = read_source_tag(src)? else {
        return Ok(());
    };
    let pictures = tag.pictures().to_vec();
    let lyrics = lyrics_of(&tag);
    if pictures.is_empty() && lyrics.is_none() {
        return Ok(());
    }
    edit_tag(dst, |t| {
        if t.pictures().is_empty() {
            pictures.into_iter().for_each(|p| t.push_picture(p));
        }
        if let Some(l) = lyrics
            && lyrics_of(t).is_none()
        {
            t.insert_text(ItemKey::UnsyncLyrics, l);
        }
    })
}

/// タグの歌詞 (ID3v2 の USLT は UnsyncLyrics、M4A の ©lyr と Vorbis の LYRICS は Lyrics になる)
pub(crate) fn lyrics_of(t: &lofty::tag::Tag) -> Option<String> {
    [ItemKey::UnsyncLyrics, ItemKey::Lyrics]
        .into_iter()
        .find_map(|k| t.get_string(k).map(str::trim).filter(|s| !s.is_empty()))
        .map(str::to_string)
}

/// 元のファイルのタグを、画像も含めて汎用の Tag として読む (タグが無ければ None)
fn read_source_tag(src: &Path) -> std::result::Result<Option<lofty::tag::Tag>, String> {
    lofty_options();
    use lofty::ogg::OggPictureStorage;
    fn err<E: std::error::Error + 'static>(e: E) -> String {
        error_chain(&e)
    }
    if ext_of(src) == "flac" {
        let mut f = File::open(src).map_err(|e| e.to_string())?;
        let mut file = lofty::flac::FlacFile::read_from(&mut f, ParseOptions::new()).map_err(err)?;
        let mut vc = file.remove_vorbis_comments().unwrap_or_default();
        // FLAC の画像はタグの外 (PICTURE ブロック) にあるので、タグに入れてから一緒に写す
        for (pic, info) in file.remove_pictures() {
            vc.insert_picture(pic, Some(info)).map_err(err)?;
        }
        Ok(Some(vc.into()))
    } else {
        let tf = lofty::read_from_path(src).map_err(err)?;
        Ok(tf.primary_tag().or_else(|| tf.first_tag()).cloned())
    }
}

#[cfg(test)]
mod tests {

    /// 落としたポッドキャストの回に書いたタグが読め、転送のときにポッドキャストと判定される (MP3 と M4A)
    #[test]
    fn tags_podcast_episodes() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("zenpod-podtag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, codec) in [("a.mp3", "libmp3lame"), ("b.m4a", "aac")] {
            let p = dir.join(name);
            let ok = std::process::Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", codec, "-metadata", "title=ep_final_v2", "-metadata", "artist=Someone"])
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            assert_eq!(detect_kind(&p, &read_info(&p).unwrap()), MediaKind::Music);
            let tags = PodcastTags {
                title: "第 12 回 ゲスト回",
                show: "番組 & Show",
                author: "Host",
                published: 1727650800,
                description: "説明",
                feed_url: "https://example.com/feed.xml",
                guid: "ep-12",
            };
            tag_podcast_episode(&p, &tags).unwrap();
            let info = read_info(&p).unwrap();
            assert_eq!(info.title.as_deref(), Some("第 12 回 ゲスト回"), "{name}");
            assert_eq!(info.album.as_deref(), Some("番組 & Show"), "{name}");
            // ファイルにあったアーティストは残す
            assert_eq!(info.artist.as_deref(), Some("Someone"), "{name}");
            assert_eq!(info.genre.as_deref(), Some("Podcast"), "{name}");
            assert_eq!(info.year, 2024, "{name}");
            assert!(info.date > 1727568000 && info.date < 1727827200, "{name}: {}", info.date);
            assert!(info.podcast_tagged, "{name}");
            assert_eq!(detect_kind(&p, &info), MediaKind::Podcast, "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 拡張子と中身が違うファイル (中身は M4A なのに .mp3) も、中身から形式を決めて読む
    #[test]
    fn reads_files_with_wrong_extension() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("zenpod-wrongext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m4a = dir.join("a.m4a");
        let ok = std::process::Command::new(&ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=2", "-c:a", "aac", "-metadata", "title=Song", "-metadata", "album=Album", "-metadata", "artist=Artist"])
            .arg(&m4a)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mp3 = dir.join("Song - Artist.mp3");
        std::fs::rename(&m4a, &mp3).unwrap();
        let info = read_info(&mp3).unwrap();
        assert_eq!(info.album.as_deref(), Some("Album"));
        assert_eq!(info.artist.as_deref(), Some("Artist"));
        assert!(info.duration_ms > 1500, "長さも読める");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ID3v2 にアルバム名が無く、ID3v1 にだけある MP3 は、ID3v1 から補う
    #[test]
    fn fills_missing_fields_from_other_tags() {
        use lofty::id3::v1::Id3v1Tag;
        use lofty::id3::v2::Id3v2Tag;
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let p = std::env::temp_dir().join(format!("zenpod-tags-{}.mp3", std::process::id()));
        let ok = std::process::Command::new(&ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "libmp3lame", "-map_metadata", "-1", "-write_id3v1", "0", "-id3v2_version", "0"])
            .arg(&p)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut v2 = Id3v2Tag::new();
        v2.set_title("Song".into());
        v2.set_artist("Artist".into());
        v2.save_to_path(&p, lofty::config::WriteOptions::default()).unwrap();
        let mut v1 = Id3v1Tag::new();
        v1.title = Some("Song".into());
        v1.album = Some("From ID3v1".into());
        v1.year = Some(2001);
        v1.save_to_path(&p, lofty::config::WriteOptions::default()).unwrap();
        let info = read_info(&p).unwrap();
        assert_eq!(info.title.as_deref(), Some("Song"));
        assert_eq!(info.artist.as_deref(), Some("Artist"));
        assert_eq!(info.album.as_deref(), Some("From ID3v1"));
        assert_eq!(info.year, 2001);
        let _ = std::fs::remove_file(&p);
    }

    use super::*;

    fn encode(img: image::RgbImage, fmt: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, fmt).unwrap();
        out.into_inner()
    }

    /// ffmpeg がある環境だけで確かめる (無音の短い曲を作って、ジャケットとタグを書き換える)
    #[test]
    fn write_cover_and_tags_keep_other_tags() {
        let Some(ffmpeg) = which_ffmpeg() else { return };
        let dir = std::env::temp_dir().join(format!("zenpod-cover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = encode(image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30])), image::ImageFormat::Png);
        let webp = encode(image::RgbImage::from_pixel(8, 8, image::Rgb([30, 30, 200])), image::ImageFormat::WebP);
        for ext in ["mp3", "m4a", "flac", "ogg", "opus"] {
            let f = dir.join(format!("a.{ext}"));
            // 配信サイトが入れるような独自タグ付きで作る (m4a は独自タグを書くのにフラグが要る)
            let ok = std::process::Command::new(&ffmpeg)
                .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "1"])
                .args(["-metadata", "title=曲", "-metadata", "album=アルバム", "-metadata", "MEDIATYPE=single"])
                .arg(&f)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                continue;
            }
            if ext == "m4a" {
                // ffmpeg は m4a の独自タグを iTunes 形式 (----:com.apple.iTunes:*) で書けないので直接入れる
                use lofty::mp4::{Atom, AtomData, AtomIdent};
                let mut file = std::fs::OpenOptions::new().read(true).write(true).open(&f).unwrap();
                let mut mp4 = Mp4File::read_from(&mut file, ParseOptions::new()).unwrap();
                let mut ilst = mp4.remove_ilst().unwrap_or_default();
                let ident = AtomIdent::Freeform { mean: "com.apple.iTunes".into(), name: "MEDIATYPE".into() };
                ilst.insert(Atom::new(ident, AtomData::UTF8("single".into())));
                mp4.set_ilst(ilst);
                use std::io::Seek;
                file.rewind().unwrap();
                mp4.save_to(&mut file, lofty::config::WriteOptions::default()).unwrap();
            }
            assert_eq!(read_release_type(&f).as_deref(), Some("single"), "{ext}");
            assert_eq!(fastread::read_info_release(&f, true).unwrap().1.as_deref(), Some("single"), "{ext}");

            write_cover(&f, Some(&png)).unwrap();
            assert_eq!(crate::media::cover::find_cover(&f).as_deref(), Some(&png[..]), "{ext}");
            // 画像があると、一覧のための読み取りは画像を飛ばす読み方になる。そこでも同じ値を取り出せる
            assert_eq!(fastread::read_info_release(&f, true).unwrap().1.as_deref(), Some("single"), "{ext}");
            // 差し替えると前の画像は残らない (WebP は JPEG にして入れる)
            write_cover(&f, Some(&webp)).unwrap();
            let got = crate::media::cover::find_cover(&f).unwrap();
            assert_eq!(image::guess_format(&got).unwrap(), image::ImageFormat::Jpeg, "{ext}");

            let edit = crate::domain::track::TagEdit { album: Some("別のアルバム".into()), ..Default::default() };
            write_tags(&f, &edit).unwrap();
            let info = read_info(&f).unwrap();
            assert_eq!(info.album.as_deref(), Some("別のアルバム"), "{ext}");
            assert_eq!(info.title.as_deref(), Some("曲"), "{ext}");
            // タグを書き換えても画像は残る
            assert!(crate::media::cover::find_cover(&f).is_some(), "{ext}");

            // 読み (ソート用タグ) を書ける。表記と一緒に変えたときは指定した読みが残る
            let edit = crate::domain::track::TagEdit {
                title: Some("新しい曲".into()),
                sort_title: Some("アタラシイキョク".into()),
                sort_artist: Some("カシュ".into()),
                ..Default::default()
            };
            write_tags(&f, &edit).unwrap();
            let info = read_info(&f).unwrap();
            assert_eq!(info.sort_title.as_deref(), Some("アタラシイキョク"), "{ext}");
            assert_eq!(info.sort_artist.as_deref(), Some("カシュ"), "{ext}");
            // 空にすると消える
            let edit = crate::domain::track::TagEdit { sort_artist: Some(String::new()), ..Default::default() };
            write_tags(&f, &edit).unwrap();
            assert_eq!(read_info(&f).unwrap().sort_artist, None, "{ext}");
            assert_eq!(read_info(&f).unwrap().sort_title.as_deref(), Some("アタラシイキョク"), "{ext}");
            let edit = crate::domain::track::TagEdit { title: Some("曲".into()), ..Default::default() };
            write_tags(&f, &edit).unwrap();

            write_cover(&f, None).unwrap();
            assert!(crate::media::cover::find_cover(&f).is_none(), "{ext}");
            // 汎用の項目に無いタグも、画像やタグを書き換えたあとに残っている
            assert_eq!(read_release_type(&f).as_deref(), Some("single"), "{ext}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ffmpeg がある環境だけ: afconvert の出力 (タグ無しの m4a) に、FLAC のタグとジャケットを写せる
    #[test]
    fn copy_flac_tags_to_m4a() {
        let Some(ffmpeg) = which_ffmpeg() else { return };
        let dir = std::env::temp_dir().join(format!("zenpod-copytags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (flac, m4a) = (dir.join("a.flac"), dir.join("a.m4a"));
        let run = |args: &[&str], out: &Path| {
            let ok = std::process::Command::new(&ffmpeg)
                .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", "1"])
                .args(args)
                .arg(out)
                .status()
                .unwrap()
                .success();
            assert!(ok);
        };
        run(&["-metadata", "title=曲", "-metadata", "artist=歌手", "-metadata", "album=アルバム", "-metadata", "track=3/12"], &flac);
        run(&["-c:a", "alac", "-map_metadata", "-1"], &m4a);
        let png = encode(image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30])), image::ImageFormat::Png);
        write_cover(&flac, Some(&png)).unwrap();

        copy_tags_to_m4a(&flac, &m4a).unwrap();
        let info = read_info(&m4a).unwrap();
        assert_eq!(info.title.as_deref(), Some("曲"));
        assert_eq!(info.artist.as_deref(), Some("歌手"));
        assert_eq!(info.album.as_deref(), Some("アルバム"));
        assert_eq!((info.track_nr, info.track_total), (3, 12));
        assert_eq!(crate::media::cover::find_cover(&m4a).as_deref(), Some(&png[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ffmpeg がある環境だけ: タグの書き換えは、余白に収まるならファイルの長さと音声を変えずにその場で書く。
    /// 収まらないときは今までどおり書き直す。どちらも ffmpeg で最後まで読める
    #[test]
    fn writes_tags_in_place() {
        let Some(ffmpeg) = which_ffmpeg() else { return };
        let dir = std::env::temp_dir().join(format!("zenpod-inplace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let make = |args: &[&str], out: &Path| {
            let ok = std::process::Command::new(&ffmpeg)
                .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=duration=3", "-metadata", "title=曲", "-metadata", "album=アルバム"])
                .args(args)
                .arg(out)
                .status()
                .unwrap()
                .success();
            assert!(ok);
        };
        let decodes = |p: &Path| {
            std::process::Command::new(&ffmpeg)
                .args(["-nostdin", "-v", "error", "-xerror", "-i"])
                .arg(p)
                .args(["-f", "null", "-"])
                .status()
                .unwrap()
                .success()
        };
        let edit = |album: &str| crate::domain::track::TagEdit { album: Some(album.into()), ..Default::default() };
        let tail = |p: &Path, n: usize| {
            let b = std::fs::read(p).unwrap();
            b[b.len() - n..].to_vec()
        };

        // FLAC (ffmpeg は PADDING を入れる)。画像も入れておく
        let flac = dir.join("a.flac");
        make(&[], &flac);
        let png = encode(image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30])), image::ImageFormat::Png);
        write_cover(&flac, Some(&png)).unwrap();
        let len = std::fs::metadata(&flac).unwrap().len();
        let audio = tail(&flac, 20000);
        write_tags(&flac, &edit("だから僕は音楽を辞めた")).unwrap();
        assert_eq!(std::fs::metadata(&flac).unwrap().len(), len, "長さは変わらない");
        assert_eq!(tail(&flac, 20000), audio, "音声はそのまま");
        assert_eq!(read_info(&flac).unwrap().album.as_deref(), Some("だから僕は音楽を辞めた"));
        assert_eq!(read_info(&flac).unwrap().title.as_deref(), Some("曲"));
        assert_eq!(crate::media::cover::find_cover(&flac).as_deref(), Some(&png[..]));
        assert!(decodes(&flac));
        // 短くしても同じ
        write_tags(&flac, &edit("A")).unwrap();
        assert_eq!(std::fs::metadata(&flac).unwrap().len(), len);
        assert_eq!(read_info(&flac).unwrap().album.as_deref(), Some("A"));
        // 余白に収まらないときは書き直す
        let long = "長".repeat(20000);
        write_tags(&flac, &edit(&long)).unwrap();
        assert!(std::fs::metadata(&flac).unwrap().len() > len);
        assert_eq!(read_info(&flac).unwrap().album.as_deref(), Some(long.as_str()));
        assert_eq!(tail(&flac, 20000), audio);
        assert!(decodes(&flac));

        // MP3 (ffmpeg の ID3v2 は余白なし): 今のタグより長くなると、lofty が余白を付けて書き直す。次からはその場で書く
        let mp3 = dir.join("a.mp3");
        make(&["-c:a", "libmp3lame"], &mp3);
        let audio = tail(&mp3, 20000);
        let before = std::fs::metadata(&mp3).unwrap().len();
        write_tags(&mp3, &edit(&"長いアルバム名".repeat(10))).unwrap();
        assert!(std::fs::metadata(&mp3).unwrap().len() > before);
        let len = std::fs::metadata(&mp3).unwrap().len();
        write_tags(&mp3, &edit("だから僕は音楽を辞めた")).unwrap();
        assert_eq!(std::fs::metadata(&mp3).unwrap().len(), len, "長さは変わらない");
        assert_eq!(tail(&mp3, 20000), audio);
        let info = read_info(&mp3).unwrap();
        assert_eq!(info.album.as_deref(), Some("だから僕は音楽を辞めた"));
        assert_eq!(info.title.as_deref(), Some("曲"));
        assert!(info.duration_ms > 2500);
        assert!(decodes(&mp3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn which_ffmpeg() -> Option<std::path::PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
    }
}
