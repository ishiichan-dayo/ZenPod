//! アーティスト名の表記ゆれ (例: "マキシマム ザ ホルモン" / "Maximum The Hormone") を見つける。
//!
//! 3 つの手がかりで同じアーティストらしい表記を束ねる:
//! 1. 表記の正規化 (全角/半角・大文字小文字・空白・記号・先頭の "The" を無視)
//! 2. 同じアルバムに別表記が混ざっている (曲番号が重ならない場合だけ)
//! 3. MusicBrainz の別名 (呼び出し側が取得して渡す)

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

/// 判定に使う曲の情報
pub struct Entry<'a> {
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    pub disc_nr: u32,
    pub track_nr: u32,
    pub compilation: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VariantName {
    pub name: String,
    /// この表記を使っている曲数 (アーティスト・アルバムアーティストのどちらか)
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantGroup {
    /// 曲数の多い順
    pub names: Vec<VariantName>,
    /// 見つけた理由: "normalize" | "album" | "alias" (複数あり得る)
    pub reasons: Vec<String>,
    /// 統一先の初期値 (曲数が一番多い表記)
    pub suggested: String,
}

/// 比較用の正規化: 全角英数→半角、ひらがな→カタカナ、小文字化、英数字とかな漢字以外を除去、先頭の "the" を除去
pub fn norm_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3041}'..='\u{3096}' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
            _ => c,
        };
        // tr: 訳さない (表記を比べるための文字)
        if c.is_alphanumeric() || c == 'ー' {
            out.extend(c.to_lowercase());
        }
    }
    let lower = s.trim_start().to_lowercase();
    if (lower.starts_with("the ") || lower.starts_with("ｔｈｅ ")) && out.len() > 3 {
        out.replace_range(..3, "");
    }
    out
}

/// "A feat. B" や "A & B" のような複数アーティストの表記
fn is_multi_artist(s: &str) -> bool {
    let l = s.to_lowercase();
    [" feat", " ft.", " & ", " x ", " × ", " with ", " vs", ", ", "、"]
        .iter()
        .any(|p| l.contains(p))
}

/// (ディスク番号, 曲番号)
type Pos = (u32, u32);

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, i: usize) -> usize {
        if self.0[i] != i {
            let r = self.find(self.0[i]);
            self.0[i] = r;
        }
        self.0[i]
    }
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[b] = a;
        }
        a != b
    }
}

/// 表記ゆれのグループを返す。`aliases` は表記 → MusicBrainz の別名一覧
pub fn find_groups(entries: &[Entry], aliases: &HashMap<String, Vec<String>>) -> Vec<VariantGroup> {
    // 表記ごとの曲数 (1 曲でアーティストとアルバムアーティストが同じなら 1 と数える)
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut names: Vec<&str> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for e in entries {
        let mut seen: [&str; 2] = ["", ""];
        for (k, n) in [e.artist, e.album_artist].into_iter().enumerate() {
            let n = n.trim();
            if n.is_empty() || seen.contains(&n) {
                continue;
            }
            seen[k] = n;
            let i = *index.entry(n).or_insert_with(|| {
                names.push(n);
                counts.push(0);
                names.len() - 1
            });
            counts[i] += 1;
        }
    }
    let mut dsu = Dsu((0..names.len()).collect());
    let mut reasons: HashMap<(usize, usize), &'static str> = HashMap::new();
    let mut link = |dsu: &mut Dsu, a: usize, b: usize, why: &'static str| {
        if dsu.union(a, b) {
            reasons.insert((a.min(b), a.max(b)), why);
        }
    };

    // 1. 正規化して一致
    let keys: Vec<String> = names.iter().map(|n| norm_key(n)).collect();
    let mut by_key: HashMap<&str, usize> = HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        if k.is_empty() {
            continue;
        }
        match by_key.get(k.as_str()) {
            Some(&j) => link(&mut dsu, j, i, "normalize"),
            None => {
                by_key.insert(k, i);
            }
        }
    }

    // 2. 同じアルバムに別表記が混ざっている
    //    "Best" のような同名アルバムを別アーティストが出している場合は曲番号が重なるので除外する
    let mut albums: BTreeMap<String, Vec<(usize, Pos)>> = BTreeMap::new();
    for e in entries {
        let artist = e.artist.trim();
        if e.compilation || artist.is_empty() || e.album.trim().is_empty() {
            continue;
        }
        let album = format!("{}\u{0}{}", norm_key(e.album), norm_key(e.album_artist));
        albums
            .entry(album)
            .or_default()
            .push((index[artist], (e.disc_nr, e.track_nr)));
    }
    for tracks in albums.values() {
        let mut by_name: BTreeMap<usize, Vec<Pos>> = BTreeMap::new();
        for (i, pos) in tracks {
            by_name.entry(*i).or_default().push(*pos);
        }
        // 表記が 2 つだけのアルバムに限る (3 つ以上はオムニバスの可能性が高い)
        let list: Vec<(&usize, &Vec<Pos>)> = by_name.iter().collect();
        let [(&a, pa), (&b, pb)] = list[..] else {
            continue;
        };
        let (na, nb) = (&keys[a], &keys[b]);
        if is_multi_artist(names[a])
            || is_multi_artist(names[b])
            || na.contains(nb.as_str())
            || nb.contains(na.as_str())
        {
            continue;
        }
        let set: HashSet<&Pos> = pa.iter().filter(|p| p.1 != 0).collect();
        if pb.iter().any(|p| p.1 != 0 && set.contains(p)) {
            continue;
        }
        link(&mut dsu, a, b, "album");
    }

    // 3. MusicBrainz の別名
    for (name, list) in aliases {
        let Some(&i) = index.get(name.trim()) else {
            continue;
        };
        for alias in list {
            if let Some(&j) = by_key.get(norm_key(alias).as_str()) {
                link(&mut dsu, i, j, "alias");
            }
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..names.len() {
        let r = dsu.find(i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<VariantGroup> = groups
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|mut g| {
            // 曲数が同じなら空白の少ない表記 ("米津 玄師" より "米津玄師") を先に
            let spaces = |i: usize| names[i].chars().filter(|c| c.is_whitespace()).count();
            g.sort_by(|&a, &b| {
                counts[b]
                    .cmp(&counts[a])
                    .then(spaces(a).cmp(&spaces(b)))
                    .then(names[a].cmp(names[b]))
            });
            let members: HashSet<usize> = g.iter().copied().collect();
            let mut why: Vec<String> = Vec::new();
            for (&(a, b), r) in &reasons {
                if members.contains(&a) && members.contains(&b) && !why.iter().any(|w| w == r) {
                    why.push(r.to_string());
                }
            }
            why.sort();
            VariantGroup {
                suggested: names[g[0]].to_string(),
                names: g
                    .iter()
                    .map(|&i| VariantName {
                        name: names[i].to_string(),
                        count: counts[i],
                    })
                    .collect(),
                reasons: why,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        let total = |g: &VariantGroup| g.names.iter().map(|n| n.count).sum::<usize>();
        total(b).cmp(&total(a)).then(a.suggested.cmp(&b.suggested))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e<'a>(artist: &'a str, album: &'a str, track_nr: u32) -> Entry<'a> {
        Entry {
            artist,
            album_artist: "",
            album,
            disc_nr: 1,
            track_nr,
            compilation: false,
        }
    }

    fn names(g: &VariantGroup) -> Vec<&str> {
        g.names.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn normalize() {
        assert_eq!(norm_key("The Beatles"), norm_key("BEATLES"));
        assert_eq!(norm_key("ＹＯＡＳＯＢＩ"), norm_key("yoasobi"));
        assert_eq!(norm_key("マキシマム ザ ホルモン"), norm_key("マキシマムザホルモン"));
        assert_eq!(norm_key("あいみょん"), norm_key("アイミョン"));
        assert_eq!(norm_key("Theatre Brook"), "theatrebrook");
    }

    #[test]
    fn groups_by_normalization() {
        let es = [e("YOASOBI", "", 0), e("yoasobi", "", 0), e("YOASOBI", "", 0), e("Ado", "", 0)];
        let g = find_groups(&es, &HashMap::new());
        assert_eq!(g.len(), 1);
        assert_eq!(names(&g[0]), ["YOASOBI", "yoasobi"]);
        assert_eq!(g[0].suggested, "YOASOBI");
        assert_eq!(g[0].reasons, ["normalize"]);

        let es = [e("米津 玄師", "", 0), e("米津玄師", "", 0)];
        assert_eq!(find_groups(&es, &HashMap::new())[0].suggested, "米津玄師");
    }

    #[test]
    fn groups_by_album() {
        let es = [
            e("マキシマム ザ ホルモン", "予襲復讐", 1),
            e("マキシマム ザ ホルモン", "予襲復讐", 2),
            e("Maximum The Hormone", "予襲復讐", 3),
        ];
        let g = find_groups(&es, &HashMap::new());
        assert_eq!(names(&g[0]), ["マキシマム ザ ホルモン", "Maximum The Hormone"]);
        assert_eq!(g[0].reasons, ["album"]);
    }

    #[test]
    fn same_album_title_by_different_artists() {
        // 別アーティストの同名アルバム: 曲番号が重なるのでまとめない
        let es = [e("A", "Best", 1), e("A", "Best", 2), e("B", "Best", 1), e("B", "Best", 2)];
        assert!(find_groups(&es, &HashMap::new()).is_empty());
        // 客演表記はまとめない
        let es = [e("A", "X", 1), e("A feat. B", "X", 2)];
        assert!(find_groups(&es, &HashMap::new()).is_empty());
        // コンピレーションはまとめない
        let mut es = [e("A", "X", 1), e("C", "X", 2)];
        es.iter_mut().for_each(|x| x.compilation = true);
        assert!(find_groups(&es, &HashMap::new()).is_empty());
    }

    #[test]
    fn groups_by_alias() {
        let es = [e("マキシマム ザ ホルモン", "A", 1), e("Maximum The Hormone", "B", 1)];
        let mut aliases = HashMap::new();
        aliases.insert(
            "マキシマム ザ ホルモン".to_string(),
            vec!["Maximum the Hormone".to_string()],
        );
        let g = find_groups(&es, &aliases);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].reasons, ["alias"]);
    }
}
