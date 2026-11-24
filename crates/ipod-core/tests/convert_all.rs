//! 「変換して転送」(配布リポジトリの Issue #16): 形式を選んで送ると、ロスレスの曲と、選んだビットレートより高い曲を
//! その形式にする。低いビットレートの曲はそのまま。iPod OS と Rockbox の両方。ffmpeg がある環境だけで確かめる

use std::path::{Path, PathBuf};

use ipod_core::domain::audio::ConvertTarget;
use ipod_core::domain::track::AddOptions;
use ipod_core::library::{Library, Mode};

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

fn song(ffmpeg: &Path, path: &Path, title: &str, codec: &[&str]) {
    let ok = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=duration=3:frequency=440", "-ac", "2"])
        .args(["-metadata", &format!("title={title}")])
        .args(codec)
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{title}");
}

/// (曲名, ファイルの種類の表示, 拡張子)
fn kinds(lib: &Library) -> Vec<(String, String, String)> {
    let mut v: Vec<_> = lib
        .tracks()
        .into_iter()
        .map(|t| {
            let ext = lib.track_file(t.id.parse().unwrap()).and_then(|p| p.extension().map(|e| e.to_string_lossy().to_lowercase())).unwrap_or_default();
            (t.title, t.filetype, ext)
        })
        .collect();
    v.sort();
    v
}

#[test]
fn convert_all_to_aac() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-convertall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let pc = base.join("pc");
    std::fs::create_dir_all(&pc).unwrap();
    let files = [
        (pc.join("alac.m4a"), "alac", vec!["-c:a", "alac"]),
        (pc.join("flac.flac"), "flac", vec!["-c:a", "flac"]),
        (pc.join("wav.wav"), "wav", vec!["-c:a", "pcm_s16le"]),
        (pc.join("mp3high.mp3"), "mp3high", vec!["-c:a", "libmp3lame", "-b:a", "320k"]),
        (pc.join("mp3low.mp3"), "mp3low", vec!["-c:a", "libmp3lame", "-b:a", "96k"]),
    ];
    for (p, title, codec) in &files {
        song(&ffmpeg, p, title, codec);
    }
    let opts = AddOptions { auto_reading: false, artwork: false, convert_all_to: Some(ConvertTarget::Aac { kbps: 128 }), ..Default::default() };

    // iPod OS: 低いビットレートの MP3 だけそのまま
    let root = base.join("ipod");
    for d in ["iTunes", "Device", "Music/F00"] {
        std::fs::create_dir_all(root.join("iPod_Control").join(d)).unwrap();
    }
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA002\n").unwrap();
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    for (p, ..) in &files {
        lib.add_file(p, Some(&ffmpeg), &opts).unwrap();
    }
    lib.save().unwrap();
    let got = kinds(&Library::open(&root, Mode::Ipod, None).unwrap());
    for (title, filetype, ext) in &got {
        if title == "mp3low" {
            assert_eq!(ext, "mp3", "{got:?}");
        } else {
            assert_eq!((filetype.as_str(), ext.as_str()), ("AAC audio file", "m4a"), "{title}: {got:?}");
        }
    }

    // 転送済みの曲を「上書き」で送り直すと、変換した曲に置き換わる (曲の数は増えない)
    let root2 = base.join("ipod2");
    for d in ["iTunes", "Device", "Music/F00"] {
        std::fs::create_dir_all(root2.join("iPod_Control").join(d)).unwrap();
    }
    std::fs::write(root2.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA002\n").unwrap();
    let mut lib = Library::open(&root2, Mode::Ipod, None).unwrap();
    let plain = AddOptions { auto_reading: false, artwork: false, ..Default::default() };
    lib.add_file(&files[0].0, Some(&ffmpeg), &plain).unwrap();
    lib.save().unwrap();
    assert_eq!(kinds(&lib)[0].2, "m4a");
    assert_eq!(kinds(&lib)[0].1, "Apple Lossless audio file");
    let replace = AddOptions { replace_duplicates: true, ..opts };
    lib.add_file(&files[0].0, Some(&ffmpeg), &replace).unwrap();
    lib.save().unwrap();
    let got = kinds(&Library::open(&root2, Mode::Ipod, None).unwrap());
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].1, "AAC audio file", "{got:?}");

    // Rockbox も同じ (ふだんは FLAC・WAV もそのまま置く)
    let rb = base.join("rockbox");
    std::fs::create_dir_all(rb.join(".rockbox")).unwrap();
    let mut lib = Library::open(&rb, Mode::Rockbox, None).unwrap();
    for (p, ..) in &files {
        lib.add_file(p, Some(&ffmpeg), &opts).unwrap();
    }
    lib.save().unwrap();
    let got = kinds(&lib);
    for (title, _, ext) in &got {
        assert_eq!(ext, if title == "mp3low" { "mp3" } else { "m4a" }, "{title}: {got:?}");
    }
    let _ = std::fs::remove_dir_all(&base);
}
