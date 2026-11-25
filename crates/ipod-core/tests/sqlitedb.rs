//! iPod nano 5G / 6G / 7G の SQLite のライブラリ (iTunes Library.itlp) を書いて、中身を読んで確かめる

use std::path::Path;

use ipod_core::domain::audio::MediaKind;
use ipod_core::ipod::itunesdb::*;
use ipod_core::ipod::sqlitedb::{self, ItlpOptions};
use rusqlite::Connection;
use sha1::{Digest, Sha1};

const DBS: [&str; 5] = ["Dynamic.itdb", "Extras.itdb", "Genius.itdb", "Library.itdb", "Locations.itdb"];

fn track(db: &mut ITunesDb, title: &str, album: Option<&str>, artist: &str, path: &str) -> u64 {
    let mut t = Track::new(db.new_dbid());
    t.set_string(MHOD_TITLE, Some(title));
    t.set_string(MHOD_ALBUM, album);
    t.set_string(MHOD_ARTIST, Some(artist));
    t.set_string(MHOD_GENRE, Some("Rock"));
    t.set_string(MHOD_FILETYPE, Some("MPEG audio file"));
    t.set_string(MHOD_PATH, Some(path));
    t.set_filetype_marker(0x4d50_3320);
    t.set_length_ms(180_000);
    t.set_bitrate(256);
    t.set_samplerate(44_100);
    t.set_size(5_000_000);
    let id = t.dbid();
    db.tracks.push(t);
    id
}

/// 曲 4 つ (アルバム名の並べ替えを試す名前) + ポッドキャスト 1 つ + 普通のプレイリスト
fn sample() -> (ITunesDb, Vec<u64>, u64) {
    let mut db = ITunesDb::new_empty("nano");
    let ids = vec![
        track(&mut db, "Z song", Some("The Zimbo Trio"), "Zimbo", ":iPod_Control:Music:F00:AAAA.mp3"),
        track(&mut db, "B song", Some("Blink 182"), "Blink", ":iPod_Control:Music:F01:BBBB.mp3"),
        track(&mut db, "No album", None, "Blink", ":iPod_Control:Music:F02:CCCC.mp3"),
        track(&mut db, "Numbers", Some("123"), "Zimbo", ":iPod_Control:Music:F03:DDDD.mp3"),
        track(&mut db, "Episode 1", Some("Show"), "Host", ":iPod_Control:Music:F04:EEEE.mp3"),
    ];
    db.tracks[0].set_playcount(3);
    db.tracks[0].set_rating(80);
    db.tracks[0].set_artwork(0x65, 1000);
    db.tracks[1].set_string(MHOD_COMPOSER, Some("Someone"));
    assert!(db.tracks[4].set_kind(MediaKind::Podcast));
    let mut pl = Playlist::new("Mix", 0x8123_4567_89AB_CDEF, false);
    pl.items = vec![ids[1], ids[0]];
    db.playlists.push(pl);
    db.serialize();
    (db, ids, 0x8123_4567_89AB_CDEF)
}

/// 署名の代わり: 渡された SHA1 の頭に印を付けて 46 バイトに
fn fake_sign(sha: &[u8; 20]) -> ipod_core::Result<Vec<u8>> {
    let mut v = vec![0xEE; 26];
    v.extend_from_slice(sha);
    Ok(v)
}

fn open(itlp: &Path, name: &str) -> Connection {
    Connection::open(itlp.join(name)).unwrap()
}

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}

#[test]
fn writes_libgpod_shaped_databases() {
    let (db, ids, mix) = sample();
    let dir = tempfile::tempdir().unwrap();
    let opts = ItlpOptions { uuid: "000A27001A2B3C4D", post_process: None };
    sqlitedb::write_itlp(dir.path(), &db, &opts, &fake_sign).unwrap();
    let itlp = dir.path().join("iTunes Library.itlp");

    for name in DBS {
        let c = open(&itlp, name);
        let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(ok, "ok", "{name}");
    }
    // 一時ファイルを残さない
    let names: Vec<String> = std::fs::read_dir(&itlp).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(names.len(), 6, "{names:?}");

    let lib = open(&itlp, "Library.itdb");
    assert_eq!(count(&lib, "item"), 5);
    // マスター (5 曲) + ポッドキャスト (1) + Mix (2)
    assert_eq!(count(&lib, "container"), 3);
    assert_eq!(count(&lib, "item_to_container"), 8);
    assert_eq!(count(&lib, "album"), 4);
    assert_eq!(count(&lib, "artist"), 3);
    assert_eq!(count(&lib, "composer"), 1);
    assert_eq!(count(&lib, "genre_map"), 1);
    assert_eq!(count(&lib, "avformat_info"), 5);

    // pid は dbid のビットのまま (符号付き。pid は rowid なので pid の順に並ぶ)
    let pids: Vec<i64> = lib.prepare("SELECT pid FROM item ORDER BY pid").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
    let mut want: Vec<i64> = ids.iter().map(|&d| d as i64).collect();
    want.sort();
    assert_eq!(pids, want);
    let (kind, podcast, song): (i64, i64, i64) = lib
        .query_row("SELECT media_kind, is_podcast, is_song FROM item WHERE pid = ?", [ids[4] as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!((kind, podcast, song), (4, 1, 0));
    let (art, cache): (i64, i64) =
        lib.query_row("SELECT artwork_status, artwork_cache_id FROM item WHERE pid = ?", [ids[0] as i64], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((art, cache), (1, 0x65));

    // プレイリスト: マスターは隠す・ポッドキャストは 11・Mix の並び
    let master_pid = db.playlists[0].id() as i64;
    let primary: i64 = lib.query_row("SELECT primary_container_pid FROM db_info", [], |r| r.get(0)).unwrap();
    assert_eq!(primary, master_pid);
    let hidden: i64 = lib.query_row("SELECT is_hidden FROM container WHERE pid = ?", [master_pid], |r| r.get(0)).unwrap();
    assert_eq!(hidden, 1);
    let podcast_kind: i64 = lib.query_row("SELECT COUNT(*) FROM container WHERE distinguished_kind = 11 AND is_hidden = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(podcast_kind, 1);
    let members: Vec<i64> = lib
        .prepare("SELECT item_pid FROM item_to_container WHERE container_pid = ? ORDER BY physical_order")
        .unwrap()
        .query_map([mix as i64], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(members, vec![ids[1] as i64, ids[0] as i64]);

    // アルバムの順位: 冠詞を除いた名前・数字は文字の後・アルバム名なしは最後
    let rank = |name: &str| -> i64 { lib.query_row("SELECT name_order FROM album WHERE name = ?", [name], |r| r.get(0)).unwrap() };
    assert!(rank("Blink 182") < rank("Show"));
    assert!(rank("Show") < rank("The Zimbo Trio"));
    assert!(rank("The Zimbo Trio") < rank("123"));
    let item_rank = |i: usize| -> i64 { lib.query_row("SELECT album_order FROM item WHERE pid = ?", [ids[i] as i64], |r| r.get(0)).unwrap() };
    assert_eq!(item_rank(0), rank("The Zimbo Trio"));
    assert!(item_rank(2) > rank("123"));
    // 同じアルバムの曲は同じ album_pid、アルバムのアーティストは曲のアーティスト
    let (album_pid, artist_pid): (i64, i64) =
        lib.query_row("SELECT album_pid, artist_pid FROM item WHERE pid = ?", [ids[0] as i64], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let album_artist: i64 = lib.query_row("SELECT artist_pid FROM album WHERE pid = ?", [album_pid], |r| r.get(0)).unwrap();
    assert_eq!(album_artist, artist_pid);
    let artwork_item: i64 = lib.query_row("SELECT artwork_item_pid FROM album WHERE pid = ?", [album_pid], |r| r.get(0)).unwrap();
    assert_eq!(artwork_item, ids[0] as i64);

    // Locations: iPod_Control/Music からの場所
    let loc = open(&itlp, "Locations.itdb");
    assert_eq!(count(&loc, "location"), 5);
    let base: String = loc.query_row("SELECT path FROM base_location WHERE id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(base, "iPod_Control/Music");
    let (path, ty, size): (String, i64, i64) = loc
        .query_row("SELECT location, location_type, file_size FROM location WHERE item_pid = ?", [ids[0] as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!((path.as_str(), ty, size), ("F00/AAAA.mp3", 0x4649_4C45, 5_000_000));

    // Dynamic: 再生回数と評価。再生していない日時は 0 から計算した値 (libgpod と同じ)
    let dynamic = open(&itlp, "Dynamic.itdb");
    assert_eq!(count(&dynamic, "item_stats"), 5);
    assert_eq!(count(&dynamic, "container_ui"), 3);
    let (played, plays, rating, date): (i64, i64, i64, i64) = dynamic
        .query_row("SELECT has_been_played, play_count_user, user_rating, date_played FROM item_stats WHERE item_pid = ?", [ids[0] as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    assert_eq!((played, plays, rating, date), (1, 3, 80, -978_307_200));

    // Extras: どの曲にも章の無いチャプタ
    let extras = open(&itlp, "Extras.itdb");
    assert_eq!(count(&extras, "chapter"), 5);
    let v: (i64, i64) = extras.query_row("SELECT user_version, device_version FROM db_schema_upgrade_info", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(v, (8, 19));

    // cbk: 署名 46 + SHA1 の一覧の SHA1 + 1024 バイトごとの SHA1
    let locations = std::fs::read(itlp.join("Locations.itdb")).unwrap();
    let cbk = std::fs::read(itlp.join("Locations.itdb.cbk")).unwrap();
    let blocks = locations.len() / 1024;
    assert_eq!(locations.len() % 1024, 0);
    assert_eq!(cbk.len(), 46 + 20 + 20 * blocks);
    let list = &cbk[66..];
    for (i, b) in locations.chunks(1024).enumerate() {
        assert_eq!(&list[i * 20..i * 20 + 20], Sha1::digest(b).as_slice());
    }
    let digest = Sha1::digest(list);
    assert_eq!(&cbk[46..66], digest.as_slice());
    assert_eq!(&cbk[..26], &[0xEE; 26]);
    assert_eq!(&cbk[26..46], digest.as_slice());

    // もう一度書いても置き換わるだけ
    sqlitedb::write_itlp(dir.path(), &db, &opts, &fake_sign).unwrap();
    assert_eq!(std::fs::read_dir(&itlp).unwrap().count(), 6);
}

fn post_process_dict() -> plist::Dictionary {
    let mut sql = plist::Dictionary::new();
    sql.insert("Old".into(), "UPDATE item SET description_long = 'old';".into());
    sql.insert(
        "CreateIndex".into(),
        "CREATE INDEX item_title_key ON item (title_order); UPDATE item SET description_long = hex(iPhoneSortKey(title)), category_id = iPhoneSortSection(iPhoneSortKey(title));".into(),
    );
    sql.insert("Attached".into(), "UPDATE \"Dynamic.itdb\".item_stats SET has_changes = 1;".into());
    sql.insert("Broken".into(), "THIS IS NOT SQL;".into());
    let set = |cmds: &[&str]| {
        let mut d = plist::Dictionary::new();
        d.insert("Commands".into(), plist::Value::Array(cmds.iter().map(|c| plist::Value::String(c.to_string())).collect()));
        plist::Value::Dictionary(d)
    };
    let mut sets = plist::Dictionary::new();
    sets.insert("8".into(), set(&["Old"]));
    // 一番大きい番号の組だけを、書いてある順に流す (失敗・無い名前は飛ばす)
    sets.insert("9".into(), set(&["CreateIndex", "Broken", "Missing", "Attached"]));
    let mut ppc = plist::Dictionary::new();
    ppc.insert("SQLCommands".into(), plist::Value::Dictionary(sql));
    ppc.insert("UserVersionCommandSets".into(), plist::Value::Dictionary(sets));
    ppc
}

#[test]
fn runs_post_process_commands_from_sysinfo_extended() {
    // SysInfoExtended の XML から取り出す
    let mut root = plist::Dictionary::new();
    root.insert("FireWireGUID".into(), "000A27001A2B3C4D".into());
    root.insert("com.apple.mobile.iTunes.SQLMusicLibraryPostProcessCommands".into(), plist::Value::Dictionary(post_process_dict()));
    let mut xml = Vec::new();
    plist::Value::Dictionary(root).to_writer_xml(&mut xml).unwrap();
    let ppc = sqlitedb::post_process_commands(std::str::from_utf8(&xml).unwrap()).unwrap();
    assert!(sqlitedb::post_process_commands("<plist><dict></dict></plist>").is_none());

    let (db, ids, _) = sample();
    let dir = tempfile::tempdir().unwrap();
    let opts = ItlpOptions { uuid: "000A27001A2B3C4D", post_process: Some(&ppc) };
    sqlitedb::write_itlp(dir.path(), &db, &opts, &fake_sign).unwrap();
    let itlp = dir.path().join("iTunes Library.itlp");

    let lib = open(&itlp, "Library.itdb");
    let idx: i64 = lib.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'item_title_key'", [], |r| r.get(0)).unwrap();
    assert_eq!(idx, 1);
    let (key, section): (String, i64) =
        lib.query_row("SELECT description_long, category_id FROM item WHERE pid = ?", [ids[1] as i64], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let expect: String = sqlitedb::iphone_sort_key(Some(b"B song")).iter().map(|b| format!("{b:02X}")).collect();
    assert_eq!(key, expect);
    assert_eq!(section, 1);
    let old: i64 = lib.query_row("SELECT COUNT(*) FROM item WHERE description_long = 'old'", [], |r| r.get(0)).unwrap();
    assert_eq!(old, 0);
    let dynamic = open(&itlp, "Dynamic.itdb");
    let changed: i64 = dynamic.query_row("SELECT COUNT(*) FROM item_stats WHERE has_changes = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(changed, 5);
    for name in DBS {
        let ok: String = open(&itlp, name).query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(ok, "ok", "{name}");
    }
}
