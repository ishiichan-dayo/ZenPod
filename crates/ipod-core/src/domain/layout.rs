//! Rockbox モードで曲を転送するときの置き場所の形 ("Music/{albumartist}/{album}/{disctrack} {title}")。
//! "/" で区切った最後がファイル名 (拡張子なし)、その前がフォルダ。部品:
//! {artist} {albumartist} {album} {title} {genre} {year} {track} {disc} {disctrack}
//! (部品の名前は画面の言語によらない。画面では「[アルバムアーティスト]」のように見せる)

use crate::error::{Error, Result};
use crate::t;
use std::collections::HashMap;

/// 今までと同じ形 (Music/アーティスト/アルバム/01 曲名)
pub const DEFAULT: &str = "Music/{albumartist}/{album}/{disctrack} {title}";

/// よく使う形。推測にも使う (先頭の "Music" は、推測のときは iPod で使われているフォルダ名に置き換える)
pub const PRESETS: &[&str] = &[
    DEFAULT,
    "Music/{albumartist}/{album}/{disctrack} - {title}",
    "Music/{albumartist}/{album}/{title}",
    "Music/{albumartist} - {album}/{disctrack} {title}",
    "Music/{album}/{disctrack} {title}",
    "Music/{album} [{year}]/{title} - {artist}",
    "Music/{album} [{year}]/{disctrack} {title}",
    "Music/{albumartist}/{album} ({year})/{disctrack} {title}",
    "Music/{albumartist}/{year} - {album}/{disctrack} {title}",
];

const TOKENS: &[&str] = &["artist", "albumartist", "album", "title", "genre", "year", "track", "disc", "disctrack"];

/// 置き場所を決めるのに使う曲の情報
#[derive(Debug, Clone, Default)]
pub struct Fields {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub genre: String,
    pub year: u32,
    pub track: u32,
    pub disc: u32,
    /// 何枚組か (2 以上なら {disctrack} を "2-01" にする)
    pub disc_total: u32,
}

impl Fields {
    fn value(&self, token: &str) -> String {
        let or = |a: &str, b: &str| if a.trim().is_empty() { b.to_string() } else { a.to_string() };
        match token {
            "artist" => or(&self.artist, &self.album_artist),
            "albumartist" => or(&self.album_artist, &self.artist),
            "album" => self.album.clone(),
            "title" => self.title.clone(),
            "genre" => self.genre.clone(),
            "year" if self.year > 0 => self.year.to_string(),
            "track" if self.track > 0 => format!("{:02}", self.track),
            "disc" if self.disc > 0 => self.disc.to_string(),
            "disctrack" if self.track > 0 => {
                if self.disc_total > 1 && self.disc > 0 {
                    format!("{}-{:02}", self.disc, self.track)
                } else {
                    format!("{:02}", self.track)
                }
            }
            _ => String::new(),
        }
    }
}

/// 形が使えるか: 部品の名前が正しい・最後 (ファイル名) に {title} がある・空の区切りや ".." が無い・
/// iPod の管理用のフォルダ (iPod_Control / .rockbox) に置かない
pub fn validate(template: &str) -> Result<()> {
    let parts: Vec<&str> = template.split('/').map(str::trim).collect();
    if parts.len() < 2 || parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
        return Err(Error::Other(t!("フォルダとファイル名を「/」で区切って指定してください")));
    }
    if !parts.last().is_some_and(|p| p.contains("{title}")) {
        return Err(Error::Other(t!("ファイル名の部分に [タイトル] を入れてください")));
    }
    let top = parts[0].to_ascii_lowercase();
    if top.starts_with('.') || top == "ipod_control" || top.contains('{') {
        return Err(Error::Other(t!("一番上のフォルダは、決まった名前 (Music など) にしてください")));
    }
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else {
            return Err(Error::Other(t!("部品の括弧が閉じていません")));
        };
        let name = &rest[i + 1..i + j];
        if !TOKENS.contains(&name) {
            return Err(Error::Other(t!("知らない部品です: {a}", a = name)));
        }
        rest = &rest[i + j + 1..];
    }
    Ok(())
}

/// 形に曲の情報を当てはめて、フォルダとファイル名 (拡張子なし) を作る。名前はそれぞれ FAT32 で使える形に直す。
/// 空になった部品のまわりの区切り ("-"・空の括弧) は取る
pub fn render(template: &str, f: &Fields, clean: impl Fn(&str, &str) -> String) -> Vec<String> {
    template
        .split('/')
        .map(|part| {
            let mut out = String::new();
            let mut rest = part.trim();
            let mut fallback = "Unknown";
            while let Some(i) = rest.find('{') {
                out.push_str(&rest[..i]);
                let Some(j) = rest[i..].find('}') else { break };
                let name = &rest[i + 1..i + j];
                fallback = match name {
                    "album" => "Unknown Album",
                    "artist" | "albumartist" => "Unknown Artist",
                    "title" => "Track",
                    _ => fallback,
                };
                out.push_str(&f.value(name));
                rest = &rest[i + j + 1..];
            }
            out.push_str(rest);
            clean(&tidy(&out), fallback)
        })
        .collect()
}

/// 部品が空になって残った飾りを取る: 空の括弧 "[]" "()"、並んだ空白、端の "-" "." "_"
fn tidy(s: &str) -> String {
    let mut s = s.to_string();
    for empty in ["[]", "()", "[ ]", "( )"] {
        s = s.replace(empty, "");
    }
    let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let t = s.trim_matches(|c: char| c == '-' || c == '_' || c == '.' || c.is_whitespace()).to_string();
        let t = t.replace(" - - ", " - ");
        if t == s {
            break;
        }
        s = t;
    }
    s
}

/// 推測の結果
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Guess {
    pub template: String,
    /// この形に合った曲の数
    pub matched: usize,
    /// 調べた曲の数 (タグにアルバム名がある曲)
    pub total: usize,
}

/// 名前の比べ方: 小文字にして、文字と数字だけ残す (道具ごとの使えない文字の直し方の違いを無視する)
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// 今ある曲の場所 (ルートからのフォルダとファイル名 (拡張子なし)) とタグから、よく使う形のどれで並んでいるかを推測する。
/// 半分以上の曲が合う形が無ければ None
pub fn guess(tracks: &[(Vec<String>, Fields)], clean: impl Fn(&str, &str) -> String) -> Option<Guess> {
    let tracks: Vec<&(Vec<String>, Fields)> = tracks.iter().filter(|(p, f)| p.len() >= 2 && !f.album.trim().is_empty()).collect();
    if tracks.len() < 3 {
        return None;
    }
    // 一番上のフォルダ (Music / MUSIC など) は、いちばん多く使われている名前
    let mut tops: HashMap<&str, usize> = HashMap::new();
    for (p, _) in &tracks {
        *tops.entry(p[0].as_str()).or_default() += 1;
    }
    let top = tops.into_iter().max_by_key(|(_, n)| *n).map(|(t, _)| t.to_string())?;
    // 曲の場所は形ごとに比べ直さないよう、先に 1 回だけ比べる形にしておく (2 万曲 × 形の数で重かった)
    let paths: Vec<Vec<String>> = tracks.iter().map(|(p, _)| p.iter().map(|x| norm(x)).collect()).collect();
    let total = tracks.len();
    let mut best: Option<Guess> = None;
    for preset in PRESETS {
        let template = format!("{top}/{}", preset.split_once('/').map(|x| x.1).unwrap_or(preset));
        let depth = template.split('/').count();
        // 選ばれるには、半分以上かつ今までで一番多く合う必要がある。残りが全部合っても届かなくなったら打ち切る
        let need = best.as_ref().map_or(0, |b| b.matched + 1).max(total.div_ceil(2)).max(1);
        let mut matched = 0;
        for (k, ((_, f), path)) in tracks.iter().zip(&paths).enumerate() {
            if matched + (total - k) < need {
                break;
            }
            // 深さが違えば当てはめるまでもない
            if path.len() != depth {
                continue;
            }
            let want = render(&template, f, &clean);
            if want.len() == path.len() && want.iter().zip(path).all(|(a, b)| norm(a) == *b) {
                matched += 1;
            }
        }
        if matched >= need {
            best = Some(Guess { template, matched, total });
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(s: &str, fallback: &str) -> String {
        let s = s.replace(['/', ':', '?'], "_");
        if s.trim().is_empty() { fallback.to_string() } else { s }
    }

    fn fields(artist: &str, album: &str, title: &str, year: u32, track: u32) -> Fields {
        Fields { artist: artist.into(), album: album.into(), title: title.into(), year, track, disc: 1, disc_total: 1, ..Default::default() }
    }

    #[test]
    fn renders_and_tidies() {
        let f = fields("Future", "I NEVER LIKED YOU", "712PM", 2022, 1);
        assert_eq!(render(DEFAULT, &f, clean), ["Music", "Future", "I NEVER LIKED YOU", "01 712PM"]);
        assert_eq!(render("Music/{album} [{year}]/{title} - {artist}", &f, clean), ["Music", "I NEVER LIKED YOU [2022]", "712PM - Future"]);
        // 年も曲順も無い曲: 空の括弧や端の区切りは残さない
        let g = fields("A", "B", "C", 0, 0);
        assert_eq!(render("Music/{album} [{year}]/{disctrack} - {title}", &g, clean), ["Music", "B", "C"]);
        // アルバムが無ければ Unknown Album、複数枚組は 2-01
        let mut h = fields("A", "", "C", 0, 3);
        h.disc = 2;
        h.disc_total = 2;
        assert_eq!(render(DEFAULT, &h, clean), ["Music", "A", "Unknown Album", "2-03 C"]);
    }

    #[test]
    fn validates() {
        assert!(validate(DEFAULT).is_ok());
        assert!(PRESETS.iter().all(|p| validate(p).is_ok()));
        assert!(validate("Music/{album}").is_err(), "ファイル名に曲名が無い");
        assert!(validate("{album}/{title}").is_err(), "一番上が部品");
        assert!(validate("iPod_Control/{title}").is_err());
        assert!(validate("Music//{title}").is_err());
        assert!(validate("Music/{albm}/{title}").is_err());
    }

    #[test]
    fn guesses_layout_from_existing_files() {
        // SpotiFLAC-Next の形: MUSIC/アルバム [年]/曲名 - アーティスト (一番上のフォルダ名も合わせる)
        let mut tracks = Vec::new();
        for (album, year, title, artist) in [
            ("I NEVER LIKED YOU", 2022, "712PM", "Future"),
            ("I NEVER LIKED YOU", 2022, "I'M DAT N****", "Future"),
            ("ASTROWORLD", 2018, "SICKO MODE", "Travis Scott"),
            ("2001", 1999, "Still D.R.E.", "Dr. Dre"),
        ] {
            let f = fields(artist, album, title, year, 1);
            let path = vec!["MUSIC".to_string(), format!("{album} [{year}]"), format!("{} - {artist}", title.replace('*', "_"))];
            tracks.push((path, f));
        }
        let g = guess(&tracks, clean).unwrap();
        assert_eq!(g.template, "MUSIC/{album} [{year}]/{title} - {artist}");
        assert_eq!((g.matched, g.total), (4, 4));
        // 今までの形はそのまま当たる
        let t: Vec<_> = tracks.iter().map(|(_, f)| (render(DEFAULT, f, clean), f.clone())).collect();
        assert_eq!(guess(&t, clean).unwrap().template, DEFAULT);
        // ばらばらなら推測しない
        let mess: Vec<_> = tracks.iter().enumerate().map(|(i, (_, f))| (vec!["Music".into(), format!("x{i}")], f.clone())).collect();
        assert_eq!(guess(&mess, clean), None);
    }
}
