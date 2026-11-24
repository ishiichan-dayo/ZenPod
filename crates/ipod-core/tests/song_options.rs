//! iPod OS の曲の iTunes の「オプション」(シャッフル時にスキップ・再生位置を記憶・音量の調整・開始 / 停止・イコライザ)。
//! 種類を変えるときは種類の既定に戻してから、指定した値を書く。ffmpeg がある環境だけで確かめる

use std::path::PathBuf;

use ipod_core::domain::audio::MediaKind;
use ipod_core::domain::track::{AddOptions, TagEdit};
use ipod_core::library::{Library, Mode};

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

#[test]
fn song_play_options() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-songopts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let src = base.join("a.mp3");
    let ok = std::process::Command::new(&ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=duration=3", "-metadata", "title=a", "-c:a", "libmp3lame"])
        .arg(&src)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok);
    let root = base.join("ipod");
    for d in ["iTunes", "Device", "Music/F00"] {
        std::fs::create_dir_all(root.join("iPod_Control").join(d)).unwrap();
    }
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA002\n").unwrap();
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    lib.add_file(&src, Some(&ffmpeg), &AddOptions { auto_reading: false, ..Default::default() }).unwrap();
    lib.save().unwrap();
    let id: u64 = lib.tracks()[0].id.parse().unwrap();

    let o = lib.track_options(id).unwrap();
    assert!(!o.skip_shuffle && !o.remember_position);
    assert_eq!((o.volume, o.start_ms, o.stop_ms, o.eq), (0, 0, 0, 0));

    // ポッドキャストにしつつ「シャッフル時にスキップ」だけ外す (種類の既定はどちらもオン)
    let e = TagEdit {
        kind: Some(MediaKind::Podcast),
        skip_shuffle: Some(false),
        volume: Some(-128),
        start_ms: Some(500),
        stop_ms: Some(2500),
        eq: Some(104),
        ..Default::default()
    };
    assert_eq!(lib.edit_tags(&[id], &e).unwrap(), 1);
    lib.save().unwrap();
    let lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let o = lib.track_options(id).unwrap();
    assert!(!o.skip_shuffle && o.remember_position);
    assert_eq!((o.volume, o.start_ms, o.stop_ms, o.eq), (-128, 500, 2500, 104));
    assert_eq!(lib.tracks()[0].kind, MediaKind::Podcast);

    // 同じ値なら変えない。イコライザの 0 は「なし」
    let mut lib = lib;
    assert_eq!(lib.edit_tags(&[id], &TagEdit { volume: Some(-128), ..Default::default() }).unwrap(), 0);
    assert_eq!(lib.edit_tags(&[id], &TagEdit { eq: Some(0), stop_ms: Some(0), ..Default::default() }).unwrap(), 1);
    lib.save().unwrap();
    let lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let o = lib.track_options(id).unwrap();
    assert_eq!((o.eq, o.stop_ms), (0, 0));
    let _ = std::fs::remove_dir_all(&base);
}
