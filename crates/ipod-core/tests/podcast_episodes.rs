//! 取得したポッドキャストの回 (タグを書いたファイル) を転送すると、iPod OS でも Rockbox でもポッドキャストとして入り、
//! 番組名 (アルバム) と回の名前 (曲名) で見分けられる。ffmpeg がある環境だけで確かめる

use std::path::{Path, PathBuf};

use ipod_core::library::{Library, Mode};
use ipod_core::domain::track::AddOptions;
use ipod_core::media;
use ipod_core::domain::audio::MediaKind;
use ipod_core::media::PodcastTags;

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

fn episode(ffmpeg: &Path, path: &Path) -> bool {
    let ok = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", "1"])
        .args(["-c:a", "libmp3lame", "-metadata", "title=ep52_final"])
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    ok && media::tag_podcast_episode(
        path,
        &PodcastTags {
            title: "#52 新しい本棚を作った話",
            show: "夜ふかしラジオ",
            author: "山田 太郎",
            published: 1_790_000_000,
            description: "",
            feed_url: "https://example.com/feed.xml",
            guid: "ep-52",
        },
    )
    .is_ok()
}

#[test]
fn episodes_go_in_as_podcasts() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-podep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let file = base.join("abc123.mp3");
    if !episode(&ffmpeg, &file) {
        return;
    }
    // 回に埋め込まれた画像を、縮めた番組の画像で入れ替える (アプリの apply_show_cover と同じ手順)
    let red = image::RgbImage::from_pixel(1500, 1500, image::Rgb([200, 30, 30]));
    let mut show = std::io::Cursor::new(Vec::new());
    red.write_to(&mut show, image::ImageFormat::Png).unwrap();
    let jpg = ipod_core::media::cover::thumbnails_jpeg(show.get_ref(), &[600]).unwrap().pop().unwrap();
    media::write_cover(&file, Some(&jpg)).unwrap();
    let embedded = ipod_core::media::cover::find_cover(&file).expect("画像が入った");
    let img = image::load_from_memory(&embedded).unwrap();
    assert_eq!((img.width(), img.height()), (600, 600));
    assert!(img.to_rgb8().get_pixel(300, 300)[0] > 150, "番組の画像 (赤) になる");
    for (mode, rockbox) in [(Mode::Ipod, false), (Mode::Rockbox, true)] {
        let root = base.join(format!("ipod-{rockbox}"));
        fake_ipod(&root, rockbox);
        let mut lib = Library::open(&root, mode, None).unwrap();
        let opts = AddOptions { auto_reading: false, ..Default::default() };
        lib.add_file(&file, None, &opts).unwrap();
        lib.save().unwrap();
        let lib = Library::open(&root, mode, None).unwrap();
        let t = lib.tracks().into_iter().next().expect("曲が入った");
        assert_eq!(t.kind, MediaKind::Podcast, "{mode:?}");
        assert_eq!(t.album, "夜ふかしラジオ", "{mode:?}");
        assert_eq!(t.title, "#52 新しい本棚を作った話", "{mode:?}");
        assert_eq!(t.artist, "山田 太郎", "{mode:?}");
        if mode == Mode::Ipod {
            // 公開日 (time_released) が入る
            let db = ipod_core::ipod::itunesdb::ITunesDb::parse(&std::fs::read(root.join("iPod_Control/iTunes/iTunesDB")).unwrap()).unwrap();
            let released = ipod_core::ipod::itunesdb::mac_to_unix(db.tracks[0].time_released_mac());
            assert!((released - 1_790_000_000).abs() < 2 * 86400, "{released}");
        }
        // 同じ回をもう一度送ると重複になる (同期で送り直さない)
        let mut lib = lib;
        let again = lib.add_file(&file, None, &AddOptions { auto_reading: false, ..Default::default() }).unwrap();
        assert!(matches!(again, ipod_core::domain::track::AddOutcome::Duplicate { .. }), "{mode:?}");
    }
    let _ = std::fs::remove_dir_all(&base);
}
