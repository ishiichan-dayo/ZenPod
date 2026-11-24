//! アーティスト名・アルバム名からジャケット画像をネット上で探す。
//!
//! API キー不要のサービスだけを使う:
//! - iTunes Search API (日本 / 米国ストア)。URL のサイズ指定を大きくすると元解像度 (最大 3000px 程度) が返る
//! - Deezer API (1000px)
//! - MusicBrainz + Cover Art Archive (元画像)
//!
//! 解像度は候補表示後に `probe` で画像の先頭だけを取得して調べる。

use crate::t;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// MusicBrainz の規約で、User-Agent にはアプリ名・版・連絡先を入れる (無いと 503 で弾かれやすい)
const USER_AGENT: &str = concat!(
    "ZenPod/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/ishiichan-dayo/ZenPod )"
);
/// ダウンロードする画像の上限
const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtCandidate {
    pub source: String,
    pub title: String,
    pub artist: String,
    /// 一覧表示用の小さい画像
    pub thumb_url: String,
    /// 実際に使う画像 (元解像度)
    pub full_url: String,
}

/// 検索の経過 (画面の「検索中」に何をしているかを出し、届いた候補から先に並べる)
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "camelCase")]
pub enum SearchProgress {
    /// アーティストの別名を確かめている (MusicBrainz)
    Aliases,
    /// iTunes (日本・米国)・Deezer・MusicBrainz を並行して検索中。終わった数 / 全体と、
    /// それまでに届いた候補 (よい順。終わるたびに並べ直す)
    Sources { done: usize, total: usize, candidates: Vec<ArtCandidate> },
    /// iTunes の回数の上限 (1 分に 20 回) のため待っている。あと何秒
    Waiting { secs: u64 },
}

/// 経過を受け取る (検索の途中で、別々のスレッドから呼ぶ)
pub type Progress = Arc<dyn Fn(SearchProgress) + Send + Sync>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageProbe {
    pub width: u32,
    pub height: u32,
    /// 分かる場合のみ
    pub bytes: Option<u64>,
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(12)))
            .user_agent(USER_AGENT)
            .build()
            .into()
    })
}

fn net_err(e: ureq::Error) -> Error {
    Error::Other(t!("通信エラー: {e}", e = e))
}

fn get_json(req: ureq::RequestBuilder<ureq::typestate::WithoutBody>) -> Result<Value> {
    let mut resp = req.call().map_err(net_err)?;
    resp.body_mut().read_json::<Value>().map_err(net_err)
}

pub(crate) fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

// ============================================================
// 正規化・類似度
// ============================================================

/// 検索用に表記を整える: 全角英数→半角 (NFKC)、括弧内の付記、" - Single"・"Disc 1" などを外す
pub fn clean_title(t: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let t: String = t.nfkc().collect();
    let mut out = String::new();
    let mut depth = 0i32;
    for c in t.chars() {
        match c {
            '(' | '[' | '{' | '【' | '〔' | '〈' | '《' => depth += 1,
            ')' | ']' | '}' | '】' | '〕' | '〉' | '》' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let mut words: Vec<&str> = out.split_whitespace().collect();
    // 末尾の " - Single" / " - EP" / "Disc 1" / "CD2" を繰り返し外す
    loop {
        let n = trailing_noise(&words);
        if n == 0 {
            break;
        }
        words.truncate(words.len() - n);
    }
    let cleaned = words.join(" ");
    if cleaned.is_empty() {
        t.trim().to_string()
    } else {
        cleaned
    }
}

/// 末尾の付記の語数 (0 = なし)
fn trailing_noise(words: &[&str]) -> usize {
    let n = words.len();
    let low = |i: usize| words[i].to_lowercase();
    let is_num = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit());
    if n >= 2 && words[n - 2] == "-" && matches!(low(n - 1).as_str(), "single" | "ep") {
        return 2;
    }
    if n >= 2 && matches!(low(n - 2).as_str(), "disc" | "disk" | "cd") && is_num(words[n - 1]) {
        return 2;
    }
    if n >= 1 {
        let l = low(n - 1);
        let num = l.strip_prefix("disc").or_else(|| l.strip_prefix("cd"));
        if num.is_some_and(is_num) || words[n - 1] == "-" {
            return 1;
        }
    }
    0
}

/// 比較用 (付記除去 → 英数字・かな漢字以外を除去 → 小文字化)
fn norm(t: &str) -> String {
    clean_title(t)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// 文字 bigram の Dice 係数
fn dice(a: &str, b: &str) -> f64 {
    let bi = |s: &str| -> Vec<(char, char)> {
        let c: Vec<char> = s.chars().collect();
        c.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (x, mut y) = (bi(a), bi(b));
    if x.is_empty() || y.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let total = x.len() + y.len();
    let mut common = 0;
    for p in &x {
        if let Some(i) = y.iter().position(|q| q == p) {
            y.swap_remove(i);
            common += 1;
        }
    }
    2.0 * common as f64 / total as f64
}

/// 0.0〜1.0 のタイトル類似度
pub(crate) fn similarity(cand: &str, want: &str) -> f64 {
    let (a, b) = (norm(cand), norm(want));
    if a.is_empty() || b.is_empty() {
        0.0
    } else if a == b {
        1.0
    } else if a.contains(&b) || b.contains(&a) {
        0.9
    } else {
        dice(&a, &b)
    }
}

#[cfg(test)]
fn loosely_matches(a: &str, b: &str) -> bool {
    similarity(a, b) >= 0.9
}

// ============================================================
// アーティストの別表記 (MusicBrainz)
// ============================================================

#[derive(Debug, Clone, Default)]
pub(crate) struct ArtistInfo {
    /// 検索に使う表記 (先頭がタグの表記)
    names: Vec<String>,
    pub(crate) mbid: Option<String>,
    /// MusicBrainz 上の正式名
    canonical: Option<String>,
}

impl ArtistInfo {
    fn matches(&self, name: &str) -> bool {
        self.names.iter().any(|n| similarity(name, n) >= 0.9)
    }
}

/// MusicBrainz は 1 秒 1 リクエストまで
pub(crate) fn mb_get(url: &str, query: &[(&str, &str)]) -> Result<Value> {
    use std::sync::Mutex;
    use std::time::Instant;
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *last {
        let wait = Duration::from_millis(1100).saturating_sub(t.elapsed());
        std::thread::sleep(wait);
    }
    // 混雑時の 503 は少し待ってやり直す (MusicBrainz の案内どおり。最大 2 回)
    let mut attempt = 0;
    loop {
        let mut req = agent().get(url);
        for (k, v) in query {
            req = req.query(*k, *v);
        }
        let r = req.call();
        *last = Some(Instant::now());
        match r {
            Err(ureq::Error::StatusCode(503)) if attempt < 2 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(1500 * attempt));
            }
            Err(e) => return Err(net_err(e)),
            Ok(mut resp) => return resp.body_mut().read_json::<Value>().map_err(net_err),
        }
    }
}

/// Shift_JIS を Latin-1 として読んだような文字化けを弾く
fn looks_garbled(t: &str) -> bool {
    t.chars()
        .any(|c| matches!(c, '\u{2018}'..='\u{2021}' | '\u{0400}'..='\u{04FF}' | '\u{FFFD}'))
}

/// 名前か別名が一致する MusicBrainz のアーティスト (一致度 90 以上)。
/// Err は通信の失敗、Ok(None) は該当なし
fn mb_artist(artist: &str) -> Result<Option<Value>> {
    let esc = artist.replace('\\', "\\\\").replace('"', "\\\"");
    let q = format!("artist:\"{esc}\" OR alias:\"{esc}\"");
    let v = mb_get(
        "https://musicbrainz.org/ws/2/artist/",
        &[("query", &q), ("fmt", "json"), ("limit", "3")],
    )?;
    Ok(v["artists"]
        .as_array()
        .and_then(|l| l.iter().find(|a| a["score"].as_i64().unwrap_or(0) >= 90))
        .cloned())
}

/// 表記ゆれの判定用: MusicBrainz 上の正式名と別名をすべて返す (見つからなければ空)。
/// 通信に失敗したときは Err (キャッシュしないため)
pub fn artist_aliases(artist: &str) -> Result<Vec<String>> {
    let artist = artist.trim();
    if artist.is_empty() {
        return Ok(Vec::new());
    }
    let Some(a) = mb_artist(artist)? else {
        return Ok(Vec::new());
    };
    let a = &a;
    let mut out = vec![s(a, "name")];
    for al in a["aliases"].as_array().into_iter().flatten() {
        let n = s(al, "name");
        // "Shiina, Ringo" のような並べ替え用表記・文字化けは除く
        if !n.is_empty() && !n.contains(", ") && !looks_garbled(&n) && !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

/// "Maximum The Hormone" → ["Maximum The Hormone", "マキシマム ザ ホルモン", ...]
pub(crate) fn resolve_artist(artist: &str) -> ArtistInfo {
    let mut info = ArtistInfo::default();
    if artist.is_empty() {
        return info;
    }
    info.names.push(artist.to_string());
    let Ok(Some(a)) = mb_artist(artist) else {
        return info;
    };
    let a = &a;
    info.mbid = Some(s(a, "id"));
    info.canonical = Some(s(a, "name"));
    // 正式名 → 日本語の別名 → 英語の別名 → ロケール指定なし → 並べ替え用表記 の順
    let aliases: Vec<&Value> = a["aliases"].as_array().into_iter().flatten().collect();
    let by_locale = |loc: &str| -> Vec<String> {
        aliases
            .iter()
            .filter(|al| s(al, "locale") == loc)
            .map(|al| s(al, "name"))
            .collect()
    };
    let mut cands = vec![s(a, "name")];
    cands.extend(by_locale("ja"));
    cands.extend(by_locale("en"));
    cands.extend(by_locale(""));
    cands.push(s(a, "sort-name"));
    for c in cands {
        // "Shiina, Ringo" のような並べ替え用表記・文字化けした別名は検索語に向かない
        // "MTH" のような短い英字の略称は無関係な作品に当たりやすいので使わない
        let short_acronym = c.chars().count() <= 3 && c.is_ascii();
        if c.is_empty()
            || c.contains(", ")
            || c.chars().count() < 2
            || short_acronym
            || looks_garbled(&c)
        {
            continue;
        }
        if !info.names.iter().any(|n| norm(n) == norm(&c)) {
            info.names.push(c);
        }
        if info.names.len() >= 4 {
            break;
        }
    }
    info
}

// ============================================================
// 検索
// ============================================================

/// 候補 + 並べ替え用の点数
struct Scored {
    c: ArtCandidate,
    /// 検索サービスが返した順位 (iTunes の関連度を並びに反映するため)
    rank: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub candidates: Vec<ArtCandidate>,
    /// 実際に使った検索語 (表記を整えたアーティスト名・アルバム名)
    pub artist_query: String,
    pub album_query: String,
    /// タグの表記以外に検索したアーティスト名 (MusicBrainz の別名)
    pub artist_aliases: Vec<String>,
}

/// 3 サービスを並列に検索して、アルバム名が近い順に返す
pub fn search(artist: &str, album: &str) -> SearchResult {
    search_with(artist, album, Arc::new(|_| {}))
}

/// `search` と同じ。経過を `progress` に知らせる
pub fn search_with(artist: &str, album: &str, progress: Progress) -> SearchResult {
    let (artist, album) = (clean_title(artist), clean_title(album));
    if artist.is_empty() && album.is_empty() {
        return SearchResult::default();
    }
    progress(SearchProgress::Aliases);
    let info = resolve_artist(&artist);
    type Job = Box<dyn FnOnce() -> Result<Vec<Scored>> + Send>;
    let jobs: Vec<Job> = vec![
        {
            let (i, b, p) = (info.clone(), album.clone(), progress.clone());
            Box::new(move || itunes(&i, &b, "jp", &p))
        },
        {
            let (i, b, p) = (info.clone(), album.clone(), progress.clone());
            Box::new(move || itunes(&i, &b, "us", &p))
        },
        {
            let (i, b) = (info.clone(), album.clone());
            Box::new(move || deezer(&i, &b))
        },
        {
            let (i, b) = (info.clone(), album.clone());
            Box::new(move || musicbrainz(&i, &b))
        },
    ];
    // 終わったサービスから受け取り、そのたびにそれまでの候補を並べ直して知らせる (終わる順はばらばら)
    let total = jobs.len();
    progress(SearchProgress::Sources { done: 0, total, candidates: Vec::new() });
    let (tx, rx) = std::sync::mpsc::channel();
    for job in jobs {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
    }
    drop(tx);
    let mut seen = HashSet::new();
    let mut all: Vec<(f64, ArtCandidate)> = Vec::new();
    for (n, r) in rx.into_iter().enumerate() {
        match r {
            Ok(list) => {
                for sc in list {
                    if !seen.insert(sc.c.full_url.clone()) {
                        continue;
                    }
                    let mut score = if album.is_empty() {
                        0.5
                    } else {
                        similarity(&sc.c.title, &album)
                    };
                    let artist_ok = info.names.is_empty() || info.matches(&sc.c.artist);
                    // iTunes は日本語でもアルバム名がローマ字で返るため、
                    // アーティストが一致するときは Apple の検索順位を信用する
                    if sc.c.source.starts_with("iTunes") && artist_ok {
                        score = score.max(0.8 - 0.1 * sc.rank as f64);
                    }
                    if !artist_ok {
                        score *= 0.5;
                    }
                    all.push((score, sc.c));
                }
            }
            Err(e) => log::warn!("artwork search: {e}"),
        }
        all.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let candidates = all.iter().take(24).map(|(_, c)| c.clone()).collect();
        progress(SearchProgress::Sources { done: n + 1, total, candidates });
    }
    SearchResult {
        candidates: all.into_iter().take(24).map(|(_, c)| c).collect(),
        artist_aliases: info.names.iter().skip(1).cloned().collect(),
        artist_query: artist,
        album_query: album,
    }
}

/// iTunes Search API は、およそ 1 分に 20 回まで (Apple の案内)。1 回の検索で最大 6 回 (名前 3 つ × 日本と米国) 問い合わせるので、
/// アルバムをまとめて探すと超える。直近 1 分の回数が上限に達していたら、空くまで待つ (待つ秒数を知らせる)
fn itunes_wait(progress: &Progress) {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Instant;
    const PER_MINUTE: usize = 20;
    static RECENT: Mutex<VecDeque<Instant>> = Mutex::new(VecDeque::new());
    loop {
        let mut q = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        while q.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            q.pop_front();
        }
        let Some(&oldest) = q.front().filter(|_| q.len() >= PER_MINUTE) else {
            q.push_back(now);
            return;
        };
        drop(q);
        let wait = Duration::from_secs(60).saturating_sub(now.duration_since(oldest));
        progress(SearchProgress::Waiting { secs: wait.as_secs_f64().ceil() as u64 });
        std::thread::sleep(wait);
    }
}

fn itunes(info: &ArtistInfo, album: &str, country: &str, progress: &Progress) -> Result<Vec<Scored>> {
    let lang = if country == "jp" { "ja_jp" } else { "en_us" };
    let mut out = Vec::new();
    let mut last_err = None;
    let names: Vec<&str> = if info.names.is_empty() {
        vec![""]
    } else {
        info.names.iter().map(String::as_str).take(3).collect()
    };
    for name in names {
        let term = format!("{name} {album}");
        itunes_wait(progress);
        let v = match get_json(
            agent()
                .get("https://itunes.apple.com/search")
                .query("term", term.trim())
                .query("entity", "album")
                .query("country", country)
                .query("lang", lang)
                .query("limit", "6"),
        ) {
            Ok(v) => v,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };
        for (rank, r) in v["results"].as_array().into_iter().flatten().enumerate() {
            let art = s(r, "artworkUrl100");
            // ".../100x100bb.jpg" のサイズ部分を差し替える
            let Some((base, _)) = art.rsplit_once('/') else {
                continue;
            };
            out.push(Scored {
                rank,
                c: ArtCandidate {
                    source: format!("iTunes ({})", country.to_uppercase()),
                    title: s(r, "collectionName"),
                    artist: s(r, "artistName"),
                    thumb_url: format!("{base}/300x300bb.jpg"),
                    full_url: format!("{base}/10000x10000bb.jpg"),
                },
            });
        }
    }
    match (out.is_empty(), last_err) {
        (true, Some(e)) => Err(e),
        _ => Ok(out),
    }
}

fn deezer_album(r: &Value, artist: Option<&str>) -> Option<ArtCandidate> {
    let full = s(r, "cover_xl");
    if full.is_empty() {
        return None;
    }
    Some(ArtCandidate {
        source: "Deezer".into(),
        title: s(r, "title"),
        artist: artist
            .map(str::to_string)
            .unwrap_or_else(|| s(&r["artist"], "name")),
        thumb_url: s(r, "cover_medium"),
        full_url: full,
    })
}

fn deezer(info: &ArtistInfo, album: &str) -> Result<Vec<Scored>> {
    let mut out = Vec::new();
    // 1) 通常の検索 (英語表記には強い)
    let q = format!(
        "{} {album}",
        info.names.first().map(String::as_str).unwrap_or("")
    );
    if let Ok(v) = get_json(
        agent()
            .get("https://api.deezer.com/search/album")
            .query("q", q.trim())
            .query("limit", "8"),
    ) {
        for (rank, r) in v["data"].as_array().into_iter().flatten().enumerate() {
            if let Some(c) = deezer_album(r, None) {
                out.push(Scored { rank, c });
            }
        }
    }
    // 2) アーティストを特定してアルバム一覧から探す (日本語のアルバム名に強い)
    if !album.is_empty() {
        for name in info.names.iter().take(3) {
            let Ok(v) = get_json(
                agent()
                    .get("https://api.deezer.com/search/artist")
                    .query("q", name)
                    .query("limit", "3"),
            ) else {
                continue;
            };
            let Some(a) = v["data"]
                .as_array()
                .and_then(|l| l.iter().find(|a| info.matches(&s(a, "name"))))
            else {
                continue;
            };
            let id = a["id"].as_i64().unwrap_or(0);
            let aname = s(a, "name");
            let Ok(list) = get_json(
                agent()
                    .get(format!("https://api.deezer.com/artist/{id}/albums"))
                    .query("limit", "300"),
            ) else {
                continue;
            };
            let mut hits: Vec<(f64, ArtCandidate)> = list["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| deezer_album(r, Some(&aname)))
                .map(|c| (similarity(&c.title, album), c))
                .filter(|(sc, _)| *sc >= 0.5)
                .collect();
            hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            out.extend(
                hits.into_iter()
                    .take(6)
                    .enumerate()
                    .map(|(rank, (_, c))| Scored { rank, c }),
            );
            break;
        }
    }
    Ok(out)
}

fn musicbrainz(info: &ArtistInfo, album: &str) -> Result<Vec<Scored>> {
    // アーティストが特定できていれば、そのアーティストの作品一覧から近いものを選ぶ
    let groups: Vec<Value> = if let (Some(mbid), false) = (&info.mbid, album.is_empty()) {
        let v = mb_get(
            "https://musicbrainz.org/ws/2/release-group",
            &[("artist", mbid), ("fmt", "json"), ("limit", "100")],
        )?;
        let mut list: Vec<(f64, Value)> = v["release-groups"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|g| (similarity(&s(g, "title"), album), g.clone()))
            .filter(|(sc, _)| *sc >= 0.5)
            .collect();
        list.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        list.into_iter().take(3).map(|(_, g)| g).collect()
    } else {
        let esc = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"");
        let mut query = Vec::new();
        if !album.is_empty() {
            query.push(format!("releasegroup:\"{}\"", esc(album)));
        }
        if let Some(a) = info.names.first() {
            query.push(format!("artist:\"{}\"", esc(a)));
        }
        let q = query.join(" AND ");
        let v = mb_get(
            "https://musicbrainz.org/ws/2/release-group/",
            &[("query", &q), ("fmt", "json"), ("limit", "3")],
        )?;
        v["release-groups"].as_array().cloned().unwrap_or_default()
    };
    let artist_name = info
        .canonical
        .clone()
        .or_else(|| info.names.first().cloned())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (rank, g) in groups.iter().enumerate() {
        let id = s(g, "id");
        let credit = g["artist-credit"]
            .as_array()
            .and_then(|a| a.first())
            .map(|a| s(a, "name"))
            .unwrap_or_else(|| artist_name.clone());
        // 画像が無いリリースグループは 404 になる
        let Ok(caa) =
            get_json(agent().get(format!("https://coverartarchive.org/release-group/{id}")))
        else {
            continue;
        };
        let images = caa["images"].as_array().cloned().unwrap_or_default();
        if let Some(img) = images.iter().find(|i| i["front"].as_bool() == Some(true)) {
            let thumb = img["thumbnails"]["500"]
                .as_str()
                .or(img["thumbnails"]["large"].as_str())
                .unwrap_or("")
                .to_string();
            out.push(Scored {
                rank,
                c: ArtCandidate {
                    source: "MusicBrainz".into(),
                    title: s(g, "title"),
                    artist: credit,
                    thumb_url: thumb,
                    full_url: s(img, "image"),
                },
            });
        }
    }
    Ok(out)
}

/// 画像の先頭だけを取得して解像度とファイルサイズを調べる
pub fn probe(url: &str) -> Result<ImageProbe> {
    for range in [128 * 1024u64, 1024 * 1024] {
        let mut resp = agent()
            .get(url)
            .header("Range", format!("bytes=0-{}", range - 1))
            .call()
            .map_err(net_err)?;
        let headers = resp.headers();
        let total = headers
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.parse::<u64>().ok())
            .or_else(|| {
                headers
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
            });
        // Range を無視して全体を返すサーバもあるので、先頭だけ読んで打ち切る
        let mut head = Vec::new();
        {
            use std::io::Read;
            let _ = resp
                .body_mut()
                .as_reader()
                .take(range)
                .read_to_end(&mut head);
        }
        if let Ok(sz) = imagesize::blob_size(&head) {
            return Ok(ImageProbe {
                width: sz.width as u32,
                height: sz.height as u32,
                bytes: total,
            });
        }
    }
    Err(Error::Media("画像サイズを判定できません".into()))
}

/// 画像をダウンロード
pub fn download(url: &str) -> Result<Vec<u8>> {
    let mut resp = agent().get(url).call().map_err(net_err)?;
    resp.body_mut()
        .with_config()
        .limit(MAX_IMAGE_BYTES)
        .read_to_vec()
        .map_err(net_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_rules() {
        assert_eq!(clean_title("ＦＯＲ　ＹＯＵ"), "FOR YOU");
        assert_eq!(clean_title("KAMAKURA (2008 Remaster)"), "KAMAKURA");
        assert_eq!(clean_title("BADモード [Disc 1]"), "BADモード");
        assert_eq!(clean_title("Open Secret - Single"), "Open Secret");
        assert_eq!(clean_title("Greatest Hits Disc 2"), "Greatest Hits");
        assert_eq!(clean_title("(What's the Story)"), "(What's the Story)");
        assert!(similarity("無罪モラトリアム", "無罪モラトリアム (Remastered)") > 0.99);
        assert!(looks_garbled("’†“‡‚Э‚д‚«"));
        assert!(!looks_garbled("マキシマム ザ ホルモン"));
        assert!(!looks_garbled("Maximum the Hormone"));
    }

    #[test]
    fn match_rules() {
        assert!(loosely_matches("椎名林檎", "椎名 林檎"));
        assert!(loosely_matches("The Beatles", "beatles"));
        assert!(!loosely_matches("Queen", "Radiohead"));
    }

    /// ネットワークが必要なので通常は実行しない

    #[test]
    #[ignore]
    fn live_search() {
        let list = search("椎名林檎", "無罪モラトリアム").candidates;
        for c in &list {
            let p = probe(&c.full_url);
            println!(
                "{} | {} | {} | {:?}",
                c.source,
                c.title,
                c.artist,
                p.map(|p| (p.width, p.height, p.bytes))
            );
        }
        assert!(!list.is_empty());
    }
}

#[cfg(test)]
mod survey {
    use super::*;

    /// 日本語アルバムで上位 3 件を表示 (ネットワーク必須)
    #[test]
    #[ignore]
    fn jp_survey() {
        let cases = [
            ("Maximum The Hormone", "予襲復讐"),
            ("宇多田ヒカル", "First Love"),
            ("あいみょん", "瞬間的シックスセンス"),
            ("back number", "ラブストーリー"),
            ("Mr.Children", "深海"),
            ("スピッツ", "ハチミツ"),
            ("中島みゆき", "寒水魚"),
            ("山下達郎", "ＦＯＲ　ＹＯＵ"),
            ("椎名林檎", "加爾基 精液 栗ノ花"),
            ("坂本龍一", "音楽図鑑"),
            ("サザンオールスターズ", "KAMAKURA (2008 Remaster)"),
            ("Sheena Ringo", "無罪モラトリアム"),
            ("宇多田ヒカル", "BADモード [Disc 1]"),
        ];
        for (ar, al) in cases {
            let info = resolve_artist(&clean_title(ar));
            let list = search(ar, al).candidates;
            let top: Vec<String> = list
                .iter()
                .take(3)
                .map(|c| format!("[{}] {} / {}", c.source, c.title, c.artist))
                .collect();
            println!(
                "{ar} / {al} (別名 {:?}) -> {}件\n    {}",
                &info.names[1..],
                list.len(),
                top.join("\n    ")
            );
        }
    }
}
