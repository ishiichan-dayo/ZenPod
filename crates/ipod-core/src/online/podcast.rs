//! ポッドキャストの購読。番組のフィード (RSS) を読み、回を PC に落としてから、ふつうの曲と同じ道で iPod に送る。
//!
//! - 購読の一覧と回ごとの状態 (聞いたか・落としたファイル) は `podcasts.json` (アプリのデータの場所) に置く
//! - 落とした回には、番組名をアルバム、回の名前を曲名、ポッドキャストの印を書き込む。iPod の曲との対応は
//!   「分類がポッドキャストで、アルバムと曲名が同じ曲」で取る (iPod OS と Rockbox のどちらでも同じ見方で済む)
//! - フィードを取りに行くのは、登録したときと、画面で更新したとき (起動時は前回から時間がたっていれば) だけ。
//!   ETag / Last-Modified を送り、変わっていなければ本文を受け取らない

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::t;

const USER_AGENT: &str = concat!("ZenPod/", env!("CARGO_PKG_VERSION"), " ( https://github.com/ishiichan-dayo/ZenPod )");
/// フィードの本文の上限 (長く続いている番組は数 MB になる)
const MAX_FEED_BYTES: u64 = 32 * 1024 * 1024;
/// 番組の画像の上限
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
/// 回のファイルの上限 (超えたら止めて消す。おかしな配信元で PC の空きを使い切らないように)
const MAX_EPISODE_BYTES: u64 = 4 << 30;
/// 回を落とすとき PC に残しておく空き
const KEEP_FREE_BYTES: u64 = 512 << 20;
/// フィードと画像の本文を受け取る時間 (ureq の recv_body は無通信の時間ではなく、ヘッダを受け取ってからの合計)
const BODY_TIMEOUT: Duration = Duration::from_secs(60);
/// 回の本文を受け取る時間の上限 (合計。止まったかは `STALL` で見る)
const EPISODE_BODY_TIMEOUT: Duration = Duration::from_secs(4 * 60 * 60);
/// 本文がこの間まったく届かなければ止める
pub(crate) const STALL: Duration = Duration::from_secs(60);
/// 登録したときの「iPod に置く回」
pub const DEFAULT_KEEP: u32 = 3;
/// 回の説明は一覧に出す分だけ持つ
const MAX_DESCRIPTION_CHARS: usize = 600;
/// 番組の説明は詳細の画面に全文を出す (長すぎるものは切る)
const MAX_SHOW_DESCRIPTION_CHARS: usize = 8000;
/// 保存の形の版。上げると、次の更新で前回の ETag を送らずに本文ごと取り直す (足した項目を古い番組にも入れるため)。
/// 1: 番組の説明の全文 (改行を残す)・回ごとの画像
pub const META_VERSION: u32 = 1;

/// 番組 (購読しているフィード)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Feed {
    /// フィードの URL から作った ID (落としたファイルのフォルダ名にも使う)
    pub id: String,
    pub url: String,
    pub title: String,
    pub author: String,
    pub description: String,
    pub image_url: String,
    pub link: String,
    /// 登録した日時 (UNIX 秒)
    pub added: i64,
    /// 最後にフィードを確かめた日時 (UNIX 秒)
    pub checked: i64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// 同期で iPod に置いておく、まだ聞いていない新しい回の数。0 = 自動では送らない
    pub keep: u32,
    /// 保存したときの形の版 (`META_VERSION` より古ければ、次の更新で本文ごと取り直す)
    pub meta_version: u32,
    /// 置いておく回を古い方から選ぶ (続きものの番組を #1 から順に聞くとき)。false なら新しい方から
    pub keep_oldest: bool,
    /// 新しい順
    pub episodes: Vec<Episode>,
}

/// 番組の 1 回
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Episode {
    /// guid (無ければ音声の URL) から作った ID
    pub id: String,
    pub guid: String,
    pub title: String,
    /// 公開日時 (UNIX 秒。分からなければ 0)
    pub published: i64,
    pub duration_ms: u32,
    /// 音声の URL (enclosure)
    pub url: String,
    pub length: u64,
    pub mime: String,
    pub description: String,
    /// 回ごとの画像 (itunes:image)。無ければ空 (番組の画像を使う)
    pub image_url: String,
    /// 聞いた (iPod で再生した・手で印を付けた)。iPod から消したあとも残す (同期で送り直さないため)
    pub played: bool,
    /// 印を最後に変えたときの、iPod の再生回数。これより増えたら iPod で聞いたことにする
    /// (手で「未再生」に戻した回が、前の再生回数ですぐ「再生済み」に戻らないように)
    pub plays_seen: u32,
    /// 落としたファイルの名前 (番組のフォルダの中)。消したら None
    pub file: Option<String>,
    /// 落としたファイルに番組の画像を入れた (前の版で落としたファイルは、次に送るときに入れ直す)
    pub cover_applied: bool,
    /// 落としたときに書き込んだアルバム名と曲名 (あとで番組名が変わっても iPod の曲と結び付けられるように)
    pub tag_album: Option<String>,
    pub tag_title: Option<String>,
    /// フィードで初めて見た日時 (UNIX 秒)
    pub first_seen: i64,
}

impl Episode {
    /// 動画の回 (今は音声だけを扱う)
    pub fn is_video(&self) -> bool {
        self.mime.starts_with("video/")
            || matches!(url_ext(&self.url).as_deref(), Some("mp4" | "m4v" | "mov")) && !self.mime.starts_with("audio/")
    }
}

/// フィードを読んだ結果 (保存している状態を除いたもの)
#[derive(Debug, Clone, Default)]
pub struct ParsedFeed {
    pub title: String,
    pub author: String,
    pub description: String,
    pub image_url: String,
    pub link: String,
    pub episodes: Vec<Episode>,
}

/// 短い ID (SHA-256 の先頭 8 バイトの 16 進)
pub fn short_id(s: &str) -> String {
    let h = Sha256::digest(s.as_bytes());
    h[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// `short_id` の形 (小文字の 16 進 16 桁) か。番組と回の ID はフォルダ名・ファイル名になるので、
/// 画面から来た ID をパスにする前に確かめる ("" や ".."・絶対パスでフォルダの外を指させない)
pub fn is_valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 番組のフォルダの中のファイル名 (区切りや .. を含まない) か
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

// ============================================================
// フィードの読み取り
// ============================================================

/// RSS 2.0 (iTunes の拡張を含む) を読む。回は新しい順に並べる
pub fn parse_feed(body: &[u8]) -> Result<ParsedFeed> {
    let text = String::from_utf8_lossy(body);
    let mut reader = Reader::from_str(&text);
    reader.config_mut().trim_text(false);
    let mut feed = ParsedFeed::default();
    let mut stack: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut item: Option<Episode> = None;
    let mut saw_channel = false;
    // 画像は itunes:image を優先し、無ければ channel/image/url
    let mut rss_image = String::new();

    loop {
        let ev = reader.read_event().map_err(|e| Error::Other(t!("フィードを読めません: {e}", e = e)))?;
        match ev {
            Event::Start(e) => {
                let name = qname(&e);
                if name == "channel" {
                    saw_channel = true;
                }
                if name == "item" && in_channel(&stack) {
                    item = Some(Episode::default());
                }
                start_attrs(&e, &name, &stack, &mut feed, item.as_mut());
                stack.push(name);
                buf.clear();
            }
            Event::Empty(e) => {
                let name = qname(&e);
                start_attrs(&e, &name, &stack, &mut feed, item.as_mut());
            }
            Event::Text(e) => buf.push_str(&e.xml10_content()),
            Event::CData(e) => buf.push_str(&e.into_inner()),
            Event::GeneralRef(e) => match e.resolve_char_ref() {
                Ok(Some(c)) => buf.push(c),
                _ => buf.push_str(entity(&e.xml10_content())),
            },
            Event::End(_) => {
                let Some(name) = stack.pop() else { continue };
                let value = buf.trim().to_string();
                buf.clear();
                let parent = stack.last().map(String::as_str).unwrap_or("");
                if let Some(ep) = item.as_mut() {
                    if name == "item" {
                        let ep = item.take().unwrap();
                        if let Some(ep) = finish_episode(ep) {
                            feed.episodes.push(ep);
                        }
                        continue;
                    }
                    if parent == "item" {
                        match name.as_str() {
                            "title" => ep.title = value,
                            "guid" => ep.guid = value,
                            "pubDate" | "dc:date" if ep.published == 0 => ep.published = parse_date(&value).unwrap_or(0),
                            "itunes:duration" => ep.duration_ms = parse_duration(&value).unwrap_or(0),
                            "description" | "itunes:summary" | "content:encoded" if ep.description.is_empty() => {
                                ep.description = plain_text(&value, MAX_DESCRIPTION_CHARS, false)
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                match (parent, name.as_str()) {
                    ("channel", "title") => feed.title = value,
                    ("channel", "itunes:author") => feed.author = value,
                    ("channel", "description" | "itunes:summary") if feed.description.is_empty() => {
                        feed.description = plain_text(&value, MAX_SHOW_DESCRIPTION_CHARS, true)
                    }
                    ("channel", "link") if feed.link.is_empty() => feed.link = value,
                    ("image", "url") if stack.len() >= 2 && stack[stack.len() - 2] == "channel" => rss_image = value,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_channel {
        return Err(Error::Other(t!("ポッドキャストのフィード (RSS) ではありません")));
    }
    if feed.image_url.is_empty() {
        feed.image_url = rss_image;
    }
    feed.episodes.sort_by_key(|e| std::cmp::Reverse(e.published));
    Ok(feed)
}

fn qname(e: &BytesStart) -> String {
    e.name().as_ref().to_string()
}

fn in_channel(stack: &[String]) -> bool {
    stack.last().is_some_and(|s| s == "channel")
}

/// 属性に値がある要素 (enclosure・itunes:image)
fn start_attrs(e: &BytesStart, name: &str, stack: &[String], feed: &mut ParsedFeed, item: Option<&mut Episode>) {
    let attr = |key: &str| {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == key)
            .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.trim().to_string()))
            .unwrap_or_default()
    };
    let parent = stack.last().map(String::as_str).unwrap_or("");
    match (name, item) {
        ("enclosure", Some(ep)) if parent == "item" && ep.url.is_empty() => {
            ep.url = attr("url");
            ep.length = attr("length").parse().unwrap_or(0);
            ep.mime = attr("type").to_ascii_lowercase();
        }
        ("itunes:image", None) if parent == "channel" => feed.image_url = attr("href"),
        ("itunes:image", Some(ep)) if parent == "item" && ep.image_url.is_empty() => ep.image_url = attr("href"),
        _ => {}
    }
}

fn finish_episode(mut ep: Episode) -> Option<Episode> {
    if ep.url.is_empty() {
        return None;
    }
    if ep.guid.is_empty() {
        ep.guid = ep.url.clone();
    }
    if ep.title.is_empty() {
        ep.title = url_file_stem(&ep.url).unwrap_or_else(|| ep.guid.clone());
    }
    ep.id = short_id(&ep.guid);
    Some(ep)
}

fn entity(name: &str) -> &'static str {
    match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        _ => "",
    }
}

/// HTML の説明を、タグを外した文にする (`max` 文字まで)。`keep_lines` なら段落と改行 (p・br・li・div) を残し、
/// そうでなければ 1 段落にまとめる (一覧に出す回の説明)
fn plain_text(html: &str, max: usize, keep_lines: bool) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_ascii_lowercase();
                let breaks = matches!(name.as_str(), "br" | "p" | "div" | "li" | "h1" | "h2" | "h3" | "h4");
                out.push(if keep_lines && breaks { '\n' } else { ' ' });
            }
            _ if in_tag => tag.push(c),
            _ => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let text = if keep_lines {
        // 行ごとに空白をまとめ、空の行は 1 つまで (段落の区切り) にする
        let mut lines: Vec<String> = Vec::new();
        for l in out.lines() {
            let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
            if l.is_empty() && lines.last().is_none_or(|x| x.is_empty()) {
                continue;
            }
            lines.push(l);
        }
        while lines.last().is_some_and(|x| x.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    } else {
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    text.chars().take(max).collect()
}

/// itunes:duration ("1:02:03"・"62:03"・"3723"・"3723.5")
pub fn parse_duration(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut secs = 0f64;
    for part in s.split(':') {
        let v: f64 = part.trim().parse().ok()?;
        secs = secs * 60.0 + v;
    }
    (0.0..1e7).contains(&secs).then_some((secs * 1000.0) as u32)
}

/// RFC 822 / 2822 の日時 ("Wed, 02 Oct 2024 13:00:00 +0900")。ISO 8601 ("2024-10-02T13:00:00Z") も読む。
/// フィードによって書き方がまちまちなので、曜日・秒・時差は無くてもよい
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() >= 10 && s.as_bytes()[4] == b'-' {
        return parse_iso_date(s);
    }
    let s = s.split_once(',').map(|(_, r)| r).unwrap_or(s);
    let mut it = s.split_whitespace();
    let day: u32 = it.next()?.parse().ok()?;
    let month = month_of(it.next()?)?;
    let mut year: i32 = it.next()?.parse().ok()?;
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }
    let (h, m, sec) = match it.next() {
        Some(t) => parse_hms(t)?,
        None => (0, 0, 0),
    };
    let offset = it.next().map(zone_offset).unwrap_or(0);
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(h, m, sec)?;
    Some(date.and_utc().timestamp() - offset)
}

fn parse_iso_date(s: &str) -> Option<i64> {
    let year: i32 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let rest = s.get(10..).unwrap_or("");
    let rest = rest.trim_start_matches(['T', ' ']);
    let (time, zone) = match rest.find(['Z', '+', '-']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let (h, m, sec) = if time.is_empty() { (0, 0, 0) } else { parse_hms(time.split('.').next().unwrap_or(""))? };
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(h, m, sec)?;
    Some(date.and_utc().timestamp() - zone_offset(&zone.replace(':', "")))
}

fn parse_hms(t: &str) -> Option<(u32, u32, u32)> {
    let mut p = t.split(':');
    let h = p.next()?.parse().ok()?;
    let m = p.next().unwrap_or("0").parse().ok()?;
    let s = p.next().unwrap_or("0").split('.').next()?.parse().ok()?;
    Some((h, m, s))
}

fn month_of(s: &str) -> Option<u32> {
    let m = s.get(..3)?.to_ascii_lowercase();
    let i = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|x| *x == m)?;
    Some(i as u32 + 1)
}

/// 時差 (秒、UTC より進んでいれば正)
fn zone_offset(z: &str) -> i64 {
    let z = z.trim();
    if let Some(sign) = z.chars().next().filter(|c| *c == '+' || *c == '-') {
        let digits: String = z[1..].chars().filter(|c| c.is_ascii_digit()).collect();
        let (h, m) = match digits.len() {
            4 => (digits[..2].parse().unwrap_or(0), digits[2..].parse().unwrap_or(0)),
            2 => (digits.parse().unwrap_or(0), 0),
            _ => (0, 0),
        };
        let v = h * 3600 + m * 60;
        return if sign == '-' { -v } else { v };
    }
    let h = match z.to_ascii_uppercase().as_str() {
        "EST" => -5,
        "EDT" => -4,
        "CST" => -6,
        "CDT" => -5,
        "MST" => -7,
        "MDT" => -6,
        "PST" => -8,
        "PDT" => -7,
        "JST" => 9,
        "CET" => 1,
        "CEST" => 2,
        "BST" => 1,
        _ => 0,
    };
    h * 3600
}

fn url_path(url: &str) -> &str {
    let u = url.split(['?', '#']).next().unwrap_or(url);
    u.split_once("://").map(|(_, r)| r).unwrap_or(u)
}

fn url_file_stem(url: &str) -> Option<String> {
    let name = url_path(url).rsplit('/').next()?;
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    (!stem.is_empty()).then(|| stem.to_string())
}

fn url_ext(url: &str) -> Option<String> {
    let name = url_path(url).rsplit('/').next()?;
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
}

// ============================================================
// 保存
// ============================================================

/// 購読の一覧 (`podcasts.json`) と、落としたファイルの置き場所
pub struct Store {
    path: PathBuf,
    dir: PathBuf,
    pub feeds: Vec<Feed>,
}

#[derive(Serialize, Deserialize, Default)]
struct StoreFile {
    #[serde(default)]
    feeds: Vec<Feed>,
}

impl Store {
    /// `path` は一覧のファイル、`dir` は落としたファイルを置くフォルダ (番組ごとにフォルダを分ける)
    pub fn load(path: PathBuf, dir: PathBuf) -> Store {
        let feeds = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<StoreFile>(&b).ok())
            .map(|f| f.feeds)
            .unwrap_or_default();
        Store { path, dir, feeds }
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec(&StoreFile { feeds: self.feeds.clone() }).map_err(|e| Error::Other(e.to_string()))?;
        fs::write(&tmp, data)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn feed(&self, id: &str) -> Option<&Feed> {
        self.feeds.iter().find(|f| f.id == id)
    }

    pub fn feed_mut(&mut self, id: &str) -> Option<&mut Feed> {
        self.feeds.iter_mut().find(|f| f.id == id)
    }

    /// 番組のフォルダ (落としたファイルと画像)。登録している番組で、ID が正しい形のときだけ
    pub fn feed_dir(&self, feed_id: &str) -> Option<PathBuf> {
        (is_valid_id(feed_id) && self.feed(feed_id).is_some()).then(|| self.dir.join(feed_id))
    }

    /// 落としたファイルの場所 (残っていれば)
    pub fn episode_file(&self, feed_id: &str, ep: &Episode) -> Option<PathBuf> {
        let name = ep.file.as_deref().filter(|n| is_plain_name(n))?;
        let p = self.feed_dir(feed_id)?.join(name);
        p.is_file().then_some(p)
    }

    pub fn cover_path(&self, feed_id: &str) -> Option<PathBuf> {
        Some(self.feed_dir(feed_id)?.join("cover"))
    }

    /// 番組を足す (同じ URL がすでにあればその番組の ID を返す)
    pub fn add(&mut self, url: &str, fetched: Fetched, now: i64) -> String {
        if let Some(f) = self.feeds.iter().find(|f| f.url == url) {
            return f.id.clone();
        }
        // 同期で置いておく回は、はじめは新しい 3 回 (登録すればそのまま新しい回が iPod に入るように)
        let mut feed = Feed { id: short_id(url), url: url.to_string(), added: now, keep: DEFAULT_KEEP, meta_version: META_VERSION, ..Default::default() };
        feed.apply(fetched, now);
        // 登録した時点の回はすべて「初めて見た」= 登録した日時。新しい回の印は、これより後に出た回にだけ付ける
        let id = feed.id.clone();
        self.feeds.push(feed);
        id
    }

    /// 番組をやめる (落としたファイルも消す)。登録していない ID では何もしない
    pub fn remove(&mut self, id: &str) {
        let Some(dir) = self.feed_dir(id) else { return };
        self.feeds.retain(|f| f.id != id);
        let _ = fs::remove_dir_all(dir);
    }

    /// 回の落としたファイルを消す
    pub fn delete_file(&mut self, feed_id: &str, ep_id: &str) {
        let Some(dir) = self.feed_dir(feed_id) else { return };
        if let Some(ep) = self.feed_mut(feed_id).and_then(|f| f.episodes.iter_mut().find(|e| e.id == ep_id))
            && let Some(name) = ep.file.take()
        {
            ep.cover_applied = false;
            if is_plain_name(&name) {
                let _ = fs::remove_file(dir.join(name));
            }
        }
    }

    /// 落としたファイルの合計の大きさ
    pub fn files_size(&self) -> u64 {
        let mut n = 0;
        for f in &self.feeds {
            for ep in &f.episodes {
                if let Some(p) = self.episode_file(&f.id, ep) {
                    n += fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        n
    }
}

impl Feed {
    /// 取ってきたフィードの内容で更新する。回は guid で結び付け、聞いたか・落としたファイルは残す。
    /// フィードから消えた回も残す (古い回を出さなくなる番組が多いが、iPod にはまだあるかもしれない)
    pub fn apply(&mut self, fetched: Fetched, now: i64) {
        self.checked = now;
        let Fetched::Updated { feed, etag, last_modified } = fetched else { return };
        self.etag = etag;
        self.last_modified = last_modified;
        if !feed.title.is_empty() {
            self.title = feed.title;
        }
        self.author = feed.author;
        self.description = feed.description;
        self.image_url = feed.image_url;
        self.link = feed.link;
        self.meta_version = META_VERSION;
        let mut seen = HashSet::new();
        let mut merged = Vec::with_capacity(feed.episodes.len().max(self.episodes.len()));
        for mut ep in feed.episodes {
            if !seen.insert(ep.id.clone()) {
                continue;
            }
            if let Some(old) = self.episodes.iter().find(|o| o.id == ep.id) {
                ep.played = old.played;
                ep.plays_seen = old.plays_seen;
                ep.file = old.file.clone();
                ep.cover_applied = old.cover_applied;
                ep.tag_album = old.tag_album.clone();
                ep.tag_title = old.tag_title.clone();
                ep.first_seen = old.first_seen;
            } else {
                ep.first_seen = now;
            }
            merged.push(ep);
        }
        for old in self.episodes.drain(..) {
            if !seen.contains(&old.id) {
                merged.push(old);
            }
        }
        merged.sort_by_key(|e| std::cmp::Reverse(e.published));
        self.episodes = merged;
    }
}

// ============================================================
// 通信
// ============================================================

fn make_agent(recv_body: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(recv_body))
        .user_agent(USER_AGENT)
        // 429 / 503 の Retry-After を読むため、状態コードは自分で見る
        .http_status_as_error(false)
        // 転送は `call_following` が自分でたどる。ureq に任せると、転送の応答はヘッダーの途中 (Location を見つけた所) で
        // 次へ進むので、ヘッダーが分かれて届くと残りを chunked の本文として読み、
        // "chunk expected crlf as next character" で失敗する (配布リポジトリの Issue #31。Cloudflare の配信の計測の転送で起きた)
        .max_redirects(0)
        .build()
        .into()
}

/// フィードと画像
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| make_agent(BODY_TIMEOUT))
}

/// 回のファイル (大きいので本文の時間は長く取り、止まったかは `read_watched` で見る)
fn download_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| make_agent(EPISODE_BODY_TIMEOUT))
}

fn net_err(e: ureq::Error) -> Error {
    Error::Other(t!("通信エラー: {e}", e = e))
}

/// Retry-After が無いときに待つ時間 (秒)
const DEFAULT_BACKOFF: i64 = 30 * 60;
/// これより長くは待たない (秒)。おかしな値で何日も止めないように
const MAX_BACKOFF: i64 = 24 * 60 * 60;
/// これ以下の Retry-After なら、待ってから 1 回だけ取り直す (秒)
const RETRY_INLINE: i64 = 60;

/// 配信元 (ホスト) ごとの「この時刻 (UNIX 秒) までは問い合わせない」。アプリを閉じるまで覚える
fn backoff() -> &'static std::sync::Mutex<std::collections::HashMap<String, i64>> {
    static B: OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> = OnceLock::new();
    B.get_or_init(Default::default)
}

fn host_of(url: &str) -> String {
    url_path(url).split('/').next().unwrap_or("").to_ascii_lowercase()
}

/// Retry-After の値 (秒数か HTTP の日時) を、今から待つ秒数にする
pub fn parse_retry_after(v: &str, now: i64) -> Option<i64> {
    let v = v.trim();
    if let Ok(n) = v.parse::<i64>() {
        return Some(n.max(0));
    }
    parse_date(v).map(|t| (t - now).max(0))
}

fn wait_error(secs: i64) -> Error {
    let minutes = (secs + 59) / 60;
    Error::Other(t!(
        "配信元から、しばらく待つよう求められました。{n} 分ほどしてからもう一度お試しください",
        n = minutes.max(1)
    ))
}

/// GET する。配信元から 429 (または Retry-After 付きの 503) が返ったら、Retry-After の間はそのホストに問い合わせない。
/// 短い待ち (RETRY_INLINE 以下) なら待ってから 1 回だけ取り直す。`cancel` が立ったら待つのをやめる
fn get(agent: &ureq::Agent, url: &str, headers: &[(&str, &str)], cancel: Option<&AtomicBool>) -> Result<ureq::http::Response<ureq::Body>> {
    let host = host_of(url);
    let now = crate::ipod::itunesdb::now_unix();
    if let Some(&until) = backoff().lock().unwrap().get(&host)
        && until > now
    {
        return Err(wait_error(until - now));
    }
    for attempt in 0..2 {
        let resp = call_following(agent, url, headers)?;
        let status = resp.status().as_u16();
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
        if status == 429 || (status == 503 && retry_after.is_some()) {
            let now = crate::ipod::itunesdb::now_unix();
            let wait = retry_after
                .as_deref()
                .and_then(|v| parse_retry_after(v, now))
                .unwrap_or(DEFAULT_BACKOFF)
                .min(MAX_BACKOFF);
            if attempt == 0 && wait <= RETRY_INLINE {
                // 少しずつ眠り、取りやめられたらすぐ戻る
                let end = std::time::Instant::now() + Duration::from_secs(wait as u64);
                while std::time::Instant::now() < end {
                    if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                        return Err(Error::Other(t!("取りやめました")));
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                continue;
            }
            // 2 回目も断られたら、短い待ちでも DEFAULT までは延ばさず、言われた時間 (最低 1 分) だけ控える
            let wait = wait.max(60);
            backoff().lock().unwrap().insert(host, now + wait);
            return Err(wait_error(wait));
        }
        if status >= 400 {
            return Err(Error::Other(t!("通信エラー: HTTP {code}", code = status)));
        }
        return Ok(resp);
    }
    // tr: 訳さない (起きない前提の panic の文言)
    unreachable!("2 回目は必ず返す")
}

/// 転送 (301 / 302 / 303 / 307 / 308) をたどる回数の上限 (ポッドキャストの回は計測の転送が 8 段ほど重なることがある)
const MAX_REDIRECTS: usize = 15;

/// GET して、転送をたどった先の応答を返す。転送の応答の本文は読まない (空の chunked などで止まらないように)。
/// 失敗したら、どこで失敗したかを zenpod.log に残す
fn call_following(agent: &ureq::Agent, url: &str, headers: &[(&str, &str)]) -> Result<ureq::http::Response<ureq::Body>> {
    let mut url = url.to_string();
    for hop in 0..=MAX_REDIRECTS {
        let mut req = agent.get(&url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req.call().map_err(|e| {
            log::warn!("podcast get failed at hop {hop}: {}: {e}", host_of(&url));
            net_err(e)
        })?;
        let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
        match (resp.status().as_u16(), location) {
            (301 | 302 | 303 | 307 | 308, Some(loc)) if hop < MAX_REDIRECTS => url = resolve_location(&url, &loc),
            (301 | 302 | 303 | 307 | 308, Some(_)) => {
                log::warn!("podcast get: too many redirects: {}", host_of(&url));
                return Err(Error::Other(t!("通信エラー: 転送が多すぎます")));
            }
            _ => return Ok(resp),
        }
    }
    // tr: 訳さない (起きない前提の panic の文言)
    unreachable!("上限の回で必ず返す")
}

/// 転送先 (Location) を、今の URL をもとに絶対の URL にする
fn resolve_location(base: &str, loc: &str) -> String {
    let loc = loc.trim();
    let lower = loc.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return loc.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    if let Some(l) = loc.strip_prefix("//") {
        return format!("{scheme}://{l}");
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if loc.starts_with('/') {
        return format!("{scheme}://{host}{loc}");
    }
    let path = rest[host.len()..].split(['?', '#']).next().unwrap_or("");
    let dir = path.rfind('/').map_or("/", |i| &path[..=i]);
    format!("{scheme}://{host}{dir}{loc}")
}

/// フィードを取ってきた結果
#[derive(Debug, Clone)]
pub enum Fetched {
    /// 前回から変わっていない (304)
    NotModified,
    Updated { feed: ParsedFeed, etag: Option<String>, last_modified: Option<String> },
}

/// フィードを取ってくる。`etag` / `last_modified` は前回の値 (変わっていなければ 304 が返る)
pub fn fetch_feed(url: &str, etag: Option<&str>, last_modified: Option<&str>) -> Result<Fetched> {
    let url = normalize_url(url)?;
    let mut headers = vec![("Accept", "application/rss+xml, application/xml;q=0.9, text/xml;q=0.8, */*;q=0.5")];
    if let Some(v) = etag {
        headers.push(("If-None-Match", v));
    }
    if let Some(v) = last_modified {
        headers.push(("If-Modified-Since", v));
    }
    let mut resp = get(agent(), &url, &headers, None)?;
    if resp.status().as_u16() == 304 {
        return Ok(Fetched::NotModified);
    }
    let header = |k: &str| resp.headers().get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let etag = header("etag");
    let last_modified = header("last-modified");
    let body = resp.body_mut().with_config().limit(MAX_FEED_BYTES).read_to_vec().map_err(net_err)?;
    let feed = parse_feed(&body)?;
    Ok(Fetched::Updated { feed, etag, last_modified })
}

/// 入力された URL を整える (前後の空白、podcast:// や feed:// の書き方、スキームの省略)
pub fn normalize_url(url: &str) -> Result<String> {
    let u = url.trim();
    let u = if let Some(rest) = u.strip_prefix("feed://").or_else(|| u.strip_prefix("podcast://")).or_else(|| u.strip_prefix("pcast://")) {
        format!("https://{rest}")
    } else if let Some(rest) = u.strip_prefix("feed:") {
        rest.to_string()
    } else if u.contains("://") {
        u.to_string()
    } else {
        format!("https://{u}")
    };
    if !(u.starts_with("http://") || u.starts_with("https://")) || u.len() < 12 {
        return Err(Error::Other(t!("フィードの URL を確かめてください")));
    }
    Ok(u)
}

/// 番組の画像を取ってくる (無ければ、または取れなければ None)
pub fn fetch_image(url: &str) -> Option<Vec<u8>> {
    if url.is_empty() {
        return None;
    }
    let mut resp = get(agent(), url, &[], None).ok()?;
    let bytes = resp.body_mut().with_config().limit(MAX_IMAGE_BYTES).read_to_vec().ok()?;
    image::guess_format(&bytes).ok()?;
    Some(bytes)
}

/// 回を落とす。`progress(受け取ったバイト数, 全体)` を途中で呼ぶ。`cancel` が立ったら止めて途中のファイルを消す。
/// 中身から形式 (mp3 / m4a) を決めて、`dir` に `<回の ID>.<拡張子>` で置く。置いたファイルの名前を返す
pub fn download_episode(
    ep: &Episode,
    dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<String> {
    // 回の ID はファイル名になる
    if !is_valid_id(&ep.id) {
        return Err(Error::Other(t!("番組か回が見つかりません")));
    }
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{}.part", ep.id));
    let result = (|| {
        let resp = get(download_agent(), &ep.url, &[], Some(cancel))?;
        let size = resp.body().content_length();
        if let Some(n) = size {
            if n > MAX_EPISODE_BYTES {
                return Err(too_large());
            }
            check_room(dir, n)?;
        }
        let total = size.unwrap_or(ep.length);
        let reader = resp.into_body().into_reader();
        let mut out = std::io::BufWriter::new(fs::File::create(&tmp)?);
        let done = copy_body(reader, &mut out, dir, STALL, MAX_EPISODE_BYTES, cancel, &mut |done| progress(done, total))?;
        out.flush()?;
        drop(out);
        progress(done, total);
        let ext = sniff_ext(&tmp).or_else(|| ext_from(ep)).unwrap_or("mp3");
        let name = format!("{}.{ext}", ep.id);
        fs::rename(&tmp, dir.join(&name))?;
        Ok(name)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn too_large() -> Error {
    Error::Other(t!("回のファイルが大きすぎます"))
}

/// これから `need` バイト書いても PC に `KEEP_FREE_BYTES` が残るか (空きが分からなければ通す)
fn check_room(dir: &Path, need: u64) -> Result<()> {
    match fs4::available_space(dir) {
        Ok(free) if free < need.saturating_add(KEEP_FREE_BYTES) => Err(Error::Other(t!("PC の空き容量が足りません"))),
        _ => Ok(()),
    }
}

/// 回の本文を `out` に書き、書いたバイト数を返す。`max` を超えた・PC の空きが減った・`stall` の間届かなかった・
/// 取りやめたときは止める。`progress(受け取ったバイト数)` は 256KB ごと
fn copy_body(
    reader: impl Read + Send + 'static,
    out: &mut impl Write,
    dir: &Path,
    stall: Duration,
    max: u64,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<u64> {
    let (mut done, mut last, mut checked) = (0u64, 0u64, 0u64);
    read_watched(reader, stall, |data| {
        if cancel.load(Ordering::SeqCst) {
            return Err(Error::Other(t!("取りやめました")));
        }
        let Some(data) = data else { return Ok(()) };
        done += data.len() as u64;
        if done > max {
            return Err(too_large());
        }
        // 大きさを知らせない応答もあるので、途中でも空きを見る
        if done - checked >= 64 << 20 {
            checked = done;
            check_room(dir, 0)?;
        }
        out.write_all(data)?;
        if done - last >= 256 * 1024 {
            last = done;
            progress(done);
        }
        Ok(())
    })?;
    Ok(done)
}

/// 応答の本文を別のスレッドで読み、届いた分ごとに `on_data(Some(..))`、届かない間は 0.2 秒ごとに `on_data(None)` を呼ぶ
/// (取りやめを見るため。Err を返せばそこで止める)。`stall` の間まったく届かなければ止める。
/// ureq の本文の時間切れはヘッダを受け取ってからの合計で、無通信を見張れないためここで見る。
/// 止めたときの読みかけのスレッドは、次に届くか ureq の時間切れで終わる
pub(crate) fn read_watched(
    reader: impl Read + Send + 'static,
    stall: Duration,
    mut on_data: impl FnMut(Option<&[u8]>) -> Result<()>,
) -> Result<()> {
    use std::sync::mpsc::{RecvTimeoutError, sync_channel};
    let (tx, rx) = sync_channel::<std::io::Result<Vec<u8>>>(4);
    std::thread::spawn(move || {
        let mut reader = reader;
        loop {
            let mut buf = vec![0u8; 64 * 1024];
            let r = reader.read(&mut buf).map(|n| {
                buf.truncate(n);
                buf
            });
            let end = !matches!(&r, Ok(b) if !b.is_empty());
            // 受け手が止めたら (送れなければ) 終わる
            if tx.send(r).is_err() || end {
                break;
            }
        }
    });
    let mut last = std::time::Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok(b)) if b.is_empty() => return Ok(()),
            Ok(Ok(b)) => {
                last = std::time::Instant::now();
                on_data(Some(&b))?;
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(RecvTimeoutError::Timeout) => {
                on_data(None)?;
                if last.elapsed() >= stall {
                    return Err(Error::Other(t!("通信が止まったため取りやめました")));
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Err(std::io::Error::other("body reader stopped").into()),
        }
    }
}

/// 中身の先頭から形式を決める (URL の拡張子と中身が違うことがある)
fn sniff_ext(path: &Path) -> Option<&'static str> {
    let mut head = [0u8; 12];
    let mut f = fs::File::open(path).ok()?;
    let n = f.read(&mut head).ok()?;
    let head = &head[..n];
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        // .m4b にするとオーディオブックとして扱われるので、ポッドキャストは m4a にする
        return Some("m4a");
    }
    if head.starts_with(b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        // ADTS の AAC (0xFFF1 / 0xFFF9) と MP3 は同じ印で始まる。層のビットが 0 なら AAC
        if head.len() >= 2 && head[0] == 0xFF && head[1] & 0x06 == 0 {
            return Some("aac");
        }
        return Some("mp3");
    }
    if head.starts_with(b"RIFF") {
        return Some("wav");
    }
    None
}

fn ext_from(ep: &Episode) -> Option<&'static str> {
    match ep.mime.as_str() {
        "audio/mpeg" | "audio/mp3" => return Some("mp3"),
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" | "audio/x-m4b" => return Some("m4a"),
        "audio/aac" | "audio/aacp" => return Some("aac"),
        _ => {}
    }
    match url_ext(&ep.url).as_deref() {
        Some("mp3") => Some("mp3"),
        Some("m4a" | "m4b" | "mp4") => Some("m4a"),
        Some("aac") => Some("aac"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {

    /// 手元の小さな HTTP サーバー。`routes` は パス → 応答 (状態行からの生の文字列。`{base}` は自分の URL)。
    /// 接続ごとに要求を順に読み、続けて同じ接続で来る要求にも答える (keep-alive)
    fn serve(routes: Vec<(&'static str, String)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: Vec<(&str, String)> = routes.into_iter().map(|(p, r)| (p, r.replace("{base}", &base))).collect();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut out = stream;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let path = line.split(' ').nth(1).unwrap_or("").to_string();
                        loop {
                            let mut h = String::new();
                            if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                                break;
                            }
                        }
                        let resp = routes.iter().find(|(p, _)| *p == path).map(|(_, r)| r.clone());
                        let resp = resp.unwrap_or_else(|| "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".into());
                        // 応答を "|" で区切った所で、分けて少し遅らせて送る (本物の配信では本文が後のパケットで届くことがある)
                        for (k, part) in resp.split('|').enumerate() {
                            if k > 0 {
                                let _ = out.flush();
                                std::thread::sleep(std::time::Duration::from_millis(30));
                            }
                            if out.write_all(part.as_bytes()).is_err() {
                                return;
                            }
                        }
                    }
                });
            }
        });
        base
    }

    #[test]
    fn redirect_locations() {
        let r = super::resolve_location;
        assert_eq!(r("https://a.com/x/y.mp3?q=1", "https://b.net/z"), "https://b.net/z");
        assert_eq!(r("https://a.com/x/y.mp3", "HTTP://B.net/z"), "HTTP://B.net/z");
        assert_eq!(r("http://a.com/x/y.mp3", "//cdn.b.net/z.mp3"), "http://cdn.b.net/z.mp3");
        assert_eq!(r("https://a.com/x/y.mp3?q=/1", "/p/q.mp3"), "https://a.com/p/q.mp3");
        assert_eq!(r("https://a.com/x/y.mp3?q=/1", "z.mp3"), "https://a.com/x/z.mp3");
        assert_eq!(r("https://a.com", "z.mp3"), "https://a.com/z.mp3");
    }

    /// 転送 (302) の応答の本文が空の chunked (`0\r\n\r\n`) でも、転送先を読める (配布リポジトリの Issue #31。
    /// Cloudflare の配信の計測 (podtrac など) がこの形で返す)
    #[test]
    fn redirect_with_empty_chunked_body() {
        let body = "x".repeat(5000);
        let other = serve(vec![(
            "/file.mp3",
            format!("HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\n\r\n{body}", body.len()),
        )]);
        let base = serve(vec![
            // ヘッダーの途中で分かれて届く (Location の後に長いヘッダーが続く。Cloudflare の応答の形)
            (
                "/a",
                "HTTP/1.1 302 Found\r\nLocation: {base}/b\r\nTransfer-Encoding: chunked\r\n|Report-To: {\"endpoints\":[{\"url\":\"https://a.nel.cloudflare.com/report\"}]}\r\nServer: cloudflare\r\n\r\n|0\r\n|\r\n".into(),
            ),
            ("/b", format!("HTTP/1.1 302 Found\r\nLocation: {other}/file.mp3\r\nTransfer-Encoding: chunked\r\n\r\n|0\r\n\r\n")),
        ]);
        for _ in 0..2 {
            let resp = super::get(super::download_agent(), &format!("{base}/a"), &[], None).unwrap();
            assert_eq!(resp.into_body().read_to_string().unwrap(), body);
        }
    }
    use super::*;

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd" xmlns:content="http://purl.org/rss/1.0/modules/content/">
<channel>
  <title>Show &amp; Tell</title>
  <link>https://example.com/</link>
  <description><![CDATA[<p>A <b>show</b> about things.</p>]]></description>
  <itunes:author>Host Name</itunes:author>
  <itunes:image href="https://example.com/cover.jpg"/>
  <image><url>https://example.com/rss.png</url><title>x</title></image>
  <item>
    <title>Episode 1</title>
    <itunes:image href="https://example.com/ep1.jpg"/>
    <guid isPermaLink="false">ep-1</guid>
    <pubDate>Mon, 30 Sep 2024 08:00:00 +0900</pubDate>
    <enclosure url="https://cdn.example.com/ep1.mp3?x=1" length="1234" type="audio/mpeg"/>
    <itunes:duration>1:02:03</itunes:duration>
    <description>First &lt;i&gt;one&lt;/i&gt;</description>
  </item>
  <item>
    <title>Episode 2</title>
    <pubDate>Tue, 1 Oct 2024 00:00:00 GMT</pubDate>
    <enclosure url="https://cdn.example.com/ep2.m4a" length="99" type="audio/x-m4a"></enclosure>
    <itunes:duration>754</itunes:duration>
  </item>
  <item>
    <title>No audio</title>
    <guid>text-only</guid>
  </item>
</channel>
</rss>"#;

    #[test]
    fn parses_rss() {
        let f = parse_feed(RSS.as_bytes()).unwrap();
        assert_eq!(f.title, "Show & Tell");
        assert_eq!(f.author, "Host Name");
        assert_eq!(f.image_url, "https://example.com/cover.jpg");
        assert_eq!(f.description, "A show about things.");
        assert_eq!(f.episodes[1].image_url, "https://example.com/ep1.jpg");
        assert_eq!(f.episodes[0].image_url, "");
        assert_eq!(f.episodes.len(), 2);
        // 新しい順
        let e2 = &f.episodes[0];
        assert_eq!(e2.title, "Episode 2");
        assert_eq!(e2.guid, "https://cdn.example.com/ep2.m4a");
        assert_eq!(e2.duration_ms, 754_000);
        assert_eq!(e2.mime, "audio/x-m4a");
        let e1 = &f.episodes[1];
        assert_eq!(e1.guid, "ep-1");
        assert_eq!(e1.id, short_id("ep-1"));
        assert_eq!(e1.duration_ms, 3_723_000);
        assert_eq!(e1.length, 1234);
        assert_eq!(e1.published, 1727650800);
        assert_eq!(e1.description, "First one");
    }

    #[test]
    fn rss_image_fallback_and_not_rss() {
        let xml = RSS.replace(r#"<itunes:image href="https://example.com/cover.jpg"/>"#, "");
        assert_eq!(parse_feed(xml.as_bytes()).unwrap().image_url, "https://example.com/rss.png");
        assert!(parse_feed(b"<html><body>hi</body></html>").is_err());
    }

    #[test]
    fn plain_text_lines() {
        let html = "<p>First  line<br>second</p><p></p><p>Third &amp; last</p>";
        assert_eq!(plain_text(html, 100, true), "First line\nsecond\n\nThird & last");
        assert_eq!(plain_text(html, 100, false), "First line second Third & last");
        assert_eq!(plain_text(html, 5, false), "First");
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("Wed, 02 Oct 2002 13:00:00 GMT"), Some(1033563600));
        assert_eq!(parse_date("Wed, 02 Oct 2002 08:00:00 EST"), Some(1033563600));
        assert_eq!(parse_date("2 Oct 2002 13:00 +0000"), Some(1033563600));
        assert_eq!(parse_date("2002-10-02T22:00:00+09:00"), Some(1033563600));
        assert_eq!(parse_date("2002-10-02T13:00:00Z"), Some(1033563600));
        assert_eq!(parse_date("2002-10-02"), Some(1033516800));
        assert_eq!(parse_date("garbage"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("45:10"), Some(2_710_000));
        assert_eq!(parse_duration("3600"), Some(3_600_000));
        assert_eq!(parse_duration("12.5"), Some(12_500));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn urls() {
        assert_eq!(normalize_url(" feed://example.com/rss ").unwrap(), "https://example.com/rss");
        assert_eq!(normalize_url("example.com/rss").unwrap(), "https://example.com/rss");
        assert_eq!(normalize_url("http://example.com/rss").unwrap(), "http://example.com/rss");
        assert!(normalize_url("ftp://example.com/rss").is_err());
        assert_eq!(url_ext("https://a.com/x/ep.MP3?y=1"), Some("mp3".into()));
        assert_eq!(url_file_stem("https://a.com/x/ep-12.mp3"), Some("ep-12".into()));
    }

    #[test]
    fn merge_keeps_state() {
        let parsed = parse_feed(RSS.as_bytes()).unwrap();
        let mut feed = Feed { id: "f".into(), ..Default::default() };
        feed.apply(Fetched::Updated { feed: parsed.clone(), etag: Some("a".into()), last_modified: None }, 100);
        assert!(feed.episodes.iter().all(|e| e.first_seen == 100));
        feed.episodes[1].played = true;
        feed.episodes[1].plays_seen = 2;
        feed.episodes[1].file = Some("x.mp3".into());
        // 次のフィードでは 1 回目が消え、新しい回が増えた
        let mut next = parsed;
        next.episodes.remove(1);
        next.episodes.insert(
            0,
            Episode { id: short_id("ep-3"), guid: "ep-3".into(), title: "Episode 3".into(), url: "u".into(), published: 1800000000, ..Default::default() },
        );
        feed.apply(Fetched::Updated { feed: next, etag: Some("b".into()), last_modified: None }, 200);
        assert_eq!(feed.etag.as_deref(), Some("b"));
        let titles: Vec<&str> = feed.episodes.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["Episode 3", "Episode 2", "Episode 1"]);
        assert_eq!(feed.episodes[0].first_seen, 200);
        assert_eq!(feed.episodes[1].first_seen, 100);
        // フィードから消えた回も、聞いた印とファイルを残す
        assert!(feed.episodes[2].played);
        assert_eq!(feed.episodes[2].plays_seen, 2);
        assert_eq!(feed.episodes[2].file.as_deref(), Some("x.mp3"));
        // 変わっていなければ確かめた日時だけ
        feed.apply(Fetched::NotModified, 300);
        assert_eq!(feed.checked, 300);
        assert_eq!(feed.episodes.len(), 3);
    }

    #[test]
    fn sniff() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a");
        fs::write(&p, b"\0\0\0\x20ftypM4A \0\0").unwrap();
        assert_eq!(sniff_ext(&p), Some("m4a"));
        fs::write(&p, b"ID3\x04\0\0\0\0\0\0").unwrap();
        assert_eq!(sniff_ext(&p), Some("mp3"));
        fs::write(&p, [0xFF, 0xFB, 0x90, 0x00]).unwrap();
        assert_eq!(sniff_ext(&p), Some("mp3"));
        fs::write(&p, [0xFF, 0xF1, 0x50, 0x80]).unwrap();
        assert_eq!(sniff_ext(&p), Some("aac"));
    }

    /// 手元の HTTP サーバーで、フィードの取得 (ETag で 304)・回の取得 (中身から拡張子を決める) を通す
    #[test]
    fn fetch_and_download_over_http() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let audio: Vec<u8> = [b"ID3\x04\0\0\0\0\0\0".as_slice(), &[0u8; 300_000]].concat();
        let feed = RSS.replace("https://cdn.example.com/ep2.m4a", &format!("{base}/media/ep2")).into_bytes();
        let server = {
            let audio = audio.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().take(3) {
                    let mut stream = stream.unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    let mut etag_hit = false;
                    loop {
                        let mut l = String::new();
                        reader.read_line(&mut l).unwrap();
                        if l.trim().is_empty() {
                            break;
                        }
                        if l.to_ascii_lowercase().starts_with("if-none-match:") && l.contains("\"v1\"") {
                            etag_hit = true;
                        }
                    }
                    let (head, body): (String, &[u8]) = if first.contains("/media/") {
                        (format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n", audio.len()), &audio)
                    } else if etag_hit {
                        ("HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\n".to_string(), &[])
                    } else {
                        (format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"v1\"\r\nContent-Type: application/rss+xml\r\n", feed.len()), &feed)
                    };
                    stream.write_all(format!("{head}Connection: close\r\n\r\n").as_bytes()).unwrap();
                    stream.write_all(body).unwrap();
                }
            })
        };
        let url = format!("{base}/feed.xml");
        let Fetched::Updated { feed, etag, .. } = fetch_feed(&url, None, None).unwrap() else { panic!("304 のはずではない") };
        assert_eq!(etag.as_deref(), Some("\"v1\""));
        assert!(matches!(fetch_feed(&url, etag.as_deref(), None).unwrap(), Fetched::NotModified));
        let ep = feed.episodes.iter().find(|e| e.title == "Episode 2").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut seen = 0;
        let name = download_episode(ep, dir.path(), &AtomicBool::new(false), |done, _| seen = done).unwrap();
        // URL は .m4a でも、中身が MP3 なら .mp3 で置く
        assert_eq!(name, format!("{}.mp3", ep.id));
        assert_eq!(fs::read(dir.path().join(&name)).unwrap(), audio);
        assert_eq!(seen, audio.len() as u64);
        assert!(!dir.path().join(format!("{}.part", ep.id)).exists());
        server.join().unwrap();
    }

    #[test]
    fn retry_after_values() {
        assert_eq!(parse_retry_after("120", 0), Some(120));
        assert_eq!(parse_retry_after(" 0 ", 0), Some(0));
        assert_eq!(parse_retry_after("Wed, 02 Oct 2002 13:00:00 GMT", 1033563600 - 90), Some(90));
        // 過去の日時は 0
        assert_eq!(parse_retry_after("Wed, 02 Oct 2002 13:00:00 GMT", 1033563600 + 5), Some(0));
        assert_eq!(parse_retry_after("soon", 0), None);
    }

    /// 429 の Retry-After が短ければ待って取り直す。長ければ止めて、その間は同じ配信元に問い合わせない
    #[test]
    fn backs_off_on_429() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let server = {
            let hits = hits.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().take(3) {
                    let mut stream = stream.unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    loop {
                        let mut l = String::new();
                        reader.read_line(&mut l).unwrap();
                        if l.trim().is_empty() {
                            break;
                        }
                    }
                    let n = hits.fetch_add(1, Ordering::SeqCst);
                    let resp: String = match n {
                        // 1 回目: 1 秒待て → 2 回目は通す
                        0 => "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\n".into(),
                        1 => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", RSS.len()),
                        // 3 回目: 1 時間待て
                        _ => "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3600\r\nContent-Length: 0\r\n".into(),
                    };
                    stream.write_all(format!("{resp}Connection: close\r\n\r\n").as_bytes()).unwrap();
                    if n == 1 {
                        stream.write_all(RSS.as_bytes()).unwrap();
                    }
                }
            })
        };
        let url = format!("{base}/feed.xml");
        let started = std::time::Instant::now();
        assert!(matches!(fetch_feed(&url, None, None).unwrap(), Fetched::Updated { .. }));
        assert!(started.elapsed() >= Duration::from_millis(900), "Retry-After の間は待つ");
        let e = fetch_feed(&url, None, None).unwrap_err().to_string();
        assert!(e.contains("60"), "{e}");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        // 待つ間は問い合わせない (サーバーには届かない)
        assert!(fetch_feed(&url, None, None).is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        server.join().unwrap();
    }

    #[test]
    fn video_episodes() {
        let ep = |url: &str, mime: &str| Episode { url: url.into(), mime: mime.into(), ..Default::default() };
        assert!(ep("https://a/x.mp4", "video/mp4").is_video());
        assert!(ep("https://a/x.m4v", "").is_video());
        assert!(!ep("https://a/x.mp4", "audio/mp4").is_video());
        assert!(!ep("https://a/x.mp3", "audio/mpeg").is_video());
    }

    #[test]
    fn id_format() {
        assert!(is_valid_id(&short_id("https://example.com/rss")));
        for bad in ["", ".", "..", "../x", "/abs", "C:\\x", "0123456789ABCDEF", "0123456789abcde", "0123456789abcdef0", "0123456789abcdeg"] {
            assert!(!is_valid_id(bad), "{bad}");
        }
        assert!(is_plain_name("abc.mp3"));
        for bad in ["", ".", "..", "../x.mp3", "a/b", "a\\b", "C:x", "/abs"] {
            assert!(!is_plain_name(bad), "{bad}");
        }
    }

    /// 画面から来た ID や壊れた一覧で、フォルダの外・登録していない番組のフォルダを触らない
    #[test]
    fn store_paths_only_for_registered_ids() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("podcasts");
        let victim = root.path().join("victim");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("x.mp3"), b"x").unwrap();
        // 形は正しいが登録していない番組
        let unknown = "0123456789abcdef";
        fs::create_dir_all(dir.join(unknown)).unwrap();
        let mut store = Store::load(root.path().join("podcasts.json"), dir.clone());
        let id = store.add("https://example.com/rss", Fetched::NotModified, 0);
        fs::create_dir_all(dir.join(&id)).unwrap();
        let abs = victim.to_string_lossy().into_owned();
        for bad in ["", ".", "..", "../victim", "/abs", "C:\\x", abs.as_str(), unknown] {
            assert!(store.feed_dir(bad).is_none(), "{bad}");
            assert!(store.cover_path(bad).is_none(), "{bad}");
            store.remove(bad);
            store.delete_file(bad, "x");
        }
        // 一覧のファイルを書き換えられて、ID が ".." の番組があっても
        store.feeds.push(Feed { id: "..".into(), ..Default::default() });
        store.remove("..");
        assert!(victim.join("x.mp3").is_file() && dir.join(unknown).is_dir() && dir.join(&id).is_dir());
        assert_eq!(store.feeds.len(), 2);
        // 回のファイル名がフォルダの外を指していても、場所を返さず消さない
        let ep = Episode { id: short_id("ep"), file: Some("../../victim/x.mp3".into()), ..Default::default() };
        assert!(store.episode_file(&id, &ep).is_none());
        store.feed_mut(&id).unwrap().episodes.push(ep.clone());
        store.delete_file(&id, &ep.id);
        assert!(victim.join("x.mp3").is_file());
        // 登録している番組はフォルダごと消える
        assert_eq!(store.cover_path(&id), Some(dir.join(&id).join("cover")));
        store.remove(&id);
        assert!(!dir.join(&id).exists());
        assert!(store.feed(&id).is_none());
    }

    /// エラーの文言 (ほかのテストが言語を切り替えることがあるので日英のどちらでも)
    fn says(e: &Error, ja: &str, en: &str) -> bool {
        let s = e.to_string();
        s.contains(ja) || s.contains(en)
    }

    /// 回の本文: 上限を超えた・止まった・取りやめたら止める
    #[test]
    fn copy_body_limits() {
        let dir = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);
        let mut out = Vec::new();
        let mut seen = 0;
        let n = copy_body(std::io::Cursor::new(vec![7u8; 300_000]), &mut out, dir.path(), STALL, 1 << 20, &no, &mut |d| seen = d).unwrap();
        assert_eq!((n, out.len()), (300_000, 300_000));
        assert!(seen >= 256 * 1024, "{seen}");
        // 上限を超えた
        let e = copy_body(std::io::repeat(1).take(200_000), &mut Vec::new(), dir.path(), STALL, 100_000, &no, &mut |_| {}).unwrap_err();
        assert!(says(&e, "大きすぎ", "too large"), "{e}");
        // 少し届いてから止まる配信元
        struct Stalls(bool);
        impl Read for Stalls {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    buf[0] = 1;
                    return Ok(1);
                }
                std::thread::sleep(Duration::from_secs(5));
                Ok(0)
            }
        }
        let started = std::time::Instant::now();
        let e = copy_body(Stalls(false), &mut Vec::new(), dir.path(), Duration::from_millis(500), u64::MAX, &no, &mut |_| {}).unwrap_err();
        assert!(says(&e, "止まった", "stalled"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(3));
        // 何も届かない間も取りやめを見る
        let yes = AtomicBool::new(true);
        let started = std::time::Instant::now();
        let e = copy_body(Stalls(true), &mut Vec::new(), dir.path(), STALL, u64::MAX, &yes, &mut |_| {}).unwrap_err();
        assert!(says(&e, "取りやめ", "Cancelled"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// 回の ID がファイル名にできない形なら落とさない (通信もしない)
    #[test]
    fn download_rejects_bad_episode_id() {
        let dir = tempfile::tempdir().unwrap();
        let ep = Episode { id: "../x".into(), url: "http://127.0.0.1:9/x.mp3".into(), ..Default::default() };
        assert!(download_episode(&ep, &dir.path().join("f"), &AtomicBool::new(false), |_, _| {}).is_err());
        assert!(!dir.path().join("x.part").exists() && !dir.path().join("f").exists());
    }
}
