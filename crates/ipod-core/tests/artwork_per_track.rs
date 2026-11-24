//! アートワークは曲ごとに mhii を作る (iPod video 5G などは mhii の song_id でしか画像を探さないため)。
//! ffmpeg がある環境だけで確かめる (無音の mp3 を作って転送する)

use std::path::{Path, PathBuf};

use ipod_core::ipod::artwork::ArtworkDb;
use ipod_core::domain::track::AddOptions;
use ipod_core::Ipod;

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

fn jpeg(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(64, 64, image::Rgb(rgb));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

/// 偽の iPod video 5G (アートワーク対応、DB 署名なし)
fn fake_ipod(dir: &Path) {
    std::fs::create_dir_all(dir.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Device")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Music/F00")).unwrap();
    std::fs::write(dir.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA002\n").unwrap();
}

fn artwork_db(root: &Path) -> ArtworkDb {
    ArtworkDb::parse(&std::fs::read(root.join("iPod_Control/Artwork/ArtworkDB")).unwrap()).unwrap()
}

/// 曲ごとに mhii があり、その song_id が曲自身を指しているか
fn assert_per_track(ipod: &Ipod, db: &ArtworkDb) {
    for t in ipod.tracks() {
        assert_ne!(t.artwork_id, 0, "{}", t.title);
        let img = db.image(t.artwork_id).expect("mhii がある");
        assert_eq!(img.song_id.to_string(), t.id, "{}", t.title);
    }
    let ids: std::collections::HashSet<u32> = ipod.tracks().iter().map(|t| t.artwork_id).collect();
    assert_eq!(ids.len(), ipod.tracks().len(), "曲ごとに別の mhii");
}

#[test]
fn artwork_is_per_track_but_pixels_are_shared() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-art-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("ipod");
    let music = base.join("music");
    fake_ipod(&root);
    std::fs::create_dir_all(&music).unwrap();
    // 同じフォルダの cover.jpg が 2 曲のジャケットになる
    std::fs::write(music.join("cover.jpg"), jpeg([200, 40, 40])).unwrap();
    for (i, title) in ["one", "two"].iter().enumerate() {
        let ok = std::process::Command::new(&ffmpeg)
            .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", "1"])
            .args(["-metadata", &format!("title={title}"), "-metadata", "album=同じアルバム"])
            .args(["-metadata", &format!("track={}", i + 1)])
            .arg(music.join(format!("{title}.mp3")))
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
    }

    let mut ipod = Ipod::open(&root).unwrap();
    let opts = AddOptions { auto_reading: false, ..Default::default() };
    for title in ["one", "two"] {
        ipod.add_file(&music.join(format!("{title}.mp3")), None, &opts).unwrap();
    }
    ipod.save().unwrap();
    let db = artwork_db(&root);
    assert_per_track(&ipod, &db);
    // 同じ画像なので ithmb の画素データは同じ場所を指す (容量を増やさない)
    let offsets: Vec<Vec<u32>> = ipod
        .tracks()
        .iter()
        .map(|t| db.image(t.artwork_id).unwrap().thumbs.iter().map(|th| th.offset).collect())
        .collect();
    assert_eq!(offsets[0], offsets[1]);

    // 画像を差し替えても曲ごとのまま。古い mhii は残らない
    let ids: Vec<u64> = ipod.tracks().iter().map(|t| t.id.parse().unwrap()).collect();
    ipod.set_artwork_from_bytes(&ids, &jpeg([40, 40, 200])).unwrap();
    ipod.save().unwrap();
    let db = artwork_db(&root);
    assert_per_track(&ipod, &db);
    assert_eq!(db.images.len(), 2);

    // もう一度開いても、分け直すものは無い
    let mut again = Ipod::open(&root).unwrap();
    assert_eq!(again.unshare_artwork(), 0);
    let _ = std::fs::remove_dir_all(&base);
}
