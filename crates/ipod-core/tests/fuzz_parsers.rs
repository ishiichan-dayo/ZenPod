//! 自前で読んでいる形式 (iTunesDB・ArtworkDB・Rockbox のデータベース・iTunes の XML・タグの読み直し) に、
//! 正しいファイルを少しずつ壊したものを読ませて、落ちない (panic しない)・止まらないことを確かめる。
//! 乱数は種を決めているので、失敗したら同じ壊し方で再現できる (メッセージに種と回数を出す)。
//! cargo-fuzz (nightly が要る) の代わりに、普段のテストと CI で回せる大きさにしている (手元で長く回すときは `ZENPOD_FUZZ_SCALE`)。
//!
//! lofty の中の panic は ZenPod では `media::read_any` / `edit_tag` がエラーに変えるので、別に数えて表示だけする

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, mpsc};
use std::time::Duration;

use ipod_core::Ipod;
use ipod_core::domain::track::{AddOptions, TagEdit};
use ipod_core::ipod::artwork::ArtworkDb;
use ipod_core::ipod::itunesdb::{ITunesDb, parse_otg};
use ipod_core::media;
use ipod_core::pc::itunesxml::XmlLibrary;
use ipod_core::rockbox::rbdb;

/// 起きた panic の場所と文 (テスト全体で 1 つの記録)
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record_panics() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
            PANICS.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{at}: {msg}"));
        }));
    });
}

fn take_panics(prefix: &str) -> Vec<String> {
    let mut all = PANICS.lock().unwrap_or_else(|e| e.into_inner());
    let (mine, rest): (Vec<String>, Vec<String>) = all.drain(..).partition(|p| p.starts_with(prefix));
    *all = rest;
    mine
}

/// 種を決めた乱数 (xorshift64*)
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// 1〜4 か所を壊す: バイトを変える・4 バイトの数を極端な値にする・途中で切る・一部を消す・一部を繰り返す
fn mutate(rng: &mut Rng, src: &[u8]) -> Vec<u8> {
    let mut b = src.to_vec();
    for _ in 0..1 + rng.below(4) {
        if b.is_empty() {
            break;
        }
        let at = rng.below(b.len());
        match rng.below(6) {
            0 => b[at] = rng.next() as u8,
            1 | 2 => {
                let v: u32 = match rng.below(6) {
                    0 => 0,
                    1 => u32::MAX,
                    2 => 0x7FFF_FFFF,
                    3 => b.len() as u32,
                    4 => (rng.next() % 4096) as u32,
                    _ => rng.next() as u32,
                };
                let bytes = if rng.below(2) == 0 { v.to_le_bytes() } else { v.to_be_bytes() };
                for (k, x) in bytes.iter().enumerate() {
                    if let Some(y) = b.get_mut(at + k) {
                        *y = *x;
                    }
                }
            }
            3 => b.truncate(at),
            4 => {
                let n = rng.below(64.min(b.len() - at)) + 1;
                b.drain(at..(at + n).min(b.len()));
            }
            _ => {
                let n = rng.below(64.min(b.len() - at)) + 1;
                let chunk = b[at..(at + n).min(b.len())].to_vec();
                b.splice(at..at, chunk);
            }
        }
    }
    b
}

/// 別のスレッドで動かし、決めた時間で終わらなければ None
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

const OURS: &str = "crates/ipod-core/src";

/// `run` を `n` 回、壊した入力で呼ぶ。自前のコードの panic・止まったら失敗
fn fuzz(name: &str, seed: u64, n: usize, input: &[u8], run: impl Fn(Vec<u8>) + Send + Sync + Clone + 'static) {
    record_panics();
    // 手元で長く回すときは ZENPOD_FUZZ_SCALE=30 などで回数を増やす (種も変える)
    let scale: usize = std::env::var("ZENPOD_FUZZ_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let n = n * scale;
    let seed = seed + 1000 * (scale as u64 - 1);
    let mut rng = Rng(seed);
    let mut problems = Vec::new();
    for i in 0..n {
        let data = mutate(&mut rng, input);
        let r = run.clone();
        match within(10, move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r(data))).is_ok()) {
            None => problems.push(format!("{name}: 終わらない (種 {seed}・{i} 回目)")),
            Some(false) => problems.push(format!("{name}: panic (種 {seed}・{i} 回目)")),
            Some(true) => {}
        }
        // テストは並んで動くので、自分のコードの panic はこの target の名前とは結びつけず、場所で見る
        for p in take_panics(&format!("{OURS}/")) {
            problems.push(format!("{name}: 自前のコードで panic (種 {seed}・{i} 回目): {p}"));
        }
        if problems.len() >= 5 {
            break;
        }
    }
    let lofty = take_panics(&std::env::var("CARGO_HOME").unwrap_or_default()).len();
    if lofty > 0 {
        eprintln!("{name}: lofty の中の panic {lofty} 回 (ZenPod はエラーにする)");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("zenpod-fuzz-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn fuzz_itunesdb_from_libgpod() {
    let data = fixture("libgpod-classic-itunesdb.bin");
    fuzz("iTunesDB (libgpod)", 1, 3000, &data, |b| {
        if let Ok(db) = ITunesDb::parse(&b) {
            let _ = parse_otg(&db, &b);
        }
    });
}

/// ZenPod が書いた iTunesDB と ArtworkDB (偽の iPod video 5G に、ジャケット付きの曲を 2 曲)
fn written_by_zenpod() -> (Vec<u8>, Vec<u8>) {
    let base = tmp("ipod");
    let root = base.join("ipod");
    let music = base.join("music");
    for d in ["iPod_Control/iTunes", "iPod_Control/Device", "iPod_Control/Music/F00"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA002\n").unwrap();
    std::fs::create_dir_all(&music).unwrap();
    let mut jpeg = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40])).write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
    std::fs::write(music.join("cover.jpg"), jpeg.into_inner()).unwrap();
    let mut ipod = Ipod::open(&root).unwrap();
    let opts = AddOptions { auto_reading: false, ..Default::default() };
    for (i, title) in ["one", "two"].iter().enumerate() {
        let p = music.join(format!("{title}.mp3"));
        std::fs::write(&p, fixture("audio/plain.mp3")).unwrap();
        media::write_tags(&p, &TagEdit { title: Some(title.to_string()), album: Some("アルバム".into()), track_nr: Some(i as u32 + 1), ..Default::default() }).unwrap();
        ipod.add_file(&p, None, &opts).unwrap();
    }
    ipod.save().unwrap();
    let db = std::fs::read(root.join("iPod_Control/iTunes/iTunesDB")).unwrap();
    let art = std::fs::read(root.join("iPod_Control/Artwork/ArtworkDB")).unwrap();
    let _ = std::fs::remove_dir_all(&base);
    (db, art)
}

#[test]
fn fuzz_itunesdb_and_artworkdb_from_zenpod() {
    let (db, art) = written_by_zenpod();
    assert!(ArtworkDb::parse(&art).is_ok());
    fuzz("iTunesDB (ZenPod)", 2, 3000, &db, |b| {
        if let Ok(mut db) = ITunesDb::parse(&b) {
            // 読めたものは書き出せる
            let _ = db.serialize();
        }
    });
    fuzz("ArtworkDB", 3, 3000, &art, |b| {
        if let Ok(db) = ArtworkDb::parse(&b) {
            let _ = db.serialize();
        }
    });
}

#[test]
fn fuzz_rockbox_database() {
    let entries: Vec<rbdb::Entry> = (0..5)
        .map(|i| rbdb::Entry {
            path: format!("/Music/アーティスト/Album {i}/0{i} 曲.mp3"),
            title: format!("曲 {i}"),
            artist: "アーティスト".into(),
            album: format!("Album {i}"),
            genre: "Jungle".into(),
            year: 2001,
            track: i + 1,
            length_ms: 1000,
            bitrate: 128,
            ..Default::default()
        })
        .collect();
    let (files, _) = rbdb::build(&entries, None);
    for (k, (name, bytes)) in files.iter().enumerate() {
        let others: Vec<(String, Vec<u8>)> = files.iter().filter(|(n, _)| n != name).cloned().collect();
        let target = name.clone();
        fuzz(&format!("Rockbox {name}"), 10 + k as u64, 400, bytes, move |b| {
            let dir = tmp(&format!("rbdb-{}", std::thread::current().name().unwrap_or("x").replace("::", "-")));
            for (n, bytes) in &others {
                std::fs::write(dir.join(n), bytes).unwrap();
            }
            std::fs::write(dir.join(&target), &b).unwrap();
            let _ = rbdb::read(&dir);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}

#[test]
fn fuzz_itunes_xml() {
    let data = fixture("itunes12-smart-playlists.plist");
    fuzz("iTunes の XML", 20, 1500, &data, |b| {
        let _ = XmlLibrary::parse(&b);
    });
}

/// 曲のタグ: 読む・詳細情報・保存 (保存は確認付き。壊れたファイルでもエラーで返ればよい)
#[test]
fn fuzz_audio_tags() {
    for (k, name) in ["plain.mp3", "aac-faststart.m4a", "aac-moov-end.m4a", "plain.flac", "plain.ogg"].into_iter().enumerate() {
        // タグの付いた元を作る
        let dir = tmp(&format!("audio-src-{k}"));
        let ext = name.rsplit('.').next().unwrap().to_string();
        let src = dir.join(format!("src.{ext}"));
        std::fs::write(&src, fixture(&format!("audio/{name}"))).unwrap();
        media::write_tags(&src, &TagEdit { title: Some("Title".into()), artist: Some("Artist".into()), album: Some("Album".into()), lyrics: Some("la la".into()), ..Default::default() }).unwrap();
        media::write_cover(&src, Some(&fixture_png())).unwrap();
        let bytes = std::fs::read(&src).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        fuzz(&format!("タグ ({name})"), 30 + k as u64, 300, &bytes, move |b| {
            let dir = tmp(&format!("audio-{}", std::thread::current().name().unwrap_or("x").replace("::", "-")));
            let p = dir.join(format!("a.{ext}"));
            std::fs::write(&p, &b).unwrap();
            let _ = media::read_info(&p);
            let _ = media::details::file_details(&p);
            let _ = media::write_tags(&p, &TagEdit { title: Some("New".into()), album: Some(String::new()), year: Some(1999), ..Default::default() });
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}

fn fixture_png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(8, 8, image::Rgb([10, 200, 30])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}
