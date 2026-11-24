//! 歌詞のある曲を iPod OS に転送すると、iTunesDB の曲に歌詞の印 (libgpod の lyrics_flag) が付く。
//! iPod は印のある曲でしかファイルの歌詞を出さない (配布リポジトリの Issue #12)。ffmpeg がある環境だけで確かめる

use std::path::{Path, PathBuf};

use ipod_core::domain::track::{AddOptions, TagEdit};
use ipod_core::ipod::itunesdb::ITunesDb;
use ipod_core::library::{Library, Mode};
use ipod_core::media;

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

fn song(ffmpeg: &Path, path: &Path, title: &str, extra: &[&str]) -> bool {
    std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1"])
        .args(["-metadata", &format!("title={title}")])
        .args(extra)
        .arg(path)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn lyrics_flag_follows_the_file() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-lyricsflag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let pc = base.join("pc");
    std::fs::create_dir_all(&pc).unwrap();
    // FLAC の UNSYNCEDLYRICS は変換で写す
    let files = [
        (pc.join("with.mp3"), "with", vec!["-c:a", "libmp3lame"]),
        (pc.join("without.mp3"), "without", vec!["-c:a", "libmp3lame"]),
        (pc.join("flac.flac"), "flac", vec!["-c:a", "flac", "-metadata", "UNSYNCEDLYRICS=la la la"]),
    ];
    for (p, title, extra) in &files {
        assert!(song(&ffmpeg, p, title, extra));
    }
    // MP3 の歌詞の枠 (USLT) は lofty で書く (ffmpeg 6 は TXXX に書く)
    {
        use lofty::config::WriteOptions;
        use lofty::prelude::*;
        let mut tf = lofty::read_from_path(&files[0].0).unwrap();
        let tag = tf.primary_tag_mut().unwrap();
        tag.insert_text(ItemKey::UnsyncLyrics, "la la la".into());
        tag.save_to_path(&files[0].0, WriteOptions::default()).unwrap();
    }

    let root = base.join("ipod");
    std::fs::create_dir_all(root.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    std::fs::create_dir_all(root.join("iPod_Control/Music/F00")).unwrap();
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA002\n").unwrap();
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let opts = AddOptions { auto_reading: false, ..Default::default() };
    for (p, ..) in &files {
        lib.add_file(p, Some(&ffmpeg), &opts).unwrap();
    }
    lib.save().unwrap();

    let db = ITunesDb::parse(&std::fs::read(root.join("iPod_Control/iTunes/iTunesDB")).unwrap()).unwrap();
    let flag = |title: &str| db.tracks.iter().find(|t| t.title() == Some(title)).unwrap().lyrics_flag();
    assert!(flag("with"));
    assert!(!flag("without"));
    assert!(flag("flac"));

    // 情報の編集で歌詞を入れる・消すと、iPod の中のファイルに書き、印と大きさも直す
    let id = |lib: &Library, title: &str| lib.tracks().into_iter().find(|t| t.title == title).unwrap().id.parse::<u64>().unwrap();
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let without = id(&lib, "without");
    let edit = |lyrics: &str| TagEdit { lyrics: Some(lyrics.into()), ..Default::default() };
    assert_eq!(lib.edit_tags(&[without], &edit("一行目\r\n二行目  \n\n")).unwrap(), 1);
    lib.save().unwrap();
    let file = lib.track_file(without).unwrap();
    assert_eq!(media::read_lyrics(&file).unwrap().as_deref(), Some("一行目\n二行目"));
    let db = ITunesDb::parse(&std::fs::read(root.join("iPod_Control/iTunes/iTunesDB")).unwrap()).unwrap();
    let t = db.tracks.iter().find(|t| t.title() == Some("without")).unwrap();
    assert!(t.lyrics_flag());
    assert_eq!(t.size() as u64, std::fs::metadata(&file).unwrap().len());

    lib.edit_tags(&[without], &edit("")).unwrap();
    lib.save().unwrap();
    assert_eq!(media::read_lyrics(&file).unwrap(), None);
    let db = ITunesDb::parse(&std::fs::read(root.join("iPod_Control/iTunes/iTunesDB")).unwrap()).unwrap();
    assert!(!db.tracks.iter().find(|t| t.title() == Some("without")).unwrap().lyrics_flag());
    let _ = std::fs::remove_dir_all(&base);
}

/// MP3 のタグを書き換えても ID3v2 の版は変えない (iPod は 2.4 を読めないことがある)。2.3 の歌詞は UTF-16 になる
#[test]
fn keeps_id3v2_version() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-id3ver-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    for version in [3u8, 4] {
        let p = base.join(format!("v{version}.mp3"));
        let v = version.to_string();
        assert!(song(&ffmpeg, &p, "t", &["-c:a", "libmp3lame", "-id3v2_version", &v]));
        assert_eq!(std::fs::read(&p).unwrap()[3], version);
        media::write_tags(&p, &TagEdit { lyrics: Some("日本語\nline".into()), artist: Some("歌手".into()), ..Default::default() }).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bytes[3], version, "v2.{version}");
        let at = bytes.windows(4).position(|w| w == b"USLT").unwrap();
        // 文字コードの印: 2.3 は UTF-16 (1)、2.4 は UTF-8 (3)
        assert_eq!(bytes[at + 10], if version == 3 { 1 } else { 3 }, "v2.{version}");
        assert_eq!(media::read_lyrics(&p).unwrap().as_deref(), Some("日本語\nline"));
        assert_eq!(media::read_info(&p).unwrap().artist.as_deref(), Some("歌手"));
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// 歌詞を形式ごとの決まった場所に書き、読み直せる (MP3 は USLT、M4A は ©lyr、FLAC は LYRICS)
#[test]
fn lyrics_round_trip() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-lyricsrt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    for (name, codec, key) in [("a.mp3", "libmp3lame", "USLT"), ("a.m4a", "aac", "\u{a9}lyr"), ("a.flac", "flac", "LYRICS")] {
        let p = base.join(name);
        assert!(song(&ffmpeg, &p, "t", &["-c:a", codec]));
        assert_eq!(media::read_lyrics(&p).unwrap(), None, "{name}");
        media::write_tags(&p, &TagEdit { lyrics: Some("こんにちは\nhello".into()), ..Default::default() }).unwrap();
        assert_eq!(media::read_lyrics(&p).unwrap().as_deref(), Some("こんにちは\nhello"), "{name}");
        let details = media::details::file_details(&p).unwrap();
        let json = serde_json::to_string(&details).unwrap();
        assert!(json.contains(key), "{name}: {json}");
        // 曲名はそのまま
        assert_eq!(media::read_info(&p).unwrap().title.as_deref(), Some("t"), "{name}");
    }
    let _ = std::fs::remove_dir_all(&base);
}
