//! 曲情報をネット (MusicBrainz) から取得して、正式な表記に直す候補を作る。
//!
//! 1. アーティスト名・アルバム名でリリース (CD などの版) を検索して候補を返す
//! 2. 選ばれたリリースの曲目と iPod 内の曲を対応付けて、項目ごとの差分を返す
//!
//! iTunes の検索は日本のアルバムでもローマ字表記が返るので、日本語の表記が揃っている MusicBrainz を使う。

use serde::Serialize;
use serde_json::Value;

use crate::online::artsearch::{clean_title, mb_get, resolve_artist, s, similarity};
use crate::error::Result;
use crate::domain::track::TrackView;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseCandidate {
    pub id: String,
    pub title: String,
    pub artist: String,
    /// "2013-07-31" のような発売日 (不明なら空)
    pub date: String,
    pub country: String,
    pub track_count: u32,
    /// "CD" / "Digital Media" など
    pub formats: String,
    /// 同名の版を見分ける注記 (例: "初回限定盤")
    pub disambiguation: String,
    /// ジャケットの縮小画像 (無い場合は読み込みに失敗する)
    pub thumb_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseTrack {
    pub disc: u32,
    pub nr: u32,
    pub title: String,
    pub artist: String,
    pub length_ms: u32,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub title: String,
    pub artist: String,
    pub year: u32,
    pub disc_count: u32,
    pub tracks: Vec<ReleaseTrack>,
    /// ローマ字・英語の表記 (Naming が Original のときは None)
    pub latin: Option<LatinNames>,
}

/// 表記の選び方 (曲情報をネットで取得するとき)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Naming {
    /// MusicBrainz の元の表記 (日本語など)
    #[default]
    Original,
    /// ローマ字・英語の表記
    Latin,
    /// 表記は元のまま、並べ替え用の読みだけローマ字・英語
    LatinSort,
}

/// ローマ字・英語の表記。曲名は音訳版 (Pseudo-Release) があるときだけ、アーティストは別名から
#[derive(Debug, Clone, Default)]
pub struct LatinNames {
    /// 音訳版のアルバム名 (無ければ None)
    pub title: Option<String>,
    pub artist: String,
    pub artist_sort: String,
    /// (ディスク, 曲順) → (曲名 (音訳版が無ければ None), アーティスト, アーティストの並べ替え用)
    pub tracks: std::collections::HashMap<(u32, u32), (Option<String>, String, String)>,
}

/// 項目の変更 1 件。`field` は "title" | "artist" | "album" | "albumArtist" | "year" | "trackNr" | "discNr"
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagChange {
    pub id: String,
    /// 曲名 (一覧表示用。変更前の表記)
    pub track_title: String,
    pub field: String,
    pub before: String,
    pub after: String,
    /// 波ダッシュ (～/〜) や全角/半角の空白など、見た目がほぼ同じ記号の違いだけ
    pub minor: bool,
}

/// 見た目がほぼ同じ記号を揃える (比較用)
fn fold_lookalike(t: &str) -> String {
    t.chars()
        .filter_map(|c| match c {
            '～' | '〜' | '~' => Some('~'),
            '‐' | '‑' | '–' | '—' | '―' | '－' => Some('-'),
            '’' | '‘' | '＇' => Some('\''),
            '“' | '”' | '＂' => Some('"'),
            '！' => Some('!'),
            '？' => Some('?'),
            '（' => Some('('),
            '）' => Some(')'),
            c if c.is_whitespace() => None,
            c => Some(c),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagDiff {
    pub changes: Vec<TagChange>,
    /// リリースの曲目と対応付けられなかった iPod の曲
    pub unmatched: Vec<String>,
    /// 渡した曲それぞれが、リリースのどの曲目 (`tracks` の添字) に対応したか。None = 対応なし
    pub matches: Vec<TrackMatch>,
    /// リリースの曲目 (対応を手で選び直すための一覧)
    pub tracks: Vec<ReleaseTrack>,
    pub disc_count: u32,
    /// ローマ字・英語の表記の出どころ: "titles" (曲名も) / "artists" (アーティスト名だけ) / "" (元の表記)
    pub latin: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMatch {
    pub id: String,
    pub track: Option<usize>,
}

/// 曲の対応を手で決めたもの: 曲 ID → リリースの曲目の添字 (None = この曲は反映しない)
pub type Assignments = std::collections::HashMap<String, Option<usize>>;

/// 曲番号。タグに無ければ曲名 (情報の無い曲はファイル名になっている) の先頭の数字
/// ("01 xxx" / "01. xxx" / "01-xxx" / "Track 01" / "トラック01") から推測する。
/// 推測した番号は長さも合うときだけ使う (`diff` の by_position) ので、"99 Luftballons" のような曲名でも誤りにくい
fn guess_track_nr(t: &TrackView) -> u32 {
    if t.track_nr != 0 {
        return t.track_nr;
    }
    let title = t.title.trim();
    let rest = if title.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("track")) {
        &title[5..]
    } else {
        // tr: 訳さない (ファイル名の読み取り)
        title.strip_prefix("トラック").unwrap_or(title)
    };
    let rest = rest.trim_start_matches([' ', '_', '-', '#']);
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    // 4 桁以上は年 ("1979") のことが多い。"1st" "7th" のように文字が続くものも番号ではない
    let next = rest[digits..].chars().next();
    if digits == 0 || digits > 3 || next.is_some_and(char::is_alphanumeric) {
        return 0;
    }
    rest[..digits].parse().unwrap_or(0)
}

/// "A" + " feat. " + "B" のようにクレジットを繋げる
fn credit(v: &Value) -> String {
    v["artist-credit"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| format!("{}{}", s(c, "name"), s(c, "joinphrase")))
        .collect()
}

fn quote(t: &str) -> String {
    format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
}

/// アーティスト名・アルバム名でリリースを探す (MusicBrainz は 1 秒 1 件なので数秒かかる)
pub fn search_releases(artist: &str, album: &str) -> Result<Vec<ReleaseCandidate>> {
    let (artist, album) = (clean_title(artist), clean_title(album));
    if album.is_empty() {
        return Ok(Vec::new());
    }
    // 表記ゆれ (カタカナ / 英語) があっても当たるよう、アーティストは ID で絞る
    let artist_q = match resolve_artist(&artist).mbid {
        Some(id) => format!("arid:{id}"),
        None if !artist.is_empty() => format!("artist:{}", quote(&artist)),
        None => String::new(),
    };
    let query = |album_q: String| {
        [album_q, artist_q.clone()]
            .into_iter()
            .filter(|q| !q.is_empty())
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let search = |q: String| {
        mb_get(
            "https://musicbrainz.org/ws/2/release/",
            &[("query", &q), ("fmt", "json"), ("limit", "15")],
        )
    };
    let mut v = search(query(format!("release:{}", quote(&album))))?;
    // 完全一致で見つからなければ語句のあいまい検索
    if v["releases"].as_array().is_none_or(|l| l.is_empty()) {
        let words: String = album
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect();
        v = search(query(format!("release:({words})")))?;
    }
    Ok(v["releases"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let id = s(r, "id");
            let formats: Vec<String> = r["media"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| s(m, "format"))
                .filter(|f| !f.is_empty())
                .collect();
            ReleaseCandidate {
                thumb_url: format!("https://coverartarchive.org/release/{id}/front-250"),
                id,
                title: s(r, "title"),
                artist: credit(r),
                date: s(r, "date"),
                country: s(r, "country"),
                track_count: r["track-count"].as_u64().unwrap_or(0) as u32,
                formats: formats.join(" + "),
                disambiguation: s(r, "disambiguation"),
            }
        })
        .collect())
}

/// リリースの曲目。naming が Original 以外なら、ローマ字・英語の表記も取る
/// (音訳版の曲目で 1 回、アーティストごとに 1 回ずつ MusicBrainz に問い合わせるので数秒かかる)
pub fn fetch_release(id: &str, naming: Naming) -> Result<Release> {
    let v = mb_get(
        &format!("https://musicbrainz.org/ws/2/release/{id}"),
        &[("inc", "recordings+artist-credits+release-groups+release-rels"), ("fmt", "json")],
    )?;
    let year = |d: String| d.get(..4).and_then(|y| y.parse().ok()).unwrap_or(0);
    let media = v["media"].as_array().cloned().unwrap_or_default();
    let mut tracks = Vec::new();
    for m in &media {
        let disc = m["position"].as_u64().unwrap_or(1) as u32;
        for t in m["tracks"].as_array().into_iter().flatten() {
            let artist = credit(t);
            tracks.push(ReleaseTrack {
                disc,
                nr: t["position"].as_u64().unwrap_or(0) as u32,
                title: s(t, "title"),
                artist: if artist.is_empty() { credit(&v) } else { artist },
                length_ms: t["length"].as_u64().unwrap_or(0) as u32,
            });
        }
    }
    Ok(Release {
        title: s(&v, "title"),
        artist: credit(&v),
        year: match year(s(&v, "date")) {
            0 => year(s(&v["release-group"], "first-release-date")),
            y => y,
        },
        disc_count: media.len() as u32,
        latin: if naming == Naming::Original { None } else { Some(latin_names(&v)?) },
        tracks,
    })
}

/// 文字がすべてラテン文字・数字・記号か (日本語などが入っていなければ訳す必要が無い)
fn is_latin(t: &str) -> bool {
    t.chars().all(|c| c.is_ascii() || matches!(c, '\u{00C0}'..='\u{024F}' | '\u{2000}'..='\u{206F}'))
}

/// アーティストのローマ字・英語の名前と並べ替え用の名前。
/// 英語の主要な別名 → 英語の別名 → 並べ替え用の名前を「名 姓」に戻したもの の順
fn latin_artist(id: &str, name: &str) -> Result<(String, String)> {
    let a = mb_get(&format!("https://musicbrainz.org/ws/2/artist/{id}"), &[("inc", "aliases"), ("fmt", "json")])?;
    let sort = s(&a, "sort-name");
    let aliases: Vec<&Value> = a["aliases"].as_array().into_iter().flatten().collect();
    let en = |primary: bool| {
        aliases
            .iter()
            .filter(|al| s(al, "locale").starts_with("en") && (!primary || al["primary"].as_bool() == Some(true)))
            .map(|al| s(al, "name"))
            .find(|n| !n.is_empty() && is_latin(n))
    };
    let latin = if is_latin(name) {
        name.to_string()
    } else {
        en(true).or_else(|| en(false)).unwrap_or_else(|| match sort.split_once(", ") {
            Some((last, first)) if is_latin(&sort) => format!("{first} {last}"),
            _ if is_latin(&sort) => sort.clone(),
            _ => name.to_string(),
        })
    };
    let sort = if is_latin(&sort) && !sort.is_empty() { sort } else { latin.clone() };
    Ok((latin, sort))
}

fn latin_names(v: &Value) -> Result<LatinNames> {
    // 音訳版 (ローマ字を優先、無ければ英語訳) の曲目
    let pseudo = v["relations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| s(r, "type") == "transl-tracklisting" && s(r, "direction") == "forward")
        .filter(|r| s(&r["release"]["text-representation"], "script") == "Latn")
        .min_by_key(|r| if s(&r["release"]["text-representation"], "language") == "jpn" { 0 } else { 1 })
        .map(|r| s(&r["release"], "id"));
    let mut titles: std::collections::HashMap<(u32, u32), String> = std::collections::HashMap::new();
    let mut album = None;
    if let Some(pid) = pseudo {
        let p = mb_get(&format!("https://musicbrainz.org/ws/2/release/{pid}"), &[("inc", "recordings"), ("fmt", "json")])?;
        album = Some(s(&p, "title")).filter(|t| !t.is_empty());
        for m in p["media"].as_array().into_iter().flatten() {
            let disc = m["position"].as_u64().unwrap_or(1) as u32;
            for t in m["tracks"].as_array().into_iter().flatten() {
                titles.insert((disc, t["position"].as_u64().unwrap_or(0) as u32), s(t, "title"));
            }
        }
    }
    // クレジットのアーティストごとに名前を引く (同じアーティストは 1 回だけ)
    let mut cache: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    let mut credit_latin = |c: &Value| -> Result<(String, String)> {
        let mut name = String::new();
        let mut sort = String::new();
        for (k, part) in c["artist-credit"].as_array().into_iter().flatten().enumerate() {
            let id = s(&part["artist"], "id");
            let credited = s(part, "name");
            let (l, st) = match cache.get(&id) {
                Some(x) => x.clone(),
                None => {
                    let x = latin_artist(&id, &credited)?;
                    cache.insert(id.clone(), x.clone());
                    x
                }
            };
            // 日本語のつなぎ (、・と など) は英語のつなぎにする
            let join = match s(part, "joinphrase").trim() {
                // tr: 訳さない (つなぎの文字)
                "、" | "," => ", ".to_string(),
                // tr: 訳さない (つなぎの文字)
                "と" | "&" => " & ".to_string(),
                // tr: 訳さない (つなぎの文字)
                "・" => ", ".to_string(),
                j if is_latin(j) => s(part, "joinphrase"),
                _ => " & ".to_string(),
            };
            name.push_str(&l);
            if k == 0 {
                sort = st;
            }
            name.push_str(&join);
        }
        Ok((name.trim_end_matches([',', '&', ' ']).to_string(), sort))
    };
    let (artist, artist_sort) = credit_latin(v)?;
    let mut tracks = std::collections::HashMap::new();
    for m in v["media"].as_array().into_iter().flatten() {
        let disc = m["position"].as_u64().unwrap_or(1) as u32;
        for t in m["tracks"].as_array().into_iter().flatten() {
            let nr = t["position"].as_u64().unwrap_or(0) as u32;
            let (a, st) = if t["artist-credit"].as_array().is_some_and(|l| !l.is_empty()) {
                credit_latin(t)?
            } else {
                (artist.clone(), artist_sort.clone())
            };
            tracks.insert((disc, nr), (titles.get(&(disc, nr)).cloned(), a, st));
        }
    }
    Ok(LatinNames { title: album, artist, artist_sort, tracks })
}

/// 曲とリリースの曲目の近さ (0〜1)。曲名・長さ・曲順で判断する
fn match_score(t: &TrackView, r: &ReleaseTrack, nr: u32) -> f64 {
    let title = similarity(&t.title, &r.title);
    let length = if t.duration_ms == 0 || r.length_ms == 0 {
        0.5
    } else {
        let diff = (t.duration_ms as f64 - r.length_ms as f64).abs();
        (1.0 - diff / 10_000.0).max(0.0)
    };
    let pos = if nr != 0 && nr == r.nr && t.disc_nr.max(1) == r.disc {
        1.0
    } else {
        0.0
    };
    0.6 * title + 0.25 * length + 0.15 * pos
}

/// iPod の曲とリリースの曲目を対応付け、変わる項目を返す。
/// 対応付けは元の表記の曲目で行い (曲名の近さを使うため)、値は naming に合わせて選ぶ。
/// `assign` にある曲は、自動の対応付けをせずにその指定に従う
pub fn diff(tracks: &[TrackView], rel: &Release, naming: Naming, assign: &Assignments) -> TagDiff {
    let mut local: Vec<Option<usize>> = vec![None; tracks.len()];
    let mut used = vec![false; rel.tracks.len()];
    let mut fixed = vec![false; tracks.len()];
    for (i, t) in tracks.iter().enumerate() {
        if let Some(&a) = assign.get(&t.id) {
            let a = a.filter(|&j| j < rel.tracks.len());
            local[i] = a;
            fixed[i] = true;
            if let Some(j) = a {
                used[j] = true;
            }
        }
    }
    // 近い組から順に確定していく (1 つの曲目に 2 曲を割り当てない)
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, t) in tracks.iter().enumerate().filter(|(i, _)| !fixed[*i]) {
        let nr = guess_track_nr(t);
        let before = pairs.len();
        for (j, r) in rel.tracks.iter().enumerate() {
            let sc = match_score(t, r, nr);
            // 曲名がまったく違っても、曲順と長さが合っていれば同じ曲とみなす (表記違い・翻訳など)
            let by_position = nr == r.nr
                && t.disc_nr.max(1) == r.disc
                && t.duration_ms.abs_diff(r.length_ms) < 3000
                && r.length_ms != 0;
            if sc >= 0.5 || by_position {
                pairs.push((if by_position { sc.max(0.5) } else { sc }, i, j));
            }
        }
        // 手がかりが何も無い曲 (曲名がファイル名など) は、長さがほぼ同じ曲目が 1 つだけならそれとみなす
        if pairs.len() == before && t.duration_ms != 0 {
            let near: Vec<usize> = (0..rel.tracks.len())
                .filter(|&j| rel.tracks[j].length_ms != 0 && t.duration_ms.abs_diff(rel.tracks[j].length_ms) < 1500)
                .collect();
            if let [j] = near[..] {
                pairs.push((0.45, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, i, j) in pairs {
        if local[i].is_none() && !used[j] {
            local[i] = Some(j);
            used[j] = true;
        }
    }
    let matches = tracks.iter().zip(&local).map(|(t, &m)| TrackMatch { id: t.id.clone(), track: m }).collect();

    let mut changes = Vec::new();
    let mut unmatched = Vec::new();
    for (t, m) in tracks.iter().zip(local) {
        let Some(j) = m else {
            unmatched.push(t.title.clone());
            continue;
        };
        let r = &rel.tracks[j];
        let mut push = |field: &str, before: String, after: String| {
            if !after.is_empty() && before != after {
                changes.push(TagChange {
                    id: t.id.clone(),
                    track_title: t.title.clone(),
                    field: field.into(),
                    minor: fold_lookalike(&before) == fold_lookalike(&after),
                    before,
                    after,
                });
            }
        };
        let num = |n: u32| if n == 0 { String::new() } else { n.to_string() };
        // ローマ字・英語の表記 (無い項目は元の表記)
        let lat = rel.latin.as_ref();
        let lt = lat.and_then(|l| l.tracks.get(&(r.disc, r.nr)));
        let l_title = lt.and_then(|x| x.0.clone()).unwrap_or_else(|| r.title.clone());
        let l_artist = lt.map(|x| x.1.clone()).unwrap_or_else(|| r.artist.clone());
        let l_artist_sort = lt.map(|x| x.2.clone()).unwrap_or_default();
        let l_album = lat.and_then(|l| l.title.clone()).unwrap_or_else(|| rel.title.clone());
        let l_album_artist = lat.map(|l| l.artist.clone()).unwrap_or_else(|| rel.artist.clone());
        let l_album_artist_sort = lat.map(|l| l.artist_sort.clone()).unwrap_or_default();
        let latin = naming == Naming::Latin;
        let pick = |orig: &String, l: &String| if latin { l.clone() } else { orig.clone() };
        push("title", t.title.clone(), pick(&r.title, &l_title));
        push("artist", t.artist.clone(), pick(&r.artist, &l_artist));
        push("album", t.album.clone(), pick(&rel.title, &l_album));
        // アルバムアーティストは曲のアーティストと違うとき (オムニバス・客演など) か、既に入っているときだけ
        let album_artist_needed = !t.album_artist.is_empty() || rel.artist != r.artist;
        if album_artist_needed {
            push("albumArtist", t.album_artist.clone(), pick(&rel.artist, &l_album_artist));
        }
        // 表記は元のまま、並べ替え用の読みだけローマ字・英語 (元の表記がラテン文字なら要らない)
        if naming == Naming::LatinSort {
            let sort = |orig: &str, l: &str| if is_latin(orig) || l.is_empty() { String::new() } else { l.to_string() };
            push("sortTitle", t.sort_title.clone(), sort(&r.title, &l_title));
            push("sortArtist", t.sort_artist.clone(), sort(&r.artist, &l_artist_sort));
            push("sortAlbum", t.sort_album.clone(), sort(&rel.title, &l_album));
            if album_artist_needed {
                push("sortAlbumArtist", t.sort_album_artist.clone(), sort(&rel.artist, &l_album_artist_sort));
            }
        }
        push("year", num(t.year), num(rel.year));
        push("trackNr", num(t.track_nr), num(r.nr));
        // 1 枚組でディスク番号が空なら、わざわざ 1 を入れない
        if rel.disc_count > 1 || t.disc_nr != 0 {
            push("discNr", num(t.disc_nr), num(r.disc));
        }
    }
    let latin = match (&rel.latin, naming) {
        (_, Naming::Original) | (None, _) => String::new(),
        (Some(l), _) if l.tracks.values().any(|x| x.0.is_some()) => "titles".into(),
        _ => "artists".into(),
    };
    TagDiff { changes, unmatched, latin, matches, tracks: rel.tracks.clone(), disc_count: rel.disc_count }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tv(id: &str, title: &str, artist: &str, nr: u32, ms: u32) -> TrackView {
        TrackView {
            id: id.into(),
            title: title.into(),
            artist: artist.into(),
            album: "予襲復讐".into(),
            track_nr: nr,
            duration_ms: ms,
            ..Default::default()
        }
    }

    fn rt(nr: u32, title: &str, ms: u32) -> ReleaseTrack {
        ReleaseTrack {
            disc: 1,
            nr,
            title: title.into(),
            artist: "マキシマム ザ ホルモン".into(),
            length_ms: ms,
        }
    }

    fn rel() -> Release {
        Release {
            title: "予襲復讐".into(),
            artist: "マキシマム ザ ホルモン".into(),
            year: 2013,
            disc_count: 1,
            tracks: vec![rt(1, "予襲復讐", 313_000), rt(2, "鬱くしき人々のうた", 278_000), rt(3, "便所サンダルダンス", 244_000)],
            latin: None,
        }
    }

    /// 音訳版の曲目が 1 曲目にだけあるローマ字の表記
    fn latin_rel() -> Release {
        let mut r = rel();
        let mut tracks = std::collections::HashMap::new();
        let a = ("Maximum The Hormone".to_string(), "Maximum The Hormone".to_string());
        tracks.insert((1, 1), (Some("Yoshuu Fukushuu".to_string()), a.0.clone(), a.1.clone()));
        tracks.insert((1, 2), (None, a.0.clone(), a.1.clone()));
        r.latin = Some(LatinNames { title: Some("Yoshuu Fukushuu".into()), artist: a.0, artist_sort: a.1, tracks });
        r
    }

    #[test]
    fn latin_names_replace_values_but_match_by_original() {
        // 照合は元の (日本語の) 曲名で行い、値だけローマ字にする。音訳版に無い曲名は元のまま
        let tracks = [tv("a", "予襲復讐", "マキシマム ザ ホルモン", 1, 313_000), tv("b", "鬱くしき人々のうた", "マキシマム ザ ホルモン", 2, 278_000)];
        let d = diff(&tracks, &latin_rel(), Naming::Latin, &Assignments::new());
        assert!(d.unmatched.is_empty());
        let get = |id: &str, f: &str| d.changes.iter().find(|c| c.id == id && c.field == f).map(|c| c.after.clone());
        assert_eq!(get("a", "title").as_deref(), Some("Yoshuu Fukushuu"));
        assert_eq!(get("a", "artist").as_deref(), Some("Maximum The Hormone"));
        assert_eq!(get("a", "album").as_deref(), Some("Yoshuu Fukushuu"));
        assert_eq!(get("b", "title"), None, "音訳版に無い曲名は変えない");
        assert_eq!(d.latin, "titles");
        assert!(!d.changes.iter().any(|c| c.field.starts_with("sort")));
    }

    #[test]
    fn latin_sort_keeps_names_and_fills_sort_fields() {
        let tracks = [tv("a", "予襲復讐", "マキシマム ザ ホルモン", 1, 313_000)];
        let d = diff(&tracks, &latin_rel(), Naming::LatinSort, &Assignments::new());
        let get = |f: &str| d.changes.iter().find(|c| c.field == f).map(|c| c.after.clone());
        assert_eq!(get("title"), None, "表記はそのまま");
        assert_eq!(get("sortTitle").as_deref(), Some("Yoshuu Fukushuu"));
        assert_eq!(get("sortArtist").as_deref(), Some("Maximum The Hormone"));
        assert_eq!(get("sortAlbum").as_deref(), Some("Yoshuu Fukushuu"));
    }

    /// 実際の MusicBrainz で確かめる (ネットにつなぐので普段は走らせない: cargo test -- --ignored)
    #[test]
    #[ignore]
    fn latin_from_musicbrainz() {
        // 宇多田ヒカル「DEEP RIVER」: ローマ字の音訳版がある
        let r = fetch_release("5c8f87e8-62e7-440d-b23d-a61090b1e3e5", Naming::Latin).unwrap();
        let l = r.latin.unwrap();
        // 英語の主要な別名 (登録どおり。"UTADA HIKARU" のような大文字の公式表記のこともある)
        assert!(is_latin(&l.artist) && !l.artist.is_empty(), "{}", l.artist);
        assert_eq!(l.artist_sort, "Utada, Hikaru");
        assert!(l.tracks.values().any(|t| t.0.as_deref() == Some("Shiawase ni Narou")), "{:?}", l.tracks);
        // 米津玄師「STRAY SHEEP」: 音訳版は無いが英語の別名がある
        let r = fetch_release("4a2c0fa1-7d3b-47eb-8fb7-9c8dd9a3f1e4", Naming::Latin).unwrap();
        let l = r.latin.unwrap();
        assert_eq!(l.artist, "Kenshi Yonezu");
        assert!(l.tracks.values().all(|t| t.0.is_none()));
    }

    #[test]
    fn latin_detection() {
        assert!(is_latin("Kenshi Yonezu"));
        assert!(is_latin("Beyoncé – Halo"));
        assert!(!is_latin("米津玄師"));
    }

    #[test]
    fn matches_by_title_and_position() {
        // 曲順が違っていても曲名で対応付ける
        let tracks = [
            tv("a", "便所サンダルダンス", "Maximum The Hormone", 5, 244_100),
            tv("b", "予襲復讐", "マキシマム ザ ホルモン", 1, 313_000),
        ];
        let d = diff(&tracks, &rel(), Naming::Original, &Assignments::new());
        assert!(d.unmatched.is_empty());
        let for_a: Vec<(&str, &str)> = d
            .changes
            .iter()
            .filter(|c| c.id == "a")
            .map(|c| (c.field.as_str(), c.after.as_str()))
            .collect();
        assert_eq!(for_a, [("artist", "マキシマム ザ ホルモン"), ("year", "2013"), ("trackNr", "3")]);
        // b は年だけ
        assert!(d.changes.iter().filter(|c| c.id == "b").all(|c| c.field == "year"));
    }

    #[test]
    fn position_and_length_match_even_if_title_differs() {
        let tracks = [tv("a", "Utsukushiki Hitobito no Uta", "", 2, 278_500)];
        let d = diff(&tracks, &rel(), Naming::Original, &Assignments::new());
        assert!(d.changes.iter().any(|c| c.field == "title" && c.after == "鬱くしき人々のうた"));
    }

    #[test]
    fn lookalike_symbols_are_minor() {
        let mut r = rel();
        r.tracks[1].title = "鬱くしきOP 〜月の爆撃機〜".into();
        let tracks = [tv("a", "鬱くしきOP ～月の爆撃機～", "マキシマム ザ ホルモン", 2, 278_000)];
        let d = diff(&tracks, &r, Naming::Original, &Assignments::new());
        let c = d.changes.iter().find(|c| c.field == "title").unwrap();
        assert!(c.minor);
        assert!(!d.changes.iter().find(|c| c.field == "year").unwrap().minor);
    }

    #[test]
    fn guesses_track_number_from_file_name() {
        let nr = |title: &str| guess_track_nr(&tv("a", title, "", 0, 0));
        assert_eq!(nr("01 Intro"), 1);
        assert_eq!(nr("02. 曲"), 2);
        assert_eq!(nr("03-xxx"), 3);
        assert_eq!(nr("Track 04"), 4);
        assert_eq!(nr("トラック05"), 5);
        assert_eq!(nr("12"), 12);
        assert_eq!(nr("1979"), 0);
        assert_eq!(nr("7th Heaven"), 0);
        assert_eq!(nr("予襲復讐"), 0);
        // タグの番号があればそちら
        assert_eq!(guess_track_nr(&tv("a", "05 xxx", "", 2, 0)), 2);
    }

    #[test]
    fn untagged_tracks_match_by_file_number_or_length() {
        // 情報の無い曲 (曲名はファイル名、番号なし)
        let tracks = [tv("a", "02", "", 0, 278_400), tv("b", "unknown", "", 0, 244_900), tv("c", "zzz", "", 0, 100_000)];
        let d = diff(&tracks, &rel(), Naming::Original, &Assignments::new());
        let m: Vec<Option<usize>> = d.matches.iter().map(|m| m.track).collect();
        // a はファイル名の番号と長さ、b は長さがほぼ同じ曲目が 1 つだけ、c は手がかりなし
        assert_eq!(m, [Some(1), Some(2), None]);
        assert!(d.changes.iter().any(|c| c.id == "a" && c.field == "title" && c.after == "鬱くしき人々のうた"));
        assert_eq!(d.unmatched, ["zzz"]);
        assert_eq!(d.tracks.len(), 3);
    }

    #[test]
    fn manual_assignment_overrides_auto_match() {
        let tracks = [tv("a", "予襲復讐", "", 1, 313_000), tv("b", "zzz", "", 0, 100_000)];
        let mut assign = Assignments::new();
        // a は反映しない、b は 3 曲目
        assign.insert("a".into(), None);
        assign.insert("b".into(), Some(2));
        let d = diff(&tracks, &rel(), Naming::Original, &assign);
        assert_eq!(d.matches.iter().map(|m| m.track).collect::<Vec<_>>(), [None, Some(2)]);
        assert!(d.changes.iter().all(|c| c.id == "b"));
        assert!(d.changes.iter().any(|c| c.field == "title" && c.after == "便所サンダルダンス"));
        // 手で選んだ曲目は、ほかの曲の自動の対応付けに使わない
        let tracks = [tv("a", "便所サンダルダンス", "", 3, 244_000), tv("b", "zzz", "", 0, 100_000)];
        let mut assign = Assignments::new();
        assign.insert("b".into(), Some(2));
        let d = diff(&tracks, &rel(), Naming::Original, &assign);
        assert_eq!(d.matches.iter().map(|m| m.track).collect::<Vec<_>>(), [None, Some(2)]);
        // 範囲外の指定は対応なし
        let mut assign = Assignments::new();
        assign.insert("b".into(), Some(99));
        assert_eq!(diff(&tracks[1..], &rel(), Naming::Original, &assign).matches[0].track, None);
    }

    #[test]
    fn unrelated_track_is_unmatched() {
        let tracks = [tv("a", "Lemon", "米津玄師", 9, 255_000)];
        let d = diff(&tracks, &rel(), Naming::Original, &Assignments::new());
        assert!(d.changes.is_empty());
        assert_eq!(d.unmatched, ["Lemon"]);
    }
}
