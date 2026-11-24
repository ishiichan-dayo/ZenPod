//! 埋め込みのジャケットを縮める (配布リポジトリの Issue #19)。転送のとき (iPod に置くコピーだけ) と、iPod にある曲をまとめて。
//! iPod OS でも Rockbox でも、縮めた画像はベースラインの JPEG で長い辺が 500px 以下になり、PC の元のファイルとほかのタグは変わらない。
//! iPod OS は iTunesDB の大きさもファイルに合わせる。ffmpeg がある環境だけで確かめる

use std::path::{Path, PathBuf};

use ipod_core::domain::track::AddOptions;
use ipod_core::library::{Library, Mode};
use ipod_core::media::{self, cover};

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

fn fake_ipod(dir: &Path, rockbox: bool) {
    std::fs::create_dir_all(dir.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Device")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Music/F00")).unwrap();
    std::fs::write(dir.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA002\n").unwrap();
    if rockbox {
        std::fs::create_dir_all(dir.join(".rockbox")).unwrap();
    }
}

fn encode(w: u32, format: image::ImageFormat) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_fn(w, w, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])).write_to(&mut out, format).unwrap();
    out.into_inner()
}

/// 曲を作り、画像を埋め込む
fn song(ffmpeg: &Path, path: &Path, codec: &str, title: &str, art: &[u8]) -> bool {
    let ok = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", codec])
        .args(["-metadata", &format!("title={title}"), "-metadata", "artist=アーティスト", "-metadata", &format!("album=アルバム {title}")])
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    ok && media::write_cover(path, Some(art)).is_ok()
}

fn embedded(path: &Path) -> Vec<u8> {
    match cover::find_cover_src(path) {
        Some((b, true)) => b,
        _ => panic!("埋め込みの画像が無い: {}", path.display()),
    }
}

#[test]
fn shrinks_embedded_artwork_on_transfer_and_in_place() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-shrink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let src = base.join("pc");
    std::fs::create_dir_all(&src).unwrap();
    let big = encode(1200, image::ImageFormat::Png);
    let small = cover::fit_jpeg(&encode(300, image::ImageFormat::Png), 500).unwrap();
    let mut files = Vec::new();
    for (name, codec, art) in [("a.mp3", "libmp3lame", &big), ("b.m4a", "aac", &big), ("c.flac", "flac", &big), ("d.mp3", "libmp3lame", &small)] {
        let p = src.join(name);
        if !song(&ffmpeg, &p, codec, name, art) {
            return;
        }
        files.push(p);
    }
    let originals: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();

    for (mode, rockbox) in [(Mode::Ipod, false), (Mode::Rockbox, true)] {
        // 転送のとき: iPod に置くコピーだけを縮める
        let root = base.join(format!("on-transfer-{rockbox}"));
        fake_ipod(&root, rockbox);
        let mut lib = Library::open(&root, mode, None).unwrap();
        let opts = AddOptions { auto_reading: false, shrink_artwork: Some(cover::EMBED_MAX), ..Default::default() };
        for f in &files {
            lib.add_file(f, Some(&ffmpeg), &opts).unwrap();
        }
        lib.save().unwrap();
        check(&root, mode, &small);
        for (f, o) in files.iter().zip(&originals) {
            assert_eq!(&std::fs::read(f).unwrap(), o, "PC の元のファイルは変えない ({mode:?})");
        }

        // iPod にある曲をまとめて
        let root = base.join(format!("in-place-{rockbox}"));
        fake_ipod(&root, rockbox);
        let mut lib = Library::open(&root, mode, None).unwrap();
        let opts = AddOptions { auto_reading: false, ..Default::default() };
        for f in &files {
            lib.add_file(f, Some(&ffmpeg), &opts).unwrap();
        }
        let ids: Vec<u64> = lib.tracks().iter().map(|t| t.id.parse().unwrap()).collect();
        assert!(ids.iter().any(|&id| !cover::fits(&embedded(&lib.track_file(id).unwrap()), cover::EMBED_MAX)), "縮める前は大きい");
        let seen = std::cell::Cell::new(0);
        let (done, failed) = lib.shrink_artwork(&ids, cover::EMBED_MAX, &|_, total| seen.set(total)).unwrap();
        assert_eq!((done, failed, seen.get()), (3, 0, 4), "小さい JPEG の曲は縮めない ({mode:?})");
        lib.save().unwrap();
        check(&root, mode, &small);
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// iPod の曲の埋め込みの画像がベースラインの JPEG で 500px 以下・小さい画像はそのまま・タグはそのまま・iPod OS の大きさはファイルと同じ
fn check(root: &Path, mode: Mode, small: &[u8]) {
    let lib = Library::open(root, mode, None).unwrap();
    let tracks = lib.tracks();
    assert_eq!(tracks.len(), 4, "{mode:?}");
    for t in &tracks {
        let f = lib.track_file(t.id.parse().unwrap()).unwrap();
        let art = embedded(&f);
        assert!(cover::fits(&art, cover::EMBED_MAX), "{} ({mode:?})", t.title);
        let img = image::load_from_memory(&art).unwrap();
        if t.title == "d.mp3" {
            assert_eq!(art, small, "小さい JPEG はそのまま ({mode:?})");
        } else {
            assert_eq!((img.width(), img.height()), (500, 500), "{} ({mode:?})", t.title);
        }
        assert_eq!(t.artist, "アーティスト", "{mode:?}");
        assert_eq!(t.album, format!("アルバム {}", t.title), "{mode:?}");
        if mode == Mode::Ipod {
            assert_eq!(t.size as u64, std::fs::metadata(&f).unwrap().len(), "iTunesDB の大きさ ({})", t.title);
        }
    }
}

/// 音声を復号した結果の md5 (中身が同じかを見る)
fn audio_md5(ffmpeg: &Path, path: &Path) -> String {
    let out = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a", "-f", "md5", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "復号できない: {}", path.display());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 縮めた分だけファイルが小さくなる。lofty は縮んだ分を余白 (M4A の free・FLAC の PADDING・ID3v2 の 0 の並び) で
/// 埋め、その場で書く作りでは元の画像の分 (数 MB) が残って容量が減らなかった (配布リポジトリの Issue #24)。
/// M4A は目次が前 (faststart) と後ろの両方。音声の位置を直すので、復号した結果が変わらないことも見る
#[test]
fn shrinking_artwork_frees_the_space() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    // 縮めても大きさが変わる、圧縮しにくい画像
    let mut seed = 1u32;
    let noise = image::RgbImage::from_fn(1500, 1500, |_, _| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
    });
    let mut big = std::io::Cursor::new(Vec::new());
    noise.write_to(&mut big, image::ImageFormat::Jpeg).unwrap();
    let big = big.into_inner();
    assert!(big.len() > 1_000_000);
    let mut problems = Vec::new();
    for (name, codec, extra) in [
        ("faststart.m4a", "aac", &["-movflags", "+faststart"][..]),
        ("moov-end.m4a", "aac", &[][..]),
        ("v23.mp3", "libmp3lame", &["-id3v2_version", "3"][..]),
        ("v24.mp3", "libmp3lame", &["-id3v2_version", "4"][..]),
        ("plain.flac", "flac", &[][..]),
    ] {
        let p = dir.path().join(name);
        let ok = std::process::Command::new(&ffmpeg)
            .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-c:a", codec])
            .args(extra)
            .args(["-metadata", "title=Girl", "-metadata", "artist=Artist"])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            continue;
        }
        let audio = audio_md5(&ffmpeg, &p);
        media::write_cover(&p, Some(&big)).unwrap();
        let before = std::fs::metadata(&p).unwrap().len();
        assert!(media::shrink_embedded_cover(&p, 320).unwrap(), "{name}: 縮めなかった");
        let after = std::fs::metadata(&p).unwrap().len();
        let art = embedded(&p).len() as u64;
        // 減るはずの分 (元の画像 - 縮めた画像) から、残してよい余白 (64KB) と見出しなどの分を引いたもの以上は減る
        let want = big.len() as u64 - art - 64 * 1024 - 4096;
        if before - after < want {
            problems.push(format!("{name}: {} バイトしか減らない ({before} → {after}、画像 {} → {art})", before - after, big.len()));
        }
        if audio_md5(&ffmpeg, &p) != audio {
            problems.push(format!("{name}: 音声が変わった"));
        }
        let info = media::read_info(&p).unwrap();
        if info.title.as_deref() != Some("Girl") || info.artist.as_deref() != Some("Artist") {
            problems.push(format!("{name}: タグが変わった: {:?} / {:?}", info.title, info.artist));
        }
        // 文字の編集 (少し縮む) は、今までどおりその場で書ける
        let e = ipod_core::domain::track::TagEdit { title: Some("G".into()), ..Default::default() };
        media::write_tags(&p, &e).unwrap();
        if audio_md5(&ffmpeg, &p) != audio {
            problems.push(format!("{name}: 文字の編集で音声が変わった"));
        }
    }
    assert!(problems.is_empty(), "問題:\n{}", problems.join("\n"));
}
