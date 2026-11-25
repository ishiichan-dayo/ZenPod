//! iPod shuffle (1G〜4G) への転送: iTunesDB と一緒に iTunesSD を書くか、再生できない形式 (ALAC・AIFF) を変換するか、
//! 本体が書いた再生回数 (iTunesStats) を取り込むか。SysInfo の無い shuffle を iTunesSD で見分けるか

use std::path::{Path, PathBuf};

use ipod_core::Ipod;
use ipod_core::domain::track::AddOptions;
use ipod_core::ipod::shuffle::{SdFormat, sd_paths};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio").join(name)
}

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

fn fake_shuffle(dir: &Path, model: Option<&str>) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Device")).unwrap();
    if let Some(m) = model {
        std::fs::write(dir.join("iPod_Control/Device/SysInfo"), format!("ModelNumStr: {m}\n")).unwrap();
    }
}

fn itunes_sd(root: &Path) -> (SdFormat, Vec<String>) {
    sd_paths(&std::fs::read(root.join("iPod_Control/iTunes/iTunesSD")).unwrap()).expect("iTunesSD を読める")
}

fn base(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zenpod-shuffle-{name}-{}", std::process::id()))
}

#[test]
fn writes_itunes_sd_for_each_generation() {
    for (model, format, name) in [
        ("xA546", SdFormat::V1, "iPod shuffle (2G) 1GB"),
        ("M9724", SdFormat::V1, "iPod shuffle (1G) 512MB"),
        ("xC584", SdFormat::V2, "iPod shuffle (4G) 2GB"),
    ] {
        let root = base(model);
        fake_shuffle(&root, Some(model));
        let mut ipod = Ipod::open(&root).unwrap();
        assert_eq!(ipod.info.shuffle, Some(format), "{model}");
        assert_eq!(ipod.info.model_name, name);
        assert!(!ipod.info.video && !ipod.info.photos && !ipod.artwork_supported(), "{model}: 画面が無い");
        let opts = AddOptions { auto_reading: false, ..Default::default() };
        let mp3 = ipod.add_file(&fixture("plain.mp3"), None, &opts).unwrap();
        let m4a = ipod.add_file(&fixture("aac-faststart.m4a"), None, &opts).unwrap();
        let id = |o: &ipod_core::domain::track::AddOutcome| match o {
            ipod_core::domain::track::AddOutcome::Added { track, .. } => track.id.parse::<u64>().unwrap(),
            _ => panic!("追加されない"),
        };
        ipod.create_playlist("好きな曲", &[id(&m4a)]).unwrap();
        ipod.save().unwrap();
        let _ = mp3;

        // Fxx は 3 つ
        let dirs = std::fs::read_dir(root.join("iPod_Control/Music")).unwrap().count();
        assert_eq!(dirs, 3, "{model}");
        let (f, paths) = itunes_sd(&root);
        assert_eq!((f, paths.len()), (format, 2), "{model}");
        for p in &paths {
            assert!(p.starts_with("/iPod_Control/Music/F0"), "{p}");
            assert!(root.join(p.trim_start_matches('/')).is_file(), "{p} が iPod にある");
        }
        // iTunesDB も読める
        assert_eq!(Ipod::open(&root).unwrap().tracks().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn recognizes_shuffle_without_sysinfo_by_itunes_sd() {
    // 型番の分かる shuffle で一度書いてから、SysInfo を消して開き直す
    let root = base("nosysinfo");
    fake_shuffle(&root, Some("xC323"));
    let mut ipod = Ipod::open(&root).unwrap();
    ipod.add_file(&fixture("plain.mp3"), None, &AddOptions { auto_reading: false, ..Default::default() }).unwrap();
    ipod.save().unwrap();
    std::fs::remove_file(root.join("iPod_Control/Device/SysInfo")).unwrap();
    let ipod = Ipod::open(&root).unwrap();
    assert_eq!(ipod.info.shuffle, Some(SdFormat::V2));
    assert_eq!(ipod.info.model_name, "iPod shuffle");
    // ふつうの iPod は shuffle にしない
    let other = base("classic");
    fake_shuffle(&other, Some("xB029"));
    assert_eq!(Ipod::open(&other).unwrap().info.shuffle, None);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn merges_play_counts_from_itunes_stats() {
    let root = base("stats");
    fake_shuffle(&root, Some("xC584"));
    let mut ipod = Ipod::open(&root).unwrap();
    let opts = AddOptions { auto_reading: false, ..Default::default() };
    ipod.add_file(&fixture("plain.mp3"), None, &opts).unwrap();
    ipod.add_file(&fixture("aac-faststart.m4a"), None, &opts).unwrap();
    ipod.save().unwrap();
    let (_, paths) = itunes_sd(&root);

    // 本体が書く iTunesStats (iTunesSD の順): 2 曲目を 3 回再生
    let mut stats = Vec::new();
    stats.extend_from_slice(&2u32.to_le_bytes());
    stats.extend_from_slice(&[0; 4]);
    for plays in [0u32, 3] {
        let mut e = [0u8; 32];
        e[0..4].copy_from_slice(&32u32.to_le_bytes());
        e[8..12].copy_from_slice(&plays.to_le_bytes());
        stats.extend_from_slice(&e);
    }
    let itunes = root.join("iPod_Control/iTunes");
    std::fs::write(itunes.join("iTunesStats"), &stats).unwrap();
    std::fs::write(itunes.join("iTunesShuffle"), [0u8; 8]).unwrap();

    let mut ipod = Ipod::open(&root).unwrap();
    assert!(ipod.is_dirty());
    let played = ipod.tracks().into_iter().find(|t| t.play_count == 3).expect("再生回数を取り込む");
    let file = ipod.track_file(played.id.parse().unwrap()).unwrap();
    assert!(file.to_string_lossy().replace('\\', "/").ends_with(&paths[1]));
    ipod.save().unwrap();
    // 取り込んだら消す (シャッフルの順も)
    assert!(!itunes.join("iTunesStats").exists() && !itunes.join("iTunesShuffle").exists());
    let ipod = Ipod::open(&root).unwrap();
    assert_eq!(ipod.tracks().iter().map(|t| t.play_count).sum::<u32>(), 3, "二重に足さない");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn converts_formats_shuffle_cannot_play() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let dir = base("src");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let aiff = dir.join("song.aiff");
    let ok = std::process::Command::new(&ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", "1"])
        .arg(&aiff)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return;
    }
    let opts = AddOptions { auto_reading: false, ..Default::default() };
    // 1G / 2G は ALAC も AIFF も再生できない → AAC
    let root = base("v1conv");
    fake_shuffle(&root, Some("xA546"));
    let mut ipod = Ipod::open(&root).unwrap();
    for src in [fixture("alac.m4a"), aiff.clone()] {
        ipod.add_file(&src, Some(&ffmpeg), &opts).unwrap();
    }
    ipod.save().unwrap();
    for t in ipod.tracks() {
        assert_eq!(t.filetype, "AAC audio file", "{}", t.title);
    }
    // 3G / 4G は ALAC はそのまま、AIFF は ALAC に
    let root2 = base("v2conv");
    fake_shuffle(&root2, Some("xC584"));
    let mut ipod = Ipod::open(&root2).unwrap();
    for src in [fixture("alac.m4a"), aiff] {
        ipod.add_file(&src, Some(&ffmpeg), &opts).unwrap();
    }
    for t in ipod.tracks() {
        assert_eq!(t.filetype, "Apple Lossless audio file", "{}", t.title);
    }
    for d in [dir, root, root2] {
        let _ = std::fs::remove_dir_all(d);
    }
}
