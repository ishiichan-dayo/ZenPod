//! 動画と写真: iPod OS (iTunesDB / Photo Database) と Rockbox (Videos / Pictures) に送って、読み直しても同じに見える。
//! 動画は ffmpeg がある環境だけで確かめる

use std::path::{Path, PathBuf};

use ipod_core::domain::photo::PhotoOutcome;
use ipod_core::domain::video::{VideoEdit, VideoKind, VideoOptions, VideoOutcome, VideoPlacement, VideoSize};
use ipod_core::library::{Library, Mode};
use ipod_core::media::video::Progress;

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

/// 偽の iPod。`model` は SysInfo の ModelNumStr (xB029 = classic、xA002 = video 5G)
fn fake_ipod(dir: &Path, model: &str) {
    std::fs::create_dir_all(dir.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Device")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Music/F00")).unwrap();
    std::fs::write(dir.join("iPod_Control/Device/SysInfo"), format!("ModelNumStr: {model}\nFirewireGuid: 0x000A27001A2B3C4D\n")).unwrap();
}

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("zenpod-vp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn make_video(ff: &Path, out: &Path, size: &str, extra: &[&str]) {
    let ok = std::process::Command::new(ff)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc=size={size}:rate=25:duration=2"), "-f", "lavfi", "-i", "sine=duration=2"])
        .args(["-pix_fmt", "yuv420p"])
        .args(extra)
        .arg(out)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

fn photo(path: &Path, w: u32, h: u32, rgb: [u8; 3]) {
    image::RgbImage::from_fn(w, h, |x, _| if x < w / 2 { image::Rgb(rgb) } else { image::Rgb([255, 255, 255]) }).save(path).unwrap();
}

const NO_PROGRESS: Progress = Progress { report: &|_| {}, cancelled: &|| false };

#[test]
fn ipod_os_videos() {
    let Some(ff) = which_ffmpeg() else { return };
    let base = tmp("ipodvideo");
    let root = base.join("ipod");
    fake_ipod(&root, "xB029");
    // そのまま送れる動画 (H.264 Baseline 320×240) と、変換が要る動画 (High 1280×720・テレビ番組のタグ)
    let ready = base.join("ready.m4v");
    make_video(&ff, &ready, "320x240", &["-c:v", "libx264", "-profile:v", "baseline", "-level", "3.0", "-c:a", "aac", "-metadata", "title=そのまま"]);
    let big = base.join("big.mkv");
    make_video(&ff, &big, "1280x720", &["-c:v", "libx264", "-profile:v", "high", "-c:a", "aac", "-metadata", "title=第 4 話", "-metadata", "show=番組", "-metadata", "season_number=1", "-metadata", "episode_sort=4"]);

    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let opts = VideoOptions { size: VideoSize::Qvga, ..Default::default() };
    let VideoOutcome::Added { video, converted } = lib.add_video(&ready, Some(&ff), &opts, &NO_PROGRESS).unwrap() else { panic!() };
    assert!(!converted);
    assert_eq!((video.title.as_str(), video.kind), ("そのまま", VideoKind::Movie));
    assert!(video.artwork_id != 0, "一覧の画像 (1 コマ) が入る");
    let VideoOutcome::Added { video: tv, converted } = lib.add_video(&big, Some(&ff), &opts, &NO_PROGRESS).unwrap() else { panic!() };
    assert!(converted);
    assert_eq!((tv.kind, tv.show.as_str(), tv.season, tv.episode), (VideoKind::TvShow, "番組", 1, 4));
    // 同じ動画は送らない
    assert!(matches!(lib.add_video(&ready, Some(&ff), &opts, &NO_PROGRESS).unwrap(), VideoOutcome::Duplicate { .. }));
    // 画面で開いているところ (ミュージックビデオのアーティスト) に落とすと、タグに無くてもその種類・アーティストになる
    let mv = base.join("mv.m4v");
    make_video(&ff, &mv, "320x240", &["-c:v", "libx264", "-profile:v", "baseline", "-level", "3.0", "-c:a", "aac", "-metadata", "title=MV"]);
    let placed = VideoOptions { placement: VideoPlacement { kind: Some(VideoKind::MusicVideo), artist: Some("歌手".into()), show: None }, ..opts.clone() };
    let VideoOutcome::Added { video: mvv, .. } = lib.add_video(&mv, Some(&ff), &placed, &NO_PROGRESS).unwrap() else { panic!() };
    assert_eq!((mvv.kind, mvv.artist.as_str()), (VideoKind::MusicVideo, "歌手"));
    lib.save().unwrap();
    // 曲の一覧とプレイリストには出ない
    assert!(lib.tracks().is_empty());
    assert!(lib.playlists().iter().all(|p| p.track_ids.is_empty()));

    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let vids = lib.videos();
    assert_eq!(vids.len(), 3);
    assert!(vids.iter().any(|v| v.kind == VideoKind::MusicVideo && v.artist == "歌手"));
    let tv = vids.iter().find(|v| v.kind == VideoKind::TvShow).unwrap();
    // 変換したファイルは iPod で再生できる形 (320×240 に収めた H.264 Baseline)
    let file = lib.video_file(tv.id.parse().unwrap()).unwrap();
    assert_eq!(file.extension().unwrap(), "m4v");
    let info = ipod_core::media::video::probe_mp4(&file).unwrap();
    assert!(ipod_core::media::video::ipod_playable(&info), "{info:?}");
    assert_eq!((info.width, info.height), (320, 180));
    // 種類と番組を変える
    let id: u64 = tv.id.parse().unwrap();
    let n = lib.edit_videos(&[id], &VideoEdit { kind: Some(VideoKind::MusicVideo), title: Some("改題".into()), ..Default::default() }).unwrap();
    assert_eq!(n, 1);
    lib.save().unwrap();
    let lib2 = Library::open(&root, Mode::Ipod, None).unwrap();
    let v = lib2.videos().into_iter().find(|v| v.id == tv.id).unwrap();
    assert_eq!((v.kind, v.title.as_str()), (VideoKind::MusicVideo, "改題"));
    drop(lib2);
    // 消すとファイルも消える
    assert_eq!(lib.remove_videos(&[id]).unwrap(), 1);
    lib.save().unwrap();
    assert!(!file.exists());
    assert_eq!(lib.videos().len(), 2);
    let _ = std::fs::remove_dir_all(&base);
}

/// iTunes の「情報を見る」と同じ項目を変えて、読み直しても同じ。選んだアートワークは一覧の画像にも使う (Issue #15)
#[test]
fn ipod_os_video_details() {
    let Some(ff) = which_ffmpeg() else { return };
    let base = tmp("ipodvideodetail");
    let root = base.join("ipod");
    fake_ipod(&root, "xB029");
    let mv = base.join("mv.m4v");
    make_video(&ff, &mv, "320x240", &["-c:v", "libx264", "-profile:v", "baseline", "-level", "3.0", "-c:a", "aac", "-metadata", "title=Live"]);
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let opts = VideoOptions { size: VideoSize::Qvga, ..Default::default() };
    let VideoOutcome::Added { video, .. } = lib.add_video(&mv, Some(&ff), &opts, &NO_PROGRESS).unwrap() else { panic!() };
    assert!(!video.custom_art, "転送で作った画像は利用者が選んだものではない");
    let id: u64 = video.id.parse().unwrap();
    let edit = VideoEdit {
        kind: Some(VideoKind::MusicVideo),
        artist: Some("椎名林檎".into()),
        album: Some("夏の歌".into()),
        album_artist: Some("Album Artist".into()),
        genre: Some("Jungle".into()),
        composer: Some("作曲".into()),
        year: Some(2005),
        track_nr: Some(2),
        disc_nr: Some(1),
        sort_artist: Some("シイナリンゴ".into()),
        show: Some("番組".into()),
        season: Some(3),
        episode: Some(4),
        episode_id: Some("S3E04".into()),
        network: Some("放送局".into()),
        description: Some("説明".into()),
        rating: Some(80),
        skip_shuffle: Some(true),
        grouping: Some("グループ".into()),
        comment: Some("コメント".into()),
        released: Some(1_700_000_000),
        volume: Some(-128),
        start_ms: Some(1500),
        stop_ms: Some(1800),
        eq: Some(107),
        sort_composer: Some("サッキョク".into()),
        ..Default::default()
    };
    assert_eq!(lib.edit_videos(&[id], &edit).unwrap(), 1);
    // 同じ内容をもう一度送っても変わらない
    assert_eq!(lib.edit_videos(&[id], &edit).unwrap(), 0);
    lib.save().unwrap();

    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let v = lib.videos().into_iter().find(|v| v.id == video.id).unwrap();
    assert_eq!(v.kind, VideoKind::MusicVideo);
    assert_eq!((v.artist.as_str(), v.sort_artist.as_str()), ("椎名林檎", "シイナリンゴ"));
    assert_eq!((v.album.as_str(), v.album_artist.as_str(), v.genre.as_str(), v.composer.as_str()), ("夏の歌", "Album Artist", "Jungle", "作曲"));
    assert_eq!((v.year, v.track_nr, v.disc_nr, v.rating), (2005, 2, 1, 80));
    // 読みを指定しなかった名前は自動で付く
    assert!(!v.sort_album.is_empty() && !v.sort_show.is_empty(), "{v:?}");
    assert_eq!((v.show.as_str(), v.season, v.episode, v.episode_id.as_str(), v.network.as_str()), ("番組", 3, 4, "S3E04", "放送局"));
    assert_eq!(v.description, "説明");
    assert_eq!((v.grouping.as_str(), v.comment.as_str(), v.released, v.sort_composer.as_str()), ("グループ", "コメント", 1_700_000_000, "サッキョク"));
    assert_eq!((v.volume, v.start_ms, v.stop_ms, v.eq), (-128, 1500, 1800, 107));
    assert_eq!((v.filetype.as_str(), v.ipod_path.starts_with(":iPod_Control:Music:")), ("MPEG-4 video file", true), "{v:?}");
    // イコライザを外す・再生回数を戻す
    lib.edit_videos(&[id], &VideoEdit { eq: Some(0), reset_play_count: true, ..Default::default() }).unwrap();
    let v = lib.videos().into_iter().find(|v| v.id == video.id).unwrap();
    assert_eq!((v.eq, v.play_count, v.last_played), (0, 0, 0));
    // ミュージックビデオの既定は「覚えない」、シャッフルで飛ばすは指定どおり
    assert_eq!((v.skip_shuffle, v.remember_position), (true, false));
    // 空にすると消える
    lib.edit_videos(&[id], &VideoEdit { album: Some(String::new()), episode_id: Some(String::new()), ..Default::default() }).unwrap();
    let v = lib.videos().into_iter().find(|v| v.id == video.id).unwrap();
    assert_eq!((v.album.as_str(), v.sort_album.as_str(), v.episode_id.as_str()), ("", "", ""));

    // 選んだアートワークは印が付き、開き直しても残る
    let png = base.join("art.png");
    photo(&png, 300, 300, [10, 200, 30]);
    lib.set_artwork_from_file(&[id], &png).unwrap();
    lib.save().unwrap();
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let v = lib.videos().into_iter().find(|v| v.id == video.id).unwrap();
    assert!(v.custom_art && v.artwork_id != 0);
    assert!(root.join("iPod_Control/Artwork/zenpod-video-art.json").exists());
    // アートワークを消すと印も消える
    lib.remove_artwork(&[id]).unwrap();
    lib.save().unwrap();
    let lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let v = lib.videos().into_iter().find(|v| v.id == video.id).unwrap();
    assert!(!v.custom_art && v.artwork_id == 0);
    assert!(!root.join("iPod_Control/Artwork/zenpod-video-art.json").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn ipod_os_photos() {
    let base = tmp("ipodphoto");
    let root = base.join("ipod");
    fake_ipod(&root, "xB029");
    let pics: Vec<PathBuf> = (0..3)
        .map(|i| {
            let p = base.join(format!("p{i}.png"));
            photo(&p, 400 + i * 100, 300, [200, (i * 60) as u8, 30]);
            p
        })
        .collect();
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    assert!(lib.photo_library().supported);
    for p in &pics[..2] {
        assert!(matches!(lib.add_photo(p, Some("旅行"), None).unwrap(), PhotoOutcome::Added { .. }));
    }
    assert!(matches!(lib.add_photo(&pics[2], None, None).unwrap(), PhotoOutcome::Added { .. }));
    // 同じ写真は 2 回入れない
    assert!(matches!(lib.add_photo(&pics[0], None, None).unwrap(), PhotoOutcome::Duplicate));
    lib.save().unwrap();
    assert!(root.join("Photos/Photo Database").is_file());
    assert!(root.join("Photos/Thumbs/F1067_1.ithmb").is_file());

    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let pl = lib.photo_library();
    assert_eq!(pl.photos.len(), 3);
    assert_eq!(pl.albums.len(), 1);
    assert_eq!((pl.albums[0].name.as_str(), pl.albums[0].photo_ids.len()), ("旅行", 2));
    // 一番大きい形 (720×480) に収めた縦横 (余白を除く)
    let big = &pl.photos[0];
    assert_eq!((big.width, big.height), (640, 480));
    let id: u64 = pl.photos[2].id.parse().unwrap();
    let jpeg = lib.photo_jpeg(id, true).unwrap();
    let img = image::load_from_memory(&jpeg).unwrap().to_rgb8();
    assert_eq!((img.width(), img.height()), (720, 360));
    // 左半分の色が残っている (I420 の行き来で少しずれる)
    let px = img.get_pixel(50, 180).0;
    assert!((px[0] as i32 - 200).abs() < 24 && (px[1] as i32 - 120).abs() < 24, "{px:?}");

    // アルバムの操作
    let other = lib.create_photo_album("家族", None, &[id]).unwrap();
    lib.rename_photo_album(other.parse().unwrap(), "家族 2024").unwrap();
    lib.save().unwrap();
    let pl = lib.photo_library();
    assert_eq!(pl.albums.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["旅行", "家族 2024"]);
    // 消すと .ithmb を詰め直す
    let size = |root: &Path| -> u64 { std::fs::read_dir(root.join("Photos/Thumbs")).unwrap().flatten().map(|e| e.metadata().unwrap().len()).sum() };
    let before = size(&root);
    let ids: Vec<u64> = pl.photos.iter().take(2).map(|p| p.id.parse().unwrap()).collect();
    assert_eq!(lib.remove_photos(&ids).unwrap(), 2);
    lib.save().unwrap();
    assert!(size(&root) < before / 2, "{} → {}", before, size(&root));
    let lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let pl = lib.photo_library();
    assert_eq!(pl.photos.len(), 1);
    assert!(lib.photo_jpeg(pl.photos[0].id.parse().unwrap(), false).is_some());
    assert_eq!(pl.albums.iter().map(|a| a.photo_ids.len()).collect::<Vec<_>>(), [0, 1]);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn photos_not_on_old_models() {
    let base = tmp("oldmodel");
    let root = base.join("ipod");
    fake_ipod(&root, "x9282"); // 白黒液晶の 4G
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    assert!(!lib.photo_library().supported);
    let p = base.join("a.png");
    photo(&p, 10, 10, [1, 2, 3]);
    assert!(lib.add_photo(&p, None, None).is_err());
    let _ = std::fs::remove_dir_all(&base);
}

/// Rockbox の写真のアルバムは Pictures の下のフォルダの階層。消すと中身は 1 つ上に移る。iPod OS では 1 つの名前
#[test]
fn photo_album_hierarchy() {
    let base = tmp("hier");
    let p = base.join("a.png");
    photo(&p, 40, 30, [9, 9, 9]);
    let q = base.join("b.png");
    photo(&q, 40, 30, [200, 9, 9]);
    let root = base.join("rb");
    std::fs::create_dir_all(root.join(".rockbox")).unwrap();
    let mut lib = Library::open(&root, Mode::Rockbox, None).unwrap();
    let PhotoOutcome::Added { id: a } = lib.add_photo(&p, Some("旅行/2024"), None).unwrap() else { panic!() };
    lib.add_photo(&q, Some("旅行"), None).unwrap();
    assert!(root.join("Pictures/旅行/2024/a.jpg").is_file());
    let pl = lib.photo_library();
    let by = |n: &str| pl.albums.iter().find(|x| x.name == n).unwrap().clone();
    let (trip, y) = (by("旅行"), by("2024"));
    assert_eq!(trip.parent, None);
    assert_eq!(y.parent.as_deref(), Some(trip.id.as_str()));
    assert_eq!((trip.photo_ids.len(), y.photo_ids.as_slice()), (1, [a.clone()].as_slice()));
    // 子のアルバムを作る・名前を変える
    let sub = lib.create_photo_album("夏", Some(trip.id.parse().unwrap()), &[]).unwrap();
    lib.rename_photo_album(sub.parse().unwrap(), "夏休み").unwrap();
    assert!(root.join("Pictures/旅行/夏休み").is_dir());
    // 外すと 1 つ上へ
    lib.remove_from_photo_album(y.id.parse().unwrap(), &[a.parse().unwrap()]).unwrap();
    assert!(root.join("Pictures/旅行/a.jpg").is_file());
    // 親を消すと、写真と子のアルバムは Pictures の直下へ
    let trip_id: u64 = lib.photo_library().albums.iter().find(|x| x.name == "旅行").unwrap().id.parse().unwrap();
    lib.delete_photo_album(trip_id).unwrap();
    assert!(root.join("Pictures/a.jpg").is_file() && root.join("Pictures/b.jpg").is_file());
    assert!(root.join("Pictures/2024").is_dir() && root.join("Pictures/夏休み").is_dir());
    assert!(!root.join("Pictures/旅行").exists());
    // 動かしても同じ写真は 2 回入れない (覚えている場所を直している)
    assert!(matches!(lib.add_photo(&p, None, None).unwrap(), PhotoOutcome::Duplicate));

    // iPod OS は平らなアルバムで、階層は 1 つの名前
    let ipod = base.join("ipod");
    fake_ipod(&ipod, "xB029");
    let mut lib = Library::open(&ipod, Mode::Ipod, None).unwrap();
    lib.add_photo(&p, Some("旅行/2024"), None).unwrap();
    let pl = lib.photo_library();
    assert_eq!((pl.albums[0].name.as_str(), pl.albums[0].parent.as_deref()), ("旅行 / 2024", None));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn rockbox_videos_and_photos() {
    let base = tmp("rockbox");
    let root = base.join("ipod");
    std::fs::create_dir_all(root.join(".rockbox")).unwrap();
    let mut lib = Library::open(&root, Mode::Rockbox, None).unwrap();

    let p = base.join("IMG_0001.png");
    photo(&p, 2000, 1000, [10, 200, 30]);
    let PhotoOutcome::Added { id } = lib.add_photo(&p, Some("旅行"), None).unwrap() else { panic!() };
    assert!(matches!(lib.add_photo(&p, None, None).unwrap(), PhotoOutcome::Duplicate));
    let file = root.join("Pictures/旅行/IMG_0001.jpg");
    let img = image::open(&file).unwrap();
    assert_eq!((img.width(), img.height()), (640, 320));
    let pl = lib.photo_library();
    assert_eq!(pl.photos.len(), 1);
    assert_eq!((pl.albums[0].name.as_str(), pl.albums[0].photo_ids.as_slice()), ("旅行", [id.clone()].as_slice()));
    // アルバムから外すと Pictures の直下へ、アルバムを消しても写真は残る
    let album: u64 = pl.albums[0].id.parse().unwrap();
    lib.remove_from_photo_album(album, &[id.parse().unwrap()]).unwrap();
    assert!(root.join("Pictures/IMG_0001.jpg").is_file());
    lib.delete_photo_album(album).unwrap();
    assert!(!root.join("Pictures/旅行").exists());
    let pl = lib.photo_library();
    assert_eq!((pl.photos.len(), pl.albums.len()), (1, 0));
    // 消した写真は、また送れる
    lib.remove_photos(&[pl.photos[0].id.parse().unwrap()]).unwrap();
    assert!(matches!(lib.add_photo(&p, None, None).unwrap(), PhotoOutcome::Added { .. }));
    // 写真は曲として数えない
    assert!(Library::open(&root, Mode::Rockbox, None).unwrap().tracks().is_empty());

    let Some(ff) = which_ffmpeg() else { return };
    let v = base.join("clip.mp4");
    make_video(&ff, &v, "640x480", &["-c:v", "libx264", "-c:a", "aac", "-metadata", "title=クリップ", "-metadata", "show=番組", "-metadata", "season_number=2", "-metadata", "episode_sort=3"]);
    let VideoOutcome::Added { video, converted } = lib.add_video(&v, Some(&ff), &VideoOptions::default(), &NO_PROGRESS).unwrap() else { panic!() };
    assert!(converted);
    assert_eq!(video.rb_path, "/Videos/番組/S02E03 クリップ.mpg");
    assert_eq!((video.kind, video.show.as_str()), (VideoKind::TvShow, "番組"));
    // 開いているところに落とすと、番組のタグがあってもそこに置く (ムービー = Videos の直下、番組 = その番組のフォルダ)
    let movie = VideoOptions { placement: VideoPlacement { kind: Some(VideoKind::Movie), ..Default::default() }, skip_duplicates: false, ..VideoOptions::default() };
    let VideoOutcome::Added { video: m, .. } = lib.add_video(&v, Some(&ff), &movie, &NO_PROGRESS).unwrap() else { panic!() };
    assert_eq!((m.rb_path.as_str(), m.kind), ("/Videos/クリップ.mpg", VideoKind::Movie));
    let other = VideoOptions { placement: VideoPlacement { kind: Some(VideoKind::TvShow), show: Some("別の番組".into()), artist: None }, skip_duplicates: false, ..VideoOptions::default() };
    let VideoOutcome::Added { video: o, .. } = lib.add_video(&v, Some(&ff), &other, &NO_PROGRESS).unwrap() else { panic!() };
    assert_eq!((o.rb_path.as_str(), o.show.as_str()), ("/Videos/別の番組/S02E03 クリップ.mpg", "別の番組"));
    lib.remove_videos(&[m.id.parse().unwrap(), o.id.parse().unwrap()]).unwrap();
    assert!((1500..2600).contains(&video.duration_ms), "{}", video.duration_ms);
    assert!(matches!(lib.add_video(&v, Some(&ff), &VideoOptions::default(), &NO_PROGRESS).unwrap(), VideoOutcome::Duplicate { .. }));
    let id: u64 = video.id.parse().unwrap();
    lib.edit_videos(&[id], &VideoEdit { show: Some(String::new()), ..Default::default() }).unwrap();
    let vids = lib.videos();
    assert_eq!(vids[0].rb_path, "/Videos/S02E03 クリップ.mpg");
    assert!(!root.join("Videos/番組").exists());
    lib.remove_videos(&[vids[0].id.parse().unwrap()]).unwrap();
    assert!(lib.videos().is_empty());
    assert!(Library::open(&root, Mode::Rockbox, None).unwrap().tracks().is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

/// libgpod で読めるかを確かめるための iPod を作る (classic と video 5G):
/// `ZENPOD_GPOD_OUT=<フォルダ> cargo test -p ipod-core --test videos_photos -- --ignored`
#[test]
#[ignore]
fn write_for_libgpod() {
    let Some(out) = std::env::var_os("ZENPOD_GPOD_OUT").map(PathBuf::from) else { return };
    let Some(ff) = which_ffmpeg() else { return };
    let base = tmp("gpod");
    let ready = base.join("ready.m4v");
    make_video(&ff, &ready, "320x240", &["-c:v", "libx264", "-profile:v", "baseline", "-level", "3.0", "-c:a", "aac", "-metadata", "title=Movie"]);
    let tv = base.join("tv.mkv");
    make_video(&ff, &tv, "1280x720", &["-c:v", "libx264", "-c:a", "aac", "-metadata", "title=Episode", "-metadata", "show=Show", "-metadata", "season_number=2", "-metadata", "episode_sort=5"]);
    let pics: Vec<PathBuf> = [(800, 600, [200, 30, 30]), (600, 900, [30, 200, 30]), (1000, 400, [30, 30, 200])]
        .iter()
        .enumerate()
        .map(|(i, &(w, h, c))| {
            let p = base.join(format!("p{i}.png"));
            photo(&p, w, h, c);
            p
        })
        .collect();
    for model in ["xB029", "xA002"] {
        let root = out.join(model);
        let _ = std::fs::remove_dir_all(&root);
        fake_ipod(&root, model);
        let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
        lib.add_video(&ready, Some(&ff), &VideoOptions::default(), &NO_PROGRESS).unwrap();
        lib.add_video(&tv, Some(&ff), &VideoOptions::default(), &NO_PROGRESS).unwrap();
        lib.add_photo(&pics[0], Some("Trip"), None).unwrap();
        lib.add_photo(&pics[1], Some("Trip"), None).unwrap();
        lib.add_photo(&pics[2], None, None).unwrap();
        lib.save().unwrap();
    }
    let _ = std::fs::remove_dir_all(&base);
}
