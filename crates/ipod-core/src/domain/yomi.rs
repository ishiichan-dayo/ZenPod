//! 日本語の曲名・アーティスト名から、iPod のソート用「読み」(カタカナ) を生成する。
//!
//! iPod 本体は漢字をコードポイント順に並べるため、「椎名林檎」がシ行に来ない。
//! ソート用フィールド (sort artist 等) に読みを入れると五十音順に並ぶ。

use std::borrow::Cow;
use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

fn segmenter() -> Option<&'static Segmenter> {
    static SEG: OnceLock<Option<Segmenter>> = OnceLock::new();
    SEG.get_or_init(|| {
        let dict = load_dictionary("embedded://ipadic").ok()?;
        Some(Segmenter::new(Mode::Normal, dict, None))
    })
    .as_ref()
}

fn is_kanji(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' | '々' | '〆' | 'ヶ')
}

fn is_hiragana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{3096}' | 'ゝ' | 'ゞ')
}

fn is_katakana(c: char) -> bool {
    matches!(c, '\u{30A1}'..='\u{30FA}' | 'ー' | 'ヽ' | 'ヾ')
}

/// 漢字またはひらがなを含むか (カタカナだけ・英数字だけなら読み不要)
pub fn needs_reading(s: &str) -> bool {
    s.chars().any(|c| is_kanji(c) || is_hiragana(c))
}

pub fn hira_to_kata(s: &str) -> String {
    s.chars()
        .map(|c| {
            if matches!(c, '\u{3041}'..='\u{3096}' | 'ゝ' | 'ゞ') {
                char::from_u32(c as u32 + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// 文字列の読み (カタカナ)。読みが不要な文字列は None。
/// 英数字・記号はそのまま残す (例: "無罪モラトリアム" → "ムザイモラトリアム")。
pub fn reading(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || !needs_reading(text) {
        return None;
    }
    let seg = segmenter()?;
    let mut out = String::with_capacity(text.len());
    // 形態素解析は空白を捨てるので、空白で区切って読みを作り元の空白で繋ぎ直す
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let ws = c.is_whitespace();
        let end = rest
            .find(|c: char| c.is_whitespace() != ws)
            .unwrap_or(rest.len());
        let (chunk, tail) = rest.split_at(end);
        if ws {
            out.push_str(chunk);
        } else {
            push_reading(seg, chunk, &mut out)?;
        }
        rest = tail;
    }
    (out != text).then_some(out)
}

fn push_reading(seg: &Segmenter, text: &str, out: &mut String) -> Option<()> {
    let tokens = seg.segment(Cow::Borrowed(text)).ok()?;
    for mut t in tokens {
        let surface = t.surface.to_string();
        if !surface.chars().any(is_kanji) {
            // かな・英数字は辞書の読みより表記を優先 (英単語の誤読を防ぐ)
            out.push_str(&hira_to_kata(&surface));
            continue;
        }
        let details = t.details();
        match details.get(7).map(|s| -> &str { s }) {
            Some(r)
                if r != "*"
                    && !r.is_empty()
                    && r.chars().all(|c| is_katakana(c) || is_hiragana(c)) =>
            {
                out.push_str(&hira_to_kata(r))
            }
            _ => out.push_str(&surface),
        }
    }
    Some(())
}

/// ジャンプテーブル用に、濁点・半濁点・小書きを外した清音カタカナにする
pub fn base_kana(c: char) -> char {
    const DAKU: &str = "ガギグゲゴザジズゼゾダヂヅデドバビブベボ";
    const SEI: &str = "カキクケコサシスセソタチツテトハヒフヘホ";
    const HANDAKU: &str = "パピプペポ";
    const SMALL: &str = "ァィゥェォッャュョヮヵヶ";
    const LARGE: &str = "アイウエオツヤユヨワカケ";
    let c = if is_hiragana(c) {
        char::from_u32(c as u32 + 0x60).unwrap_or(c)
    } else {
        c
    };
    for (list, to) in [(DAKU, SEI), (HANDAKU, "ハヒフヘホ"), (SMALL, LARGE)] {
        if let Some(i) = list.chars().position(|x| x == c) {
            return to.chars().nth(i).unwrap_or(c);
        }
    }
    if c == 'ヴ' {
        return 'ウ';
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings() {
        for (src, want) in [
            ("椎名林檎", "シイナリンゴ"),
            ("宇多田ヒカル", "ウタダヒカル"),
            ("無罪モラトリアム", "ムザイモラトリアム"),
            ("丸の内サディスティック", "マルノウチサディスティック"),
            ("ここでキスして。", "ココデキスシテ。"),
            (
                "Maximum The Hormone II ～これからの麺カタコッテリの話をしよう～",
                "Maximum The Hormone II ～コレカラノメンカタコッテリノハナシヲシヨウ～",
            ),
            ("東京　事変", "トウキョウ　ジヘン"),
        ] {
            assert_eq!(reading(src).as_deref(), Some(want), "{src}");
        }
        assert_eq!(reading("THE BOOK"), None);
        assert_eq!(reading("モラトリアム"), None);
    }

    #[test]
    fn kana_base() {
        assert_eq!(base_kana('ガ'), 'カ');
        assert_eq!(base_kana('ぱ'), 'ハ');
        assert_eq!(base_kana('ッ'), 'ツ');
        assert_eq!(base_kana('ア'), 'ア');
        assert_eq!(base_kana('A'), 'A');
    }
}

#[cfg(test)]
mod probe {
    #[test]
    #[ignore]
    fn print_samples() {
        for s in [
            "米津玄師",
            "感電",
            "夜に駆ける",
            "東京事変",
            "中島みゆき",
            "山下達郎",
            "久石譲",
            "坂本龍一",
            "藍井エイル",
            "千と千尋の神隠し",
            "紅蓮華",
            "残酷な天使のテーゼ",
            "あいみょん",
            "マリーゴールド",
            "星野源",
            "恋",
            "小沢健二",
            "今夜はブギー・バック",
            "松任谷由実",
            "安全地帯",
        ] {
            println!("{s} => {:?}", super::reading(s));
        }
    }
}
