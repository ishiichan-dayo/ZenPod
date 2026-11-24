//! Rockbox のデータベース (tagcache、`.rockbox/database_*.tcd`) を PC で作る。
//!
//! Rockbox は本体でファイルのタグを読んでデータベースを作るが、曲が多いと何時間もかかる。
//! 同じ形式のファイルを PC で書いておけば、本体は読むだけで済む。
//!
//! 形式は Rockbox 4.0 (apps/tagcache.c、ヘッダの版 0x54434810) に合わせる。コードは移植ではなく、
//! 形式だけを読んで書いた。正しさは Rockbox の PC 用ツール (tools/database) の出力と中身を突き合わせて確かめる。
//!
//! - `database_idx.tcd`: 曲ごとの固定長の項目 (23 個の i32 と flag)。文字列の項目はタグのファイルの中の位置、
//!   数値の項目 (年・トラック番号・長さ・再生回数など) は値そのもの
//! - `database_<n>.tcd`: 文字列の項目ごとのファイル。n は項目の番号 (0〜8 と 12)。
//!   ファイル名 (4) 以外は大文字小文字を区別せずに並べ、曲名 (3) 以外は同じ文字列を 1 つにまとめる
//! - 数値はすべてリトルエンディアン (iPod の ARM も同じ)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::pc::folder::PcTrack;

/// ヘッダの版 ('TCH' + 0x10)。Rockbox 4.0 以降
pub const MAGIC: i32 = 0x5443_4810;
const TAG_COUNT: usize = 23;
const ENTRY_SIZE: usize = TAG_COUNT * 4 + 4;
const MASTER_HEADER_SIZE: usize = 24;
const TAG_HEADER_SIZE: usize = 12;
/// タグの文字列の長さの上限 (MAX_PATH * 2)。これより長いものは切る (ファイル名なら入れない)
const TAG_MAXLEN: usize = 520;
const UNTAGGED: &str = "<Untagged>";
/// 並べるファイルの項目は 8 バイト単位に詰め物をする
const CHUNK: usize = 8;

// 項目の番号 (tagcache.h の enum tag_type)
const ARTIST: usize = 0;
const ALBUM: usize = 1;
const GENRE: usize = 2;
const TITLE: usize = 3;
const FILENAME: usize = 4;
const COMPOSER: usize = 5;
const COMMENT: usize = 6;
const ALBUMARTIST: usize = 7;
const GROUPING: usize = 8;
const YEAR: usize = 9;
const DISCNUMBER: usize = 10;
const TRACKNUMBER: usize = 11;
const CANONICALARTIST: usize = 12;
const BITRATE: usize = 13;
const LENGTH: usize = 14;
const PLAYCOUNT: usize = 15;
const RATING: usize = 16;
const PLAYTIME: usize = 17;
const LASTPLAYED: usize = 18;
const COMMITID: usize = 19;
const MTIME: usize = 20;
const LASTELAPSED: usize = 21;
const LASTOFFSET: usize = 22;

/// 文字列の項目 (ファイルがある項目)
const STRING_TAGS: [usize; 10] = [ARTIST, ALBUM, GENRE, TITLE, FILENAME, COMPOSER, COMMENT, ALBUMARTIST, GROUPING, CANONICALARTIST];
/// 本体で再生して増える記録 (作り直しても前のデータベースから引き継ぐ)
const STAT_TAGS: [usize; 7] = [PLAYCOUNT, RATING, PLAYTIME, LASTPLAYED, COMMITID, LASTELAPSED, LASTOFFSET];

/// データベースのファイルを書くときの鍵 (裏の作り直しと、評価の書き換えがぶつからないように)
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const FLAG_DELETED: i32 = 0x0001;
const FLAG_DIRTYNUM: i32 = 0x0004;
/// トラック番号がタグに無く、本体がファイル名から作った
const FLAG_TRKNUMGEN: i32 = 0x0008;

/// Rockbox がタグを読んでデータベースに入れる拡張子 (lib/rbcodec/metadata/metadata.c の audio_formats) のうち、
/// こちら (lofty) でもタグを読めるもの
const ROCKBOX_EXTS: &[&str] = &[
    "mpa", "mp2", "mp3", "mp1", "aiff", "aif", "wav", "ogg", "oga", "flac", "mpc", "wv", "m4a", "m4b", "mp4", "spx", "ape",
    "mac", "opus",
];
/// Rockbox はデータベースに入れるが、こちらではタグを読めない拡張子 (本体の自動更新に任せる)
const DEVICE_ONLY_EXTS: &[&str] = &[
    "wma", "wmv", "asf", "at3", "a52", "ac3", "shn", "sid", "adx", "nsf", "nsfe", "spc", "mod", "sap", "rm", "ra", "rmvb",
    "cmc", "cm3", "cmr", "cms", "dmc", "dlt", "mpt", "mpd", "rmt", "tmc", "tm8", "tm2", "oma", "aa3", "smaf", "mmf", "au",
    "snd", "vox", "w64", "tta", "ay", "gbs", "hes", "sgc", "vgm", "vgz", "kss", "vtx", "mid", "midi", "aac",
];

/// データベースに入れる 1 曲
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    /// iPod のルートからのパス ("/Music/Artist/Album/01 Title.flac")
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub composer: String,
    pub comment: String,
    pub album_artist: String,
    pub grouping: String,
    pub year: i32,
    pub disc: i32,
    pub track: i32,
    pub length_ms: i32,
    pub bitrate: i32,
    /// 本体が見る更新日時 (FAT の日時を UTC とみなした秒)
    pub mtime: i32,
}

/// 読み取ったデータベースの 1 曲 (確認と引き継ぎ用)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DbEntry {
    /// 文字列の項目 (STRING_TAGS の番号 → 文字列)
    pub strings: HashMap<usize, String>,
    /// すべての項目の値 (文字列の項目はファイルの中の位置)
    pub seek: [i32; TAG_COUNT],
    pub flag: i32,
}

impl DbEntry {
    fn s(&self, tag: usize) -> &str {
        self.strings.get(&tag).map(String::as_str).unwrap_or("")
    }
}

/// 読み取ったデータベース
#[derive(Debug, Clone, Default)]
pub struct Db {
    pub serial: i32,
    pub commitid: i32,
    pub dirty: bool,
    /// 消した印の付いた曲も含む
    pub entries: Vec<DbEntry>,
}

/// 作った結果
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Built {
    pub tracks: usize,
    /// 再生回数などを前のデータベースから引き継いだ曲
    pub kept_stats: usize,
    /// タグを読めず入れなかったファイル (本体の自動更新に任せる)
    pub skipped: usize,
}

fn rd(b: &[u8], off: usize) -> Option<i32> {
    b.get(off..off + 4).map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Rockbox の strncasecmp: 英字だけ小文字にして、符号なしのバイトで比べる (ARM の char は符号なし)
fn rb_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| s.bytes().map(|c| c.to_ascii_lowercase()).collect::<Vec<u8>>();
    // <Untagged> は常に先頭
    match (a == UNTAGGED, b == UNTAGGED) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => key(a).cmp(&key(b)),
    }
}

/// 空なら <Untagged>、長すぎれば切る (文字の途中では切らない)
fn tag_str(s: &str) -> String {
    let s = s.trim_end_matches('\0');
    if s.is_empty() {
        return UNTAGGED.into();
    }
    let mut end = s.len().min(TAG_MAXLEN);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn is_unique(tag: usize) -> bool {
    !matches!(tag, TITLE | FILENAME)
}

/// 曲の文字列の項目 (Rockbox の add_tagcache と同じ決まり)
fn strings_of(e: &Entry) -> [(usize, String); 10] {
    // アーティストが無ければアルバムアーティストを、グループが無ければ曲名を使う
    let canon = if e.artist.is_empty() { &e.album_artist } else { &e.artist };
    let grouping = if e.grouping.is_empty() { &e.title } else { &e.grouping };
    [
        (ARTIST, tag_str(&e.artist)),
        (ALBUM, tag_str(&e.album)),
        (GENRE, tag_str(&e.genre)),
        (TITLE, tag_str(&e.title)),
        (FILENAME, e.path.clone()),
        (COMPOSER, tag_str(&e.composer)),
        (COMMENT, tag_str(&e.comment)),
        (ALBUMARTIST, tag_str(&e.album_artist)),
        (GROUPING, tag_str(grouping)),
        (CANONICALARTIST, tag_str(canon)),
    ]
}

/// タグのファイル 1 つを作る。戻り値は (ファイルの中身, 曲ごとの位置)
fn build_tag_file(tag: usize, values: &[String]) -> (Vec<u8>, Vec<i32>) {
    let mut out = vec![0u8; TAG_HEADER_SIZE];
    let mut seeks = vec![0i32; values.len()];
    let mut count: i32 = 0;
    if tag == FILENAME {
        // 並べずに曲の順。詰め物なし
        for (i, v) in values.iter().enumerate() {
            seeks[i] = out.len() as i32;
            out.extend_from_slice(&((v.len() + 1) as i32).to_le_bytes());
            out.extend_from_slice(&(i as i32).to_le_bytes());
            out.extend_from_slice(v.as_bytes());
            out.push(0);
            count += 1;
        }
    } else {
        // (文字列, その文字列を使う曲)。まとめる項目は、大文字小文字だけ違うものも同じとみなし、最初に出たつづりを使う
        let mut slots: Vec<(String, Vec<usize>)> = Vec::new();
        let mut index: HashMap<Vec<u8>, usize> = HashMap::new();
        for (i, v) in values.iter().enumerate() {
            if is_unique(tag) {
                let key: Vec<u8> = v.bytes().map(|c| c.to_ascii_lowercase()).collect();
                if let Some(&s) = index.get(&key) {
                    slots[s].1.push(i);
                    continue;
                }
                index.insert(key, slots.len());
            }
            slots.push((v.clone(), vec![i]));
        }
        slots.sort_by(|a, b| rb_cmp(&a.0, &b.0));
        for (v, users) in &slots {
            let pos = out.len() as i32;
            let len = v.len() + 1;
            // 項目の頭 (8 バイト) と合わせて 8 の倍数にする
            let padded = (len + 8).div_ceil(CHUNK) * CHUNK - 8;
            out.extend_from_slice(&(padded as i32).to_le_bytes());
            let idx_id = if is_unique(tag) { -1 } else { users[0] as i32 };
            out.extend_from_slice(&idx_id.to_le_bytes());
            out.extend_from_slice(v.as_bytes());
            out.push(0);
            out.extend(std::iter::repeat_n(b'X', padded - len));
            for &u in users {
                seeks[u] = pos;
            }
            count += 1;
        }
    }
    let datasize = (out.len() - TAG_HEADER_SIZE) as i32;
    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&datasize.to_le_bytes());
    out[8..12].copy_from_slice(&count.to_le_bytes());
    (out, seeks)
}

/// データベースのファイルを作る (ファイル名 → 中身)。`old` は前のデータベース (再生回数などを引き継ぐ)。
/// 戻り値の 2 つ目は引き継いだ曲の数
pub fn build(entries: &[Entry], old: Option<&Db>) -> (Vec<(String, Vec<u8>)>, usize) {
    let n = entries.len();
    let strings: Vec<[(usize, String); 10]> = entries.iter().map(strings_of).collect();
    let mut idx = vec![[0i32; TAG_COUNT]; n];
    let mut flags = vec![0i32; n];
    let mut files = Vec::new();
    let mut datasize_sum = 0i64;
    for (k, &tag) in STRING_TAGS.iter().enumerate() {
        let values: Vec<String> = strings.iter().map(|s| s[k].1.clone()).collect();
        let (bytes, seeks) = build_tag_file(tag, &values);
        if tag != FILENAME {
            datasize_sum += (bytes.len() - TAG_HEADER_SIZE) as i64;
        }
        for (i, s) in seeks.into_iter().enumerate() {
            idx[i][tag] = s;
        }
        files.push((format!("database_{tag}.tcd"), bytes));
    }

    // 前のデータベースの記録: まずファイル名、無ければアーティスト・アルバム・曲名・長さが同じ曲 (ファイルを動かしたとき)
    let mut by_path: HashMap<&str, &DbEntry> = HashMap::new();
    let mut by_meta: HashMap<(&str, &str, &str, i32), &DbEntry> = HashMap::new();
    if let Some(db) = old {
        for e in db.entries.iter().filter(|e| e.flag & FLAG_DELETED == 0) {
            by_path.entry(e.s(FILENAME)).or_insert(e);
            by_meta.entry((e.s(ARTIST), e.s(ALBUM), e.s(TITLE), e.seek[LENGTH])).or_insert(e);
        }
    }
    let old_commit = old.map(|d| d.commitid).unwrap_or(0);
    let mut kept = 0;
    for (i, e) in entries.iter().enumerate() {
        let v = &mut idx[i];
        v[YEAR] = e.year;
        v[DISCNUMBER] = e.disc;
        v[TRACKNUMBER] = if e.track <= 0 { -1 } else { e.track };
        v[LENGTH] = e.length_ms;
        v[BITRATE] = e.bitrate;
        v[MTIME] = e.mtime;
        let s = &strings[i];
        let prev = by_path
            .get(e.path.as_str())
            .or_else(|| by_meta.get(&(s[0].1.as_str(), s[1].1.as_str(), s[3].1.as_str(), e.length_ms)))
            .filter(|p| STAT_TAGS.iter().any(|&t| p.seek[t] != 0));
        if let Some(p) = prev {
            for t in STAT_TAGS {
                v[t] = p.seek[t];
            }
            kept += 1;
        }
        if v[COMMITID] != 0 {
            flags[i] |= FLAG_DIRTYNUM;
        } else if old_commit > 0 {
            // 本体と同じく、新しく入った曲には今の commit の番号を付ける
            v[COMMITID] = old_commit;
            flags[i] |= FLAG_DIRTYNUM;
        }
    }

    let mut master = Vec::with_capacity(MASTER_HEADER_SIZE + n * ENTRY_SIZE);
    let datasize = MASTER_HEADER_SIZE as i64 + (ENTRY_SIZE * n) as i64 + datasize_sum;
    master.extend_from_slice(&MAGIC.to_le_bytes());
    master.extend_from_slice(&(datasize as i32).to_le_bytes());
    master.extend_from_slice(&(n as i32).to_le_bytes());
    master.extend_from_slice(&old.map(|d| d.serial).unwrap_or(0).to_le_bytes());
    master.extend_from_slice(&old_commit.saturating_add(1).to_le_bytes());
    master.extend_from_slice(&0i32.to_le_bytes());
    for (v, f) in idx.iter().zip(&flags) {
        for x in v {
            master.extend_from_slice(&x.to_le_bytes());
        }
        master.extend_from_slice(&f.to_le_bytes());
    }
    files.push(("database_idx.tcd".into(), master));
    (files, kept)
}

/// データベースを読む。無い・形式が違う (Rockbox の版が違う) ときは None
pub fn read(dir: &Path) -> Option<Db> {
    read_with(dir, &STRING_TAGS, false)
}

/// 作り直すときに要る分だけ読む: 再生回数などの記録がある曲と、その照合に使う項目 (ファイル名・アーティスト・アルバム・曲名) だけ。
/// 記録のある曲が 1 つも無ければ、タグのファイルは読まない (曲が多いと 20MB ほどあるので)
fn read_for_stats(dir: &Path) -> Option<Db> {
    read_with(dir, &[ARTIST, ALBUM, TITLE, FILENAME], true)
}

fn read_with(dir: &Path, want: &[usize], stats_only: bool) -> Option<Db> {
    let m = fs::read(dir.join("database_idx.tcd")).ok()?;
    if rd(&m, 0)? != MAGIC {
        return None;
    }
    let count = rd(&m, 8)?.max(0) as usize;
    // 数は壊れていることがあるので、ファイルに入りきる数までしか先に確保しない
    let cap = count.min(m.len().saturating_sub(MASTER_HEADER_SIZE) / ENTRY_SIZE);
    let mut db = Db { serial: rd(&m, 12)?, commitid: rd(&m, 16)?, dirty: rd(&m, 20)? != 0, entries: Vec::with_capacity(cap) };
    let mut entries = Vec::with_capacity(cap);
    for i in 0..count {
        let base = MASTER_HEADER_SIZE + i * ENTRY_SIZE;
        let mut e = DbEntry::default();
        for (t, seek) in e.seek.iter_mut().enumerate() {
            *seek = rd(&m, base + t * 4)?;
        }
        e.flag = rd(&m, base + TAG_COUNT * 4)?;
        if stats_only && (e.flag & FLAG_DELETED != 0 || STAT_TAGS.iter().all(|&t| e.seek[t] == 0)) {
            continue;
        }
        entries.push(e);
    }
    if stats_only && entries.is_empty() {
        return Some(db);
    }
    let tags: HashMap<usize, Vec<u8>> = want
        .iter()
        .filter_map(|&t| {
            let b = fs::read(dir.join(format!("database_{t}.tcd"))).ok()?;
            (rd(&b, 0)? == MAGIC).then_some((t, b))
        })
        .collect();
    if tags.len() != want.len() {
        return None;
    }
    for mut e in entries {
        if e.flag & FLAG_DELETED == 0 {
            for &t in want {
                let b = &tags[&t];
                let pos = e.seek[t].max(0) as usize;
                let len = rd(b, pos)?.max(0) as usize;
                let data = b.get(pos + 8..pos + 8 + len)?;
                let end = data.iter().position(|&c| c == 0).unwrap_or(data.len());
                e.strings.insert(t, String::from_utf8_lossy(&data[..end]).into_owned());
            }
        }
        db.entries.push(e);
    }
    Some(db)
}

/// データベースにある曲の記録。評価は 0〜10 (★ 1 つが 2)、再生回数は本体が再生したときに数えたもの
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub rating: u8,
    pub play_count: u32,
}

/// データベースにある評価と再生回数。キーは iPod の中の場所 ("/Music/…") を小文字にしたもの。どちらも無い曲は入れない
/// データベースの曲の場所 (`FILENAME`) と、曲の場所 (`rel_path`) を突き合わせる鍵。
/// FAT は大文字小文字を区別しないので小文字にし、濁点・アクセントの形 (NFC / NFD) の違いも同じとみなす
pub fn path_key(path: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    path.nfc().collect::<String>().to_lowercase()
}

pub fn stats(root: &Path) -> HashMap<String, Stats> {
    let Some(db) = read_with(&db_dir(root), &[FILENAME], true) else { return HashMap::new() };
    db.entries
        .iter()
        .filter(|e| e.flag & FLAG_DELETED == 0)
        .map(|e| (path_key(e.s(FILENAME)), Stats { rating: e.seek[RATING].clamp(0, 10) as u8, play_count: e.seek[PLAYCOUNT].max(0) as u32 }))
        .filter(|(_, s)| *s != Stats::default())
        .collect()
}

/// データベースの曲の評価を書き換える (作り直さずに、その曲の数字だけ)。`ratings` のキーは `stats` と同じ。
/// 戻り値はデータベースで見つからなかった曲 (まだデータベースに入っていない曲)
pub fn set_ratings(root: &Path, ratings: &HashMap<String, u8>) -> Result<Vec<String>> {
    let dir = db_dir(root);
    let _g = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(db) = read_with(&dir, &[FILENAME], false) else { return Ok(ratings.keys().cloned().collect()) };
    let idx = dir.join("database_idx.tcd");
    let mut m = fs::read(&idx)?;
    let mut found = std::collections::HashSet::new();
    for (i, e) in db.entries.iter().enumerate() {
        if e.flag & FLAG_DELETED != 0 {
            continue;
        }
        let key = path_key(e.s(FILENAME));
        let Some(&r) = ratings.get(&key) else { continue };
        let base = MASTER_HEADER_SIZE + i * ENTRY_SIZE;
        m[base + RATING * 4..base + RATING * 4 + 4].copy_from_slice(&(r.min(10) as i32).to_le_bytes());
        // 数字の項目を変えた印 (本体が変更の記録に書き出すときに使う)
        let flag = rd(&m, base + TAG_COUNT * 4).unwrap_or(0) | FLAG_DIRTYNUM;
        m[base + TAG_COUNT * 4..base + TAG_COUNT * 4 + 4].copy_from_slice(&flag.to_le_bytes());
        found.insert(key);
    }
    if !found.is_empty() {
        let tmp = dir.join("database_idx.tcd.zenpod-tmp");
        fs::write(&tmp, &m)?;
        fs::rename(&tmp, &idx)?;
    }
    Ok(ratings.keys().filter(|k| !found.contains(*k)).cloned().collect())
}

/// データベースにある曲を、タグを読んだ結果の代わりに使える形にする (Rockbox の iPod を初めて開くとき、
/// 1 曲ずつファイルを開かずに済むように。HDD の iPod では 1 曲に数十ミリ秒かかる)。
/// キーは iPod の中の場所を小文字にしたもの ("music/…"、先頭の / なし)、値は (本体が見た更新日時, 曲)。
/// 使う側は更新日時が今のファイルと同じ曲だけ使う。データベースに無い項目 (並べ替え用のタグ・埋め込みのジャケットの有無) は
/// 空のままなので、あとでファイルから読み直す
pub fn seed_tracks(root: &Path) -> HashMap<String, (i32, PcTrack)> {
    let Some(db) = read(&db_dir(root)) else { return HashMap::new() };
    let text = |e: &DbEntry, tag: usize| match e.s(tag) {
        UNTAGGED => String::new(),
        s => s.to_string(),
    };
    let num = |e: &DbEntry, tag: usize| e.seek[tag].max(0) as u32;
    db.entries
        .iter()
        .filter(|e| e.flag & FLAG_DELETED == 0)
        .filter_map(|e| {
            let path = e.s(FILENAME);
            let key = path.trim_start_matches('/').to_lowercase();
            if key.is_empty() {
                return None;
            }
            let title = text(e, TITLE);
            // 本体はグループが無ければ曲名を入れる
            let grouping = Some(text(e, GROUPING)).filter(|g| *g != title).unwrap_or_default();
            let t = PcTrack {
                path: String::new(),
                // タグが無ければファイル名を曲名にする (タグを読んだときと同じ)
                title: if title.is_empty() {
                    Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                } else {
                    title.clone()
                },
                title_tagged: !title.is_empty(),
                artist: text(e, ARTIST),
                album: text(e, ALBUM),
                album_artist: text(e, ALBUMARTIST),
                genre: text(e, GENRE),
                composer: text(e, COMPOSER),
                comment: text(e, COMMENT),
                grouping,
                duration_ms: num(e, LENGTH),
                track_nr: if e.flag & FLAG_TRKNUMGEN != 0 { 0 } else { num(e, TRACKNUMBER) },
                disc_nr: num(e, DISCNUMBER),
                year: num(e, YEAR),
                bitrate_kbps: num(e, BITRATE),
                size: 0,
                release_type: String::new(),
                sort_title: String::new(),
                sort_artist: String::new(),
                sort_album: String::new(),
                sort_album_artist: String::new(),
                has_picture: false,
                from_db: true,
                read_error: String::new(),
            };
            Some((key, (e.seek[MTIME], t)))
        })
        .collect()
}

/// iPod のファイルの更新日時を、本体が見る値にする。
/// FAT は現地時刻で日時を持ち、Rockbox はそれをそのまま UTC として秒にする (2 秒単位)
pub fn device_mtime(t: std::time::SystemTime) -> i32 {
    use chrono::{DateTime, Local, Timelike};
    let local: DateTime<Local> = t.into();
    let naive = local.naive_local();
    let naive = naive.with_second(naive.second() / 2 * 2).unwrap_or(naive).with_nanosecond(0).unwrap_or(naive);
    naive.and_utc().timestamp() as i32
}

/// `.rockbox/config.cfg` の設定 1 つ
fn config_value(root: &Path, key: &str) -> Option<String> {
    let cfg = fs::read_to_string(root.join(".rockbox").join("config.cfg")).ok()?;
    cfg.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

/// 本体の場所 ("/.rockbox") を PC の場所にする。iPod の外を指す場所 ("/../x"・"C:\x" など) は None
fn device_path(root: &Path, p: &str) -> Option<PathBuf> {
    crate::domain::devpath::join_inside(root, p, crate::domain::devpath::DEVICE_SEPS)
}

/// データベースを置くフォルダ (設定の「database path」、既定は /.rockbox。使えない場所なら既定)
pub fn db_dir(root: &Path) -> PathBuf {
    config_value(root, "database path")
        .filter(|v| !v.is_empty())
        .and_then(|v| device_path(root, &v))
        .unwrap_or_else(|| root.join(".rockbox"))
}

/// Rockbox と同じ決まりで、データベースに入れるファイルを探す。
/// 探すのは設定の「database scan paths」(既定は "/"、":" で区切る)。フォルダに database.ignore があれば
/// その下は入れず、database.unignore があればまた入れる (両方あれば親のまま)。
/// 戻り値の 2 つ目は、Rockbox は入れるがこちらではタグを読めないファイルの数。
/// 更新日時と大きさはフォルダを読んだときの情報 (Windows ではファイルごとの問い合わせが要らない) を使う
pub fn scan_files(root: &Path) -> (Vec<ScanFile>, usize) {
    let paths = config_value(root, "database scan paths").filter(|v| !v.is_empty()).unwrap_or_else(|| "/".into());
    let mut out = Vec::new();
    let mut skipped = 0;
    let mut roots: Vec<String> = Vec::new();
    for p in paths.split(':').map(str::trim).filter(|p| !p.is_empty()) {
        // 入れ子の場所は一度だけ
        if roots.iter().any(|r| p.starts_with(r.as_str())) {
            continue;
        }
        // iPod の外を指す場所は探さない
        let Some(dir) = device_path(root, p) else { continue };
        roots.push(p.to_string());
        walk(&dir, true, &mut out, &mut skipped);
    }
    // 使える場所が 1 つも無ければ既定 ("/")
    if roots.is_empty() {
        walk(root, true, &mut out, &mut skipped);
    }
    (out, skipped)
}

/// データベースに入れるファイル 1 つ
pub struct ScanFile {
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
    pub size: u64,
}

fn walk(dir: &Path, add: bool, out: &mut Vec<ScanFile>, skipped: &mut usize) {
    let ext_of = |name: &std::ffi::OsStr| Path::new(name).extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let Ok(mut items) = crate::dirlist::read(dir, &|name| ROCKBOX_EXTS.contains(&ext_of(name).as_str())) else { return };
    // 本体は FAT のフォルダの順に読むが、順は中身に関係しない (並べ替えるので)。見やすいよう名前順にする
    items.sort_by(|a, b| a.name.cmp(&b.name));
    // database.ignore / unignore は一覧の名前で見る (FAT は大文字小文字を区別しない)
    let has = |name: &str| items.iter().any(|e| e.name.to_string_lossy().eq_ignore_ascii_case(name));
    let (ignore, unignore) = (has("database.ignore"), has("database.unignore"));
    let add = if ignore != unignore { unignore } else { add };
    for e in items {
        let path = dir.join(&e.name);
        match e.kind {
            crate::dirlist::Kind::Dir => walk(&path, add, out, skipped),
            crate::dirlist::Kind::File if add => {
                let ext = ext_of(&e.name);
                if ROCKBOX_EXTS.contains(&ext.as_str()) {
                    out.push(ScanFile { path, modified: e.modified, size: e.len });
                } else if DEVICE_ONLY_EXTS.contains(&ext.as_str()) {
                    *skipped += 1;
                }
            }
            _ => {}
        }
    }
}

/// iPod のルートからのパス ("/Music/...")。本体はファイル名をバイトのまま比べるので、iPod の中の名前のとおりに書く。
/// macOS は FAT の名前 (iPod の中は NFC) を NFD で返すので、macOS だけ NFC に戻す。
/// Windows / Linux はファイル名をそのまま返すので変えない (NFD の名前のファイルを NFC で書くと、本体で見つからない)
pub fn rel_path(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let parts: Vec<String> = rel
        .components()
        .map(|c| {
            let name = c.as_os_str().to_string_lossy();
            #[cfg(target_os = "macos")]
            let name: String = {
                use unicode_normalization::UnicodeNormalization;
                name.nfc().collect()
            };
            name.into()
        })
        .collect();
    format!("/{}", parts.join("/"))
}

/// 読み取った曲をデータベースの 1 曲にする。`modified` はファイルの更新日時
pub fn entry_of(root: &Path, t: &PcTrack, modified: std::time::SystemTime) -> Entry {
    let path = Path::new(&t.path);
    let mtime = device_mtime(modified);
    Entry {
        path: rel_path(root, path),
        title: if t.title_tagged { t.title.clone() } else { String::new() },
        artist: t.artist.clone(),
        album: t.album.clone(),
        genre: t.genre.clone(),
        composer: t.composer.clone(),
        comment: t.comment.clone(),
        album_artist: t.album_artist.clone(),
        grouping: t.grouping.clone(),
        year: t.year as i32,
        disc: t.disc_nr as i32,
        track: t.track_nr as i32,
        length_ms: t.duration_ms as i32,
        bitrate: t.bitrate_kbps as i32,
        mtime,
    }
}

/// iPod にあるデータベースの曲の数 (索引のヘッダだけ読む)。無い・形式が違うときは None
pub fn entry_count(root: &Path) -> Option<usize> {
    use std::io::Read;
    let mut h = [0u8; 12];
    fs::File::open(db_dir(root).join("database_idx.tcd")).ok()?.read_exact(&mut h).ok()?;
    (rd(&h, 0)? == MAGIC).then(|| rd(&h, 8).unwrap_or(0).max(0) as usize)
}

/// このデータベースを書いてよい Rockbox か。前のデータベースが同じ版なら書く。
/// 無ければ rockbox-info.txt の版が 4.0 以降のとき (開発版 "abcdef-250101" のような版は今の形式とみなす)
pub fn supported(root: &Path) -> bool {
    use std::io::Read;
    let mut h = [0u8; 4];
    if fs::File::open(db_dir(root).join("database_idx.tcd")).and_then(|mut f| f.read_exact(&mut h)).is_ok() {
        return rd(&h, 0) == Some(MAGIC);
    }
    let Some(info) = crate::rockbox::installed(root) else { return false };
    let v = info.version;
    match v.split('.').next().and_then(|m| m.parse::<u32>().ok()) {
        Some(major) => major >= 4,
        None => !v.is_empty(),
    }
}

/// ファイルを探してタグを読み (`cache` はタグの読み取りのキャッシュ)、データベースを作って iPod に書く
pub fn rebuild(root: &Path, cache: Option<&Path>) -> Result<Built> {
    if !supported(root) {
        return Err(Error::Other(crate::t!("この Rockbox の版のデータベースには対応していません")));
    }
    let (files, skipped) = scan_files(root);
    write(root, &files, cache, skipped)
}

/// `files` (`scan_files` で探したもの) のタグを読み (`cache` はタグの読み取りのキャッシュ)、データベースを作って iPod に書く。
/// 前のファイルは初回だけ `zenpod-db-backup` に控える
pub fn write(root: &Path, files: &[ScanFile], cache: Option<&Path>, skipped: usize) -> Result<Built> {
    if !supported(root) {
        return Err(Error::Other(crate::t!("この Rockbox の版のデータベースには対応していません")));
    }
    let dir = db_dir(root);
    fs::create_dir_all(&dir)?;
    let _g = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stat: Vec<_> = files.iter().map(|f| (f.path.clone(), f.modified, f.size)).collect();
    let tracks = crate::pc::folder::scan_device_files(stat, cache, root, |_, _| {});
    let modified: HashMap<&Path, std::time::SystemTime> = files.iter().map(|f| (f.path.as_path(), f.modified)).collect();
    let entries: Vec<Entry> = tracks
        .iter()
        .map(|t| entry_of(root, t, modified.get(Path::new(&t.path)).copied().unwrap_or(std::time::UNIX_EPOCH)))
        // 本体はパスが長すぎる曲を入れない
        .filter(|e| e.path.len() <= TAG_MAXLEN)
        .collect();
    let old = read_for_stats(&dir);
    let (files, kept) = build(&entries, old.as_ref());

    let backup = dir.join("zenpod-db-backup");
    crate::device::adopt_legacy_name(&backup, "ipodsync-db-backup");
    if old.is_some() && !backup.exists() {
        fs::create_dir_all(&backup)?;
        for (name, _) in &files {
            let src = dir.join(name);
            if src.is_file() {
                fs::copy(&src, backup.join(name))?;
            }
        }
    }
    // 一時ファイルに書いてから置き換える。途中で止まっても本体が壊れた DB を使わないよう、
    // 置き換えの間は今の索引に「作りかけ」(dirty) の印を付け、索引は最後に置き換える
    for (name, bytes) in &files {
        fs::write(dir.join(format!("{name}.zenpod-tmp")), bytes)?;
    }
    let idx = dir.join("database_idx.tcd");
    if let Ok(mut f) = fs::OpenOptions::new().write(true).open(&idx) {
        use std::io::{Seek, SeekFrom, Write};
        f.seek(SeekFrom::Start(20))?;
        f.write_all(&1i32.to_le_bytes())?;
    }
    for (name, _) in files.iter().filter(|(n, _)| n != "database_idx.tcd") {
        fs::rename(dir.join(format!("{name}.zenpod-tmp")), dir.join(name))?;
    }
    fs::rename(dir.join("database_idx.tcd.zenpod-tmp"), &idx)?;
    // 本体の読みかけ (次の起動で今の DB に足される) は、作り直したので要らない
    let _ = fs::remove_file(dir.join("database_tmp.tcd"));
    Ok(Built { tracks: entries.len(), kept_stats: kept, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, title: &str, artist: &str, album: &str) -> Entry {
        Entry { path: path.into(), title: title.into(), artist: artist.into(), album: album.into(), length_ms: 1000, ..Default::default() }
    }

    fn write_to(dir: &Path, files: &[(String, Vec<u8>)]) {
        for (n, b) in files {
            fs::write(dir.join(n), b).unwrap();
        }
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let es = vec![
            entry("/Music/b.mp3", "Song B", "beta", "X"),
            entry("/Music/a.mp3", "Song A", "Beta", "X"),
            Entry { path: "/Music/c.flac".into(), track: 0, ..Default::default() },
        ];
        let (files, _) = build(&es, None);
        write_to(dir.path(), &files);
        let db = read(dir.path()).unwrap();
        assert_eq!(db.entries.len(), 3);
        assert_eq!(db.commitid, 1);
        let by: HashMap<&str, &DbEntry> = db.entries.iter().map(|e| (e.s(FILENAME), e)).collect();
        // 大文字小文字だけ違うアーティストは、最初に出たつづりにまとまる
        assert_eq!(by["/Music/a.mp3"].s(ARTIST), "beta");
        assert_eq!(by["/Music/a.mp3"].seek[ARTIST], by["/Music/b.mp3"].seek[ARTIST]);
        assert_eq!(by["/Music/c.flac"].s(TITLE), UNTAGGED);
        assert_eq!(by["/Music/c.flac"].seek[TRACKNUMBER], -1);
        // アーティストが無ければアルバムアーティスト、グループが無ければ曲名
        assert_eq!(by["/Music/b.mp3"].s(GROUPING), "Song B");
        assert_eq!(by["/Music/b.mp3"].s(CANONICALARTIST), "beta");
    }

    #[test]
    fn sorted_and_padded() {
        let es = vec![entry("/1", "b", "zz", ""), entry("/2", "A", "", ""), entry("/3", "c", "Aa", "")];
        let (files, _) = build(&es, None);
        let artist = &files.iter().find(|(n, _)| n == "database_0.tcd").unwrap().1;
        // <Untagged> が先頭、あとは大文字小文字を区別せずに並ぶ。各項目は 8 バイトの倍数
        let mut pos = TAG_HEADER_SIZE;
        let mut names = Vec::new();
        while pos < artist.len() {
            let len = rd(artist, pos).unwrap() as usize;
            assert_eq!((len + 8) % 8, 0);
            assert_eq!(rd(artist, pos + 4).unwrap(), -1);
            let data = &artist[pos + 8..pos + 8 + len];
            names.push(String::from_utf8_lossy(&data[..data.iter().position(|&c| c == 0).unwrap()]).into_owned());
            pos += 8 + len;
        }
        assert_eq!(names, [UNTAGGED, "Aa", "zz"]);
    }

    #[test]
    fn keeps_stats() {
        let dir = tempfile::tempdir().unwrap();
        let (files, _) = build(&[entry("/Music/a.mp3", "A", "x", "y")], None);
        write_to(dir.path(), &files);
        // 本体で再生した記録を書き込んだことにする
        let mut m = fs::read(dir.path().join("database_idx.tcd")).unwrap();
        let base = MASTER_HEADER_SIZE;
        m[base + PLAYCOUNT * 4..base + PLAYCOUNT * 4 + 4].copy_from_slice(&5i32.to_le_bytes());
        m[base + COMMITID * 4..base + COMMITID * 4 + 4].copy_from_slice(&1i32.to_le_bytes());
        fs::write(dir.path().join("database_idx.tcd"), m).unwrap();
        let old = read(dir.path()).unwrap();
        // ファイルを動かしても、アーティスト・アルバム・曲名・長さが同じなら引き継ぐ
        let (files, kept) = build(&[entry("/Music/new/a.mp3", "A", "x", "y"), entry("/Music/b.mp3", "B", "x", "y")], Some(&old));
        assert_eq!(kept, 1);
        write_to(dir.path(), &files);
        let db = read(dir.path()).unwrap();
        assert_eq!(db.commitid, 2);
        assert_eq!(db.entries[0].seek[PLAYCOUNT], 5);
        assert_eq!(db.entries[0].flag & FLAG_DIRTYNUM, FLAG_DIRTYNUM);
        // 新しい曲には前の commit の番号
        assert_eq!(db.entries[1].seek[COMMITID], 1);
    }

    /// 本体が数えた再生回数と評価を、iPod の中の場所 (小文字) で引ける。記録の無い曲は入らない
    #[test]
    fn reads_stats() {
        let root = tempfile::tempdir().unwrap();
        let dir = db_dir(root.path());
        fs::create_dir_all(&dir).unwrap();
        let (files, _) = build(&[entry("/Music/A.mp3", "A", "x", "y"), entry("/Music/b.mp3", "B", "x", "y")], None);
        write_to(&dir, &files);
        let mut m = fs::read(dir.join("database_idx.tcd")).unwrap();
        let base = MASTER_HEADER_SIZE;
        m[base + PLAYCOUNT * 4..base + PLAYCOUNT * 4 + 4].copy_from_slice(&7i32.to_le_bytes());
        m[base + RATING * 4..base + RATING * 4 + 4].copy_from_slice(&6i32.to_le_bytes());
        fs::write(dir.join("database_idx.tcd"), m).unwrap();
        let st = stats(root.path());
        assert_eq!(st.len(), 1);
        assert_eq!(st.get("/music/a.mp3"), Some(&Stats { rating: 6, play_count: 7 }));
    }

    /// 作り直すときは、記録のある曲と照合に使う項目だけ読む。記録が無ければタグのファイルは読まない
    #[test]
    fn reads_only_what_stats_need() {
        let dir = tempfile::tempdir().unwrap();
        let (files, _) = build(&[entry("/a.mp3", "A", "x", "y"), entry("/b.mp3", "B", "x", "y")], None);
        write_to(dir.path(), &files);
        let db = read_for_stats(dir.path()).unwrap();
        assert!(db.entries.is_empty());
        // タグのファイルが無くても読める (読まないので)
        fs::remove_file(dir.path().join("database_6.tcd")).unwrap();
        let mut m = fs::read(dir.path().join("database_idx.tcd")).unwrap();
        let base = MASTER_HEADER_SIZE + ENTRY_SIZE;
        m[base + RATING * 4..base + RATING * 4 + 4].copy_from_slice(&8i32.to_le_bytes());
        fs::write(dir.path().join("database_idx.tcd"), m).unwrap();
        let db = read_for_stats(dir.path()).unwrap();
        assert_eq!(db.entries.len(), 1);
        assert_eq!(db.entries[0].s(FILENAME), "/b.mp3");
        assert_eq!(db.entries[0].s(TITLE), "B");
        assert_eq!(db.entries[0].seek[RATING], 8);
    }

    /// データベースの曲を一覧用の形に戻す: <Untagged> は空、曲名が無ければファイル名、
    /// 本体が曲名で埋めたグループと、ファイル名から作ったトラック番号は使わない
    #[test]
    fn seeds_tracks_from_database() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut a = entry("/Music/A/X/01 One.mp3", "One", "A", "X");
        a.year = 2001;
        a.track = 1;
        a.mtime = 1234;
        a.album_artist = "AA".into();
        let mut b = entry("/Music/B/02 Two.flac", "", "", "");
        b.grouping = "G".into();
        let (files, _) = build(&[a, b], None);
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        write_to(&root.join(".rockbox"), &files);
        // 1 曲目に、トラック番号をファイル名から作った印を付ける
        let idx = root.join(".rockbox/database_idx.tcd");
        let mut m = fs::read(&idx).unwrap();
        let db = read(&root.join(".rockbox")).unwrap();
        let i = db.entries.iter().position(|e| e.s(FILENAME) == "/Music/A/X/01 One.mp3").unwrap();
        let flag = MASTER_HEADER_SIZE + i * ENTRY_SIZE + TAG_COUNT * 4;
        m[flag..flag + 4].copy_from_slice(&FLAG_TRKNUMGEN.to_le_bytes());
        fs::write(&idx, m).unwrap();

        let seeds = seed_tracks(root);
        let (mtime, one) = &seeds["music/a/x/01 one.mp3"];
        assert_eq!(*mtime, 1234);
        assert_eq!((one.title.as_str(), one.artist.as_str(), one.album.as_str(), one.album_artist.as_str()), ("One", "A", "X", "AA"));
        assert_eq!((one.year, one.track_nr, one.duration_ms, one.grouping.as_str()), (2001, 0, 1000, ""));
        assert!(one.title_tagged && one.from_db);
        let (_, two) = &seeds["music/b/02 two.flac"];
        assert_eq!((two.title.as_str(), two.title_tagged, two.artist.as_str(), two.genre.as_str()), ("02 Two", false, "", ""));
        assert_eq!(two.grouping, "G");
    }

    #[test]
    fn mtime_is_local_wall_clock() {
        use chrono::{Local, TimeZone};
        let t = Local.with_ymd_and_hms(2026, 9, 28, 12, 34, 57).unwrap();
        let sys: std::time::SystemTime = t.into();
        // 秒は 2 秒単位に切り捨て、現地時刻をそのまま UTC として数える
        let want = chrono::NaiveDate::from_ymd_opt(2026, 9, 28).unwrap().and_hms_opt(12, 34, 56).unwrap().and_utc().timestamp();
        assert_eq!(device_mtime(sys) as i64, want);
    }
}
