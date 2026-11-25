//! iPod nano 5G / 6G / 7G が読む SQLite のライブラリ (`iPod_Control/iTunes/iTunes Library.itlp/`)。
//!
//! これらの機種は iTunesCDB (iTunes 用) とは別に、5 つの SQLite のデータベースを読んで曲を出す:
//! - `Library.itdb`: 曲 (item)・アルバム・アーティスト・作曲者・ジャンル・プレイリスト (container) とその中身
//! - `Locations.itdb`: 曲のファイルの場所 (`iPod_Control/Music` からの相対パス)
//! - `Dynamic.itdb`: 再生回数・評価・ブックマークと、プレイリストの表示の設定
//! - `Extras.itdb`: チャプタ (歌詞は書かない)
//! - `Genius.itdb`: 表だけ (Genius の中身は作らない)
//!
//! と、`Locations.itdb.cbk` (Locations.itdb の 1024 バイトごとの SHA1 の一覧とその SHA1 に署名したもの)。
//! 本体の SysInfoExtended に後処理の SQL (`com.apple.mobile.iTunes.SQLMusicLibraryPostProcessCommands`) があれば、
//! Library.itdb にほかの 4 つをつないで順に流す (索引や並べ替えの値を本体の版に合わせて作り直すもの)。
//!
//! 由来: libgpod 0.8.3 の `src/itdb_sqlite.c` と `src/itdb_sqlite_queries.h`
//! (Copyright (C) 2009 Nikias Bassen, Christophe Fergeau, Hector Martin。LGPL-2.1-or-later) を移植した。
//! 表の定義・書く値・iPhoneSortKey / iPhoneSortSection の計算・cbk の形は libgpod と同じ。違うところ:
//! - アルバムの並び (album の name_order と item の album_order) は、後処理の無い nano 6G / 7G でも
//!   正しく並ぶように自前で順位を付ける (`album_rank_key`。iOpenPod の調べに合わせた)。libgpod は album の
//!   name_order にアーティストの順位を入れていて、同じアーティストのアルバムがみな同じ値になる
//! - アルバム・アーティスト・作曲者の pid は libgpod では乱数。ここでは名前から決める (保存のたびに変わらない)
//! - video_info の sort_series_name は libgpod では書き損じで NULL になる。ここでは並べ替え用の番組名を書く
//! - 文字列の順位 (title_order など) の比較は、libgpod の g_utf8_collate (ロケール次第) の代わりに
//!   大文字小文字を区別しない比較 (同じなら元の文字列で) にした
//! - 時刻は iTunesDB の Mac の時刻をそのまま UTC として扱う (`itunesdb::mac_to_unix` と同じ。libgpod は
//!   本体の時間帯の分をずらす)

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::functions::FunctionFlags;
use rusqlite::types::{Value, ValueRef};
use rusqlite::Connection;
use sha1::{Digest, Sha1};

use super::itunesdb::*;
use crate::error::{Error, Result};
use crate::t;

pub const ITLP_DIR: &str = "iTunes Library.itlp";
const POST_PROCESS_KEY: &str = "com.apple.mobile.iTunes.SQLMusicLibraryPostProcessCommands";
const CBK_BLOCK: usize = 1024;
/// 作る順 (iTunes Library.itlp に置き換える順でもある)
const FILES: [&str; 6] = [
    "Dynamic.itdb",
    "Extras.itdb",
    "Genius.itdb",
    "Library.itdb",
    "Locations.itdb",
    "Locations.itdb.cbk",
];

// ---- mediatype のビット (libgpod の ITDB_MEDIATYPE_*) ----
const MEDIA_MOVIE: u32 = 1 << 1;
const MEDIA_MUSICVIDEO: u32 = 1 << 5;
const MEDIA_TVSHOW: u32 = 1 << 6;

/// 書くときの設定
pub struct ItlpOptions<'a> {
    /// FireWire GUID / UUID (16 進の文字列)。libgpod は iPhone の後処理の SQL を本体から取るときにだけ使う。
    /// nano では今は使わない (iPhone 用の base_location も書かない)
    pub uuid: &'a str,
    /// SysInfoExtended の `com.apple.mobile.iTunes.SQLMusicLibraryPostProcessCommands` (`post_process_commands` で取る)
    pub post_process: Option<&'a plist::Dictionary>,
}

/// `<itunes_dir>/iTunes Library.itlp/` に 5 つのデータベースと `Locations.itdb.cbk` を書く。
///
/// `db` は `ITunesDb::serialize` を済ませたもの (トラックの番号・マスタープレイリストの中身が決まっている)。
/// 一時フォルダで全部作ってから iPod に写し、最後にまとめて名前を付け替える (libgpod も一時フォルダで作って写す)。
/// `sign` は Locations.itdb の SHA1 の一覧の SHA1 (20 バイト) から cbk の先頭に置く署名を作る
/// (hash72 なら 46 バイト・hashAB なら 57 バイト。機種ごとの方式は呼び出し側が決める)
pub fn write_itlp(
    itunes_dir: &Path,
    db: &ITunesDb,
    opts: &ItlpOptions,
    sign: &dyn Fn(&[u8; 20]) -> Result<Vec<u8>>,
) -> Result<()> {
    let itlp = itunes_dir.join(ITLP_DIR);
    if itlp.exists() && !itlp.is_dir() {
        return Err(Error::Other(t!("iTunes Library.itlp がフォルダではありません")));
    }
    fs::create_dir_all(&itlp)?;

    let tmp = TempDir::new()?;
    build_itdb_files(tmp.path(), db, opts, sign)?;

    // 先に全部を隣に写してから名前を付け替える (途中で失敗しても古い組のまま残る)
    let staged: Vec<(PathBuf, PathBuf)> = FILES
        .iter()
        .map(|f| (itlp.join(format!("{f}.zenpod-tmp")), itlp.join(f)))
        .collect();
    let copied = FILES.iter().zip(&staged).try_for_each(|(f, (part, _))| copy_synced(&tmp.path().join(f), part));
    if let Err(e) = copied {
        for (part, _) in &staged {
            let _ = fs::remove_file(part);
        }
        return Err(e.into());
    }
    for (part, dst) in &staged {
        fs::rename(part, dst)?;
    }
    Ok(())
}

/// SysInfoExtended (XML の plist) から後処理の SQL の辞書を取り出す。無ければ None
pub fn post_process_commands(sysinfo_ext: &str) -> Option<plist::Dictionary> {
    let v = plist::Value::from_reader_xml(std::io::Cursor::new(sysinfo_ext.as_bytes())).ok()?;
    v.as_dictionary()?.get(POST_PROCESS_KEY)?.as_dictionary().cloned()
}

/// Locations.itdb の中身から cbk を作る (libgpod の mk_Locations_cbk)。
/// 形: 署名 (`sign` の結果) + 一覧の SHA1 (20) + 1024 バイトごとの SHA1 (20 ずつ)。
/// 1024 バイトに満たない最後の端は libgpod と同じく数えない (SQLite のファイルはページの倍数なので端は出ない)
pub fn locations_cbk(locations: &[u8], sign: &dyn Fn(&[u8; 20]) -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    let mut blocks = Vec::with_capacity(locations.len() / CBK_BLOCK * 20);
    for b in locations.as_chunks::<CBK_BLOCK>().0 {
        blocks.extend_from_slice(&Sha1::digest(b));
    }
    if blocks.is_empty() {
        return Err(Error::Other(t!("Locations.itdb が小さすぎます")));
    }
    let digest: [u8; 20] = Sha1::digest(&blocks).into();
    let mut out = sign(&digest)?;
    out.extend_from_slice(&digest);
    out.extend_from_slice(&blocks);
    Ok(out)
}

// ============================================================
// 作る
// ============================================================

fn build_itdb_files(dir: &Path, db: &ITunesDb, opts: &ItlpOptions, sign: &dyn Fn(&[u8; 20]) -> Result<Vec<u8>>) -> Result<()> {
    let cx = Cx::new(db);
    mk_dynamic(dir, &cx)?;
    mk_extras(dir, &cx)?;
    mk_genius(dir)?;
    mk_library(dir, &cx)?;
    mk_locations(dir, &cx)?;
    if let Some(dict) = opts.post_process {
        run_post_process_commands(dir, dict);
    }
    let cbk = locations_cbk(&fs::read(dir.join("Locations.itdb"))?, sign)?;
    fs::write(dir.join("Locations.itdb.cbk"), cbk)?;
    Ok(())
}

/// DB 全体から決まる値
struct Cx<'a> {
    db: &'a ITunesDb,
    /// mhbd 0x6C (libgpod の tzoffset)
    tzoffset: u32,
}

impl<'a> Cx<'a> {
    fn new(db: &'a ITunesDb) -> Self {
        Cx { db, tzoffset: rd32(&db.header, 0x6C) }
    }
    /// libgpod の timeconv: Core Data の時刻 (2001-01-01 からの秒) - tzoffset。0 (無し) もそのまま計算する
    fn time(&self, mac: u32) -> Value {
        let unix = mac_to_unix(mac);
        Value::Integer((unix - 978_307_200 - self.tzoffset as i64) as u32 as i32 as i64)
    }
}

/// SQLite のエラーを、どのファイルを作っていたかを添えて
fn sql_err(file: &'static str) -> impl Fn(rusqlite::Error) -> Error {
    move |e| Error::Other(t!("SQLite のデータベースを作れません ({file}): {e}", file = file, e = e))
}

fn open_new(dir: &Path, file: &'static str, schema: &str) -> Result<Connection> {
    let path = dir.join(file);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    let conn = Connection::open(&path).map_err(sql_err(file))?;
    conn.execute_batch("PRAGMA synchronous = OFF;").map_err(sql_err(file))?;
    conn.execute_batch(schema).map_err(sql_err(file))?;
    // ANALYZE で、組み込みの SQLite (STAT4 つき) は libgpod の SQLite が作らない sqlite_stat4 も作るので消す
    conn.execute_batch("DROP TABLE IF EXISTS sqlite_stat4;").map_err(sql_err(file))?;
    Ok(conn)
}

fn int(v: impl Into<i64>) -> Value {
    Value::Integer(v.into())
}
fn pid(v: u64) -> Value {
    Value::Integer(v as i64)
}
fn text(s: Option<&str>) -> Value {
    s.map_or(Value::Null, |s| Value::Text(s.to_string()))
}

/// 1 行入れる。`fatal` でなければ失敗は記録だけして続ける (libgpod も多くは表示して続ける)
fn insert(stmt: &mut rusqlite::Statement, vals: &[Value], file: &'static str, fatal: bool) -> Result<()> {
    match stmt.execute(rusqlite::params_from_iter(vals.iter())) {
        Ok(_) => Ok(()),
        Err(e) if fatal => Err(sql_err(file)(e)),
        Err(e) => {
            log::warn!("{file}: {e}");
            Ok(())
        }
    }
}

fn mk_dynamic(dir: &Path, cx: &Cx) -> Result<()> {
    const F: &str = "Dynamic.itdb";
    let conn = open_new(dir, F, DYNAMIC_CREATE)?;
    let e = sql_err(F);
    conn.execute_batch("BEGIN;").map_err(&e)?;
    {
        let mut st = conn
            .prepare("INSERT INTO \"item_stats\" (item_pid,has_been_played,date_played,play_count_user,play_count_recent,date_skipped,skip_count_user,skip_count_recent,bookmark_time_ms,bookmark_time_ms_common,user_rating,user_rating_common) VALUES(?,?,?,?,?,?,?,?,?,?,?,?);")
            .map_err(&e)?;
        for t in cx.db.tracks.iter().filter(|t| t.ipod_path().is_some()) {
            let vals = [
                pid(t.dbid()),
                int(t.playcount() > 0),
                cx.time(t.last_played_mac()),
                int(t.playcount() as i32),
                // 最近の再生・スキップの回数は Play Counts にしか無い (取り込んだあとは 0)
                int(0),
                cx.time(t.last_skipped_mac()),
                int(t.skipcount() as i32),
                int(0),
                Value::Real(t.bookmark_ms() as f64),
                Value::Real(t.bookmark_ms() as f64),
                int(t.rating()),
                int(t.app_rating()),
            ];
            insert(&mut st, &vals, F, false)?;
        }
        let mut st = conn.prepare("INSERT INTO \"container_ui\" VALUES(?,?,?,?,?,?,?);").map_err(&e)?;
        for pl in &cx.db.playlists {
            // play_order は libgpod も意味が分からないまま 24 → 39、ほかは 7 にしている
            let play_order = if pl.sort_order() == 24 { 39 } else { 7 };
            let vals = [pid(pl.id()), int(play_order), int(0), int(1), int(0), int(0), int(0)];
            insert(&mut st, &vals, F, false)?;
        }
    }
    conn.execute_batch("COMMIT;").map_err(&e)?;
    Ok(())
}

fn mk_extras(dir: &Path, cx: &Cx) -> Result<()> {
    const F: &str = "Extras.itdb";
    let conn = open_new(dir, F, EXTRAS_CREATE)?;
    let e = sql_err(F);
    conn.execute_batch("BEGIN;").map_err(&e)?;
    {
        let mut st = conn.prepare("INSERT INTO \"chapter\" VALUES(?,?);").map_err(&e)?;
        // libgpod はどの曲にもチャプタの情報 (空なら章の無い形) を持たせるので、全曲に 1 行ずつ書く
        for t in &cx.db.tracks {
            let blob = chapter_blob(t.chapter_mhod());
            insert(&mut st, &[pid(t.dbid()), Value::Blob(blob)], F, false)?;
        }
    }
    conn.execute_batch("COMMIT;").map_err(&e)?;
    Ok(())
}

fn mk_genius(dir: &Path) -> Result<()> {
    open_new(dir, "Genius.itdb", GENIUS_CREATE)?;
    Ok(())
}

// ---------------- Library.itdb ----------------

/// libgpod の compute_key_orders の項目 (enum sort_order)
#[derive(Clone, Copy)]
enum OrderField {
    Title,
    Artist,
    Genre,
    Composer,
    AlbumArtist,
    AlbumByArtist,
}

/// libgpod の sort_field: 並べ替え用の値が無ければ元の値。空は無いものとして扱う
fn sort_field(t: &Track, f: OrderField) -> Option<&str> {
    let first = |ids: &[u32]| ids.iter().find_map(|&id| t.string(id));
    match f {
        OrderField::Title => first(&[MHOD_SORT_TITLE, MHOD_TITLE]),
        OrderField::Artist => first(&[MHOD_SORT_ARTIST, MHOD_ARTIST]),
        OrderField::Genre => t.genre(),
        OrderField::Composer => first(&[MHOD_SORT_COMPOSER, MHOD_COMPOSER]),
        // libgpod は ORDER_ALBUM_ARTIST にもアーティストを使っている (そのまま)
        OrderField::AlbumArtist => first(&[MHOD_SORT_ARTIST, MHOD_ARTIST]),
        OrderField::AlbumByArtist => first(&[MHOD_SORT_ALBUM_ARTIST, MHOD_ALBUM_ARTIST, MHOD_SORT_ARTIST, MHOD_ARTIST]),
    }
}

/// 値ごとの順位 (100, 200, …)。libgpod と同じく、値の無い曲は 100 (先頭の値と同じ)
struct Orders<'t>(HashMap<&'t str, i64>);

impl<'t> Orders<'t> {
    fn new(tracks: &'t [Track], f: OrderField) -> Self {
        let set: BTreeSet<(String, &str)> = tracks.iter().filter_map(|t| sort_field(t, f)).map(|s| (s.to_lowercase(), s)).collect();
        Orders(set.into_iter().enumerate().map(|(i, (_, s))| (s, (i as i64 + 1) * 100)).collect())
    }
    fn of(&self, t: &Track, f: OrderField) -> i64 {
        match sort_field(t, f) {
            None => 100,
            Some(k) => self.0.get(k).copied().unwrap_or(0),
        }
    }
}

/// アルバムの並べ替えの鍵 (群, 文字列)。並べ替え用のアルバム名があればそれを、無ければアルバム名の頭の
/// 英語の冠詞 (A / An / The) を除いたものを、大文字小文字を区別せずに使う。
/// 群は 0 = 文字で始まる、1 = 数字・記号で始まる (文字の後)、2 = アルバム名なし (最後)
fn album_rank_key(t: &Track) -> (u8, String) {
    let key = match t.string(MHOD_SORT_ALBUM) {
        Some(s) => s.trim().to_lowercase(),
        None => strip_article(&t.album().unwrap_or("").trim().to_lowercase()).to_string(),
    };
    let group = match key.chars().next() {
        None => 2,
        Some(c) if c.is_alphabetic() => 0,
        Some(_) => 1,
    };
    (group, key)
}

fn strip_article(s: &str) -> &str {
    for a in ["the ", "an ", "a "] {
        if let Some(rest) = s.strip_prefix(a).map(str::trim_start).filter(|r| !r.is_empty()) {
            return rest;
        }
    }
    s
}

/// アルバムの順位 (100 刻み)。同じ鍵は同じ順位
fn album_ranks(tracks: &[Track]) -> HashMap<(u8, String), i64> {
    let keys: BTreeSet<(u8, String)> = tracks.iter().map(album_rank_key).collect();
    keys.into_iter().enumerate().map(|(i, k)| (k, (i as i64 + 1) * 100)).collect()
}

/// アルバム・アーティスト・作曲者の pid。名前から決める (libgpod は乱数)
fn derived_pid(library: u64, kind: &str, key: &str) -> u64 {
    let mut h = Sha1::new();
    h.update(library.to_le_bytes());
    h.update(kind.as_bytes());
    h.update([0]);
    h.update(key.as_bytes());
    let d = h.finalize();
    let v = u64::from_le_bytes(d[..8].try_into().expect("8 bytes"));
    if v == 0 { 1 } else { v }
}

fn mk_library(dir: &Path, cx: &Cx) -> Result<()> {
    const F: &str = "Library.itdb";
    let db = cx.db;
    let h = &db.header;
    let library_pid = rd64(h, 0x48);
    let conn = open_new(dir, F, LIBRARY_CREATE)?;
    let e = sql_err(F);
    conn.execute_batch("BEGIN;").map_err(&e)?;

    // version_info。版は ITunesDb::serialize が書く値 (0x30 以上) に合わせる
    let version = rd32(h, 0x10).max(0x30);
    conn.execute(
        "INSERT INTO \"version_info\" VALUES(?,?,?,?,?,?,?);",
        rusqlite::params![1, if version >= 0x28 { 2 } else { 1 }, version as i32, 0, 0, 0, rd16(h, 0x20)],
    )
    .map_err(&e)?;

    // genre_map (曲の順に出てきた順で 1 から)
    let mut genres: HashMap<&str, i64> = HashMap::new();
    {
        let mut st = conn.prepare("INSERT INTO \"genre_map\" (id,genre,genre_order) VALUES(?,?,0);").map_err(&e)?;
        for g in db.tracks.iter().filter_map(|t| t.genre()) {
            if genres.contains_key(g) {
                continue;
            }
            let id = genres.len() as i64 + 1;
            genres.insert(g, id);
            insert(&mut st, &[int(id), text(Some(g))], F, true)?;
        }
    }

    // プレイリスト (container) とその中身 (item_to_container)
    let by_dbid: HashMap<u64, &Track> = db.tracks.iter().map(|t| (t.dbid(), t)).collect();
    {
        let mut st_member = conn.prepare("INSERT INTO \"item_to_container\" VALUES(?,?,?,?);").map_err(&e)?;
        let mut st_container = conn
            .prepare("INSERT INTO \"container\" VALUES(?,?,?,?,:name,?,?,?,?,?,?,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL);")
            .map_err(&e)?;
        for (pos, pl) in db.playlists.iter().enumerate() {
            let mut kinds = 0u32;
            let mut order = 0i64;
            for d in &pl.items {
                let Some(t) = by_dbid.get(d) else { continue };
                insert(&mut st_member, &[pid(*d), pid(pl.id()), int(order), Value::Null], F, false)?;
                order += 1;
                kinds |= t.mediatype();
            }
            let created = cx.time(pl.timestamp_mac());
            let vals = [
                pid(pl.id()),
                // distinguished_kind: ポッドキャストは 11
                int(if pl.is_podcast() { 11 } else { 0 }),
                created.clone(),
                created,
                text(Some(&pl.name)),
                int(pos as i64),
                int(0),
                int(kinds as i32),
                int(0),
                // マスターとポッドキャストは隠す
                int(pl.is_podcast() || pl.is_master()),
                int(0),
            ];
            insert(&mut st_container, &vals, F, false)?;
        }
    }

    // db_info
    let master = db.playlists.iter().find(|p| p.is_master()).ok_or_else(|| Error::Other(t!("マスタープレイリストがありません")))?;
    conn.execute(
        "INSERT INTO \"db_info\" VALUES(?,?,?,?,?,?,?,?);",
        rusqlite::params![
            library_pid as i64,
            master.id() as i64,
            Value::Null,
            rd16(h, 0xA0),
            rd16(h, 0xA2),
            text(db.genius_cuid().as_deref()),
            Value::Null,
            Value::Null
        ],
    )
    .map_err(&e)?;

    // 曲
    let fields = [
        OrderField::Title,
        OrderField::Artist,
        OrderField::Genre,
        OrderField::Composer,
        OrderField::AlbumArtist,
        OrderField::AlbumByArtist,
    ];
    let orders: Vec<Orders> = fields.iter().map(|&f| Orders::new(&db.tracks, f)).collect();
    let order = |t: &Track, i: usize| orders[i].of(t, fields[i]);
    let album_rank = album_ranks(&db.tracks);
    let album_order = |t: &Track| album_rank.get(&album_rank_key(t)).copied().unwrap_or(0);
    {
        let mut st_item = conn
            .prepare(concat!(
                "INSERT INTO \"item\" ",
                "(pid,media_kind,date_modified,year,is_compilation,remember_bookmark,exclude_from_shuffle,artwork_status,artwork_cache_id,",
                "start_time_ms,stop_time_ms,total_time_ms,total_burn_time_ms,track_number,track_count,disc_number,disc_count,",
                "bpm,relative_volume,eq_preset,radio_stream_status,genre_id,album_pid,artist_pid,composer_pid,title,artist,album,",
                "album_artist,composer,sort_title,sort_artist,sort_album,sort_album_artist,sort_composer,title_order,artist_order,",
                "album_order,genre_order,composer_order,album_artist_order,album_by_artist_order,series_name_order,comment,grouping,",
                "description,description_long) ",
                "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?);"
            ))
            .map_err(&e)?;
        let mut st_kind = conn.prepare("INSERT OR IGNORE INTO \"location_kind_map\" VALUES(?,:kind);").map_err(&e)?;
        let mut st_av = conn.prepare("INSERT INTO \"avformat_info\" VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?);").map_err(&e)?;
        let mut st_album = conn.prepare("INSERT OR IGNORE INTO \"album\" VALUES(?,?,?,?,?,?,?,?,?,?,?);").map_err(&e)?;
        let mut st_artist = conn.prepare("INSERT OR IGNORE INTO \"artist\" VALUES(?,?,?,?,?,?,?);").map_err(&e)?;
        let mut st_composer =
            conn.prepare("INSERT OR IGNORE INTO \"composer\" (pid,name,name_order,sort_name) VALUES(?,?,?,?);").map_err(&e)?;
        let mut st_video = conn
            .prepare("INSERT INTO \"video_info\" VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?);")
            .map_err(&e)?;

        for t in &db.tracks {
            let album_pid = t
                .album()
                .map(|a| derived_pid(library_pid, "album", &format!("{a}\0{}", t.album_artist().or(t.artist()).unwrap_or(""))));
            let artist_pid = t.artist().map(|a| derived_pid(library_pid, "artist", a));
            let composer_pid = t.composer().map(|c| derived_pid(library_pid, "composer", c));
            let genre_id = t.genre().and_then(|g| genres.get(g)).copied().unwrap_or(0);
            let first = |ids: &[u32]| text(ids.iter().find_map(|&id| t.string(id)));
            let hdr = &t.hdr;
            let vals = [
                pid(t.dbid()),
                int(t.mediatype() as i32),
                cx.time(t.time_modified_mac()),
                int(t.year() as i32),
                int(hdr[0x1E]),
                int(hdr[0xA6]),
                int(hdr[0xA5]),
                int(t.artwork_flag()),
                int(t.mhii_link() as i32),
                Value::Real(t.start_ms() as f64),
                Value::Real(t.stop_ms() as f64),
                Value::Real(t.length_ms() as i32 as f64),
                Value::Null,
                int(t.track_nr() as i32),
                int(t.track_total() as i32),
                int(t.cd_nr() as i32),
                int(t.cd_total() as i32),
                int(t.bpm()),
                int(t.volume()),
                Value::Null,
                Value::Null,
                int(genre_id),
                pid(album_pid.unwrap_or(0)),
                pid(artist_pid.unwrap_or(0)),
                pid(composer_pid.unwrap_or(0)),
                text(t.title()),
                text(t.artist()),
                text(t.album()),
                text(t.album_artist()),
                text(t.composer()),
                first(&[MHOD_SORT_TITLE, MHOD_TITLE]),
                first(&[MHOD_SORT_ARTIST, MHOD_ARTIST]),
                first(&[MHOD_SORT_ALBUM, MHOD_ALBUM]),
                first(&[MHOD_SORT_ALBUM_ARTIST, MHOD_ALBUM_ARTIST]),
                first(&[MHOD_SORT_COMPOSER, MHOD_COMPOSER]),
                int(order(t, 0)),
                int(order(t, 1)),
                int(album_order(t)),
                int(order(t, 2)),
                int(order(t, 3)),
                int(order(t, 4)),
                int(order(t, 5)),
                // series_name_order: libgpod は番組名を見ていない (常に 100)
                int(100),
                text(t.string(MHOD_COMMENT)),
                text(t.string(MHOD_GROUPING)),
                text(t.string(MHOD_DESCRIPTION)),
                Value::Null,
            ];
            insert(&mut st_item, &vals, F, true)?;

            insert(&mut st_kind, &[int(t.mediatype() as i32), text(t.string(MHOD_FILETYPE))], F, false)?;

            let audio_format = match t.filetype_marker() {
                0x4d50_3320 => 301,                             // "MP3 "
                0x4d34_4120 | 0x4d34_5220 | 0x4d50_3420 => 502, // "M4A " "M4R " "MP4 "
                _ => 0,
            };
            let vals = [
                pid(t.dbid()),
                int(0),
                int(audio_format),
                int(t.bitrate() as i32),
                Value::Real(t.samplerate() as f64),
                int(0),
                int(t.gapless_track_flag()),
                int(t.pregap() as i32),
                int(t.postgap() as i32),
                int(t.gapless_data() as i32),
                int(0),
                int(0),
                int(t.soundcheck() as i32),
            ];
            insert(&mut st_av, &vals, F, false)?;

            if let (Some(a), Some(apid)) = (t.album(), album_pid) {
                let vals = [
                    pid(apid),
                    int(2),
                    int(0),
                    int(0),
                    pid(artist_pid.unwrap_or(0)),
                    int(0),
                    text(Some(a)),
                    int(album_order(t)),
                    int(0),
                    Value::Null,
                    int(0),
                ];
                insert(&mut st_album, &vals, F, false)?;
            }
            if let (Some(a), Some(apid)) = (t.artist(), artist_pid) {
                let vals = [pid(apid), int(2), int(0), int(0), text(Some(a)), int(order(t, 1)), text(Some(a))];
                insert(&mut st_artist, &vals, F, false)?;
            }
            if let (Some(c), Some(cpid)) = (t.composer(), composer_pid) {
                insert(&mut st_composer, &[pid(cpid), text(Some(c)), int(order(t, 3)), text(Some(c))], F, false)?;
            }
            if t.mediatype() & (MEDIA_MOVIE | MEDIA_MUSICVIDEO | MEDIA_TVSHOW) != 0 {
                let vals = [
                    pid(t.dbid()),
                    int(0),
                    int(0),
                    int(0),
                    int(0),
                    int(0),
                    int(0),
                    int(0),
                    int(t.season() as i32),
                    int(0),
                    int(0),
                    int(0),
                    int(0),
                    int(0),
                    int(0),
                    text(t.string(MHOD_TV_SHOW)),
                    text(t.string(MHOD_SORT_TV_SHOW).or(t.string(MHOD_TV_SHOW))),
                    text(t.string(MHOD_TV_EPISODE)),
                    int(((t.season() << 16) | t.episode()) as i32),
                    text(t.string(MHOD_TV_NETWORK)),
                    Value::Null,
                    Value::Null,
                ];
                insert(&mut st_video, &vals, F, false)?;
            }
        }
    }
    if let Err(err) = conn.execute_batch(
        "UPDATE album SET artwork_item_pid = (SELECT item.pid FROM item WHERE item.artwork_cache_id != 0 AND item.album_pid = album.pid LIMIT 1);",
    ) {
        log::warn!("{F}: {err}");
    }
    conn.execute_batch("COMMIT;").map_err(&e)?;
    Ok(())
}

// ---------------- Locations.itdb ----------------

/// iTunesDB の場所 (`:iPod_Control:Music:F00:ABCD.mp3`) を base_location (iPod_Control/Music) からの場所
/// (`F00/ABCD.mp3`) に。libgpod と同じく `:` を `/` にして、3 つ目の `:` の後ろから
fn location_path(ipod_path: &str) -> String {
    let start = ipod_path.match_indices(':').nth(2).map_or(0, |(i, _)| i + 1);
    ipod_path[start..].replace(':', "/")
}

fn mk_locations(dir: &Path, cx: &Cx) -> Result<()> {
    const F: &str = "Locations.itdb";
    let conn = open_new(dir, F, LOCATIONS_CREATE)?;
    let e = sql_err(F);
    conn.execute_batch("BEGIN;").map_err(&e)?;
    conn.execute("INSERT INTO \"base_location\" (id, path) VALUES (?,?);", rusqlite::params![1, "iPod_Control/Music"])
        .map_err(&e)?;
    {
        let mut st = conn
            .prepare("INSERT INTO \"location\" (item_pid, sub_id, base_location_id, location_type, location, extension, kind_id, date_created, file_size) VALUES(?,?,?,?,?,?,?,?,?);")
            .map_err(&e)?;
        for t in &cx.db.tracks {
            let Some(path) = t.ipod_path() else { continue };
            let vals = [
                pid(t.dbid()),
                int(0),
                int(1),
                int(0x4649_4C45), // "FILE"
                Value::Text(location_path(path)),
                int(t.filetype_marker() as i32),
                int(t.mediatype() as i32),
                cx.time(t.time_modified_mac()),
                int(t.size() as i32),
            ];
            insert(&mut st, &vals, F, false)?;
        }
    }
    conn.execute_batch("COMMIT;").map_err(&e)?;
    Ok(())
}

// ---------------- Extras.itdb のチャプタ ----------------

/// チャプタの MHOD (17) から Extras.itdb の chapter.data を作る。libgpod は読んだ章を組み立て直して書くので、
/// ここでも読んでから同じ形で組み立てる (読めない・無いときは章の無い形)
fn chapter_blob(mhod: Option<&[u8]>) -> Vec<u8> {
    let (unk, chapters) = mhod.and_then(parse_chapters).unwrap_or(([0; 3], Vec::new()));
    build_chapter_blob(unk, &chapters)
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// libgpod の get_mhod (MHOD_ID_CHAPTERDATA) と同じ読み方。None = 読めない (libgpod は章の無い形のままにする)
#[allow(clippy::type_complexity)]
fn parse_chapters(m: &[u8]) -> Option<([u32; 3], Vec<(u32, String)>)> {
    let d = m.get(rd32(m, 4) as usize..)?;
    let le = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?)) };
    let unk = [le(0)?, le(4)?, le(8)?];
    let tag = |o: usize, t: &[u8]| d.get(o + 4..o + 8) == Some(t);
    let mut chapters = Vec::new();
    let mut s = 12;
    if tag(s, b"sean") {
        let n = be32(d, s + 12)?.wrapping_sub(1);
        s += 20;
        for _ in 0..n {
            // 章でなければ libgpod は位置を進めないので、残りも読めない
            if !tag(s, b"chap") {
                break;
            }
            let start = be32(d, s + 8)?;
            let children = be32(d, s + 12)?;
            s += 20;
            for _ in 0..children {
                let len = be32(d, s)? as usize;
                if tag(s, b"name") {
                    let n = u16::from_be_bytes(d.get(s + 20..s + 22)?.try_into().ok()?) as usize;
                    let units: Vec<u16> = d
                        .get(s + 22..s + 22 + n * 2)?
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_be_bytes(*c))
                        .take_while(|&u| u != 0)
                        .collect();
                    chapters.push((start.max(1), String::from_utf16_lossy(&units)));
                }
                if len == 0 {
                    break;
                }
                s += len;
            }
        }
    }
    Some((unk, chapters))
}

/// libgpod の itdb_chapterdata_build_chapter_blob_internal と同じ形
fn build_chapter_blob(unk: [u32; 3], chapters: &[(u32, String)]) -> Vec<u8> {
    let mut b = Vec::new();
    let be = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_be_bytes());
    for u in unk {
        b.extend_from_slice(&u.to_le_bytes());
    }
    let atom = b.len();
    be(&mut b, 0);
    b.extend_from_slice(b"sean");
    be(&mut b, 1);
    be(&mut b, chapters.len() as u32 + 1);
    be(&mut b, 0);
    for (start, title) in chapters {
        let units: Vec<u16> = title.encode_utf16().collect();
        let n = units.len() as u32;
        be(&mut b, 42 + 2 * n);
        b.extend_from_slice(b"chap");
        be(&mut b, *start);
        be(&mut b, 1);
        be(&mut b, 0);
        be(&mut b, 22 + 2 * n);
        b.extend_from_slice(b"name");
        be(&mut b, 1);
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&(n as u16).to_be_bytes());
        for u in units {
            b.extend_from_slice(&u.to_be_bytes());
        }
    }
    be(&mut b, 28);
    b.extend_from_slice(b"hedr");
    be(&mut b, 1);
    be(&mut b, 0);
    b.extend_from_slice(&[0; 8]);
    be(&mut b, 1);
    let len = (b.len() - atom) as u32;
    b[atom..atom + 4].copy_from_slice(&len.to_be_bytes());
    b
}

// ============================================================
// 後処理 (SysInfoExtended の SQL)
// ============================================================

/// libgpod の run_post_process_commands。UserVersionCommandSets の一番大きい番号の Commands の順に、
/// SQLCommands の SQL を流す。失敗は記録だけして続ける (libgpod と同じ)
fn run_post_process_commands(dir: &Path, dict: &plist::Dictionary) {
    let (Some(sql_cmds), Some(sets)) = (dict.get("SQLCommands"), dict.get("UserVersionCommandSets")) else {
        log::warn!("post process: SQLCommands / UserVersionCommandSets not found");
        return;
    };
    let mut best: Option<&plist::Value> = None;
    let mut max = 0u64;
    for (k, v) in sets.as_dictionary().into_iter().flatten() {
        let n = strtoul(k);
        if n > 0 && n > max {
            best = Some(v);
            max = n;
        }
    }
    let Some(cmds) = best.and_then(|b| b.as_dictionary()).and_then(|b| b.get("Commands")).and_then(|c| c.as_array()) else {
        log::warn!("post process: no command set");
        return;
    };
    let map: HashMap<&str, &str> = sql_cmds
        .as_dictionary()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.as_str(), v.as_string()?)))
        .collect();

    let conn = match Connection::open(dir.join("Library.itdb")) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("post process: {e}");
            return;
        }
    };
    for other in ["Dynamic.itdb", "Extras.itdb", "Genius.itdb", "Locations.itdb"] {
        let path = dir.join(other).to_string_lossy().replace('\'', "''");
        if let Err(e) = conn.execute_batch(&format!("ATTACH DATABASE '{path}' AS '{other}';")) {
            log::warn!("post process: could not attach {other}: {e}");
        }
    }
    if let Err(e) = register_sort_functions(&conn) {
        log::warn!("post process: {e}");
    }
    let _ = conn.execute_batch("BEGIN;");
    let mut ok = 0;
    for name in cmds.iter().filter_map(|c| c.as_string()) {
        match map.get(name) {
            Some(sql) => match conn.execute_batch(sql) {
                Ok(()) => ok += 1,
                Err(e) => log::warn!("post process: {name}: {e}"),
            },
            None => log::warn!("post process: {name} not found"),
        }
    }
    let _ = conn.execute_batch("COMMIT;");
    log::info!("post process: {ok} / {} commands", cmds.len());
}

/// C の strtoul(s, NULL, 0) (先頭の空白・符号・0x / 0 の基数。読めた所まで)
fn strtoul(s: &str) -> u64 {
    let s = s.trim_start();
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) = match s.get(..2) {
        Some("0x" | "0X") if s[2..].starts_with(|c: char| c.is_ascii_hexdigit()) => (16, &s[2..]),
        _ if s.starts_with('0') => (8, s),
        _ => (10, s),
    };
    let end = digits.find(|c: char| !c.is_digit(radix)).unwrap_or(digits.len());
    if end == 0 {
        return 0;
    }
    let v = u64::from_str_radix(&digits[..end], radix).unwrap_or(u64::MAX);
    if neg { v.wrapping_neg() } else { v }
}

/// 後処理の SQL が使う関数 (iPhoneSortKey・iPhoneSortSection) を登録する
fn register_sort_functions(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function("iPhoneSortKey", 1, FunctionFlags::SQLITE_UTF8, |ctx| {
        Ok(match ctx.get_raw(0) {
            // libgpod は sqlite3_value_text を strlen で読むので、途中の 0 で切る
            ValueRef::Text(b) => Value::Blob(iphone_sort_key(Some(b.split(|&c| c == 0).next().unwrap_or(b)))),
            ValueRef::Null => Value::Blob(vec![0x31, 0x01, 0x01, 0x00]),
            _ => Value::Null,
        })
    })?;
    conn.create_scalar_function("iPhoneSortSection", 1, FunctionFlags::SQLITE_UTF8, |ctx| {
        Ok(match ctx.get_raw(0) {
            ValueRef::Text(b) | ValueRef::Blob(b) => iphone_sort_section(b),
            _ => 26,
        })
    })
}

/// libgpod の sqlite_func_iphone_sort_key (iPhoneSortKey) の計算。`None` は値を文字列にできなかったとき。
/// 英数字は大文字にして 2c - 0x55、空白は 0x06 (語の重みを後ろに置く)、記号は 2 バイト
pub fn iphone_sort_key(sval: Option<&[u8]>) -> Vec<u8> {
    let Some(s) = sval else { return vec![0x31, 0, 0, 0] };
    let (size, wo) = sort_key_boundaries(s);
    let mut buf = vec![0u8; size];
    buf[0] = 0x31;
    if size <= 4 {
        buf[1] = 0x01;
        buf[2] = 0x01;
        return buf;
    }
    let mut put = |i: usize, v: u8| {
        if let Some(b) = buf.get_mut(i) {
            *b = v;
        }
    };
    put(0, 0x30);
    let mut bi = 1;
    let mut word_count = 0usize;
    let mut word_len: i32 = 0;
    for &c in s {
        let c = c.to_ascii_uppercase();
        word_len += 1;
        if c.is_ascii_alphanumeric() {
            put(bi, c.wrapping_add(c).wrapping_sub(0x55));
            bi += 1;
            continue;
        }
        let pair: [u8; 2] = match c {
            b' ' => {
                put(bi, 0x06);
                bi += 1;
                word_len -= 1;
                put(wo + word_count * 2, 0x8f);
                put(wo + word_count * 2 + 1, (0x86 - word_len) as u8);
                word_count += 1;
                word_len = 0;
                continue;
            }
            b':' => [0x07, 0xd8],
            b'-' => [0x07, 0x90],
            b',' => [0x07, 0xb2],
            b'.' => [0x08, 0x51],
            b'\'' => [0x07, 0x31],
            // libgpod も ほかの字はみな "-" と同じにしている
            _ => [0x07, 0x90],
        };
        put(bi, pair[0]);
        put(bi + 1, pair[1]);
        bi += 2;
    }
    put(wo + word_count * 2, 0x8f);
    put(wo + word_count * 2 + 1, (3 + word_len) as u8);
    put(wo - 3, 0x01);
    put(wo - 2, (s.len() + 4) as u8);
    put(wo - 1, 0x01);
    buf
}

/// (全体の長さ, 語の重みを置く位置)
fn sort_key_boundaries(s: &[u8]) -> (usize, usize) {
    if s.is_empty() {
        return (4, 0);
    }
    let mut words = 0;
    let mut l = 0;
    for &c in s {
        if c.is_ascii_alphanumeric() {
            l += 1;
        } else if c == b' ' {
            words += 1;
            l += 1;
        } else {
            l += 2;
        }
    }
    words += 1;
    let o = 1 + l + 3;
    (o + words * 2 + 1, o)
}

/// libgpod の sqlite_func_iphone_sort_section (iPhoneSortSection)。A〜Z は 0〜25、ほかは 26
pub fn iphone_sort_section(v: &[u8]) -> i32 {
    match v {
        [0x30, c, ..] if (0x2D..=0x5F).contains(c) => (*c as i32 - 0x2D) / 2,
        _ => 26,
    }
}

// ============================================================
// ファイル
// ============================================================

/// 写して、iPod に書き終わるまで待つ (Windows では書き込みで開き直さないと sync_all が失敗する)
fn copy_synced(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::copy(src, dst)?;
    fs::OpenOptions::new().write(true).open(dst)?.sync_all()
}

/// 作業用の一時フォルダ (消し忘れないよう Drop で消す)
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<Self> {
        let p = std::env::temp_dir().join(format!("zenpod-itlp-{:016x}", random_id()));
        fs::create_dir_all(&p)?;
        Ok(TempDir(p))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// ============================================================
// 表の定義 (libgpod の itdb_sqlite_queries.h そのまま)
// ============================================================

const DYNAMIC_CREATE: &str = concat!(
    "BEGIN TRANSACTION;",
    "CREATE TABLE item_stats (item_pid INTEGER NOT NULL, has_been_played INTEGER DEFAULT 0, date_played INTEGER DEFAULT 0, play_count_user INTEGER DEFAULT 0, play_count_recent INTEGER DEFAULT 0, date_skipped INTEGER DEFAULT 0, skip_count_user INTEGER DEFAULT 0, skip_count_recent INTEGER DEFAULT 0, bookmark_time_ms REAL, bookmark_time_ms_common REAL, user_rating INTEGER DEFAULT 0, user_rating_common INTEGER DEFAULT 0, rental_expired INTEGER DEFAULT 0, hidden INTEGER DEFAULT 0, deleted INTEGER DEFAULT 0, has_changes INTEGER DEFAULT 0, PRIMARY KEY (item_pid));",
    "CREATE TABLE container_ui (container_pid INTEGER NOT NULL, play_order INTEGER DEFAULT 0, is_reversed INTEGER DEFAULT 0, album_field_order INTEGER DEFAULT 0, repeat_mode INTEGER DEFAULT 0, shuffle_items INTEGER DEFAULT 0, has_been_shuffled INTEGER DEFAULT 0, PRIMARY KEY (container_pid));",
    "CREATE TABLE rental_info (item_pid INTEGER NOT NULL, rental_date_started INTEGER DEFAULT 0, rental_duration INTEGER DEFAULT 0, rental_playback_date_started INTEGER DEFAULT 0, rental_playback_duration INTEGER DEFAULT 0, is_demo INTEGER DEFAULT 0, PRIMARY KEY (item_pid));",
    "COMMIT;"
);

const EXTRAS_CREATE: &str = concat!(
    "BEGIN TRANSACTION;",
    "CREATE TABLE chapter (item_pid INTEGER NOT NULL, data BLOB, PRIMARY KEY (item_pid));",
    "CREATE TABLE lyrics (item_pid INTEGER NOT NULL, checksum INTEGER, lyrics TEXT, PRIMARY KEY (item_pid));",
    "ANALYZE sqlite_master;",
    "CREATE TABLE db_schema_upgrade_info (user_version INTEGER, device_version INTEGER);",
    "INSERT INTO \"db_schema_upgrade_info\" VALUES(8,19);",
    "COMMIT;"
);

const GENIUS_CREATE: &str = concat!(
    "BEGIN TRANSACTION;",
    "CREATE TABLE genius_metadata (genius_id INTEGER NOT NULL, version INTEGER, data BLOB, PRIMARY KEY (genius_id));",
    "CREATE TABLE genius_similarities (genius_id INTEGER NOT NULL, version INTEGER, data BLOB, PRIMARY KEY (genius_id));",
    "CREATE TABLE genius_config (id INTEGER NOT NULL, version INTEGER, default_num_results INTEGER DEFAULT 0, min_num_results INTEGER DEFAULT 0, data BLOB, PRIMARY KEY (id), UNIQUE (version));",
    "COMMIT;"
);

const LIBRARY_CREATE: &str = concat!(
    "BEGIN TRANSACTION;",
    "CREATE TABLE version_info (id INTEGER PRIMARY KEY, major INTEGER, minor INTEGER, compatibility INTEGER DEFAULT 0, update_level INTEGER DEFAULT 0, device_update_level INTEGER DEFAULT 0, platform INTEGER DEFAULT 0);",
    "CREATE TABLE db_info (pid INTEGER NOT NULL, primary_container_pid INTEGER, media_folder_url TEXT, audio_language INTEGER, subtitle_language INTEGER, genius_cuid TEXT, bib BLOB, rib BLOB, PRIMARY KEY (pid));",
    "CREATE TABLE item (pid INTEGER NOT NULL, revision_level INTEGER, media_kind INTEGER DEFAULT 0, is_song INTEGER DEFAULT 0, is_audio_book INTEGER DEFAULT 0, is_music_video INTEGER DEFAULT 0, is_movie INTEGER DEFAULT 0, is_tv_show INTEGER DEFAULT 0, is_ringtone INTEGER DEFAULT 0, is_voice_memo INTEGER DEFAULT 0, is_rental INTEGER DEFAULT 0, is_podcast INTEGER DEFAULT 0, date_modified INTEGER DEFAULT 0, date_backed_up INTEGER DEFAULT 0, year INTEGER DEFAULT 0, content_rating INTEGER DEFAULT 0, content_rating_level INTEGER DEFAULT 0, is_compilation INTEGER, is_user_disabled INTEGER DEFAULT 0, remember_bookmark INTEGER DEFAULT 0, exclude_from_shuffle INTEGER DEFAULT 0, part_of_gapless_album INTEGER DEFAULT 0, artwork_status INTEGER, artwork_cache_id INTEGER DEFAULT 0, start_time_ms REAL DEFAULT 0, stop_time_ms REAL DEFAULT 0, total_time_ms REAL DEFAULT 0, total_burn_time_ms REAL, track_number INTEGER DEFAULT 0, track_count INTEGER DEFAULT 0, disc_number INTEGER DEFAULT 0, disc_count INTEGER DEFAULT 0, bpm INTEGER DEFAULT 0, relative_volume INTEGER, eq_preset TEXT, radio_stream_status TEXT, genius_id INTEGER DEFAULT 0, genre_id INTEGER DEFAULT 0, category_id INTEGER DEFAULT 0, album_pid INTEGER DEFAULT 0, artist_pid INTEGER DEFAULT 0, composer_pid INTEGER DEFAULT 0, title TEXT, artist TEXT, album TEXT, album_artist TEXT, composer TEXT, sort_title TEXT, sort_artist TEXT, sort_album TEXT, sort_album_artist TEXT, sort_composer TEXT, title_order INTEGER, artist_order INTEGER, album_order INTEGER, genre_order INTEGER, composer_order INTEGER, album_artist_order INTEGER, album_by_artist_order INTEGER, series_name_order INTEGER, comment TEXT, grouping TEXT, description TEXT, description_long TEXT, PRIMARY KEY (pid));",
    "CREATE TABLE avformat_info (item_pid INTEGER NOT NULL, sub_id INTEGER NOT NULL DEFAULT 0, audio_format INTEGER, bit_rate INTEGER DEFAULT 0, sample_rate REAL DEFAULT 0, duration INTEGER, gapless_heuristic_info INTEGER, gapless_encoding_delay INTEGER, gapless_encoding_drain INTEGER, gapless_last_frame_resynch INTEGER, analysis_inhibit_flags INTEGER, audio_fingerprint INTEGER, volume_normalization_energy INTEGER, PRIMARY KEY (item_pid,sub_id));",
    "CREATE TABLE video_info (item_pid INTEGER NOT NULL, has_alternate_audio INTEGER, has_subtitles INTEGER, characteristics_valid INTEGER, has_closed_captions INTEGER, is_self_contained INTEGER, is_compressed INTEGER, is_anamorphic INTEGER, season_number INTEGER, audio_language INTEGER, audio_track_index INTEGER, audio_track_id INTEGER, subtitle_language INTEGER, subtitle_track_index INTEGER, subtitle_track_id INTEGER, series_name TEXT, sort_series_name TEXT, episode_id TEXT, episode_sort_id INTEGER, network_name TEXT, extended_content_rating TEXT, movie_info TEXT, PRIMARY KEY (item_pid));",
    "CREATE TABLE video_characteristics (item_pid INTEGER, sub_id INTEGER DEFAULT 0, track_id INTEGER, height INTEGER, width INTEGER, depth INTEGER, codec INTEGER, frame_rate REAL, percentage_encrypted REAL, bit_rate INTEGER, peak_bit_rate INTEGER, buffer_size INTEGER, profile INTEGER, level INTEGER, complexity_level INTEGER, UNIQUE (item_pid,sub_id,track_id));",
    "CREATE TABLE store_info (item_pid INTEGER NOT NULL, store_kind INTEGER, date_purchased INTEGER DEFAULT 0, date_released INTEGER DEFAULT 0, account_id INTEGER, key_versions INTEGER, key_platform_id INTEGER, key_id INTEGER, key_id2 INTEGER, store_item_id INTEGER, artist_id INTEGER, composer_id INTEGER, genre_id INTEGER, playlist_id INTEGER, storefront_id INTEGER, store_link_id INTEGER, relevance REAL, popularity REAL, PRIMARY KEY (item_pid));",
    "CREATE TABLE store_link (id INTEGER NOT NULL, url TEXT, PRIMARY KEY (id));",
    "CREATE TABLE podcast_info (item_pid INTEGER NOT NULL, date_released INTEGER DEFAULT 0, external_guid TEXT, feed_url TEXT, feed_keywords TEXT, PRIMARY KEY (item_pid));",
    "CREATE TABLE container (pid INTEGER NOT NULL, distinguished_kind INTEGER, date_created INTEGER, date_modified INTEGER, name TEXT, name_order INTEGER, parent_pid INTEGER, media_kinds INTEGER, workout_template_id INTEGER, is_hidden INTEGER, smart_is_folder INTEGER, smart_is_dynamic INTEGER, smart_is_filtered INTEGER, smart_is_genius INTEGER, smart_enabled_only INTEGER, smart_is_limited INTEGER, smart_limit_kind INTEGER, smart_limit_order INTEGER, smart_limit_value INTEGER, smart_reverse_limit_order INTEGER, smart_criteria BLOB, description TEXT, PRIMARY KEY (pid));",
    "CREATE TABLE item_to_container (item_pid INTEGER, container_pid INTEGER, physical_order INTEGER, shuffle_order INTEGER);",
    "CREATE TABLE container_seed (container_pid INTEGER NOT NULL, item_pid INTEGER NOT NULL, seed_order INTEGER DEFAULT 0, UNIQUE (container_pid,item_pid));",
    "CREATE TABLE album (pid INTEGER NOT NULL, kind INTEGER, artwork_status INTEGER, artwork_item_pid INTEGER, artist_pid INTEGER, user_rating INTEGER, name TEXT, name_order INTEGER, all_compilations INTEGER, feed_url TEXT, season_number INTEGER, PRIMARY KEY (pid));",
    "CREATE TABLE item_to_album (item_pid INTEGER, album_pid INTEGER);",
    "CREATE TABLE artist (pid INTEGER NOT NULL, kind INTEGER, artwork_status INTEGER, artwork_album_pid INTEGER, name TEXT, name_order INTEGER, sort_name TEXT, PRIMARY KEY (pid));",
    "CREATE TABLE item_to_artist (item_pid INTEGER, artist_pid INTEGER);",
    "CREATE TABLE composer (pid INTEGER NOT NULL, name TEXT, name_order INTEGER, sort_name TEXT, PRIMARY KEY (pid));",
    "CREATE TABLE item_to_composer (item_pid INTEGER, composer_pid INTEGER);",
    "CREATE TABLE location_kind_map (id INTEGER NOT NULL, kind TEXT NOT NULL, PRIMARY KEY (id), UNIQUE (kind));",
    "CREATE TABLE genre_map (id INTEGER NOT NULL, genre TEXT NOT NULL, genre_order INTEGER DEFAULT 0, PRIMARY KEY (id), UNIQUE (genre));",
    "CREATE TABLE category_map (id INTEGER NOT NULL, category TEXT NOT NULL, PRIMARY KEY (id), UNIQUE (category));",
    "CREATE TRIGGER insert_item AFTER INSERT ON item BEGIN ",
    "UPDATE item SET is_song=((new.media_kind&1)!=0), is_audio_book=((new.media_kind&8)!=0), is_music_video=((new.media_kind&32)!=0), is_movie=((new.media_kind&2)!=0), is_tv_show=((new.media_kind&64)!=0), is_ringtone=((new.media_kind&16384)!=0), is_podcast=((new.media_kind&4)!=0), is_rental=((new.media_kind&32768)!=0) WHERE pid = new.pid;",
    "END;",
    "CREATE TRIGGER update_item_media_kind AFTER UPDATE OF media_kind ON item BEGIN ",
    "UPDATE item SET is_song=((new.media_kind&1)!=0), is_audio_book=((new.media_kind&8)!=0), is_music_video=((new.media_kind&32)!=0), is_movie=((new.media_kind&2)!=0), is_tv_show=((new.media_kind&64)!=0), is_ringtone=((new.media_kind&16384)!=0), is_podcast=((new.media_kind&4)!=0), is_rental=((new.media_kind&32768)!=0) WHERE pid = new.pid;",
    "END;",
    "COMMIT;"
);

const LOCATIONS_CREATE: &str = concat!(
    "BEGIN TRANSACTION;",
    "CREATE TABLE location (item_pid INTEGER NOT NULL, sub_id INTEGER NOT NULL DEFAULT 0, base_location_id INTEGER DEFAULT 0, location_type INTEGER, location TEXT, extension INTEGER, kind_id INTEGER DEFAULT 0, date_created INTEGER DEFAULT 0, file_size INTEGER DEFAULT 0, file_creator INTEGER, file_type INTEGER, num_dir_levels_file INTEGER, num_dir_levels_lib INTEGER, PRIMARY KEY (item_pid,sub_id));",
    "CREATE TABLE base_location (id INTEGER NOT NULL, path TEXT, PRIMARY KEY (id));",
    "ANALYZE sqlite_master;",
    "INSERT INTO \"sqlite_stat1\" VALUES('location','sqlite_autoindex_location_1','1 1 1');",
    "COMMIT;"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_key_matches_libgpod() {
        // "AB": 0x30, A→0x2D, B→0x2F, 長さ (01 06 01), 語の重み (8F 05), 終わりの 0
        assert_eq!(iphone_sort_key(Some(b"ab")), [0x30, 0x2D, 0x2F, 0x01, 0x06, 0x01, 0x8F, 0x05, 0x00]);
        // 空白で語が分かれる。1 語目の重みは 0x86 - 長さ
        assert_eq!(
            iphone_sort_key(Some(b"A b")),
            [0x30, 0x2D, 0x06, 0x2F, 0x01, 0x07, 0x01, 0x8F, 0x85, 0x8F, 0x04, 0x00]
        );
        // 記号は 2 バイト、数字は 2c - 0x55
        assert_eq!(iphone_sort_key(Some(b"1.")), [0x30, 0x0D, 0x08, 0x51, 0x01, 0x06, 0x01, 0x8F, 0x05, 0x00]);
        assert_eq!(iphone_sort_key(Some(b"")), [0x31, 0x01, 0x01, 0x00]);
        assert_eq!(iphone_sort_key(None), [0x31, 0x00, 0x00, 0x00]);
        // UTF-8 の字は 1 バイトずつ "-" と同じ扱い
        let k = iphone_sort_key(Some("é".as_bytes()));
        assert_eq!(&k[..5], &[0x30, 0x07, 0x90, 0x07, 0x90]);
        assert_eq!(k.len(), sort_key_boundaries("é".as_bytes()).0);
    }

    #[test]
    fn sort_section() {
        assert_eq!(iphone_sort_section(&iphone_sort_key(Some(b"apple"))), 0);
        assert_eq!(iphone_sort_section(&iphone_sort_key(Some(b"Zoo"))), 25);
        assert_eq!(iphone_sort_section(&iphone_sort_key(Some(b"123"))), 26);
        assert_eq!(iphone_sort_section(&iphone_sort_key(Some(b""))), 26);
        assert_eq!(iphone_sort_section(b"0"), 26);
    }

    #[test]
    fn strtoul_like_c() {
        assert_eq!(strtoul("9"), 9);
        assert_eq!(strtoul(" 12abc"), 12);
        assert_eq!(strtoul("0x10"), 16);
        assert_eq!(strtoul("010"), 8);
        assert_eq!(strtoul("abc"), 0);
        assert_eq!(strtoul(""), 0);
    }

    #[test]
    fn location_paths() {
        assert_eq!(location_path(":iPod_Control:Music:F00:ABCD.mp3"), "F00/ABCD.mp3");
        assert_eq!(location_path(":iPod_Control:Music:F12:日本語.m4a"), "F12/日本語.m4a");
        assert_eq!(location_path("a:b"), "a/b");
    }

    #[test]
    fn chapter_blob_roundtrip() {
        // 章の無い形 (libgpod が全曲に書くもの): 12 + sean 20 + hedr 28
        let empty = chapter_blob(None);
        assert_eq!(empty.len(), 60);
        assert_eq!(&empty[12..20], &[0, 0, 0, 48, b's', b'e', b'a', b'n']);
        // MHOD 17 にした章を読み直すと同じ形になる。開始 0 は 1 に
        let body = build_chapter_blob([1, 2, 3], &[(0, "Intro".into()), (60_000, "第二章".into())]);
        let mut m = vec![0u8; 24];
        m[0..4].copy_from_slice(b"mhod");
        m[4] = 24;
        m[12] = 17;
        m.extend_from_slice(&body);
        let (unk, ch) = parse_chapters(&m).unwrap();
        assert_eq!(unk, [1, 2, 3]);
        assert_eq!(ch, vec![(1, "Intro".to_string()), (60_000, "第二章".to_string())]);
        assert_eq!(chapter_blob(Some(&m)), build_chapter_blob([1, 2, 3], &ch));
    }

    #[test]
    fn album_rank_policy() {
        let mut t = Track::new(1);
        let mut key = |album: Option<&str>, sort: Option<&str>| {
            t.set_string(MHOD_ALBUM, album);
            t.set_string(MHOD_SORT_ALBUM, sort);
            album_rank_key(&t)
        };
        assert_eq!(key(Some("The Zimbo Trio"), None), (0, "zimbo trio".into()));
        assert_eq!(key(Some("A"), None), (0, "a".into()));
        assert_eq!(key(Some("An Apple"), None), (0, "apple".into()));
        assert_eq!(key(Some("Theory"), None), (0, "theory".into()));
        assert_eq!(key(Some("The Zimbo Trio"), Some("Aardvark")), (0, "aardvark".into()));
        assert_eq!(key(Some("123"), None), (1, "123".into()));
        assert_eq!(key(None, None), (2, "".into()));
    }
}
