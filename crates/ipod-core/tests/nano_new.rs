//! iPod nano 5G / 6G / 7G: 縮めた iTunesCDB を読み書きし、署名 (5G は hash72 + hash58、6G / 7G は hashAB) をかけるか。
//! 5G は iTunes が署名した iTunesCDB から署名の材料 (HashInfo) を取り出すか

use std::path::{Path, PathBuf};

use ipod_core::Ipod;
use ipod_core::device::ChecksumKind;
use ipod_core::domain::track::AddOptions;
use ipod_core::ipod::{cdb, hash58, hash72, itunesdb::ITunesDb};

const GUID: &str = "000A27001A2B3C4D";
const FWID: [u8; 8] = [0x00, 0x0A, 0x27, 0x00, 0x1A, 0x2B, 0x3C, 0x4D];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio").join(name)
}

fn base(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zenpod-nano-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn fake_nano(dir: &Path, model: &str, db_version: u32) {
    std::fs::create_dir_all(dir.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Device")).unwrap();
    std::fs::write(dir.join("iPod_Control/Device/SysInfo"), format!("ModelNumStr: {model}\nFirewireGuid: 0x{GUID}\n")).unwrap();
    std::fs::write(
        dir.join("iPod_Control/Device/SysInfoExtended"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>FireWireGUID</key><string>{GUID}</string><key>DBVersion</key><integer>{db_version}</integer><key>SQLiteDB</key><true/></dict></plist>"
        ),
    )
    .unwrap();
}

fn itunes(root: &Path) -> PathBuf {
    root.join("iPod_Control/iTunes")
}

/// iTunes が書いたつもりの iTunesCDB (縮めて hash72 → hash58 の順に署名)
fn itunes_signed_cdb(root: &Path, info: &hash72::HashInfo) {
    let mut db = ITunesDb::new_empty("nano").serialize();
    db[0x0C] = 2;
    let mut packed = cdb::compress(&db).unwrap();
    hash72::write_hash72(&mut packed, info, 1);
    hash58::write_hash58(&mut packed, &FWID);
    std::fs::write(itunes(root).join("iTunesCDB"), &packed).unwrap();
    std::fs::write(itunes(root).join("iTunesDB"), []).unwrap();
}

/// iTunes Library.itlp の 5 つの DB と cbk。cbk の署名の部分 (先頭 `head` バイト) と、その後ろの SHA1 の一覧の SHA1 を返す
fn check_itlp(root: &Path, tracks: i64, head: usize) -> (Vec<u8>, [u8; 20]) {
    use sha1::{Digest, Sha1};
    let itlp = itunes(root).join("iTunes Library.itlp");
    for f in ["Library.itdb", "Locations.itdb", "Dynamic.itdb", "Extras.itdb", "Genius.itdb"] {
        let c = rusqlite::Connection::open(itlp.join(f)).unwrap();
        let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(ok, "ok", "{f}");
    }
    let lib = rusqlite::Connection::open(itlp.join("Library.itdb")).unwrap();
    let n: i64 = lib.query_row("SELECT count(*) FROM item", [], |r| r.get(0)).unwrap();
    assert_eq!(n, tracks);
    let loc = std::fs::read(itlp.join("Locations.itdb")).unwrap();
    let cbk = std::fs::read(itlp.join("Locations.itdb.cbk")).unwrap();
    let blocks: Vec<[u8; 20]> = loc.as_chunks::<1024>().0.iter().map(|b| Sha1::digest(b).into()).collect();
    assert_eq!(cbk.len(), head + 20 + 20 * blocks.len());
    let mut all = Sha1::new();
    for (i, b) in blocks.iter().enumerate() {
        assert_eq!(&cbk[head + 20 + 20 * i..head + 40 + 20 * i], b);
        all.update(b);
    }
    let sum: [u8; 20] = all.finalize().into();
    assert_eq!(&cbk[head..head + 20], &sum);
    (cbk[..head].to_vec(), sum)
}

fn add_and_save(root: &Path) -> Ipod {
    let mut ipod = Ipod::open(root).unwrap();
    ipod.check_writable().unwrap();
    ipod.add_file(&fixture("plain.mp3"), None, &AddOptions { auto_reading: false, ..Default::default() }).unwrap();
    ipod.save().unwrap();
    ipod
}

#[test]
fn nano5_recovers_hash_info_and_signs_twice() {
    let root = base("5g");
    fake_nano(&root, "xC027", 3);
    let info = hash72::HashInfo::new(&FWID, *b"RANDOMBYTES!", [0x42; 16]);
    itunes_signed_cdb(&root, &info);

    let ipod = add_and_save(&root);
    assert_eq!(ipod.info.checksum, ChecksumKind::Hash72);
    assert!(ipod.info.sqlite_db);
    // 取り出した材料を HashInfo に覚える
    let saved = std::fs::read(root.join("iPod_Control/Device/HashInfo")).unwrap();
    assert_eq!(hash72::HashInfo::parse(&saved, &FWID), Some(info.clone()));

    let cdb_bytes = std::fs::read(itunes(&root).join("iTunesCDB")).unwrap();
    assert!(cdb::is_compressed(&cdb_bytes));
    assert_eq!(std::fs::metadata(itunes(&root).join("iTunesDB")).unwrap().len(), 0);
    assert_eq!(&cdb_bytes[0x30..0x32], &[1, 0], "hashing_scheme は 1");
    // hash72 は同じ材料で確かめられる
    assert_eq!(hash72::extract_from_db(&cdb_bytes, &FWID), Some(info.clone()));
    // hash58 も正しい (かけ直しても同じ)
    let mut again = cdb_bytes.clone();
    hash58::write_hash58(&mut again, &FWID);
    assert_eq!(again, cdb_bytes);

    // Locations.itdb.cbk も同じ材料の hash72
    let (sig, sum) = check_itlp(&root, 1, 46);
    assert_eq!(hash72::HashInfo::extract(&sig, &sum, &FWID), Some(info));
    // 開き直すと曲が読める
    let ipod = Ipod::open(&root).unwrap();
    assert_eq!(ipod.tracks().len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn nano5_without_itunes_sync_is_read_only() {
    let root = base("5g-new");
    fake_nano(&root, "xC060", 3);
    let ipod = Ipod::open(&root).unwrap();
    assert!(ipod.check_writable().is_err(), "署名の材料が無ければ書き込まない");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn nano6_and_7_sign_with_hashab() {
    for (model, name, pid_model) in [("xC525", "6g", "iPod nano (6G) 8GB"), ("", "7g", "iPod")] {
        let root = base(name);
        fake_nano(&root, model, 5);
        let ipod = add_and_save(&root);
        assert_eq!(ipod.info.checksum, ChecksumKind::HashAb, "{name}");
        assert!(ipod.info.model_name.starts_with(pid_model), "{}", ipod.info.model_name);
        let bytes = std::fs::read(itunes(&root).join("iTunesCDB")).unwrap();
        assert_eq!(&bytes[0x30..0x32], &[3, 0], "hashing_scheme は 3");
        assert_eq!(&bytes[0xAB..0xAD], &[3, 0], "hashAB の署名");
        // かけ直しても同じ (署名を除いた中身から決まる)
        let mut again = bytes.clone();
        ipod_core::ipod::hashab::write_hashab(&mut again, &FWID);
        assert_eq!(again, bytes);
        let (sig, sum) = check_itlp(&root, 1, 57);
        assert_eq!(sig, ipod_core::ipod::hashab::calc_hashab(&sum, &FWID, ipod_core::ipod::hashab::HASHAB_RND));
        assert_eq!(Ipod::open(&root).unwrap().tracks().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn merges_play_counts_plist() {
    let root = base("plist");
    fake_nano(&root, "xC525", 5);
    let ipod = add_and_save(&root);
    let id: u64 = ipod.tracks()[0].id.parse().unwrap();
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>tracks</key><array>\
         <dict><key>persistentID</key><integer>{}</integer><key>playCount</key><integer>4</integer>\
         <key>playMacOSDate</key><integer>3700000000</integer><key>userRating</key><integer>80</integer></dict>\
         <dict><key>persistentID</key><integer>12345</integer><key>playCount</key><integer>9</integer></dict>\
         </array></dict></plist>",
        id as i64
    );
    std::fs::write(itunes(&root).join("PlayCounts.plist"), plist).unwrap();
    let mut ipod = Ipod::open(&root).unwrap();
    assert!(ipod.is_dirty());
    let t = &ipod.tracks()[0];
    assert_eq!((t.play_count, t.rating), (4, 80));
    ipod.save().unwrap();
    assert!(!itunes(&root).join("PlayCounts.plist").exists());
    assert_eq!(Ipod::open(&root).unwrap().tracks()[0].play_count, 4);
    let _ = std::fs::remove_dir_all(&root);
}
