//! iTunes / ミュージックの XML からの取り込みと、スマートプレイリストの書き出しを、偽の iPod で通して確かめる。
//! ffmpeg がある環境だけで確かめる (無音の mp3 を作って転送する)

use std::path::{Path, PathBuf};

use ipod_core::library::{Library, Mode};
use ipod_core::pc::itunesxml::XmlLibrary;
use ipod_core::domain::track::AddOptions;

fn which_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|f| f.is_file())
}

fn song(ffmpeg: &Path, out: &Path, title: &str, artist: &str) -> bool {
    std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", "2"])
        .args(["-metadata", &format!("title={title}"), "-metadata", &format!("artist={artist}"), "-metadata", "album=Album"])
        .arg(out)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn import_and_export() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-xml-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("ipod");
    std::fs::create_dir_all(root.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    // iPod photo (署名の要らない機種)
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA079\n").unwrap();
    let a = base.join("a.mp3");
    let b = base.join("b.mp3");
    if !song(&ffmpeg, &a, "First", "KANA-BOON") || !song(&ffmpeg, &b, "Second", "Other") {
        return;
    }
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let opts = AddOptions { auto_reading: false, artwork: false, ..Default::default() };
    lib.add_file(&a, None, &opts).unwrap();
    lib.add_file(&b, None, &opts).unwrap();
    lib.save().unwrap();

    // 普通のプレイリスト (1 曲は iPod に無い) と、アーティストで選ぶスマートプレイリスト
    let fixture = include_bytes!("fixtures/itunes12-smart-playlists.plist");
    let fv: plist::Value = plist::from_bytes(fixture).unwrap();
    let kb = fv.as_dictionary().unwrap()["Playlists"].as_array().unwrap()[0].as_dictionary().unwrap().clone();
    // KANA-BOONアルバム の「購入した項目」を外したもの: 対象メディア「ミュージック」・アーティストに KANA-BOON を含む
    let mut rules = ipod_core::smartpl::Rules::parse(kb["Smart Criteria"].as_data().unwrap()).unwrap();
    if let ipod_core::smartpl::Rule::Group(g) = &mut rules.items[1] {
        g.items.remove(0);
    }
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>Tracks</key><dict>
    <key>1</key><dict><key>Track ID</key><integer>1</integer><key>Name</key><string>Second</string><key>Artist</key><string>Other</string><key>Album</key><string>Album</string><key>Total Time</key><integer>2000</integer></dict>
    <key>2</key><dict><key>Track ID</key><integer>2</integer><key>Name</key><string>Not Here</string><key>Artist</key><string>Nobody</string></dict>
  </dict>
  <key>Playlists</key><array>
    <dict><key>Name</key><string>手作り</string><key>Playlist Persistent ID</key><string>0000000000ABCDEF</string>
      <key>Playlist Items</key><array><dict><key>Track ID</key><integer>2</integer></dict><dict><key>Track ID</key><integer>1</integer></dict></array></dict>
    <dict><key>Name</key><string>KB</string><key>Playlist Persistent ID</key><string>E16748E03448B57C</string>
      <key>Smart Info</key><data>{}</data><key>Smart Criteria</key><data>{}</data></dict>
  </array>
</dict></plist>"#,
        b64(kb["Smart Info"].as_data().unwrap()),
        b64(&rules.to_bytes()),
    );
    let x = XmlLibrary::parse(xml.as_bytes()).unwrap();
    let c = lib.import_candidates(&x);
    assert_eq!((c[0].matched, c[0].missing_count, c[0].as_smart), (1, 1, false));
    assert!(c[1].as_smart && !c[1].exists);
    let sum = lib.import_itunes(&x, &[0, 1]).unwrap();
    assert_eq!((sum.playlists, sum.tracks, sum.missing), (2, 1, 1));
    lib.save().unwrap();

    // 開き直しても残っている。スマートプレイリストは iTunes の Persistent ID のまま
    let lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let tracks = lib.tracks();
    let title = |id: &str| tracks.iter().find(|t| t.id == id).unwrap().title.clone();
    let pls = lib.playlists();
    let plain = pls.iter().find(|p| p.name == "手作り").unwrap();
    assert_eq!(plain.id, 0xABCDEFu64.to_string());
    assert_eq!(plain.track_ids.iter().map(|i| title(i)).collect::<Vec<_>>(), ["Second"]);
    let kb_pl = pls.iter().find(|p| p.name == "KB").unwrap();
    assert!(kb_pl.smart);
    assert_eq!(kb_pl.id, 0xE16748E03448B57Cu64.to_string());
    assert_eq!(kb_pl.track_ids.iter().map(|i| title(i)).collect::<Vec<_>>(), ["First"]);

    // 書き出すと、条件がそのまま戻る
    let out = lib.export_smart_playlists(&[0xE16748E03448B57C]).unwrap();
    let back = XmlLibrary::parse(&out).unwrap();
    let (_, r) = back.playlists[0].smart.as_ref().unwrap();
    assert_eq!(r, &rules);
    let _ = std::fs::remove_dir_all(&base);
}

fn b64(d: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(d)
}

/// .m3u8 の曲が iPod に無ければ、PC のファイルを転送してから取り込むと結び付く
#[test]
fn m3u_transfer_then_import() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-m3u-int-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("ipod");
    std::fs::create_dir_all(root.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA079\n").unwrap();
    let music = base.join("music");
    std::fs::create_dir_all(&music).unwrap();
    if !song(&ffmpeg, &music.join("a.mp3"), "Track A", "Artist") {
        return;
    }
    let m3u = music.join("お出かけ.m3u8");
    std::fs::write(&m3u, "#EXTM3U\n#EXTINF:2,Artist - Track A\na.mp3\n").unwrap();
    let read = |p: &Path| {
        let i = ipod_core::media::read_info(p).ok()?;
        Some(ipod_core::pc::itunesxml::XmlTrack {
            name: i.title.unwrap_or_default(),
            artist: i.artist.unwrap_or_default(),
            album: i.album.unwrap_or_default(),
            total_time_ms: i.duration_ms,
            ..Default::default()
        })
    };
    let x = XmlLibrary::from_m3u(&std::fs::read(&m3u).unwrap(), &m3u, &read);

    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let c = lib.import_candidates(&x);
    assert_eq!((c[0].matched, c[0].missing_count), (0, 1));
    assert_eq!(c[0].transferable, [music.join("a.mp3").to_string_lossy().into_owned()]);
    // 画面は転送が終わってから取り込む
    let opts = AddOptions { auto_reading: false, artwork: false, ..Default::default() };
    lib.add_file(Path::new(&c[0].transferable[0]), None, &opts).unwrap();
    let sum = lib.import_itunes(&x, &[0]).unwrap();
    assert_eq!((sum.playlists, sum.tracks, sum.missing), (1, 1, 0));
    let pl = lib.playlists().into_iter().find(|p| p.name == "お出かけ").unwrap();
    assert_eq!(pl.track_ids.len(), 1);
    let _ = std::fs::remove_dir_all(&base);
}

/// プレイリストの並びを変えると、iTunesDB の中の順番が変わる (ライブラリはいつも先頭)
#[test]
fn reorder_playlists() {
    let base = std::env::temp_dir().join(format!("zenpod-plorder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("ipod");
    std::fs::create_dir_all(root.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA079\n").unwrap();
    let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let a: u64 = lib.create_playlist("A", &[]).unwrap().parse().unwrap();
    let b: u64 = lib.create_playlist("B", &[]).unwrap().parse().unwrap();
    let c: u64 = lib.create_playlist("C", &[]).unwrap().parse().unwrap();
    lib.reorder_playlists(&[c, a, b]).unwrap();
    lib.save().unwrap();
    let lib = Library::open(&root, Mode::Ipod, None).unwrap();
    let names: Vec<String> = lib.playlists().into_iter().map(|p| p.name).collect();
    assert_eq!(names[1..], ["C", "A", "B"]);
    assert!(lib.playlists()[0].master);
    let _ = std::fs::remove_dir_all(&base);
}

/// Rockbox: スマートプレイリストは条件を .m3u8 のコメント行に書き、保存のたびに中身を作り直す
#[test]
fn rockbox_smart_playlist() {
    use ipod_core::smartpl::{MediaTarget, NumValue, Rule, Rules, SmartDef, SmartPrefs, Value};
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-rbsmart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("ipod");
    std::fs::create_dir_all(root.join(".rockbox")).unwrap();
    let a = base.join("a.mp3");
    let b = base.join("b.mp3");
    let c = base.join("c.mp3");
    if !song(&ffmpeg, &a, "One", "KANA-BOON") || !song(&ffmpeg, &b, "Two", "Other") || !song(&ffmpeg, &c, "Three", "KANA-BOON") {
        return;
    }
    let opts = AddOptions { auto_reading: false, artwork: false, ..Default::default() };
    let mut lib = Library::open(&root, Mode::Rockbox, None).unwrap();
    lib.add_file(&a, None, &opts).unwrap();
    lib.add_file(&b, None, &opts).unwrap();
    // アーティストに kana を含む、ライブアップデート
    let def = SmartDef {
        media: MediaTarget::Music,
        rules: Rules {
            any: false,
            items: vec![Rule::Cond { field: 0x04, action: 0x0100_0002, value: Value::Str("kana".into()) }],
        },
        prefs: SmartPrefs::default(),
    };
    let id = lib.save_smart_playlist(None, "KB", def.clone()).unwrap();
    // 「再生回数が 0 より大きい」も足せる (Rockbox の曲は 0 なので合わない)
    let none = SmartDef {
        rules: Rules {
            any: false,
            items: vec![Rule::Cond { field: 0x16, action: 0x10, value: Value::Num(NumValue::single(0)) }],
        },
        ..def.clone()
    };
    lib.save_smart_playlist(None, "Played", none).unwrap();
    lib.save().unwrap();
    let text = std::fs::read_to_string(root.join("Playlists/KB.m3u8")).unwrap();
    assert!(text.contains("#ZENPOD-SMART-RULES:"), "{text}");
    assert_eq!(text.lines().filter(|l| !l.starts_with('#')).count(), 1, "{text}");

    // 開き直しても条件が残り、曲を足して保存すると中身が作り直される
    let mut lib = Library::open(&root, Mode::Rockbox, None).unwrap();
    assert_eq!(lib.smart_playlist(id.parse().unwrap()).unwrap(), def);
    lib.add_file(&c, None, &opts).unwrap();
    lib.save().unwrap();
    let lib = Library::open(&root, Mode::Rockbox, None).unwrap();
    let pls = lib.playlists();
    let kb = pls.iter().find(|p| p.name == "KB").unwrap();
    assert!(kb.smart);
    assert_eq!(kb.track_ids.len(), 2);
    assert!(pls.iter().find(|p| p.name == "Played").unwrap().track_ids.is_empty());
    // iTunes 用にも書き出せる
    let out = lib.export_smart_playlists(&[id.parse().unwrap()]).unwrap();
    assert_eq!(XmlLibrary::parse(&out).unwrap().playlists[0].name, "KB");
    let _ = std::fs::remove_dir_all(&base);
}

/// 1 台目の iPod のプレイリストを .m3u8 に書き出し、同じ曲が入った 2 台目で取り込む (iPod OS → Rockbox)
#[test]
fn m3u8_export_to_another_ipod() {
    let Some(ffmpeg) = which_ffmpeg() else { return };
    let base = std::env::temp_dir().join(format!("zenpod-m3u8-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let a = base.join("a.mp3");
    let b = base.join("b.mp3");
    if !song(&ffmpeg, &a, "First", "KANA-BOON") || !song(&ffmpeg, &b, "Second", "Other") {
        return;
    }
    let opts = AddOptions { auto_reading: false, artwork: false, ..Default::default() };
    let one = base.join("one");
    std::fs::create_dir_all(one.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(one.join("iPod_Control/Device")).unwrap();
    std::fs::write(one.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA079\n").unwrap();
    let mut lib = Library::open(&one, Mode::Ipod, None).unwrap();
    lib.add_file(&a, None, &opts).unwrap();
    lib.add_file(&b, None, &opts).unwrap();
    let ids: Vec<u64> = lib.tracks().iter().map(|t| t.id.parse().unwrap()).collect();
    // 並びは逆にしておく
    let pl: u64 = lib.create_playlist("夜/ドライブ", &[ids[1], ids[0]]).unwrap().parse().unwrap();
    let files = lib.export_m3u8(&[pl]).unwrap();
    assert_eq!(files[0].0, "夜/ドライブ");
    let text = &files[0].1;
    assert!(text.starts_with("#EXTM3U\n#PLAYLIST:夜/ドライブ\n#EXTINF:2,Other - Second\n#EXTART:Other\n#EXTALB:Album\n/iPod_Control/Music/F"), "{text}");
    assert!(lib.export_m3u8(&[12345]).is_err());

    let two = base.join("two");
    std::fs::create_dir_all(two.join(".rockbox")).unwrap();
    let mut other = Library::open(&two, Mode::Rockbox, None).unwrap();
    other.add_file(&a, None, &opts).unwrap();
    other.add_file(&b, None, &opts).unwrap();
    // ファイル名とは別に、#PLAYLIST の名前で取り込む
    let m3u = base.join("夜_ドライブ.m3u8");
    std::fs::write(&m3u, text).unwrap();
    let x = XmlLibrary::from_m3u(text.as_bytes(), &m3u, &|_| None);
    let sum = other.import_itunes(&x, &[0]).unwrap();
    assert_eq!((sum.playlists, sum.tracks, sum.missing), (1, 2, 0));
    let tracks = other.tracks();
    let title = |id: &String| tracks.iter().find(|t| &t.id == id).unwrap().title.clone();
    let pl = other.playlists().into_iter().find(|p| p.name == "夜/ドライブ").unwrap();
    assert_eq!(pl.track_ids.iter().map(title).collect::<Vec<_>>(), ["Second", "First"]);
    // Rockbox の曲は "/Music/..." で書き出す
    let id: u64 = pl.id.parse().unwrap();
    assert!(other.export_m3u8(&[id]).unwrap()[0].1.contains("\n/Music/"));
    let _ = std::fs::remove_dir_all(&base);
}
