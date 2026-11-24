//! iTunes / ミュージックの XML (「ライブラリを書き出す」「プレイリストを書き出す」) の読み書きと、.m3u / .m3u8 の読み書き。
//!
//! - 読む: プレイリストと曲を読み、iPod の曲と結び付ける (曲名・アーティスト・アルバム・長さ)。
//!   .m3u / .m3u8 も同じ形 (`XmlLibrary`) にして、取り込みは同じ流れで行う
//! - 書く: スマートプレイリストを、iTunes の「プレイリストを読み込む」で条件ごと戻せる形にする。
//!   プレイリストを .m3u8 にする (別の iPod へ取り込み直したり、ほかのソフトで使ったりする)

use crate::t;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::domain::track::{DupKey, TrackView, dup_key};
use crate::smartpl::{Rules, SmartPrefs};

/// XML の 1 曲 (結び付けに使う項目だけ)
#[derive(Debug, Clone, Default)]
pub struct XmlTrack {
    pub name: String,
    pub artist: String,
    pub album: String,
    pub total_time_ms: u32,
    pub track_number: u32,
    pub disc_number: u32,
    /// PC の場所 (file:// を外して URL デコードしたもの)。Apple Music のストリーミングの曲などは無い
    pub location: Option<String>,
}

#[derive(Debug, Clone)]
pub struct XmlPlaylist {
    /// iTunes の Playlist Persistent ID (無ければ 0)
    pub pid: u64,
    pub name: String,
    /// スマートプレイリストの設定と条件
    pub smart: Option<(SmartPrefs, Rules)>,
    /// 曲 (XML の Track ID)。スマートプレイリストは iTunes が書き出したときの中身
    pub items: Vec<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct XmlLibrary {
    pub tracks: HashMap<u64, XmlTrack>,
    /// 取り込める (ユーザーが作った) プレイリストだけ。ライブラリ・ミュージック・Genius などの組み込みと、フォルダは除く
    pub playlists: Vec<XmlPlaylist>,
}

fn dict_str(d: &plist::Dictionary, k: &str) -> String {
    d.get(k).and_then(|v| v.as_string()).unwrap_or("").to_string()
}

fn dict_u64(d: &plist::Dictionary, k: &str) -> u64 {
    d.get(k)
        .and_then(|v| v.as_unsigned_integer().or_else(|| v.as_signed_integer().map(|i| i as u64)))
        .unwrap_or(0)
}

/// "file://localhost/C:/Users/..%20.m4a" → "C:/Users/.. .m4a" (Mac は "/Users/...")
fn file_url_to_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = rest.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(h);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(out).ok()?;
    // Windows のドライブ ("/C:/...") は先頭の / を外す
    let b = s.as_bytes();
    Some(if b.len() > 2 && b[0] == b'/' && b[2] == b':' { s[1..].to_string() } else { s })
}

/// XML の要素の入れ子の深さの上限 (本物のライブラリは 7 段ほど)
const MAX_DEPTH: usize = 64;

/// plist に渡してよい形か: plist の XML で、入れ子が深すぎない。
/// plist は入れ子を再帰で読み・捨てるので、数千段でスタックがあふれてアプリごと落ちる。
/// その前に quick-xml (再帰しない) で深さだけを数える。XML でないもの (バイナリ・テキストの plist) は読まない
fn plausible_xml(bytes: &[u8]) -> bool {
    use quick_xml::Reader;
    use quick_xml::events::Event;
    let mut r = Reader::from_reader(bytes);
    let mut depth = 0usize;
    let mut root = false;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                root |= depth == 0 && e.local_name().as_ref() == "plist";
                depth += 1;
                if depth > MAX_DEPTH {
                    return false;
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => return root,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

impl XmlLibrary {
    pub fn parse(bytes: &[u8]) -> Result<XmlLibrary> {
        let bad = || Error::Other(t!("iTunes / ミュージックの XML ではありません"));
        if !plausible_xml(bytes) {
            return Err(bad());
        }
        let v: plist::Value = plist::from_bytes(bytes).map_err(|_| bad())?;
        let root = v.as_dictionary().ok_or_else(bad)?;
        let mut lib = XmlLibrary::default();
        if let Some(tracks) = root.get("Tracks").and_then(|t| t.as_dictionary()) {
            for t in tracks.values().filter_map(|t| t.as_dictionary()) {
                let id = dict_u64(t, "Track ID");
                lib.tracks.insert(
                    id,
                    XmlTrack {
                        name: dict_str(t, "Name"),
                        artist: dict_str(t, "Artist"),
                        album: dict_str(t, "Album"),
                        total_time_ms: dict_u64(t, "Total Time") as u32,
                        track_number: dict_u64(t, "Track Number") as u32,
                        disc_number: dict_u64(t, "Disc Number") as u32,
                        location: t
                            .get("Location")
                            .and_then(|l| l.as_string())
                            .and_then(file_url_to_path),
                    },
                );
            }
        }
        let playlists = root.get("Playlists").and_then(|p| p.as_array()).ok_or_else(bad)?;
        for p in playlists.iter().filter_map(|p| p.as_dictionary()) {
            let flag = |k: &str| p.get(k).and_then(|v| v.as_boolean()).unwrap_or(false);
            // ライブラリ・組み込み (ミュージック・ダウンロード済み・Genius・購入した項目など)・フォルダは取り込まない
            if flag("Master")
                || flag("Folder")
                || p.contains_key("Distinguished Kind")
                || p.get("Visible").and_then(|v| v.as_boolean()) == Some(false)
            {
                continue;
            }
            let smart = match (
                p.get("Smart Info").and_then(|v| v.as_data()),
                p.get("Smart Criteria").and_then(|v| v.as_data()),
            ) {
                (Some(info), Some(crit)) => Some((SmartPrefs::from_xml_info(info)?, Rules::parse(crit)?)),
                _ => None,
            };
            let items = p
                .get("Playlist Items")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|i| i.as_dictionary())
                        .map(|i| dict_u64(i, "Track ID"))
                        .collect()
                })
                .unwrap_or_default();
            lib.playlists.push(XmlPlaylist {
                pid: u64::from_str_radix(&dict_str(p, "Playlist Persistent ID"), 16).unwrap_or(0),
                name: dict_str(p, "Name"),
                smart,
                items,
            });
        }
        Ok(lib)
    }
}

/// 結び付けの索引の 1 曲分: (長さ ms, (ディスク, トラック), id)
type MatchEntry = (u32, (u32, u32), u64);

impl XmlLibrary {
    /// ほかのファイルから読んだものを後ろに足す (曲の番号はぶつからないように付け直す)
    pub fn append(&mut self, other: XmlLibrary) {
        // 番号はファイルに書かれた値そのままなので、u64 の端の値でもあふれないようにする
        let base = self.tracks.keys().max().map_or(0, |m| m.saturating_add(1));
        for (id, t) in other.tracks {
            self.tracks.insert(base.saturating_add(id), t);
        }
        for mut p in other.playlists {
            p.items.iter_mut().for_each(|i| *i = i.saturating_add(base));
            self.playlists.push(p);
        }
    }

    /// .m3u / .m3u8 を 1 つのプレイリストとして読む (名前は `#PLAYLIST:` か、無ければファイル名)。
    /// 各行のファイルがあれば `read` でタグを読み、無ければ `#EXTINF` の「アーティスト - 曲名」と長さ
    /// (と、あれば `#EXTART:` のアーティスト・`#EXTALB:` のアルバム) を使う
    pub fn from_m3u(bytes: &[u8], m3u: &Path, read: &dyn Fn(&Path) -> Option<XmlTrack>) -> XmlLibrary {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_start_matches('\u{feff}');
        let dir = m3u.parent().unwrap_or(Path::new(""));
        let mut lib = XmlLibrary::default();
        let mut items = Vec::new();
        let mut name = None;
        let mut extinf: Option<(u32, String)> = None;
        let (mut extart, mut extalb) = (None::<String>, None::<String>);
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if let Some(info) = line.strip_prefix("#EXTINF:") {
                // "#EXTINF:262,KANA-BOON - ないものねだり"
                let (secs, rest) = info.split_once(',').unwrap_or((info, ""));
                let secs = secs.trim().parse::<f64>().ok().filter(|s| *s > 0.0).unwrap_or(0.0);
                extinf = Some(((secs * 1000.0) as u32, rest.to_string()));
                continue;
            }
            if let Some(v) = line.strip_prefix("#EXTART:") {
                extart = Some(v.trim().to_string());
                continue;
            }
            if let Some(v) = line.strip_prefix("#EXTALB:") {
                extalb = Some(v.trim().to_string());
                continue;
            }
            if let Some(v) = line.strip_prefix("#PLAYLIST:") {
                name = Some(v.trim().to_string()).filter(|n| !n.is_empty());
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            let path = if line.starts_with("file://") {
                file_url_to_path(line).map(PathBuf::from)
            } else {
                let p = PathBuf::from(line);
                Some(if p.is_absolute() || line.starts_with('/') || line.starts_with('\\') { p } else { dir.join(p) })
            };
            let (info, artist, album) = (extinf.take(), extart.take(), extalb.take());
            let Some(path) = path else { continue };
            let track = (path.is_file().then(|| read(&path)).flatten())
                .or_else(|| {
                    info.map(|(ms, rest)| {
                        // #EXTART があれば、「アーティスト - 」を外した残りが曲名 (アーティスト名に " - " があっても分けられる)
                        let (artist, title) = match artist {
                            Some(a) => {
                                let title = rest.strip_prefix(a.as_str()).and_then(|r| r.strip_prefix(" - ")).unwrap_or(&rest);
                                (a.clone(), title)
                            }
                            None => rest.split_once(" - ").map_or((String::new(), rest.as_str()), |(a, t)| (a.trim().to_string(), t)),
                        };
                        XmlTrack {
                            name: title.trim().to_string(),
                            artist,
                            album: album.unwrap_or_default(),
                            total_time_ms: ms,
                            ..Default::default()
                        }
                    })
                })
                .unwrap_or_else(|| XmlTrack {
                    name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    ..Default::default()
                });
            let id = lib.tracks.len() as u64;
            lib.tracks.insert(id, XmlTrack { location: Some(path.to_string_lossy().into_owned()), ..track });
            items.push(id);
        }
        lib.playlists.push(XmlPlaylist {
            pid: 0,
            name: name.unwrap_or_else(|| m3u.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            smart: None,
            items,
        });
        lib
    }
}

/// XML の曲を、iPod の曲 (どちらのモードでも) と結び付ける索引
pub struct TrackMatcher {
    /// (曲名, アーティスト, アルバム) ごと
    by_key: HashMap<DupKey, Vec<MatchEntry>>,
    /// (曲名, アーティスト) ごと。アルバムが分からない曲 (.m3u の #EXTINF だけの行) に使う
    by_title: HashMap<(String, String), Vec<MatchEntry>>,
}

impl TrackMatcher {
    pub fn new(tracks: &[TrackView]) -> Self {
        let mut by_key: HashMap<_, Vec<_>> = HashMap::new();
        let mut by_title: HashMap<_, Vec<_>> = HashMap::new();
        for t in tracks {
            let Ok(id) = t.id.parse::<u64>() else { continue };
            let e = (t.duration_ms, (t.disc_nr, t.track_nr), id);
            let (title, artist, album) = dup_key(Some(&t.title), Some(&t.artist), Some(&t.album));
            by_title.entry((title.clone(), artist.clone())).or_default().push(e);
            by_key.entry((title, artist, album)).or_default().push(e);
        }
        TrackMatcher { by_key, by_title }
    }

    /// 同じ曲名・アーティスト・アルバムの曲。いくつもあれば、長さ (2 秒以内) と番号で選ぶ
    pub fn find(&self, x: &XmlTrack) -> Option<u64> {
        let (title, artist, album) = dup_key(Some(&x.name), Some(&x.artist), Some(&x.album));
        let list = if album.is_empty() {
            self.by_title.get(&(title, artist))?
        } else {
            self.by_key.get(&(title, artist, album))?
        };
        let near = |ms: u32| x.total_time_ms == 0 || ms == 0 || ms.abs_diff(x.total_time_ms) <= 2000;
        let num = |n: (u32, u32)| {
            let eq = |a: u32, b: u32| a == 0 || b == 0 || a == b;
            eq(n.0, x.disc_number) && eq(n.1, x.track_number)
        };
        list.iter()
            .find(|(ms, n, _)| near(*ms) && num(*n))
            .or_else(|| list.iter().find(|(ms, ..)| near(*ms)))
            .map(|(.., id)| *id)
    }
}

/// 取り込みの前に見せる、プレイリスト 1 つ分の様子
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCandidate {
    /// 取り込むときに選ぶ番号 (XML の中の順)
    pub index: usize,
    pub name: String,
    pub smart: bool,
    /// XML に入っている曲の数
    pub total: usize,
    /// iPod の曲と結び付いた数
    pub matched: usize,
    /// 見つからない曲 ("曲名 — アーティスト"。多いときは先頭だけ)
    pub missing: Vec<String>,
    pub missing_count: usize,
    /// 見つからない曲のうち、PC にファイルがあって転送できるもの (場所)
    pub transferable: Vec<String>,
    /// iPod に同じプレイリスト (同じ id か同じ名前) があり、上書きになる
    pub exists: bool,
    /// スマートプレイリストを条件ごと取り込む。false なら iTunes が入れた曲のまま固定で取り込む
    pub as_smart: bool,
    /// 開いている iPod で判定できないので、外して取り込む条件の項目 (購入した項目・ラブなど)
    pub dropped_fields: Vec<u32>,
}

/// 見つからない曲を何曲まで名前で見せるか
const MISSING_SHOWN: usize = 100;

impl XmlLibrary {
    /// プレイリストの曲を iPod の曲の id にする (見つからない曲は飛ばし、同じ曲は 1 回だけ)
    pub fn resolve(&self, pl: &XmlPlaylist, m: &TrackMatcher) -> (Vec<u64>, Vec<String>) {
        let (ids, missing) = self.resolve_tracks(pl, m);
        let names = missing
            .iter()
            .map(|x| if x.artist.is_empty() { x.name.clone() } else { format!("{} — {}", x.name, x.artist) })
            .collect();
        (ids, names)
    }

    fn resolve_tracks<'a>(&'a self, pl: &XmlPlaylist, m: &TrackMatcher) -> (Vec<u64>, Vec<&'a XmlTrack>) {
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        let mut missing = Vec::new();
        for tid in &pl.items {
            let Some(x) = self.tracks.get(tid) else { continue };
            match m.find(x) {
                Some(id) => {
                    if seen.insert(id) {
                        ids.push(id);
                    }
                }
                None => missing.push(x),
            }
        }
        (ids, missing)
    }

    /// 取り込みの前の一覧。`existing` は iPod のプレイリスト (id, 名前)、`smart_ok` は条件ごと取り込めるか、
    /// `unavailable` は開いている iPod で判定できない項目 (その条件は外して取り込む)
    pub fn candidates(&self, m: &TrackMatcher, existing: &[(u64, String)], smart_ok: bool, unavailable: &[u32]) -> Vec<ImportCandidate> {
        self.playlists
            .iter()
            .enumerate()
            .map(|(index, pl)| {
                let (ids, missing) = self.resolve(pl, m);
                let as_smart = smart_ok && pl.smart.is_some();
                // スマートプレイリストも、iTunes が入れていた曲 (Playlist Items) のうち iPod に無いものを送れる。
                // 送ったあとに条件で選び直すので、送った曲のタグが条件に合えば入る
                let mut seen = HashSet::new();
                let transferable = self
                    .resolve_tracks(pl, m)
                    .1
                    .into_iter()
                    .filter_map(|x| x.location.clone())
                    .filter(|l| Path::new(l).is_file() && seen.insert(l.clone()))
                    .collect();
                ImportCandidate {
                    index,
                    name: pl.name.clone(),
                    smart: pl.smart.is_some(),
                    total: pl.items.len(),
                    matched: ids.len(),
                    missing_count: missing.len(),
                    missing: missing.into_iter().take(MISSING_SHOWN).collect(),
                    transferable,
                    exists: existing.iter().any(|(id, name)| (pl.pid != 0 && *id == pl.pid) || *name == pl.name),
                    as_smart,
                    dropped_fields: match (&pl.smart, as_smart) {
                        (Some((_, rules)), true) => rules.fields_in(unavailable),
                        _ => Vec::new(),
                    },
                }
            })
            .collect()
    }
}

/// 書き出すスマートプレイリスト 1 つ分
pub struct ExportSmart<'a> {
    pub name: &'a str,
    pub prefs: &'a SmartPrefs,
    pub rules: &'a Rules,
}

/// スマートプレイリストを、iTunes / ミュージックの「プレイリストを読み込む」で条件ごと戻せる XML にする。
/// 曲は入れない (iTunes が自分のライブラリから条件で選ぶ)。Persistent ID も入れない (iTunes 側の別のプレイリストとぶつからないように)。
/// 見出しの値は iTunes 12.13 が書くものと同じ (この形で読み込めることを確かめた)
pub fn export_smart_playlists(pls: &[ExportSmart]) -> Vec<u8> {
    use plist::{Dictionary, Value};
    let mut root = Dictionary::new();
    root.insert("Major Version".into(), Value::Integer(1.into()));
    root.insert("Minor Version".into(), Value::Integer(1.into()));
    root.insert("Application Version".into(), Value::String("12.13.11.1".into()));
    root.insert("Date".into(), Value::Date(std::time::SystemTime::now().into()));
    root.insert("Features".into(), Value::Integer(5.into()));
    root.insert("Show Content Ratings".into(), Value::Boolean(true));
    root.insert("Tracks".into(), Value::Dictionary(Dictionary::new()));
    let list = pls
        .iter()
        .map(|p| {
            let mut d = Dictionary::new();
            d.insert("Name".into(), Value::String(p.name.to_string()));
            d.insert("Description".into(), Value::String(String::new()));
            d.insert("All Items".into(), Value::Boolean(true));
            d.insert("Smart Info".into(), Value::Data(p.prefs.to_xml_info(None)));
            d.insert("Smart Criteria".into(), Value::Data(p.rules.to_bytes()));
            d.insert("Playlist Items".into(), Value::Array(Vec::new()));
            Value::Dictionary(d)
        })
        .collect();
    root.insert("Playlists".into(), Value::Array(list));
    let mut out = Vec::new();
    // XML の書き出しは失敗しない (メモリへ書くだけ)
    let _ = plist::to_writer_xml(&mut out, &Value::Dictionary(root));
    out
}

/// プレイリストを .m3u8 (UTF-8、BOM なし) にする。曲は `XmlTrack` の曲名・アーティスト・アルバム・長さと、`location` (行に書く場所)。
/// ZenPod の取り込みは場所のファイルが無くても `#EXTINF` / `#EXTART` / `#EXTALB` で曲を結び付けられる
pub fn m3u8_text(name: &str, tracks: &[XmlTrack]) -> String {
    // 改行が入ると行の区切りがずれるので空白にする
    let one_line = |s: &str| s.replace(['\r', '\n'], " ");
    let mut out = format!("#EXTM3U\n#PLAYLIST:{}\n", one_line(name));
    for t in tracks {
        // 長さが分からなければ -1 (拡張 M3U の決まり)
        let secs = if t.total_time_ms > 0 { (t.total_time_ms as i64 + 500) / 1000 } else { -1 };
        let label = if t.artist.is_empty() { one_line(&t.name) } else { format!("{} - {}", one_line(&t.artist), one_line(&t.name)) };
        out.push_str(&format!("#EXTINF:{secs},{label}\n"));
        if !t.artist.is_empty() {
            out.push_str(&format!("#EXTART:{}\n", one_line(&t.artist)));
        }
        if !t.album.is_empty() {
            out.push_str(&format!("#EXTALB:{}\n", one_line(&t.album)));
        }
        out.push_str(&one_line(t.location.as_deref().unwrap_or_default()));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Tracks</key><dict>
    <key>10</key><dict><key>Track ID</key><integer>10</integer><key>Name</key><string>ないものねだり</string><key>Artist</key><string>KANA-BOON</string><key>Album</key><string>僕がCDを出したら - EP</string><key>Total Time</key><integer>262693</integer><key>Track Number</key><integer>1</integer>
      <key>Location</key><string>file://localhost/C:/Users/me/Music/01%20%E3%81%AA%E3%81%84.m4a</string></dict>
    <key>11</key><dict><key>Track ID</key><integer>11</integer><key>Name</key><string>Missing Song</string><key>Artist</key><string>Nobody</string></dict>
  </dict>
  <key>Playlists</key><array>
    <dict><key>Name</key><string>ライブラリ</string><key>Master</key><true/><key>Visible</key><false/><key>Playlist Items</key><array><dict><key>Track ID</key><integer>10</integer></dict></array></dict>
    <dict><key>Name</key><string>ミュージック</string><key>Distinguished Kind</key><integer>4</integer></dict>
    <dict><key>Name</key><string>お気に入り</string><key>Playlist Persistent ID</key><string>E16748E03448B57C</string>
      <key>Playlist Items</key><array><dict><key>Track ID</key><integer>10</integer></dict><dict><key>Track ID</key><integer>11</integer></dict><dict><key>Track ID</key><integer>10</integer></dict></array></dict>
    <dict><key>Name</key><string>フォルダ</string><key>Folder</key><true/></dict>
  </array>
</dict></plist>"#;

    fn view(id: u64, title: &str, artist: &str, album: &str, ms: u32) -> TrackView {
        TrackView {
            id: id.to_string(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_ms: ms,
            track_nr: 1,
            ..Default::default()
        }
    }

    #[test]
    fn parse_and_match() {
        let lib = XmlLibrary::parse(LIB.as_bytes()).unwrap();
        // 組み込みとフォルダは読み飛ばす
        assert_eq!(lib.playlists.len(), 1);
        let pl = &lib.playlists[0];
        assert_eq!(pl.pid, 0xE16748E03448B57C);
        assert_eq!(lib.tracks[&10].location.as_deref(), Some("C:/Users/me/Music/01 ない.m4a"));
        let ipod = [
            view(500, "ないものねだり", "kana-boon", "僕がCDを出したら - EP", 263_000),
            // 長さが違う同じ名前の曲 (別の版) は選ばない
            view(501, "ないものねだり", "KANA-BOON", "僕がCDを出したら - EP", 300_000),
        ];
        let m = TrackMatcher::new(&ipod);
        let (ids, missing) = lib.resolve(pl, &m);
        assert_eq!(ids, [500]);
        assert_eq!(missing, ["Missing Song — Nobody"]);
        let c = lib.candidates(&m, &[(1, "お気に入り".into())], true, &[]);
        assert!(c[0].exists && !c[0].as_smart);
        assert_eq!((c[0].total, c[0].matched, c[0].missing_count), (3, 1, 1));
    }

    #[test]
    fn m3u_playlists() {
        let dir = std::env::temp_dir().join(format!("zenpod-m3u-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/a.mp3"), b"x").unwrap();
        let m3u = dir.join("ドライブ用.m3u8");
        let text = "\u{feff}#EXTM3U\n#EXTINF:263,KANA-BOON - ないものねだり\nsub/a.mp3\n#EXTINF:200,Nobody - Gone\n/nowhere/gone.mp3\n";
        // ファイルがある行はタグを読む (ここでは決まった値を返す)
        let read = |p: &Path| {
            assert!(p.ends_with("sub/a.mp3"));
            Some(XmlTrack { name: "ないものねだり".into(), artist: "KANA-BOON".into(), album: "僕がCDを出したら - EP".into(), total_time_ms: 262_693, ..Default::default() })
        };
        let mut lib = XmlLibrary::from_m3u(text.as_bytes(), &m3u, &read);
        assert_eq!(lib.playlists[0].name, "ドライブ用");
        assert_eq!(lib.playlists[0].items.len(), 2);
        // 2 つ目のファイルとまとめても、曲の番号はぶつからない
        lib.append(XmlLibrary::from_m3u(b"/nowhere/x.mp3\n", &dir.join("b.m3u"), &|_| None));
        assert_eq!(lib.tracks.len(), 3);
        assert_eq!(lib.playlists[1].items, [2]);
        assert_eq!(lib.tracks[&2].name, "x");

        let ipod = [view(500, "ないものねだり", "KANA-BOON", "僕がCDを出したら - EP", 263_000), view(501, "Gone", "Nobody", "Some Album", 200_000)];
        let m = TrackMatcher::new(&ipod);
        let c = lib.candidates(&m, &[], true, &[]);
        // 1 行目はタグで、2 行目は #EXTINF の「アーティスト - 曲名」で (アルバム無しで) 見つかる
        assert_eq!((c[0].matched, c[0].missing_count), (2, 0));
        assert_eq!(c[1].missing_count, 1);
        // 無いファイルは転送できない
        assert!(c[1].transferable.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn m3u8_round_trip() {
        let tracks = [
            XmlTrack { name: "Song - Live".into(), artist: "A - B".into(), album: "Live".into(), total_time_ms: 262_600, location: Some("/Music/A - B/Live/01.mp3".into()), ..Default::default() },
            XmlTrack { name: "No Artist".into(), location: Some("/iPod_Control/Music/F00/ABCD.m4a".into()), ..Default::default() },
        ];
        let text = m3u8_text("ドライブ/夜", &tracks);
        assert_eq!(
            text,
            "#EXTM3U\n#PLAYLIST:ドライブ/夜\n#EXTINF:263,A - B - Song - Live\n#EXTART:A - B\n#EXTALB:Live\n/Music/A - B/Live/01.mp3\n#EXTINF:-1,No Artist\n/iPod_Control/Music/F00/ABCD.m4a\n"
        );
        // 場所のファイルが無くても、名前・アーティスト・アルバム・長さで読める
        let lib = XmlLibrary::from_m3u(text.as_bytes(), Path::new("/nowhere/x.m3u8"), &|_| None);
        assert_eq!(lib.playlists[0].name, "ドライブ/夜");
        let t = &lib.tracks[&lib.playlists[0].items[0]];
        assert_eq!((t.name.as_str(), t.artist.as_str(), t.album.as_str(), t.total_time_ms), ("Song - Live", "A - B", "Live", 263_000));
        let t = &lib.tracks[&lib.playlists[0].items[1]];
        assert_eq!((t.name.as_str(), t.artist.as_str(), t.total_time_ms), ("No Artist", "", 0));
        // 同じ曲名・アーティストでもアルバムで選べる
        let ipod = [view(500, "Song - Live", "A - B", "Studio", 263_000), view(501, "Song - Live", "A - B", "Live", 263_000)];
        let m = TrackMatcher::new(&ipod);
        assert_eq!(m.find(&lib.tracks[&lib.playlists[0].items[0]]), Some(501));
    }

    #[test]
    fn transferable_tracks() {
        let dir = std::env::temp_dir().join(format!("zenpod-xfer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.m4a"), b"x").unwrap();
        let url = format!("file://localhost{}", dir.join("a.m4a").to_string_lossy());
        let xml = format!(
            r#"<plist version="1.0"><dict><key>Tracks</key><dict>
              <key>1</key><dict><key>Track ID</key><integer>1</integer><key>Name</key><string>A</string><key>Location</key><string>{url}</string></dict>
              <key>2</key><dict><key>Track ID</key><integer>2</integer><key>Name</key><string>B</string><key>Location</key><string>file://localhost/nowhere/b.m4a</string></dict>
            </dict><key>Playlists</key><array><dict><key>Name</key><string>P</string><key>Playlist Items</key><array>
              <dict><key>Track ID</key><integer>1</integer></dict><dict><key>Track ID</key><integer>2</integer></dict><dict><key>Track ID</key><integer>1</integer></dict>
            </array></dict></array></dict></plist>"#
        );
        let lib = XmlLibrary::parse(xml.as_bytes()).unwrap();
        let c = lib.candidates(&TrackMatcher::new(&[]), &[], true, &[]);
        assert_eq!(c[0].transferable, [dir.join("a.m4a").to_string_lossy().into_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn smart_playlist_tracks_can_be_sent() {
        let dir = std::env::temp_dir().join(format!("zenpod-smartxfer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.m4a"), b"x").unwrap();
        let mut lib = XmlLibrary::default();
        lib.tracks.insert(1, XmlTrack { name: "A".into(), artist: "KANA-BOON".into(), ..Default::default() });
        lib.tracks.insert(
            2,
            XmlTrack { name: "B".into(), artist: "KANA-BOON".into(), location: Some(dir.join("b.m4a").to_string_lossy().into_owned()), ..Default::default() },
        );
        let rules = Rules { any: false, items: Vec::new() };
        lib.playlists.push(XmlPlaylist { pid: 1, name: "KB".into(), smart: Some((SmartPrefs::default(), rules)), items: vec![1, 2] });
        let ipod = [view(500, "A", "KANA-BOON", "", 0)];
        let c = lib.candidates(&TrackMatcher::new(&ipod), &[], true, &[]);
        // 条件ごと取り込むが、iTunes が入れていた曲のうち iPod に無い B は送れる
        assert!(c[0].as_smart);
        assert_eq!((c[0].matched, c[0].missing_count), (1, 1));
        assert_eq!(c[0].transferable, [dir.join("b.m4a").to_string_lossy().into_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn smart_round_trip_through_xml() {
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/itunes12-smart-playlists.plist"));
        let v: plist::Value = plist::from_bytes(bytes).unwrap();
        let src = v.as_dictionary().unwrap()["Playlists"].as_array().unwrap()[0].as_dictionary().unwrap();
        let prefs = SmartPrefs::from_xml_info(src["Smart Info"].as_data().unwrap()).unwrap();
        let rules = Rules::parse(src["Smart Criteria"].as_data().unwrap()).unwrap();
        let xml = export_smart_playlists(&[ExportSmart { name: "テスト", prefs: &prefs, rules: &rules }]);
        let back = XmlLibrary::parse(&xml).unwrap();
        assert_eq!(back.playlists.len(), 1);
        assert_eq!(back.playlists[0].name, "テスト");
        let (p, r) = back.playlists[0].smart.as_ref().unwrap();
        assert_eq!((p, r), (&prefs, &rules));
        assert_eq!(r.to_bytes(), src["Smart Criteria"].as_data().unwrap());
    }
}
