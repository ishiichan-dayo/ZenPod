//! 古い iPod (photo / nano 1G / mini / 4G) に転送できるか。
//! 機種ごとの Fxx フォルダの数と、アートワークの形式 (画像を出せない機種は作らない) を確かめる。
//! ffmpeg がある環境だけで確かめる (無音の mp3 を作って転送する)

use std::path::{Path, PathBuf};

use ipod_core::ipod::artwork::ArtworkDb;
use ipod_core::domain::track::AddOptions;
use ipod_core::Ipod;

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

fn jpeg() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(64, 64, image::Rgb([30, 160, 90]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

/// Music フォルダの無い偽の iPod (初めて曲を入れる状態)
fn fake_ipod(dir: &Path, model: &str) {
    std::fs::create_dir_all(dir.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Device")).unwrap();
    std::fs::write(dir.join("iPod_Control/Device/SysInfo"), format!("ModelNumStr: {model}\n")).unwrap();
}

fn music_dirs(root: &Path) -> usize {
    std::fs::read_dir(root.join("iPod_Control/Music")).map(|rd| rd.flatten().count()).unwrap_or(0)
}

#[test]
fn transfer_to_older_models() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-old-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let music = base.join("music");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(music.join("cover.jpg"), jpeg()).unwrap();
    let song = music.join("song.mp3");
    let ok = std::process::Command::new(&ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", "1"])
        .args(["-metadata", "title=曲", "-metadata", "album=アルバム"])
        .arg(&song)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return;
    }

    // (型番, Fxx の数, アートワークの (幅, 高さ)。空なら作らない)
    type Case = (&'static str, usize, &'static [(u16, u16)]);
    let cases: &[Case] = &[
        ("xA079", 50, &[(56, 56), (140, 140)]), // photo
        ("xA350", 3, &[(42, 42), (100, 100)]),  // nano 1G
        ("x9160", 6, &[]),                      // mini 1G
        ("x9282", 50, &[]),                     // 4G (白黒)
    ];
    for (model, dirs, sizes) in cases {
        let root = base.join(model);
        fake_ipod(&root, model);
        let mut ipod = Ipod::open(&root).unwrap();
        let opts = AddOptions { auto_reading: false, ..Default::default() };
        ipod.add_file(&song, None, &opts).unwrap();
        ipod.save().unwrap();

        assert_eq!(music_dirs(&root), *dirs, "{model}: Fxx の数");
        let again = Ipod::open(&root).unwrap();
        assert_eq!(again.tracks().len(), 1, "{model}");
        let art = root.join("iPod_Control/Artwork/ArtworkDB");
        if sizes.is_empty() {
            assert!(!art.exists(), "{model}: 画像を出せない機種には ArtworkDB を作らない");
            assert_eq!(again.tracks()[0].artwork_id, 0, "{model}");
            continue;
        }
        let db = ArtworkDb::parse(&std::fs::read(&art).unwrap()).unwrap();
        let img = db.image(again.tracks()[0].artwork_id).expect("mhii がある");
        let mut got: Vec<(u16, u16)> = img.thumbs.iter().map(|t| (t.width, t.height)).collect();
        got.sort();
        assert_eq!(got, *sizes, "{model}: 画像の大きさ");
    }
    let _ = std::fs::remove_dir_all(&base);
}
