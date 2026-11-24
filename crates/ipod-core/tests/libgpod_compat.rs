//! libgpod 0.8.3 が iPod classic (xB029, FireWire GUID 000A27001A2B3C4D) 向けに書き出した
//! iTunesDB を使った互換性テスト。

use ipod_core::ipod::hash58;
use ipod_core::ipod::itunesdb::ITunesDb;

const DB: &[u8] = include_bytes!("fixtures/libgpod-classic-itunesdb.bin");
const FWID: &str = "000A27001A2B3C4D";

#[test]
fn hash58_matches_libgpod() {
    let fwid = hash58::parse_firewire_id(FWID).unwrap();
    let mut copy = DB.to_vec();
    hash58::write_hash58(&mut copy, &fwid);
    assert_eq!(&copy[0x58..0x6C], &DB[0x58..0x6C]);
    assert_eq!(copy, DB);
}

#[test]
fn parses_libgpod_output() {
    let db = ITunesDb::parse(DB).unwrap();
    assert_eq!(db.hashing_scheme(), 1);
    assert_eq!(db.tracks.len(), 5);
    assert_eq!(db.tracks[0].title(), Some("夜に駆ける"));
    assert_eq!(db.tracks[0].artist(), Some("YOASOBI"));
    assert_eq!(db.tracks[2].string(6), Some("Apple Lossless audio file"));
    assert_eq!(db.playlists.len(), 2);
    assert!(db.playlists[0].is_master());
    assert_eq!(db.playlists[1].name, "追加分");
    assert_eq!(db.playlists[1].items.len(), 5);
}

#[test]
fn reserialize_keeps_structure() {
    let mut db = ITunesDb::parse(DB).unwrap();
    let out = db.serialize();
    // mhia/mhii の sql_id は乱数なので長さと再パース結果で比較
    assert_eq!(out.len(), DB.len());
    let again = ITunesDb::parse(&out).unwrap();
    assert_eq!(again.tracks.len(), 5);
    assert_eq!(again.playlists[1].items, db.playlists[1].items);
}
