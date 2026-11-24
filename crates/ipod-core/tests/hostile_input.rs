//! 壊れた・細工された iPod (中古・「フォルダを開く」で開いたもの) で、iPod の外のファイルを触らず、落ちもしない。
//! データは小さく、テストの中で作る

use std::path::{Path, PathBuf};

use ipod_core::Ipod;
use ipod_core::ipod::artwork::{Thumb, read_thumb};
use ipod_core::ipod::ipod_to_fs;
use ipod_core::ipod::itunesdb::{ITunesDb, MHOD_PATH, MHOD_TITLE, Track, parse_otg};
use ipod_core::rockbox::rbdb;
use ipod_core::smartpl::Rules;

fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("zenpod-hostile-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn ipod_path_stays_inside_root() {
    let root = Path::new("/media/ipod");
    let music = root.join("iPod_Control").join("Music").join("F00").join("ABCD.mp3");
    assert_eq!(ipod_to_fs(root, ":iPod_Control:Music:F00:ABCD.mp3"), Some(music.clone()));
    assert_eq!(ipod_to_fs(root, "iPod_Control/Music\\F00/ABCD.mp3"), Some(music));
    for bad in [
        ":iPod_Control:..:..:..:etc:passwd",
        "..",
        ":iPod_Control:Music:..\\..\\..\\x",
        "a/../b",
        ":iPod_Control:\0:x",
        "",
        ":::",
    ] {
        assert_eq!(ipod_to_fs(root, bad), None, "{bad:?}");
    }
    // 絶対パス・ドライブ文字・UNC は区切りで割るので iPod の中になる
    for p in ["\\\\server\\share\\x.mp3", "/etc/passwd", "C:\\Windows\\win.ini", "\\\\?\\UNC\\server\\share"] {
        let got = ipod_to_fs(root, p);
        assert!(got.as_deref().is_none_or(|g| g.starts_with(root) && g != root), "{p:?} → {got:?}");
    }
}

/// iTunesDB の曲の場所が iPod の外を指していても、曲を消したときに外のファイルを消さず、再生用の場所も出さない
#[test]
fn removing_hostile_track_keeps_outside_file() {
    let base = tmp("remove");
    let root = base.join("ipod");
    std::fs::create_dir_all(root.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    std::fs::create_dir_all(root.join("iPod_Control/Music/F00")).unwrap();
    std::fs::write(root.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xA002\n").unwrap();
    let outside = base.join("outside.txt");
    std::fs::write(&outside, "keep").unwrap();
    let inside = root.join("iPod_Control/Music/F00/OK.mp3");
    std::fs::write(&inside, "x").unwrap();

    let mut db = ITunesDb::new_empty("iPod");
    let mut evil = Track::new(db.new_dbid());
    evil.set_string(MHOD_TITLE, Some("evil"));
    evil.set_string(MHOD_PATH, Some(":iPod_Control:..:..:outside.txt"));
    let evil_id = evil.dbid();
    db.tracks.push(evil);
    let mut ok = Track::new(db.new_dbid());
    ok.set_string(MHOD_TITLE, Some("ok"));
    ok.set_string(MHOD_PATH, Some(":iPod_Control:Music:F00:OK.mp3"));
    let ok_id = ok.dbid();
    db.tracks.push(ok);
    std::fs::write(root.join("iPod_Control/iTunes/iTunesDB"), db.serialize()).unwrap();

    let mut ipod = Ipod::open(&root).unwrap();
    assert_eq!(ipod.track_file(evil_id), None);
    assert_eq!(ipod.track_file(ok_id), Some(inside.clone()));
    assert_eq!(ipod.remove_tracks(&[evil_id, ok_id]).unwrap(), 2);
    assert!(outside.exists(), "iPod の外のファイルは消さない");
    assert!(!inside.exists());
    let _ = std::fs::remove_dir_all(&base);
}

/// mhbd + mhsd (type 1) + mhlt (曲の数 n、中身なし)
fn itunesdb_with_mhlt(n: u32) -> Vec<u8> {
    let mut mhlt = Vec::new();
    mhlt.extend_from_slice(b"mhlt");
    le32(&mut mhlt, 12);
    le32(&mut mhlt, n);
    let mut mhsd = Vec::new();
    mhsd.extend_from_slice(b"mhsd");
    le32(&mut mhsd, 16);
    le32(&mut mhsd, 16 + mhlt.len() as u32);
    le32(&mut mhsd, 1);
    mhsd.extend_from_slice(&mhlt);
    let mut db = vec![0u8; 0x68];
    db[0..4].copy_from_slice(b"mhbd");
    db[4..8].copy_from_slice(&0x68u32.to_le_bytes());
    db[0x0C..0x10].copy_from_slice(&1u32.to_le_bytes());
    db[0x14..0x18].copy_from_slice(&1u32.to_le_bytes());
    db.extend_from_slice(&mhsd);
    let total = db.len() as u32;
    db[8..12].copy_from_slice(&total.to_le_bytes());
    db
}

#[test]
fn huge_track_count_is_an_error() {
    assert_eq!(ITunesDb::parse(&itunesdb_with_mhlt(0)).unwrap().tracks.len(), 0);
    assert!(ITunesDb::parse(&itunesdb_with_mhlt(u32::MAX)).is_err());
}

#[test]
fn huge_otg_count_reads_only_what_is_there() {
    let mut db = ITunesDb::new_empty("x");
    let t = Track::new(db.new_dbid());
    let id = t.dbid();
    db.tracks.push(t);
    let mut otg = Vec::new();
    otg.extend_from_slice(b"mhpo");
    le32(&mut otg, 16);
    le32(&mut otg, 4);
    le32(&mut otg, u32::MAX);
    assert!(parse_otg(&db, &otg).is_empty());
    le32(&mut otg, 0);
    assert_eq!(parse_otg(&db, &otg), vec![id]);
    // 見出しの長さが壊れていても
    otg[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse_otg(&db, &otg).is_empty());
}

#[test]
fn huge_rockbox_count_is_safe() {
    let base = tmp("tcd");
    let dir = base.join(".rockbox");
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = Vec::new();
    m.extend_from_slice(&rbdb::MAGIC.to_le_bytes());
    m.extend_from_slice(&0i32.to_le_bytes());
    m.extend_from_slice(&i32::MAX.to_le_bytes());
    m.extend_from_slice(&[0u8; 12]);
    std::fs::write(dir.join("database_idx.tcd"), &m).unwrap();
    assert!(rbdb::read(&dir).is_none());
    assert!(rbdb::stats(&base).is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

/// config.cfg の database path / scan paths が iPod の外を指していれば既定の場所を使う
#[test]
fn rockbox_paths_stay_inside_root() {
    let base = tmp("rbcfg");
    let root = base.join("ipod");
    std::fs::create_dir_all(root.join(".rockbox")).unwrap();
    std::fs::create_dir_all(root.join("Music")).unwrap();
    std::fs::create_dir_all(base.join("outside")).unwrap();
    std::fs::write(base.join("outside/song.mp3"), "x").unwrap();
    std::fs::write(root.join("Music/a.mp3"), "x").unwrap();
    for bad in ["/../outside", "C:\\outside", "/Music/../../outside"] {
        std::fs::write(root.join(".rockbox/config.cfg"), format!("database path: {bad}\ndatabase scan paths: {bad}\n")).unwrap();
        assert_eq!(rbdb::db_dir(&root), root.join(".rockbox"), "{bad:?}");
        let (files, _) = rbdb::scan_files(&root);
        assert!(files.iter().all(|f| f.path.starts_with(&root)), "{bad:?}");
    }
    std::fs::write(root.join(".rockbox/config.cfg"), "database path: /Music\ndatabase scan paths: /Music:/../outside\n").unwrap();
    assert_eq!(rbdb::db_dir(&root), root.join("Music"));
    let (files, _) = rbdb::scan_files(&root);
    assert_eq!(files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), vec![root.join("Music").join("a.mp3")]);
    let _ = std::fs::remove_dir_all(&base);
}

fn slst_header(n: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"SLst");
    be32(&mut b, 0x0001_0001);
    be32(&mut b, n);
    be32(&mut b, 0);
    b.resize(136, 0);
    b
}

/// グループを `depth` 段入れ子にした SLst
fn nested_slst(depth: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for k in 0..depth {
        let inner_len = (depth - k - 1) * 192 + 136;
        out.extend_from_slice(&slst_header(1));
        let mut rule = vec![0u8; 56];
        rule[52..56].copy_from_slice(&(inner_len as u32).to_be_bytes());
        out.extend_from_slice(&rule);
    }
    out.extend_from_slice(&slst_header(0));
    out
}

#[test]
fn hostile_smart_rules_are_errors() {
    assert!(Rules::parse(&slst_header(u32::MAX)).is_err());
    assert!(Rules::parse(&nested_slst(4)).is_ok());
    // 深い入れ子はスタックを使い切る前にやめる
    assert!(Rules::parse(&nested_slst(20_000)).is_err());
}

#[test]
fn hostile_thumbs_are_errors() {
    let dir = tmp("thumb");
    std::fs::write(dir.join("F9999_1.ithmb"), [0u8; 16]).unwrap();
    let thumb = |file: &str, size: u32, w: u16, h: u16| Thumb {
        format_id: 9999,
        file: file.into(),
        offset: 0,
        size,
        vpad: 0,
        hpad: 0,
        height: h,
        width: w,
    };
    assert_eq!(read_thumb(&dir, &thumb(":F9999_1.ithmb", 8, 2, 2)).unwrap().dimensions(), (2, 2));
    // ファイルより大きい
    assert!(read_thumb(&dir, &thumb(":F9999_1.ithmb", u32::MAX, 1, 1)).is_err());
    // 巨大な幅と高さ
    assert!(read_thumb(&dir, &thumb(":F9999_1.ithmb", 2, 65535, 65535)).is_err());
    let mut t = thumb(":F9999_1.ithmb", 2, 65535, 65535);
    t.hpad = 65535;
    t.vpad = 65535;
    assert!(read_thumb(&dir, &t).is_err());
    // フォルダの外
    let outside = dir.parent().unwrap().join(format!("zenpod-hostile-out-{}.ithmb", std::process::id()));
    std::fs::write(&outside, [0u8; 16]).unwrap();
    let name = outside.file_name().unwrap().to_str().unwrap();
    assert!(read_thumb(&dir, &thumb(&format!(":..:{name}"), 8, 2, 2)).is_err());
    assert!(read_thumb(&dir, &thumb(&outside.to_string_lossy(), 8, 2, 2)).is_err());
    let _ = std::fs::remove_file(&outside);
    let _ = std::fs::remove_dir_all(&dir);
}
