//! 曲のファイルの詳細 (形式・音質・タグの中身・埋め込み画像) を読む。「詳細情報」の画面用。
//! 汎用の `Tag` に変換すると対応表に無い項目が見えなくなるので、タグは形式ごとに読んで
//! ファイルに入っているままのキーで並べる。

use crate::t;
use std::fs::File;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::FileType;
use lofty::id3::v2::{Frame, Id3v2Tag, Id3v2Version};
use lofty::mp4::{AtomData, AtomIdent, Mp4Codec, Mp4File};
use lofty::mpeg::{ChannelMode, Emphasis, Layer, MpegFile, MpegVersion};
use lofty::prelude::*;
use lofty::tag::ItemValue;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::media;
use crate::domain::audio::Format;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDetails {
    pub path: String,
    pub file_size: u64,
    /// 更新日時 (UNIX 秒)。取れなければ 0
    pub modified: i64,
    /// 入れ物の形式 ("MPEG-4 オーディオ (M4A)" など)
    pub container: String,
    /// 符号化方式 ("AAC-LC" / "ALAC" / "MPEG-1 Layer III" など)
    pub codec: String,
    pub lossless: Option<bool>,
    pub duration_ms: u64,
    /// kbps
    pub audio_bitrate: Option<u32>,
    pub overall_bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    /// 形式に固有の情報 (項目名, 値)
    pub extra: Vec<(String, String)>,
    /// iPod に送るときの扱い (PC の曲だけ意味がある)
    pub transfer: String,
    pub tags: Vec<TagBlock>,
    pub pictures: Vec<PictureInfo>,
    /// タグの読み取りで出たエラー (原因の連なり付き)。読めなくても形式と音質は出す
    pub tag_error: Option<String>,
    /// 読めない項目を飛ばして読めたときのお知らせ (飛ばした項目と理由)。エラーではない (情報は読めていて、編集もできる)
    pub tag_note: Option<String>,
    /// タグの構造 (名前と大きさだけ。`media::structure`)。タグを読めない曲の報告に貼ってもらう
    pub structure: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagBlock {
    /// "ID3v2.4" / "Vorbis コメント" / "MP4 (iTunes)" など
    pub kind: String,
    pub items: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PictureInfo {
    /// "表紙" など
    pub kind: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

pub fn file_details(path: &Path) -> Result<FileDetails> {
    media::lofty_options();
    let meta = std::fs::metadata(path)?;
    // 普通に読めないファイル (拡張子と中身が違うなど) は、曲の情報と同じ緩めた読み方で読む。
    // それでも読めなければタグを読まずに読み、形式と音質だけでも出す (エラーは tag_error に)
    // 読めない枠がある MP3・読めない項目がある M4A は、それを飛ばして読む (salvaged。タグの一覧もそこから)
    let mut tag_error = None;
    let mut tag_note = None;
    let mut salvaged = None;
    let mut salvaged_mp4 = None;
    let skipped = |list: String| t!("次のタグの項目を読めなかったため、飛ばして読みました (編集しても元のまま残します):\n{a}", a = list);
    let tf = match lofty::read_from_path(path) {
        // lofty は読めたが、区切りを見失って黙って枠を捨てていた MP3 / 項目を捨てていた M4A
        Ok(tf) => {
            if let Some((tf, s)) = media::id3salvage::read_if_lost(path) {
                tag_note = Some(skipped(s.describe()));
                salvaged = Some(s);
                tf
            } else if let Some((tf, s)) = media::mp4salvage::read_if_lost(path) {
                tag_note = Some(skipped(s.describe()));
                salvaged_mp4 = Some(s);
                tf
            } else {
                tf
            }
        }
        Err(e) => {
            let reason = media::error_chain(&e);
            // 読めない枠がある MP3 は、緩めた読み方より先に、その枠を飛ばして読む
            if let Some((tf, s)) = media::id3salvage::read(path) {
                tag_note = Some(skipped(s.describe()));
                salvaged = Some(s);
                tf
            } else if let Some(tf) = media::moovscan::read(path) {
                tag_error = Some(t!(
                    "ファイルの構造の一部が壊れていたため、壊れた部分を飛ばして読みました (この曲のタグは書き換えられません):\n{a}",
                    a = reason
                ));
                tf
            } else if let Some(tf) = media::read_leniently(path) {
                tag_error = Some(reason);
                tf
            } else {
                let opts = ParseOptions::new().read_tags(false);
                let tf = lofty::probe::Probe::open(path).ok().and_then(|p| p.options(opts).guess_file_type().ok()?.read().ok());
                tag_error = Some(reason.clone());
                tf.ok_or(Error::Media(reason))?
            }
        }
    };
    let p = tf.properties();
    let mut d = FileDetails {
        path: path.to_string_lossy().into_owned(),
        file_size: meta.len(),
        modified: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        duration_ms: p.duration().as_millis() as u64,
        audio_bitrate: p.audio_bitrate().filter(|&b| b > 0),
        overall_bitrate: p.overall_bitrate().filter(|&b| b > 0),
        sample_rate: p.sample_rate().filter(|&r| r > 0),
        bit_depth: p.bit_depth().filter(|&b| b > 0),
        channels: p.channels().filter(|&c| c > 0),
        tag_error,
        tag_note,
        structure: media::structure::tag_structure(path).unwrap_or_default(),
        ..Default::default()
    };

    let ft = tf.file_type();
    let (container, codec, lossless) = base_format(ft);
    d.container = crate::i18n::tr(container);
    d.codec = codec;
    d.lossless = lossless;

    // 形式ごとの詳しい情報とタグ (タグを読めなかったファイルは、タグを読まずにコーデックなどだけ)
    let opts = ParseOptions::new().read_cover_art(false).read_tags(d.tag_error.is_none() || salvaged.is_some() || salvaged_mp4.is_some());
    let mut specific_tags = false;
    match ft {
        FileType::Mpeg => {
            let mpeg = match &salvaged {
                Some(s) => MpegFile::read_from(&mut s.reader(path)?, opts),
                None => MpegFile::read_from(&mut File::open(path)?, opts),
            };
            if let Ok(f) = mpeg {
                let mp = f.properties();
                d.codec = mpeg_codec(mp);
                let mode = match mp.channel_mode() {
                    ChannelMode::Stereo => t!("ステレオ"),
                    ChannelMode::JointStereo => t!("ジョイントステレオ"),
                    ChannelMode::DualChannel => t!("デュアルチャンネル"),
                    ChannelMode::SingleChannel => t!("モノラル"),
                };
                d.extra.push((t!("チャンネルモード"), mode));
                if let Some(e) = mp.emphasis() {
                    let e = match e {
                        Emphasis::MS5015 => "50/15 ms".to_string(),
                        Emphasis::Reserved => t!("予約値"),
                        Emphasis::CCIT_J17 => "CCITT J.17".to_string(),
                    };
                    d.extra.push((t!("エンファシス"), e));
                }
                if let Some(t) = f.id3v2() {
                    d.tags.push(id3v2_block(t));
                }
                if let Some(t) = f.ape() {
                    d.tags.push(ape_block(t));
                }
                if let Some(t) = f.id3v1() {
                    d.tags.push(generic_block("ID3v1", &lofty::tag::Tag::from(t.clone())));
                }
                specific_tags = true;
            }
        }
        FileType::Mp4 => {
            let mp4 = match &salvaged_mp4 {
                Some(s) => Mp4File::read_from(&mut s.reader(path)?, opts),
                None => Mp4File::read_from(&mut File::open(path)?, opts),
            };
            if let Ok(f) = mp4 {
                let mp = f.properties();
                let (codec, lossless) = mp4_codec(mp);
                d.codec = codec;
                d.lossless = Some(lossless);
                let ftyp = mp.ftyp().trim();
                if !ftyp.is_empty() {
                    d.extra.push((t!("ブランド (ftyp)"), ftyp.into()));
                }
                if mp.is_drm_protected() {
                    d.extra.push(("DRM".into(), t!("保護あり (再生できない可能性)")));
                }
                let moov_last = crate::media::convert::moov_after_mdat(path);
                d.extra.push((
                    t!("目次 (moov) の位置"),
                    if moov_last {
                        t!("末尾 (転送時に先頭へ並べ替える)")
                    } else {
                        t!("先頭")
                    },
                ));
                if let Some(t) = f.ilst() {
                    d.tags.push(ilst_block(t));
                }
                // 飛ばして読めない形 (keys の箱など) で、読めなかった項目があれば知らせる
                let unread = if salvaged_mp4.is_some() { 0 } else { media::mp4_unread_items(&mut File::open(path)?, f.ilst(), false) };
                if unread > 0 && d.tag_error.is_none() {
                    d.tag_error = Some(t!("タグの一部の項目を読めませんでした (この曲のタグは書き換えられません)"));
                }
                specific_tags = true;
            }
        }
        FileType::Flac => {
            if let Ok(f) = lofty::flac::FlacFile::read_from(&mut File::open(path)?, opts) {
                let md5 = f.properties().signature();
                if md5 != 0 {
                    d.extra.push((t!("音声の MD5"), format!("{md5:032x}")));
                }
                if let Some(t) = f.vorbis_comments() {
                    d.tags.push(vorbis_block(t.items()));
                }
                if let Some(t) = f.id3v2() {
                    d.tags.push(id3v2_block(t));
                }
                specific_tags = true;
            }
        }
        FileType::Vorbis => {
            if let Ok(f) = lofty::ogg::VorbisFile::read_from(&mut File::open(path)?, opts) {
                d.tags.push(vorbis_block(f.vorbis_comments().items()));
                specific_tags = true;
            }
        }
        FileType::Opus => {
            if let Ok(f) = lofty::ogg::OpusFile::read_from(&mut File::open(path)?, opts) {
                d.tags.push(vorbis_block(f.vorbis_comments().items()));
                specific_tags = true;
            }
        }
        FileType::Wav => {
            if let Ok(f) = lofty::iff::wav::WavFile::read_from(&mut File::open(path)?, opts) {
                if let Some(t) = f.id3v2() {
                    d.tags.push(id3v2_block(t));
                }
                if let Some(t) = f.riff_info() {
                    let items = t.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                    d.tags.push(TagBlock { kind: "RIFF INFO".into(), items });
                }
                specific_tags = true;
            }
        }
        FileType::Aiff => {
            if let Ok(f) = lofty::iff::aiff::AiffFile::read_from(&mut File::open(path)?, opts) {
                if let Some(t) = f.id3v2() {
                    d.tags.push(id3v2_block(t));
                }
                if let Some(t) = f.text_chunks() {
                    d.tags.push(generic_block(&t!("AIFF テキスト"), &lofty::tag::Tag::from(t.clone())));
                }
                specific_tags = true;
            }
        }
        _ => {}
    }
    if !specific_tags {
        for t in tf.tags() {
            d.tags.push(generic_block(&format!("{:?}", t.tag_type()), t));
        }
    }

    // 埋め込み画像 (FLAC の PICTURE ブロックも lofty が Vorbis コメント側にまとめる)
    for t in tf.tags() {
        for pic in t.pictures() {
            let (width, height) = imagesize::blob_size(pic.data())
                .map(|s| (s.width as u32, s.height as u32))
                .unwrap_or((0, 0));
            d.pictures.push(PictureInfo {
                // MP4 の covr には種類が無い (Other になる) ので表紙として扱う
                kind: if ft == FileType::Mp4 { t!("表紙") } else { picture_kind(pic.pic_type()) },
                mime: pic.mime_type().map(|m| m.as_str().to_string()).unwrap_or_default(),
                width,
                height,
                bytes: pic.data().len(),
            });
        }
    }

    d.transfer = match media::classify(path) {
        Ok(Format::ConvertLossless) => t!("ALAC (16bit) に変換して転送"),
        Ok(Format::ConvertLossy) => t!("AAC 256kbps に変換して転送"),
        Ok(Format::Alac) if d.bit_depth.is_some_and(|b| b > 16) || d.sample_rate.is_some_and(|r| r > 48000) => {
            t!("そのまま転送 (iPod classic は 16bit / 48kHz を超えると再生できないことがある)")
        }
        Ok(_) => t!("そのまま転送"),
        Err(_) => t!("転送できない形式"),
    };
    Ok(d)
}

/// 一覧の下に出す形式の要約。タグも画像も読まない
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSummary {
    pub codec: String,
    pub lossless: Option<bool>,
    /// kbps
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
}

pub fn audio_summary(path: &Path) -> Result<AudioSummary> {
    media::lofty_options();
    let opts = ParseOptions::new().read_tags(false).read_cover_art(false);
    let tf = lofty::probe::Probe::open(path)
        .and_then(|p| p.options(opts).read())
        .map_err(|e| Error::Media(media::error_chain(&e)))?;
    let p = tf.properties();
    let ft = tf.file_type();
    let (container, mut codec, mut lossless) = base_format(ft);
    match ft {
        FileType::Mpeg => {
            if let Ok(f) = MpegFile::read_from(&mut File::open(path)?, opts) {
                codec = mpeg_codec(f.properties());
            }
        }
        FileType::Mp4 => {
            if let Ok(f) = Mp4File::read_from(&mut File::open(path)?, opts) {
                let (c, l) = mp4_codec(f.properties());
                codec = c;
                lossless = Some(l);
            }
        }
        _ => {}
    }
    if codec.is_empty() {
        codec = crate::i18n::tr(container);
    }
    Ok(AudioSummary {
        codec,
        lossless,
        bitrate: p.audio_bitrate().or(p.overall_bitrate()).filter(|&b| b > 0),
        sample_rate: p.sample_rate().filter(|&r| r > 0),
        bit_depth: p.bit_depth().filter(|&b| b > 0),
        channels: p.channels().filter(|&c| c > 0),
    })
}

/// (入れ物の形式, 符号化方式, 可逆か)。MP3 と MP4 の符号化方式は中身を読んで決める
fn base_format(ft: FileType) -> (&'static str, String, Option<bool>) {
    match ft {
        // tr: 使う側で訳す (入れ物の形式の名前)
        FileType::Mpeg => ("MPEG オーディオ", String::new(), Some(false)),
        // tr: 使う側で訳す
        FileType::Mp4 => ("MPEG-4 オーディオ", String::new(), None),
        FileType::Flac => ("FLAC", "FLAC".into(), Some(true)),
        FileType::Wav => ("WAV", "PCM".into(), Some(true)),
        FileType::Aiff => ("AIFF", "PCM".into(), Some(true)),
        FileType::Aac => ("AAC (ADTS)", "AAC".into(), Some(false)),
        FileType::Vorbis => ("Ogg", "Vorbis".into(), Some(false)),
        FileType::Opus => ("Ogg", "Opus".into(), Some(false)),
        FileType::Speex => ("Ogg", "Speex".into(), Some(false)),
        FileType::Ape => ("Monkey's Audio", "APE".into(), Some(true)),
        FileType::WavPack => ("WavPack", "WavPack".into(), None),
        FileType::Mpc => ("Musepack", "Musepack".into(), Some(false)),
        // tr: 使う側で訳す
        _ => ("不明", String::new(), None),
    }
}

fn mpeg_codec(mp: &lofty::mpeg::MpegProperties) -> String {
    let v = match mp.version() {
        MpegVersion::V1 => "MPEG-1",
        MpegVersion::V2 => "MPEG-2",
        MpegVersion::V2_5 => "MPEG-2.5",
        MpegVersion::V4 => "MPEG-4",
    };
    let l = match mp.layer() {
        Layer::Layer1 => "Layer I",
        Layer::Layer2 => "Layer II",
        Layer::Layer3 => "Layer III",
    };
    format!("{v} {l}")
}

fn mp4_codec(mp: &lofty::mp4::Mp4Properties) -> (String, bool) {
    match mp.codec() {
        Some(Mp4Codec::AAC) => {
            let aot = mp.audio_object_type().map(|a| format!("{a:?}")).unwrap_or_default();
            let name = match aot.as_str() {
                "AacLowComplexity" => "AAC-LC".to_string(),
                "AacMain" => "AAC Main".to_string(),
                "SpectralBandReplication" => "HE-AAC (SBR)".to_string(),
                "ParametricStereo" => "HE-AAC v2 (PS)".to_string(),
                "" => "AAC".to_string(),
                x => format!("AAC ({x})"),
            };
            (name, false)
        }
        Some(Mp4Codec::ALAC) => ("ALAC (Apple Lossless)".into(), true),
        Some(Mp4Codec::FLAC) => ("FLAC".into(), true),
        Some(Mp4Codec::MP3) => ("MP3".into(), false),
        _ => (t!("不明"), false),
    }
}

fn picture_kind(t: lofty::picture::PictureType) -> String {
    use lofty::picture::PictureType as P;
    match t {
        P::CoverFront => t!("表紙"),
        P::CoverBack => t!("裏表紙"),
        P::Leaflet => t!("リーフレット"),
        P::Media => t!("ディスク面"),
        P::Artist | P::LeadArtist => t!("アーティスト"),
        P::Icon | P::OtherIcon => t!("アイコン"),
        P::Other => t!("その他"),
        x => format!("{x:?}"),
    }
}

fn vorbis_block<'a>(items: impl Iterator<Item = (&'a str, &'a str)>) -> TagBlock {
    TagBlock {
        kind: t!("Vorbis コメント"),
        items: items.map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

fn id3v2_block(t: &Id3v2Tag) -> TagBlock {
    let ver = match t.original_version() {
        Id3v2Version::V2 => "ID3v2.2",
        Id3v2Version::V3 => "ID3v2.3",
        Id3v2Version::V4 => "ID3v2.4",
    };
    let mut items = Vec::new();
    for f in t {
        let id = f.id_str().to_string();
        let (key, val) = match f {
            Frame::Text(x) => (id, x.value.replace('\0', " / ")),
            Frame::UserText(x) => (format!("{id}:{}", x.description), x.content.replace('\0', " / ")),
            Frame::Comment(x) => (comment_key(&id, &x.description, &x.language), x.content.to_string()),
            Frame::UnsynchronizedText(x) => (
                comment_key(&id, &x.description, &x.language),
                t!("歌詞 {n} 文字", n = x.content.chars().count()),
            ),
            Frame::Url(x) => (id, x.url().to_string()),
            Frame::UserUrl(x) => (format!("{id}:{}", x.description), x.content.to_string()),
            Frame::Timestamp(x) => (id, x.timestamp.to_string()),
            Frame::Popularimeter(x) => (id, t!("{a} 評価 {b} / 再生 {c}", a = x.email, b = x.rating, c = x.counter)),
            Frame::KeyValue(x) => (
                id,
                x.key_value_pairs
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<_>>()
                    .join(" / "),
            ),
            Frame::UniqueFileIdentifier(x) => (
                format!("{id}:{}", x.owner),
                String::from_utf8(x.identifier.to_vec()).unwrap_or_else(|_| t!("{n} バイト", n = x.identifier.len())),
            ),
            Frame::Private(x) => (format!("{id}:{}", x.owner), t!("{n} バイト", n = x.private_data.len())),
            Frame::Picture(x) => (id, t!("画像 {n} バイト", n = x.picture.data().len())),
            Frame::Binary(x) => (id, t!("{n} バイト", n = x.data.len())),
            _ => (id, "…".into()),
        };
        items.push((key, val));
    }
    TagBlock { kind: ver.into(), items }
}

fn comment_key(id: &str, desc: &str, lang: &[u8; 3]) -> String {
    let lang = String::from_utf8_lossy(lang).trim_matches('\0').to_string();
    match (desc.is_empty(), lang.is_empty()) {
        (true, true) => id.to_string(),
        (true, false) => format!("{id} [{lang}]"),
        (false, true) => format!("{id}:{desc}"),
        (false, false) => format!("{id}:{desc} [{lang}]"),
    }
}

fn ilst_block(t: &lofty::mp4::Ilst) -> TagBlock {
    let mut items = Vec::new();
    for atom in t {
        let key = match atom.ident() {
            AtomIdent::Fourcc(cc) => cc.iter().map(|&b| if b == 0xA9 { '©' } else { b as char }).collect(),
            AtomIdent::Freeform { mean, name } => {
                if mean == "com.apple.iTunes" {
                    format!("----:{name}")
                } else {
                    format!("----:{mean}:{name}")
                }
            }
        };
        let is_pair = matches!(atom.ident(), AtomIdent::Fourcc(cc) if cc == b"trkn" || cc == b"disk");
        let vals: Vec<String> = atom
            .data()
            .map(|d| match d {
                AtomData::UTF8(s) | AtomData::UTF16(s) => s.clone(),
                AtomData::SignedInteger(n) => n.to_string(),
                AtomData::UnsignedInteger(n) => n.to_string(),
                AtomData::Bool(b) => b.to_string(),
                AtomData::Picture(p) => t!("画像 {n} バイト", n = p.data().len()),
                AtomData::Unknown { data, .. } if is_pair && data.len() >= 6 => {
                    // trkn / disk は 0,0,番号(2),総数(2)
                    let n = u16::from_be_bytes([data[2], data[3]]);
                    let total = u16::from_be_bytes([data[4], data[5]]);
                    if total > 0 { format!("{n} / {total}") } else { n.to_string() }
                }
                AtomData::Unknown { data, .. } => t!("{n} バイト", n = data.len()),
            })
            .collect();
        items.push((key, vals.join(" / ")));
    }
    TagBlock { kind: "MP4 (iTunes)".into(), items }
}

fn ape_block(t: &lofty::ape::ApeTag) -> TagBlock {
    let items = t
        .into_iter()
        .map(|i| {
            let v = match i.value() {
                ItemValue::Text(s) | ItemValue::Locator(s) => s.clone(),
                ItemValue::Binary(b) => t!("{n} バイト", n = b.len()),
            };
            (i.key().to_string(), v)
        })
        .collect();
    TagBlock { kind: "APEv2".into(), items }
}

fn generic_block(kind: &str, t: &lofty::tag::Tag) -> TagBlock {
    let items = t
        .items()
        .map(|i| {
            let v = match i.value() {
                ItemValue::Text(s) | ItemValue::Locator(s) => s.clone(),
                ItemValue::Binary(b) => t!("{n} バイト", n = b.len()),
            };
            (format!("{:?}", i.key()), v)
        })
        .collect();
    TagBlock { kind: kind.into(), items }
}
