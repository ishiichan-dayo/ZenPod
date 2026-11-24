//! 変わった構造のタグを持つファイルで、読む・編集する・読み直すが正しく動くか。
//! 利用者の報告 (配布リポジトリの Issue #7・#8・#13・#14) はどれも、ほかのアプリが書いた変わった構造が原因だった。
//! 実際の曲は使わず、ffmpeg で作った 0.3 秒の音 (`fixtures/audio/`) にタグを組み立てて付ける。
//!
//! 1 つの形ごとに次を確かめ、問題をまとめて 1 回で出す:
//! - 元のファイルを読める (落ちない・止まらない)
//! - 編集 (値を変える・空にする・数) を保存すると、読み直してそのとおりになる。保存できないならエラーになる (黙って失敗しない)
//! - 編集しなかった独自の項目 (ZENPOD_KEEP) が残る
//! - 音声のバイトが変わらない

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use ipod_core::domain::track::TagEdit;
use ipod_core::media;

const KEEP: &str = "keep me";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio").join(name)).unwrap()
}

/// 別のスレッドで動かし、決めた時間で終わらなければ None (止まったままのテストにしない)
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

fn edit() -> TagEdit {
    TagEdit {
        title: Some("Новый 曲 ✓".into()),
        artist: Some("Artist B".into()),
        album: Some(String::new()),
        genre: Some("Drum & Bass".into()),
        year: Some(2001),
        track_nr: Some(3),
        ..Default::default()
    }
}

/// 試す形 1 つ
struct Case {
    name: String,
    ext: &'static str,
    bytes: Vec<u8>,
    /// 音声の部分 (編集のあとも、このバイトの並びがそのまま残っているはず)。None なら確かめない (Ogg はページを組み直す)
    audio: Option<Vec<u8>>,
    /// 元のタイトル (読めているかを見る)
    title: &'static str,
}

#[derive(Default)]
struct Report {
    problems: Vec<String>,
    errors: Vec<String>,
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn utf16be(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
}

/// 独自の項目の値がファイルに残っているか (文字の形は書き直しで変わることがある)
fn kept(bytes: &[u8]) -> bool {
    contains(bytes, KEEP.as_bytes()) || contains(bytes, &utf16le(KEEP)) || contains(bytes, &utf16be(KEEP))
}

fn run(dir: &Path, case: Case, r: &mut Report) {
    let p: PathBuf = dir.join(format!("{}.{}", case.name, case.ext));
    std::fs::write(&p, &case.bytes).unwrap();
    let name = case.name.clone();

    let q = p.clone();
    let info = match within(10, move || std::panic::catch_unwind(|| media::read_info(&q))) {
        None => return r.problems.push(format!("{name}: 読み込みが終わらない")),
        Some(Err(_)) => return r.problems.push(format!("{name}: 読み込みで落ちた")),
        Some(Ok(Err(e))) => return r.problems.push(format!("{name}: 読めない: {e}")),
        Some(Ok(Ok(info))) => info,
    };
    if info.title.as_deref() != Some(case.title) {
        r.problems.push(format!("{name}: 元のタイトルが {:?} ({:?} のはず)", info.title, case.title));
    }

    let q = p.clone();
    let res = match within(10, move || std::panic::catch_unwind(|| media::write_tags(&q, &edit()))) {
        None => return r.problems.push(format!("{name}: 保存が終わらない")),
        Some(Err(_)) => return r.problems.push(format!("{name}: 保存で落ちた")),
        Some(Ok(res)) => res,
    };
    let after = std::fs::read(&p).unwrap();
    if let Some(audio) = &case.audio
        && !contains(&after, audio)
    {
        r.problems.push(format!("{name}: 音声が変わった"));
    }
    if let Err(e) = res {
        // 保存できないのはよいが、理由を出すこと (黙って失敗しない)
        r.errors.push(format!("{name}: {e}"));
        return;
    }
    // write_tags も読み直して確かめているが、ここでも別に見る
    let Ok(info) = media::read_info(&p) else {
        return r.problems.push(format!("{name}: 保存したあと読めない"));
    };
    let e = edit();
    let got = (info.title.as_deref(), info.artist.as_deref(), info.album.as_deref(), info.genre.as_deref(), info.year, info.track_nr);
    let want = (e.title.as_deref(), e.artist.as_deref(), None, e.genre.as_deref(), 2001, 3);
    if got != want {
        r.problems.push(format!("{name}: 保存したのに {got:?} ({want:?} のはず)"));
    }
    if !kept(&after) {
        r.problems.push(format!("{name}: 独自の項目 (ZENPOD_KEEP) が消えた"));
    }
    // 2 回目の保存も通る (1 回目で書いた形を読み書きできる)
    let again = TagEdit { title: Some("Again".into()), ..Default::default() };
    if let Err(e) = media::write_tags(&p, &again) {
        r.problems.push(format!("{name}: 2 回目の保存に失敗: {e}"));
    }
}

// ---- MP3 ----

/// ID3v2 のフレーム (版 2・3・4)。flags はフレームの印
fn frame(v: u8, id: &str, flags: u16, content: &[u8]) -> Vec<u8> {
    let mut f = id.as_bytes().to_vec();
    let n = content.len() as u32;
    match v {
        2 => f.extend_from_slice(&n.to_be_bytes()[1..]),
        3 => f.extend_from_slice(&n.to_be_bytes()),
        _ => f.extend_from_slice(&syncsafe(n)),
    }
    if v != 2 {
        f.extend_from_slice(&flags.to_be_bytes());
    }
    f.extend_from_slice(content);
    f
}

fn syncsafe(n: u32) -> [u8; 4] {
    [((n >> 21) & 0x7F) as u8, ((n >> 14) & 0x7F) as u8, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]
}

/// 文字の枠の中身。enc 0 = Latin-1、1 = UTF-16 (BOM 付き LE)、2 = UTF-16BE、3 = UTF-8
fn text(enc: u8, s: &str) -> Vec<u8> {
    let mut v = vec![enc];
    match enc {
        0 => v.extend(s.chars().map(|c| c as u8)),
        1 => {
            v.extend_from_slice(&[0xFF, 0xFE]);
            v.extend(utf16le(s));
        }
        2 => v.extend(utf16be(s)),
        _ => v.extend_from_slice(s.as_bytes()),
    }
    v
}

/// TXXX (説明 + 値)
fn txxx(enc: u8, desc: &str, value: &str) -> Vec<u8> {
    let mut v = text(enc, desc);
    v.extend_from_slice(if matches!(enc, 1 | 2) { &[0, 0] } else { &[0] });
    v.extend(text(enc, value).into_iter().skip(1));
    v
}

/// ID3v2 の見出し + 中身
fn tag(v: u8, flags: u8, body: &[u8]) -> Vec<u8> {
    let mut t = vec![b'I', b'D', b'3', v, 0, flags];
    t.extend_from_slice(&syncsafe(body.len() as u32));
    t.extend_from_slice(body);
    t
}

/// よくある枠一式 (版 v、文字の形 enc)。末尾に余白
fn frames(v: u8, enc: u8, padding: usize) -> Vec<u8> {
    let mut b = Vec::new();
    if v == 2 {
        b.extend(frame(2, "TT2", 0, &text(enc, "Old Title")));
        b.extend(frame(2, "TP1", 0, &text(enc, "Old Artist")));
        b.extend(frame(2, "TAL", 0, &text(enc, "Old Album")));
        b.extend(frame(2, "TYE", 0, &text(0, "1999")));
        b.extend(frame(2, "TRK", 0, &text(0, "1/9")));
        b.extend(frame(2, "TXX", 0, &txxx(enc, "ZENPOD_KEEP", KEEP)));
    } else {
        b.extend(frame(v, "TIT2", 0, &text(enc, "Old Title")));
        b.extend(frame(v, "TPE1", 0, &text(enc, "Old Artist")));
        b.extend(frame(v, "TALB", 0, &text(enc, "Old Album")));
        b.extend(frame(v, if v == 3 { "TYER" } else { "TDRC" }, 0, &text(0, "1999")));
        b.extend(frame(v, "TRCK", 0, &text(0, "1/9")));
        b.extend(frame(v, "TCON", 0, &text(0, "(17)")));
        b.extend(frame(v, "TXXX", 0, &txxx(enc, "ZENPOD_KEEP", KEEP)));
    }
    b.resize(b.len() + padding, 0);
    b
}

/// ID3v1 (曲名・アーティスト・アルバム・年・コメント・トラック・ジャンル)
fn id3v1(title: &str) -> Vec<u8> {
    let mut t = b"TAG".to_vec();
    for (s, n) in [(title, 30), ("V1 Artist", 30), ("V1 Album", 30), ("1998", 4)] {
        let mut f = s.as_bytes().to_vec();
        f.resize(n, 0);
        t.extend(f);
    }
    // コメント (28 バイト) に独自の値
    let mut c = KEEP.as_bytes().to_vec();
    c.resize(28, 0);
    t.extend(c);
    t.extend_from_slice(&[0, 5, 17]);
    t
}

/// APEv2 (見出しと終わり付き)
fn ape(items: &[(&str, &str)]) -> Vec<u8> {
    ape_with(items, true)
}

/// APEv2。`with_header` が false なら終わりだけ (ヘッダーなしも決まりにある)
fn ape_with(items: &[(&str, &str)], with_header: bool) -> Vec<u8> {
    let mut body = Vec::new();
    for (k, v) in items {
        body.extend_from_slice(&(v.len() as u32).to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(k.as_bytes());
        body.push(0);
        body.extend_from_slice(v.as_bytes());
    }
    let head = |is_header: bool| {
        let mut h = b"APETAGEX".to_vec();
        h.extend_from_slice(&2000u32.to_le_bytes());
        h.extend_from_slice(&(body.len() as u32 + 32).to_le_bytes());
        h.extend_from_slice(&(items.len() as u32).to_le_bytes());
        let flags: u32 = if with_header { 1 << 31 } else { 0 } | if is_header { 1 << 29 } else { 0 };
        h.extend_from_slice(&flags.to_le_bytes());
        h.extend_from_slice(&[0; 8]);
        h
    };
    let mut t = if with_header { head(true) } else { Vec::new() };
    t.extend_from_slice(&body);
    t.extend(head(false));
    t
}

/// 前に項目の無い APE タグの終わり (142 バイトの 3 項目のタグがあると言うが、前は音声)
fn stray_ape_footer() -> Vec<u8> {
    let mut f = b"APETAGEX".to_vec();
    for n in [2000u32, 142, 3, 0xA000_0000] {
        f.extend_from_slice(&n.to_le_bytes());
    }
    f.extend_from_slice(&[0; 8]);
    f
}

/// Lyrics3v2 (ID3v1 の前に置く)
fn lyrics3() -> Vec<u8> {
    let mut fields = b"LYRICSBEGIN".to_vec();
    for (id, v) in [("IND", "10"), ("LYR", "la la la"), ("ETT", "L3 Title")] {
        fields.extend_from_slice(id.as_bytes());
        fields.extend_from_slice(format!("{:05}", v.len()).as_bytes());
        fields.extend_from_slice(v.as_bytes());
    }
    let n = fields.len();
    fields.extend_from_slice(format!("{n:06}LYRICS200").as_bytes());
    fields
}

/// 非同期化 (0xFF の後ろに 0x00 を入れる)
fn unsync(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, &x) in b.iter().enumerate() {
        out.push(x);
        if x == 0xFF && b.get(i + 1).is_none_or(|&n| n == 0 || n >= 0xE0) {
            out.push(0);
        }
    }
    out
}

fn mp3_cases() -> Vec<Case> {
    let audio = fixture("plain.mp3");
    let case = |name: &str, head: Vec<u8>, tail: Vec<u8>, title: &'static str| {
        let mut bytes = head;
        bytes.extend_from_slice(&audio);
        bytes.extend(tail);
        Case { name: format!("mp3-{name}"), ext: "mp3", bytes, audio: Some(audio.clone()), title }
    };
    let v23 = tag(3, 0, &frames(3, 0, 256));
    let mut v = vec![
        case("v23", v23.clone(), vec![], "Old Title"),
        case("v23-no-padding", tag(3, 0, &frames(3, 0, 0)), vec![], "Old Title"),
        case("v23-utf16", tag(3, 0, &frames(3, 1, 256)), vec![], "Old Title"),
        case("v24-utf8", tag(4, 0, &frames(4, 3, 256)), vec![], "Old Title"),
        case("v24-utf16be", tag(4, 0, &frames(4, 2, 256)), vec![], "Old Title"),
        case("v22", tag(2, 0, &frames(2, 0, 256)), vec![], "Old Title"),
        case("v23-and-v1", v23.clone(), id3v1("V1 Title"), "Old Title"),
        case("v1-only", vec![], id3v1("V1 Title"), "V1 Title"),
        case("v23-lyrics3-v1", v23.clone(), [lyrics3(), id3v1("V1 Title")].concat(), "Old Title"),
        case(
            "ape-only",
            vec![],
            ape(&[("Title", "Old Title"), ("Album", "APE Album"), ("ZENPOD_KEEP", KEEP)]),
            "Old Title",
        ),
        case("v23-ape-v1", v23.clone(), [ape(&[("Album", "APE Album")]), id3v1("V1 Title")].concat(), "Old Title"),
        // ヘッダーなしの APE タグ + 余白なしの ID3v2 (収まらないので書き直す)。lofty 0.25.5 は APE タグを書き直すときに
        // その前の 32 バイトを消す (上流の Issue #759)
        case(
            "v23-no-padding-headerless-ape",
            tag(3, 0, &frames(3, 0, 0)),
            ape_with(&[("Title", "Old Title"), ("Comment", "x")], false),
            "Old Title",
        ),
        // 末尾に APE タグの終わりの 32 バイトだけがある (前は音声)。lofty 0.25.5 はファイルごと読めない (上流の Issue #685)
        case("v23-stray-ape-footer", v23.clone(), stray_ape_footer(), "Old Title"),
        // 2 つ重なったタグ (Issue #14)
        case("stacked", [tag(4, 0, &frames(4, 3, 64)), tag(3, 0, &frames(3, 0, 64))].concat(), vec![], "Old Title"),
        // タグと音声の間に 0 の並び
        case("zeros-after-tag", [v23.clone(), vec![0; 300]].concat(), vec![], "Old Title"),
    ];

    // 拡張ヘッダー (2.3: 大きさ 6 + 印 2 + 余白の大きさ 4、2.4: 大きさ 6 (自分を含む) + 印のバイト数 1 + 印 1)
    let mut b = vec![0, 0, 0, 6, 0, 0, 0, 0, 0, 0];
    b.extend(frames(3, 0, 128));
    v.push(case("v23-ext-header", tag(3, 0x40, &b), vec![], "Old Title"));
    let mut b = vec![0, 0, 0, 6, 1, 0];
    b.extend(frames(4, 3, 128));
    v.push(case("v24-ext-header", tag(4, 0x40, &b), vec![], "Old Title"));

    // フッター付き (2.4、余白なし)
    let mut t = tag(4, 0x10, &frames(4, 3, 0));
    let mut foot = t[..10].to_vec();
    foot[..3].copy_from_slice(b"3DI");
    t.extend(foot);
    v.push(case("v24-footer", t, vec![], "Old Title"));

    // 非同期化 (2.3、UTF-16 の BOM FF FE が FF 00 FE になる)
    v.push(case("v23-unsync", tag(3, 0x80, &unsync(&frames(3, 1, 128))), vec![], "Old Title"));

    // 同じ枠が 2 つ (1 つのタグに 1 つだけのはず。lofty は後ろを使う)
    let mut b = frame(3, "TIT2", 0, &text(0, "Old Title"));
    b.extend(frame(3, "TIT2", 0, &text(0, "Second Title")));
    b.extend(frames(3, 0, 64).into_iter().skip(frame(3, "TIT2", 0, &text(0, "Old Title")).len()));
    v.push(case("v23-dup-tit2", tag(3, 0, &b), vec![], "Second Title"));

    // 2.4 の複数の値 (\0 区切り) と、データの長さの印 (フレームの印 0x0001 + 4 バイトの長さ)
    let mut b = frame(4, "TIT2", 0, &text(3, "Old Title"));
    b.extend(frame(4, "TPE1", 0, &text(3, "Old Artist\0Feat Artist")));
    let content = text(3, "Old Album");
    let mut dli = syncsafe(content.len() as u32).to_vec();
    dli.extend_from_slice(&content);
    b.extend(frame(4, "TALB", 0x0001, &dli));
    b.extend(frame(4, "TXXX", 0, &txxx(3, "ZENPOD_KEEP", KEEP)));
    b.resize(b.len() + 64, 0);
    v.push(case("v24-multi-dli", tag(4, 0, &b), vec![], "Old Title"));

    // 知らない枠・PRIV・画像
    let mut b = frames(3, 0, 0);
    b.extend(frame(3, "PRIV", 0, b"WM/MediaClassPrimaryID\0\x01\x02\x03\x04"));
    b.extend(frame(3, "XYZW", 0, b"unknown frame"));
    let mut apic = vec![0];
    apic.extend_from_slice(b"image/png\0\x03\0");
    apic.extend_from_slice(&tiny_png());
    b.extend(frame(3, "APIC", 0, &apic));
    b.resize(b.len() + 64, 0);
    v.push(case("v23-priv-unknown-apic", tag(3, 0, &b), vec![], "Old Title"));
    v
}

fn tiny_png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(4, 4, image::Rgb([10, 200, 30])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn quirky_mp3() {
    check(mp3_cases());
}

// ---- M4A ----

/// 箱 (大きさ + 名前 + 中身)
fn atom(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut a = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    a.extend_from_slice(name);
    a.extend_from_slice(body);
    a
}

/// ilst の項目 (data の箱 1 つ、種類 1 = UTF-8)
fn ilst_item(name: &[u8; 4], value: &str) -> Vec<u8> {
    let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
    data.extend_from_slice(value.as_bytes());
    atom(name, &atom(b"data", &data))
}

/// ---- の項目 (mean + name + data)
fn freeform(name: &str, value: &str) -> Vec<u8> {
    let mut body = atom(b"mean", &[&[0u8; 4][..], b"com.apple.iTunes"].concat());
    body.extend(atom(b"name", &[&[0u8; 4][..], name.as_bytes()].concat()));
    let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
    data.extend_from_slice(value.as_bytes());
    body.extend(atom(b"data", &data));
    atom(b"----", &body)
}

/// moov の中に udta/meta/ilst を足し、ファイル全体の箱を並べ直す。
/// `order` は上の段の箱の並び ("ftyp" "moov" "mdat" "free")。mdat が動くので stco を直す
fn m4a(base: &[u8], ilst: &[u8], order: &[&str], mdat64: bool) -> (Vec<u8>, Vec<u8>) {
    // 上の段の箱
    let mut top: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let mut at = 0;
    while at + 8 <= base.len() {
        let size = u32::from_be_bytes(base[at..at + 4].try_into().unwrap()) as usize;
        top.push((base[at + 4..at + 8].try_into().unwrap(), base[at + 8..at + size].to_vec()));
        at += size;
    }
    let get = |n: &[u8; 4]| top.iter().find(|(k, _)| k == n).map(|(_, b)| b.clone()).unwrap();
    let mdat_body = get(b"mdat");
    let mut moov = get(b"moov");
    // udta がすでにあれば外す (ffmpeg が入れることがある)
    moov = strip_child(&moov, b"udta");
    let mut hdlr = vec![0; 8];
    hdlr.extend_from_slice(b"mdirappl");
    hdlr.extend_from_slice(&[0; 9]);
    let mut meta = vec![0, 0, 0, 0];
    meta.extend(atom(b"hdlr", &hdlr));
    meta.extend(atom(b"ilst", ilst));
    moov.extend(atom(b"udta", &atom(b"meta", &meta)));

    // 並べてから stco を直す (mdat の中身の位置が変わる)
    let build = |moov: &[u8]| {
        let mut out = Vec::new();
        let mut data_at = 0;
        for n in order {
            match *n {
                "ftyp" => out.extend(atom(b"ftyp", &get(b"ftyp"))),
                "moov" => out.extend(atom(b"moov", moov)),
                "free" => out.extend(atom(b"free", &[0; 64])),
                "mdat" if mdat64 => {
                    out.extend_from_slice(&1u32.to_be_bytes());
                    out.extend_from_slice(b"mdat");
                    out.extend_from_slice(&((mdat_body.len() + 16) as u64).to_be_bytes());
                    data_at = out.len();
                    out.extend_from_slice(&mdat_body);
                }
                "mdat" => {
                    out.extend(atom(b"mdat", &mdat_body));
                    data_at = out.len() - mdat_body.len();
                }
                _ => unreachable!(),
            }
        }
        (out, data_at)
    };
    let old_data_at = {
        let mut at = 0;
        let mut found = 0;
        while at + 8 <= base.len() {
            let size = u32::from_be_bytes(base[at..at + 4].try_into().unwrap()) as usize;
            if &base[at + 4..at + 8] == b"mdat" {
                found = at + 8;
            }
            at += size;
        }
        found
    };
    let (_, new_data_at) = build(&moov);
    let delta = new_data_at as i64 - old_data_at as i64;
    let moov = shift_stco(&moov, delta);
    let (out, _) = build(&moov);
    (out, mdat_body)
}

/// 箱の中身から、名前が `name` の子を外す (moov の直下だけ)
fn strip_child(body: &[u8], name: &[u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= body.len() {
        let size = u32::from_be_bytes(body[at..at + 4].try_into().unwrap()) as usize;
        if &body[at + 4..at + 8] != name {
            out.extend_from_slice(&body[at..at + size]);
        }
        at += size;
    }
    out
}

/// moov の中の stco の値を delta だけずらす
fn shift_stco(moov: &[u8], delta: i64) -> Vec<u8> {
    let mut m = moov.to_vec();
    let mut i = 0;
    while let Some(p) = m[i..].windows(4).position(|w| w == b"stco") {
        let at = i + p + 4;
        let n = u32::from_be_bytes(m[at + 4..at + 8].try_into().unwrap()) as usize;
        for k in 0..n {
            let o = at + 8 + k * 4;
            let v = u32::from_be_bytes(m[o..o + 4].try_into().unwrap()) as i64 + delta;
            m[o..o + 4].copy_from_slice(&(v as u32).to_be_bytes());
        }
        i = at;
    }
    m
}

fn m4a_cases() -> Vec<Case> {
    let mut ilst = ilst_item(b"\xa9nam", "Old Title");
    ilst.extend(ilst_item(b"\xa9ART", "Old Artist"));
    ilst.extend(ilst_item(b"\xa9alb", "Old Album"));
    ilst.extend(ilst_item(b"\xa9day", "1999-05-01T00:00:00Z"));
    ilst.extend(freeform("ZENPOD_KEEP", KEEP));
    let mut cases = Vec::new();
    for (base_name, base) in [("aac", fixture("aac-faststart.m4a")), ("alac", fixture("alac.m4a"))] {
        for (name, order, mdat64) in [
            ("moov-first", &["ftyp", "moov", "mdat"][..], false),
            ("moov-last", &["ftyp", "mdat", "moov"][..], false),
            ("free-between", &["ftyp", "moov", "free", "mdat"][..], false),
            ("mdat64-first", &["ftyp", "mdat", "moov"][..], true),
            ("mdat64-last", &["ftyp", "moov", "mdat"][..], true),
        ] {
            let (bytes, audio) = m4a(&base, &ilst, order, mdat64);
            cases.push(Case { name: format!("m4a-{base_name}-{name}"), ext: "m4a", bytes, audio: Some(audio), title: "Old Title" });
        }
    }
    // トラック番号の箱が短い (trkn の中身が 6 バイト)・ジャンルが番号 (gnre)
    let mut ilst2 = ilst.clone();
    ilst2.extend(atom(b"trkn", &atom(b"data", &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 9])));
    ilst2.extend(atom(b"gnre", &atom(b"data", &[0, 0, 0, 0, 0, 0, 0, 0, 0, 18])));
    let (bytes, audio) = m4a(&fixture("aac-faststart.m4a"), &ilst2, &["ftyp", "moov", "mdat"], false);
    cases.push(Case { name: "m4a-short-trkn-gnre".into(), ext: "m4a", bytes, audio: Some(audio), title: "Old Title" });
    cases
}

#[test]
fn quirky_m4a() {
    check(m4a_cases());
}

/// ilst がちょうど 8 バイト縮むと、lofty 0.25.5 は余白の箱を書くところの assert で panic する (上流の Issue #758)。
/// ilst の後ろに余白が無い M4A で、タイトルを 8 文字短くする
#[test]
fn m4a_ilst_shrinks_by_8() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.m4a");
    let mut ilst = ilst_item(b"\xa9nam", "0123456789abcdef01234567");
    ilst.extend(freeform("ZENPOD_KEEP", KEEP));
    let (bytes, audio) = m4a(&fixture("aac-faststart.m4a"), &ilst, &["ftyp", "moov", "mdat"], false);
    std::fs::write(&p, &bytes).unwrap();
    let q = p.clone();
    let res = within(10, move || std::panic::catch_unwind(|| media::write_tags(&q, &TagEdit { title: Some("0123456789abcdef".into()), ..Default::default() })))
        .expect("終わらない")
        .expect("panic した");
    res.unwrap();
    assert_eq!(media::read_info(&p).unwrap().title.as_deref(), Some("0123456789abcdef"));
    let after = std::fs::read(&p).unwrap();
    assert!(kept(&after) && contains(&after, &audio));
}

// ---- FLAC / Ogg ----

/// FLAC のメタデータの箱 (種類, 中身)。最後の箱に印を付けて並べる
fn flac(blocks: &[(u8, Vec<u8>)], audio: &[u8]) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    for (i, (kind, body)) in blocks.iter().enumerate() {
        let last = if i + 1 == blocks.len() { 0x80 } else { 0 };
        out.push(kind | last);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(body);
    }
    out.extend_from_slice(audio);
    out
}

fn vorbis_comment(items: &[(&str, &str)]) -> Vec<u8> {
    let mut b = Vec::new();
    let vendor = b"zenpod test";
    b.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    b.extend_from_slice(vendor);
    b.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for (k, v) in items {
        let s = format!("{k}={v}");
        b.extend_from_slice(&(s.len() as u32).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }
    b
}

fn flac_picture() -> Vec<u8> {
    let png = tiny_png();
    let mut b = 3u32.to_be_bytes().to_vec();
    for s in ["image/png", ""] {
        b.extend_from_slice(&(s.len() as u32).to_be_bytes());
        b.extend_from_slice(s.as_bytes());
    }
    for n in [4u32, 4, 24, 0, png.len() as u32] {
        b.extend_from_slice(&n.to_be_bytes());
    }
    b.extend_from_slice(&png);
    b
}

fn flac_cases() -> Vec<Case> {
    let base = fixture("plain.flac");
    // ffmpeg の FLAC を箱に分ける (STREAMINFO と、ほかの箱、音声)
    let mut at = 4;
    let mut streaminfo = Vec::new();
    loop {
        let kind = base[at] & 0x7F;
        let last = base[at] & 0x80 != 0;
        let len = u32::from_be_bytes([0, base[at + 1], base[at + 2], base[at + 3]]) as usize;
        if kind == 0 {
            streaminfo = base[at + 4..at + 4 + len].to_vec();
        }
        at += 4 + len;
        if last {
            break;
        }
    }
    let audio = base[at..].to_vec();
    let items = [("TITLE", "Old Title"), ("ARTIST", "Old Artist"), ("ALBUM", "Old Album"), ("DATE", "1999"), ("TRACKNUMBER", "1"), ("ZENPOD_KEEP", KEEP)];
    let vc = vorbis_comment(&items);
    let case = |name: &str, bytes: Vec<u8>| Case { name: format!("flac-{name}"), ext: "flac", bytes, audio: Some(audio.clone()), title: "Old Title" };
    vec![
        case("padding", flac(&[(0, streaminfo.clone()), (4, vc.clone()), (1, vec![0; 512])], &audio)),
        case("no-padding", flac(&[(0, streaminfo.clone()), (4, vc.clone())], &audio)),
        case("picture", flac(&[(0, streaminfo.clone()), (4, vc.clone()), (6, flac_picture()), (1, vec![0; 64])], &audio)),
        case(
            "lowercase-keys",
            flac(
                &[(0, streaminfo.clone()), (4, vorbis_comment(&[("title", "Old Title"), ("album", "Old Album"), ("zenpod_keep", KEEP)])), (1, vec![0; 128])],
                &audio,
            ),
        ),
        case(
            "track-total-dup-title",
            flac(
                &[
                    (0, streaminfo.clone()),
                    (4, vorbis_comment(&[("TITLE", "Old Title"), ("TITLE", "Second"), ("TRACKNUMBER", "1/12"), ("ZENPOD_KEEP", KEEP)])),
                    (1, vec![0; 128]),
                ],
                &audio,
            ),
        ),
        // 先頭に ID3v2 が付いた FLAC (古いソフトが書く)
        case("id3v2-prefix", [tag(3, 0, &frames(3, 0, 64)), flac(&[(0, streaminfo.clone()), (4, vc.clone()), (1, vec![0; 128])], &audio)].concat()),
        // アプリ独自の箱 (APPLICATION) が間にある
        case(
            "application-block",
            flac(&[(0, streaminfo.clone()), (2, [b"ZNPD".as_slice(), &[1, 2, 3, 4]].concat()), (4, vc.clone()), (1, vec![0; 128])], &audio),
        ),
    ]
}

#[test]
fn quirky_flac() {
    check(flac_cases());
}

/// Ogg Vorbis / Opus は lofty で独自の項目を足してから試す (ページの組み立ては lofty に任せる)
#[test]
fn quirky_ogg() {
    use lofty::config::WriteOptions;
    use lofty::ogg::tag::VorbisComments;
    use lofty::tag::TagExt;
    let dir = tempfile::tempdir().unwrap();
    let mut cases = Vec::new();
    for (name, ext, file) in [("vorbis", "ogg", "plain.ogg"), ("opus", "opus", "plain.opus")] {
        let p = dir.path().join(format!("base.{ext}"));
        std::fs::write(&p, fixture(file)).unwrap();
        let mut vc = VorbisComments::default();
        for (k, v) in [("TITLE", "Old Title"), ("ALBUM", "Old Album"), ("ZENPOD_KEEP", KEEP)] {
            vc.push(k.to_string(), v.to_string());
        }
        vc.save_to_path(&p, WriteOptions::default()).unwrap();
        cases.push(Case { name: format!("ogg-{name}"), ext, bytes: std::fs::read(&p).unwrap(), audio: None, title: "Old Title" });
    }
    check(cases);
}

fn check(cases: Vec<Case>) {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Report::default();
    for case in cases {
        run(dir.path(), case, &mut r);
    }
    if !r.errors.is_empty() {
        eprintln!("保存できなかった (理由は出た) 形:\n{}", r.errors.join("\n"));
    }
    assert!(r.problems.is_empty(), "問題:\n{}", r.problems.join("\n"));
}

/// ID3v2.3 の MP3 の読み (TSOP・TSOA・TSOT)。lofty 0.25 は 2.3 で書くときに 2.4 の枠としてこれらを捨てるので、
/// 読みを入れても保存されず、ほかの項目の編集でも元の読みが消えていた (配布リポジトリの Issue #23)。
/// iTunes も 2.3 にこれらの枠を書く。余白に収まる (その場で書く)・収まらない (書き直す)・読めない枠がある (salvage) の 3 通り
#[test]
fn id3v23_sort_frames() {
    let audio = fixture("plain.mp3");
    let sorted = |padding: usize, bad: bool| {
        let mut b = frames(3, 0, 0);
        b.extend(frame(3, "TSOP", 0, &text(0, "Old Sort Artist")));
        b.extend(frame(3, "TSOA", 0, &text(1, "Old Sort Album")));
        b.extend(frame(3, "TSOT", 0, &text(0, "Old Sort Title")));
        b.extend(frame(3, "TSO2", 0, &text(0, "Old Sort Album Artist")));
        if bad {
            // UTF-8 と書いてあるのに中身が UTF-8 でない (2.3 に UTF-8 は無い。lofty はファイルごと読めない)
            b.extend(frame(3, "TCOM", 0, &[3, 0xC3, 0x28]));
        }
        b.resize(b.len() + padding, 0);
        [tag(3, 0, &b), audio.clone()].concat()
    };
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, bytes) in [("padding", sorted(512, false)), ("no-padding", sorted(0, false)), ("salvage", sorted(64, true))] {
        let p = dir.path().join(format!("{name}.mp3"));
        std::fs::write(&p, &bytes).unwrap();
        let info = media::read_info(&p).unwrap();
        if info.sort_artist.as_deref() != Some("Old Sort Artist") {
            problems.push(format!("{name}: 元の読みが読めない: {:?}", info.sort_artist));
        }
        // 読みと関係ない項目だけを変えても、読みは残る
        let genre = TagEdit { genre: Some("House".into()), ..Default::default() };
        if let Err(e) = media::write_tags(&p, &genre) {
            problems.push(format!("{name}: ジャンルの保存に失敗: {e}"));
        }
        let info = media::read_info(&p).unwrap();
        let got = (info.sort_title.as_deref(), info.sort_artist.as_deref(), info.sort_album.as_deref(), info.sort_album_artist.as_deref());
        if got != (Some("Old Sort Title"), Some("Old Sort Artist"), Some("Old Sort Album"), Some("Old Sort Album Artist")) {
            problems.push(format!("{name}: ジャンルを変えたら読みが変わった: {got:?}"));
        }
        // 読みを変える (Latin-1 に収まらない字も)
        let e = TagEdit {
            sort_title: Some("Wanderlust".into()),
            sort_artist: Some("Etienne De Crécy".into()),
            sort_album: Some("曲 ✓".into()),
            ..Default::default()
        };
        if let Err(e) = media::write_tags(&p, &e) {
            problems.push(format!("{name}: 読みの保存に失敗: {e}"));
            continue;
        }
        let after = std::fs::read(&p).unwrap();
        if after[3] != 3 {
            problems.push(format!("{name}: ID3v2 の版が 2.{} に変わった", after[3]));
        }
        if !contains(&after, &audio) {
            problems.push(format!("{name}: 音声が変わった"));
        }
        let info = media::read_info(&p).unwrap();
        let got = (info.sort_title.as_deref(), info.sort_artist.as_deref(), info.sort_album.as_deref(), info.title.as_deref());
        if got != (Some("Wanderlust"), Some("Etienne De Crécy"), Some("曲 ✓"), Some("Old Title")) {
            problems.push(format!("{name}: 読みが保存されていない: {got:?}"));
        }
        // 空にすると消える
        let e = TagEdit { sort_artist: Some(String::new()), ..Default::default() };
        if let Err(e) = media::write_tags(&p, &e) {
            problems.push(format!("{name}: 読みを空にできない: {e}"));
        }
    }
    assert!(problems.is_empty(), "問題:\n{}", problems.join("\n"));
}
