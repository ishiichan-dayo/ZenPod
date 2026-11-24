//! ユースケースを Tauri もネットも使わずに動かす (ポートは偽物、iPod は一時フォルダに作った空の iPod)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ipod_core::online::artsearch::{ImageProbe, SearchResult};
use ipod_core::online::tagsearch::{Naming, Release, ReleaseCandidate};
use zenpod_app::application::events::AppEvent;
use zenpod_app::application::ports::*;
use zenpod_app::application::{Deps, ZenPod};
use zenpod_app::{AppError, AppResult};

/// テストで使う偽物のポートと、組み立てた `ZenPod`
struct World {
    zen: ZenPod,
    events: Arc<Recorder>,
    catalog: Arc<FakeCatalog>,
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Recorder::default());
        let catalog = Arc::new(FakeCatalog::default());
        let dirs = Arc::new(Dirs(dir.path().join("app")));
        let zen = ZenPod::new(Deps {
            events: events.clone(),
            dirs,
            prefs: Arc::new(MemoryPrefs::default()),
            thumbs: Arc::new(NoThumbs),
            aliases: Arc::new(MemoryAliases::default()),
            transcoder: Arc::new(NoTranscoder),
            online: catalog.clone(),
            ffmpeg_installer: Arc::new(NoInstaller),
            podcast_source: Arc::new(NoPodcasts),
        });
        World { zen, events, catalog, dir }
    }

    /// iTunesDB がまだ無い iPod (iPod OS モードで開くと空の DB を作る)
    fn empty_ipod(&self) -> PathBuf {
        let root = self.dir.path().join("ipod");
        std::fs::create_dir_all(root.join("iPod_Control/iTunes")).unwrap();
        root
    }
}

#[test]
fn open_edit_playlists_and_eject_a_folder_ipod() {
    let w = World::new();
    let root = w.empty_ipod();
    let snap = w.zen.devices.open(&root, None).unwrap();
    assert_eq!(snap.tracks.as_ref().map(Vec::len), Some(0));
    assert!(w.zen.devices.present());
    // 開いたときだけ、開くのにかかった時間の内訳が付く (利用状況に送る)
    assert!(snap.times.is_some());
    assert!(w.zen.session.snapshot().unwrap().times.is_none());

    let snap = w.zen.playlists.create("ドライブ", &[]).unwrap();
    // プレイリストだけを変えた操作は曲の一覧を返さない
    assert!(snap.tracks.is_none());
    let pl = snap.playlists.iter().find(|p| p.name == "ドライブ").expect("作ったプレイリスト");
    let snap = w.zen.playlists.rename(&pl.id, "通勤").unwrap();
    assert!(snap.playlists.iter().any(|p| p.name == "通勤"));
    assert!(w.zen.playlists.rename("x", "y").is_err());

    // フォルダで開いた iPod は OS に取り出してもらわず、閉じるだけ
    assert!(!w.zen.devices.eject().unwrap());
    assert!(matches!(w.zen.playlists.create("a", &[]), Err(AppError::NotConnected)));
    assert!(!w.zen.devices.present());

    // 開き直すと保存したプレイリストが残っている
    let snap = w.zen.devices.open(&root, None).unwrap();
    assert!(snap.playlists.iter().any(|p| p.name == "通勤"));
}

/// Rockbox の iPod を初めて開くと、データベースから埋めて先に一覧を出し、裏でファイルから読み直して反映する
#[test]
fn rockbox_first_open_fills_from_database_and_rereads() {
    let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
        return;
    };
    let w = World::new();
    let root = w.dir.path().join("rb");
    std::fs::create_dir_all(root.join(".rockbox")).unwrap();
    std::fs::write(root.join(".rockbox/rockbox-info.txt"), "Target: ipod6g\nVersion: v4.0\n").unwrap();
    let src = w.dir.path().join("src.flac");
    let ok = std::process::Command::new(&ff)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"])
        .args(["-metadata", "title=T", "-metadata", "artist=A", "-metadata", "ARTISTSORT=Sort A", "-metadata", "album=X"])
        .arg(&src)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let album = root.join("Music/A/X");
    std::fs::create_dir_all(&album).unwrap();
    for i in 0..60 {
        std::fs::copy(&src, album.join(format!("{i:02}.flac"))).unwrap();
    }
    ipod_core::rockbox::rbdb::rebuild(&root, None).unwrap();

    let snap = w.zen.devices.open(&root, None).unwrap();
    let scan = snap.scan.unwrap();
    assert_eq!((scan.from_db, scan.reread), (60, 0));
    // 読み直し終わるまで待つ
    let started = std::time::Instant::now();
    while !w.events.names().contains(&"rockbox-reread-done") {
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "読み直しが終わらない");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let snap = w.zen.session.snapshot().unwrap();
    let tracks = snap.tracks.unwrap();
    assert_eq!(tracks.len(), 60);
    assert!(tracks.iter().all(|t| t.sort_artist == "Sort A" && t.artist == "A"));
    // 開き直すとキャッシュから (データベースもタグも読まない)。取り出し (閉じるだけ) は止まらない
    assert!(!w.zen.devices.eject().unwrap());
    let scan = w.zen.devices.open(&root, None).unwrap().scan.unwrap();
    assert_eq!((scan.from_db, scan.reread), (0, 0));
}

#[test]
fn transfer_rejects_empty_drop_and_frees_busy() {
    let w = World::new();
    w.zen.devices.open(&w.empty_ipod(), None).unwrap();
    let txt = w.dir.path().join("notes.txt");
    std::fs::write(&txt, b"").unwrap();
    let opts = serde_json::from_str(r#"{"skipDuplicates":true,"autoReading":false,"artwork":false}"#).unwrap();
    assert!(w.zen.transfer.start(&[txt.to_string_lossy().into_owned()], None, &opts).is_err());
    // 失敗しても次の操作を受け付ける
    assert!(!w.zen.session.is_busy());
    assert!(w.zen.playlists.create("a", &[]).is_ok());
}

/// 写真のフォルダと動画をドロップすると、写真はフォルダの名前のアルバムに、動画は「ビデオ」に入る。書き出し・削除もできる
/// 「転送してプレイリストに追加」: 重複として飛ばした曲も、iPod にある方の曲をプレイリストに入れる (配布リポジトリの Issue #21)。
/// 別のディスクの同じ曲名の曲は重複にしない (Rockbox も番号を比べる)。ffmpeg がある環境だけで確かめる
#[test]
fn transfer_to_playlist_adds_duplicates() {
    let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
        return;
    };
    for mode in [ipod_core::library::Mode::Ipod, ipod_core::library::Mode::Rockbox] {
        let w = World::new();
        let root = w.empty_ipod();
        if mode == ipod_core::library::Mode::Rockbox {
            std::fs::create_dir_all(root.join(".rockbox")).unwrap();
            std::fs::write(root.join(".rockbox/rockbox-info.txt"), "Target: ipod6g\nVersion: v4.0\n").unwrap();
        }
        w.zen.devices.open(&root, Some(mode)).unwrap();
        let src = w.dir.path().join("pc");
        let copy = src.join("コピー");
        std::fs::create_dir_all(&copy).unwrap();
        let mk = |p: PathBuf, title: &str, track: &str, disc: &str| {
            let ok = std::process::Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "flac"])
                .args(["-metadata", &format!("title={title}"), "-metadata", "artist=アーティスト", "-metadata", "album=アルバム"])
                .args(["-metadata", &format!("track={track}"), "-metadata", &format!("disc={disc}")])
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            p.to_string_lossy().into_owned()
        };
        let a = mk(src.join("a.flac"), "曲 A", "1", "1");
        // 同じ曲が PC の 2 か所にある
        let a2 = mk(copy.join("a.flac"), "曲 A", "1", "1");
        // デラックス版の 2 枚目の同じ曲名の曲 (別の曲)
        let a_disc2 = mk(src.join("a-disc2.flac"), "曲 A", "1", "2");
        let b = mk(src.join("b.flac"), "曲 B", "2", "1");
        let paths = [a, a2, a_disc2, b];
        let opts = serde_json::from_str(r#"{"skipDuplicates":true,"autoReading":false,"artwork":false}"#).unwrap();

        let transfer = |name: &str| {
            let snap = w.zen.playlists.create(name, &[]).unwrap();
            let pl = snap.playlists.iter().find(|p| p.name == name).unwrap().id.clone();
            *w.events.1.lock().unwrap() = None;
            w.zen.transfer.start(&paths, Some(&pl), &opts).unwrap();
            let started = std::time::Instant::now();
            while w.events.last_transfer().is_none() {
                assert!(started.elapsed() < std::time::Duration::from_secs(60), "転送が終わらない");
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let done = w.events.last_transfer().unwrap();
            let snap = w.zen.session.snapshot().unwrap();
            let items = snap.playlists.iter().find(|p| p.id == pl).unwrap().track_ids.len();
            (done, items, snap.tracks.map(|t| t.len()).unwrap_or(0))
        };
        // 1 回目: コピーの方だけ重複。プレイリストには iPod にある 3 曲が全部入る
        let (done, items, tracks) = transfer("ロック");
        assert_eq!((done["added"].as_u64(), done["duplicates"].as_u64(), items, tracks), (Some(3), Some(1), 3, 3), "{mode:?} {done}");
        // 2 回目: 全部 iPod にあるので重複。それでもプレイリストには全部入る
        let (done, items, tracks) = transfer("もう一度");
        assert_eq!((done["added"].as_u64(), done["duplicates"].as_u64(), items, tracks), (Some(0), Some(4), 3, 3), "{mode:?} {done}");
    }
}

#[test]
fn transfer_photos_and_videos() {
    let w = World::new();
    let root = w.empty_ipod();
    std::fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xB029\nFirewireGuid: 0x000A27001A2B3C4D\n").unwrap();
    w.zen.devices.open(&root, None).unwrap();
    let trip = w.dir.path().join("旅行");
    std::fs::create_dir_all(&trip).unwrap();
    for i in 0..2u8 {
        image::RgbImage::from_pixel(300, 200, image::Rgb([i * 100, 50, 50])).save(trip.join(format!("IMG_{i}.png"))).unwrap();
    }
    let mut paths = vec![trip.to_string_lossy().into_owned()];
    let ff = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file()));
    if let Some(ff) = &ff {
        let v = w.dir.path().join("clip.mkv");
        let ok = std::process::Command::new(ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=640x360:rate=25:duration=1", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-metadata", "title=クリップ"])
            .arg(&v)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        paths.push(v.to_string_lossy().into_owned());
    }
    let opts = serde_json::from_str(r#"{"skipDuplicates":true,"autoReading":false,"artwork":true,"videoSize":"320"}"#).unwrap();
    w.zen.transfer.start(&paths, None, &opts).unwrap();
    let started = std::time::Instant::now();
    while w.events.last_transfer().is_none() {
        assert!(started.elapsed() < std::time::Duration::from_secs(60), "転送が終わらない");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let done = w.events.last_transfer().unwrap();
    assert_eq!(done["photos"], 2, "{done}");
    assert_eq!(done["videos"], if ff.is_some() { 1 } else { 0 }, "{done}");
    assert_eq!(done["errors"].as_array().unwrap().len(), 0, "{done}");

    let pl = w.zen.visual.photo_library().unwrap();
    assert_eq!(pl.photos.len(), 2);
    assert_eq!((pl.albums[0].name.as_str(), pl.albums[0].photo_ids.len()), ("旅行", 2));
    assert!(w.zen.visual.photo_thumb(&pl.photos[0].id, false).unwrap().starts_with("data:image/jpeg;base64,"));
    let out = w.dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let ids: Vec<String> = pl.photos.iter().map(|p| p.id.clone()).collect();
    assert_eq!(w.zen.visual.export_photos(&ids, &out.to_string_lossy()).unwrap(), 2);
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2);
    let r = w.zen.visual.remove_photos(&ids[..1]).unwrap();
    assert_eq!(r.library.photos.len(), 1);
    if ff.is_some() {
        let vids = w.zen.visual.videos().unwrap();
        assert_eq!(vids[0].title, "クリップ");
        // 一覧の画像は動画から取り出した 1 コマで、元の縦横比 (16:9) のまま (iPod に置いた正方形の画像ではない)
        let url = w.zen.visual.video_thumb(&vids[0].id).unwrap();
        use base64::Engine as _;
        let jpeg = base64::engine::general_purpose::STANDARD.decode(url.split_once(',').unwrap().1).unwrap();
        let img = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(img.width() * 9, img.height() * 16, "{}x{}", img.width(), img.height());
        // 動画は曲の一覧に出ない
        assert_eq!(w.zen.session.snapshot().unwrap().tracks.unwrap().len(), 0);
        // 再生: iPod 用の H.264 はファイルのまま。変換して流すと WebM が最後まで届く
        let id = vids[0].id.clone();
        assert!(w.zen.video_player.plan(&id, &["webm".into()], false).unwrap().native);
        let plan = w.zen.video_player.plan(&id, &["webm".into()], true).unwrap();
        assert_eq!((plan.native, plan.format), (false, Some("webm")));
        let stream = w.zen.video_player.start(&id, "webm", 0).unwrap();
        let mut got = Vec::new();
        loop {
            let c = w.zen.video_player.read(stream).unwrap();
            got.extend(c.data);
            if c.ended {
                break;
            }
        }
        assert!(got.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) && got.len() > 1000, "{}", got.len());
        // 新しく始めたら古い番号は終わり
        let old = w.zen.video_player.start(&id, "webm", 500).unwrap();
        let _ = w.zen.video_player.start(&id, "webm", 0).unwrap();
        assert!(w.zen.video_player.read(old).unwrap().ended);
        w.zen.video_player.stop();
        let r = w.zen.visual.remove_videos(&[vids[0].id.clone()]).unwrap();
        assert!(r.videos.is_empty());
    }
}

/// 貼り付けた画像は一時ファイルになり (知らない形式は断る)、それを写真として送れる
#[test]
fn pasted_image_becomes_a_file() {
    use base64::Engine as _;
    let w = World::new();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(png.get_ref());
    let path = w.zen.visual.save_pasted_image(&b64, "png").unwrap();
    assert!(path.ends_with(".png"));
    assert_eq!(std::fs::read(&path).unwrap(), *png.get_ref());
    assert!(w.zen.visual.save_pasted_image(&b64, "exe").is_err());
    let _ = std::fs::remove_file(path);
}

#[test]
fn release_is_fetched_once_per_id_and_naming() {
    let w = World::new();
    let preview = |naming| w.zen.tags.preview_release_pc(&[], "rel-1", naming, None).unwrap();
    preview(None);
    preview(None);
    assert_eq!(w.catalog.fetches.load(Ordering::SeqCst), 1);
    preview(Some(Naming::Latin));
    assert_eq!(w.catalog.fetches.load(Ordering::SeqCst), 2);
}

#[test]
fn fetched_image_becomes_data_url_with_its_type() {
    let w = World::new();
    *w.catalog.image.lock().unwrap() = b"\x89PNG....".to_vec();
    assert!(w.zen.artwork.fetch_image("https://example.com/a.png").unwrap().starts_with("data:image/png;base64,"));
    *w.catalog.image.lock().unwrap() = Vec::new();
    w.catalog.fail.store(true, Ordering::SeqCst);
    assert!(w.zen.artwork.fetch_image("https://example.com/a.png").is_none());
}

#[test]
fn alias_lookup_needs_an_open_ipod() {
    let w = World::new();
    assert!(matches!(w.zen.aliases.lookup(), Err(AppError::NotConnected)));
    w.zen.devices.open(&w.empty_ipod(), None).unwrap();
    // 曲が無ければ調べるものも無い
    assert_eq!(w.zen.aliases.lookup().unwrap(), 0);
    assert!(w.events.names().iter().all(|n| *n != "alias-progress"));
}

/// 画面から来た番組の ID で、ポッドキャストのフォルダの外や登録していない番組のフォルダを消さない・読まない
#[test]
fn podcast_ids_from_the_screen_stay_inside() {
    let w = World::new();
    let data = w.dir.path().join("app/data");
    let pods = data.join("podcasts");
    let victim = data.join("victim");
    let unknown = "0123456789abcdef";
    std::fs::create_dir_all(pods.join(unknown)).unwrap();
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("cover"), b"x").unwrap();
    let abs = victim.to_string_lossy().into_owned();
    for bad in ["", ".", "..", "../victim", "/abs", "C:\\x", abs.as_str(), unknown] {
        w.zen.podcasts.unsubscribe(bad).unwrap();
        assert!(w.zen.podcasts.cover(bad).is_none(), "{bad}");
        let items: Vec<zenpod_app::application::podcasts::EpRef> = serde_json::from_value(serde_json::json!([{ "feedId": bad, "epId": "x" }])).unwrap();
        w.zen.podcasts.delete_files(&items).unwrap();
        assert!(w.zen.podcasts.image(bad, "").is_none(), "{bad}");
    }
    assert!(victim.join("cover").is_file());
    assert!(pods.join(unknown).is_dir());
}

/// 曲の操作のあとは写真のフォルダを数え直さず、写真を変えうる操作 (プレイリスト・写真・転送) や
/// 画面からの読み直しのあとは数え直す
#[test]
fn media_folder_sizes_are_reused_after_track_edits() {
    let w = World::new();
    let root = w.empty_ipod();
    let photos = root.join("Photos").join("Full Resolution");
    std::fs::create_dir_all(&photos).unwrap();
    let add = |name: &str, len: usize| std::fs::write(photos.join(name), vec![0u8; len]).unwrap();
    let photo_bytes = |s: zenpod_app::application::snapshot::Snapshot| s.usage.unwrap().photos;
    add("a.jpg", 100);
    assert_eq!(photo_bytes(w.zen.devices.open(&root, None).unwrap()), 100);

    // 曲の操作 (edit): 1 回目に数えて覚え、2 回目は覚えた値
    assert_eq!(photo_bytes(w.zen.devices.rename("A").unwrap()), 100);
    add("b.jpg", 50);
    assert_eq!(photo_bytes(w.zen.devices.rename("B").unwrap()), 100);

    // プレイリスト・写真の操作 (edit_playlists) のあとは数え直す
    w.zen.playlists.create("x", &[]).unwrap();
    assert_eq!(photo_bytes(w.zen.devices.rename("C").unwrap()), 150);

    // 画面からの読み直し (転送のあとなど) は数え直して覚える
    add("c.jpg", 25);
    assert_eq!(photo_bytes(w.zen.session.snapshot().unwrap()), 175);
    assert_eq!(photo_bytes(w.zen.devices.rename("D").unwrap()), 175);

    // 転送などの busy の処理のあとも数え直す
    add("d.jpg", 5);
    drop(w.zen.session.try_begin_exclusive().unwrap());
    assert_eq!(photo_bytes(w.zen.devices.rename("E").unwrap()), 180);
}

#[test]
fn screen_opens_only_connected_ipods_and_picked_folders() {
    let w = World::new();
    let root = w.empty_ipod();
    // 画面から渡された場所だけでは開かない (つながっている iPod でもダイアログで選んだフォルダでもない)
    assert!(w.zen.devices.open_requested(&root, None).is_err());
    assert!(!w.zen.devices.present());
    w.zen.devices.allow_folder(root.clone());
    w.zen.devices.open_requested(&root, None).unwrap();
    assert!(w.zen.devices.present());
}

#[test]
fn ffmpeg_is_chosen_only_after_checking() {
    let w = World::new();
    let bin = w.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let other = bin.join("calc.exe");
    std::fs::write(&other, b"").unwrap();
    assert!(w.zen.ffmpeg.choose(&other).is_err());
    assert!(w.zen.ffmpeg.choose(&bin.join("ffmpeg.exe")).is_err(), "無いファイル");
    assert_eq!(w.zen.prefs.ffmpeg_path(), None);
    let ff = bin.join("ffmpeg.exe");
    std::fs::write(&ff, b"").unwrap();
    let s = w.zen.ffmpeg.choose(&ff).unwrap();
    assert_eq!(s.ffmpeg_chosen.as_deref(), Some(&*ff.to_string_lossy()));
    assert_eq!(w.zen.ffmpeg.path(), Some(ff.clone()));
    // 画面の set_pref では変えられない
    assert!(w.zen.prefs.set("ffmpegPath".into(), Some(other.to_string_lossy().into_owned())).is_err());
    assert_eq!(w.zen.ffmpeg.path(), Some(ff.clone()));
    // 選んだファイルが無くなれば、自動で探すのに戻る
    std::fs::remove_file(&ff).unwrap();
    assert_ne!(w.zen.ffmpeg.path(), Some(ff));
    assert!(w.zen.ffmpeg.forget_choice().unwrap().ffmpeg_chosen.is_none());
}

#[test]
fn pc_folder_scan_and_edits_stay_inside_registered_folders() {
    let w = World::new();
    let music = w.dir.path().join("music");
    let other = w.dir.path().join("other");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("a.mp3"), b"").unwrap();
    let music_s = w.zen.prefs.add_pc_folder(&music).unwrap();
    assert_eq!(w.zen.pc.scan(&music_s).unwrap().len(), 0);
    assert!(w.zen.pc.scan(&other.to_string_lossy()).is_err());
    assert!(w.zen.pc.rename(&other.join("a.mp3").to_string_lossy(), "b").is_err());
    // 画面から登録フォルダを書き換えて広げることはできない
    let all = serde_json::to_string(&[w.dir.path().to_string_lossy()]).unwrap();
    assert!(w.zen.prefs.set("pcFolders".into(), Some(all)).is_err());
    assert!(w.zen.pc.rename(&other.join("a.mp3").to_string_lossy(), "b").is_err());
    w.zen.prefs.remove_pc_folder(&music_s).unwrap();
    assert!(w.zen.pc.scan(&music_s).is_err());
}

// ---------------- 偽物のポート ----------------

#[derive(Default)]
struct Recorder(Mutex<Vec<&'static str>>, Mutex<Option<serde_json::Value>>);
impl Recorder {
    fn names(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
    /// 最後の転送の結果 (transfer-done の中身)
    fn last_transfer(&self) -> Option<serde_json::Value> {
        self.1.lock().unwrap().clone()
    }
}
impl EventSink for Recorder {
    fn emit(&self, event: AppEvent) {
        if let AppEvent::TransferDone(s) = &event {
            *self.1.lock().unwrap() = serde_json::to_value(s).ok();
        }
        self.0.lock().unwrap().push(event.name());
    }
}

struct Dirs(PathBuf);
impl AppDirs for Dirs {
    fn config_dir(&self) -> Option<PathBuf> {
        Some(self.0.join("config"))
    }
    fn cache_dir(&self) -> Option<PathBuf> {
        Some(self.0.join("cache"))
    }
    fn local_data_dir(&self) -> Option<PathBuf> {
        Some(self.0.join("data"))
    }
    fn resource_dir(&self) -> Option<PathBuf> {
        None
    }
}

#[derive(Default)]
struct MemoryPrefs(Mutex<HashMap<String, String>>);
impl PrefsStore for MemoryPrefs {
    fn load(&self) -> HashMap<String, String> {
        self.0.lock().unwrap().clone()
    }
    fn save(&self, prefs: &HashMap<String, String>) -> AppResult<()> {
        *self.0.lock().unwrap() = prefs.clone();
        Ok(())
    }
}

struct NoThumbs;
impl ThumbStore for NoThumbs {
    fn load(&self, _: &str, _: &[u32]) -> Option<Vec<Vec<u8>>> {
        None
    }
    fn save(&self, _: &str, _: &[u32], _: &[Vec<u8>]) {}
    fn load_refs(&self) -> HashMap<String, ArtRef> {
        HashMap::new()
    }
    fn save_refs(&self, _: &HashMap<String, ArtRef>) {}
}

#[derive(Default)]
struct MemoryAliases(Mutex<HashMap<String, Vec<String>>>);
impl AliasStore for MemoryAliases {
    fn load(&self) -> HashMap<String, Vec<String>> {
        self.0.lock().unwrap().clone()
    }
    fn save(&self, aliases: &HashMap<String, Vec<String>>) {
        *self.0.lock().unwrap() = aliases.clone();
    }
}

struct NoTranscoder;
impl Transcoder for NoTranscoder {
    fn to_flac(&self, _: &Path, _: &Path) -> Option<Vec<u8>> {
        None
    }
    /// 名前が ffmpeg のファイルだけ ffmpeg とみなす
    fn is_ffmpeg(&self, path: &Path) -> bool {
        path.file_stem().is_some_and(|s| s == "ffmpeg")
    }
}

struct NoInstaller;
impl FfmpegInstaller for NoInstaller {
    fn install(&self, _: &Path, _: &AtomicBool, _: &mut dyn FnMut(u64, u64)) -> ipod_core::Result<PathBuf> {
        Err(ipod_core::Error::Other("offline".into()))
    }
}

#[derive(Default)]
struct FakeCatalog {
    fetches: AtomicUsize,
    image: Mutex<Vec<u8>>,
    fail: AtomicBool,
}
impl OnlineCatalog for FakeCatalog {
    fn search_artwork(&self, _: &str, _: &str, _: ipod_core::online::artsearch::Progress) -> SearchResult {
        SearchResult::default()
    }
    fn probe_image(&self, _: &str) -> ipod_core::Result<ImageProbe> {
        Err(ipod_core::Error::Other("offline".into()))
    }
    fn download_image(&self, _: &str) -> ipod_core::Result<Vec<u8>> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ipod_core::Error::Other("offline".into()));
        }
        Ok(self.image.lock().unwrap().clone())
    }
    fn artist_aliases(&self, _: &str) -> ipod_core::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn search_releases(&self, _: &str, _: &str) -> ipod_core::Result<Vec<ReleaseCandidate>> {
        Ok(Vec::new())
    }
    fn fetch_release(&self, _: &str, _: Naming) -> ipod_core::Result<Release> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Ok(Release { title: "A".into(), artist: "B".into(), year: 2024, disc_count: 1, tracks: Vec::new(), latin: None })
    }
}

struct NoPodcasts;
impl PodcastSource for NoPodcasts {
    fn fetch_feed(&self, _: &str, _: Option<&str>, _: Option<&str>) -> ipod_core::Result<ipod_core::online::podcast::Fetched> {
        Err(ipod_core::Error::Other("offline".into()))
    }
    fn fetch_image(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn download_episode(&self, _: &ipod_core::online::podcast::Episode, _: &Path, _: &AtomicBool, _: &mut dyn FnMut(u64, u64)) -> ipod_core::Result<String> {
        Err(ipod_core::Error::Other("offline".into()))
    }
}
