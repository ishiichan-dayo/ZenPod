//! 文言の翻訳 (画面側の src/i18n.ts と同じ仕組み・同じ辞書)。
//! 日本語の文言そのものをキーにして、辞書 (src/locales/*.json、ビルド時に組み込む) から引く。
//! 辞書に無い文言は日本語のまま返す。
//!
//! - 値の埋め込み: `t!("{n} 曲を更新しました", n = count)`
//! - 英語の単数 / 複数: 訳を "{n} song|{n} songs" のように | で分けると、n が 1 なら前、それ以外は後
//! - 言語は画面が起動時に `set_lang` で伝える (設定 / OS の言語)

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

const EN_JSON: &str = include_str!("../../../src/locales/en.json");

/// 0 = 日本語, 1 = 英語
static LANG: AtomicU8 = AtomicU8::new(0);

fn en() -> &'static HashMap<String, String> {
    static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
    EN.get_or_init(|| serde_json::from_str(EN_JSON).unwrap_or_default())
}

/// "ja" なら日本語、それ以外は英語
pub fn set_lang(code: &str) {
    LANG.store(if code == "ja" { 0 } else { 1 }, Ordering::Relaxed);
}

pub fn lang() -> &'static str {
    if LANG.load(Ordering::Relaxed) == 0 { "ja" } else { "en" }
}

fn lookup(key: &str) -> &str {
    match LANG.load(Ordering::Relaxed) {
        0 => key,
        _ => en().get(key).map(String::as_str).unwrap_or(key),
    }
}

pub fn tr(key: &str) -> String {
    lookup(key).to_string()
}

/// 値を埋め込む。vars は (名前, 値)。n があれば単数 / 複数を選ぶ
pub fn tr_vars(key: &str, vars: &[(&str, String)]) -> String {
    let mut s = lookup(key).to_string();
    if s.contains('|') {
        let n = vars
            .iter()
            .find(|(k, _)| *k == "n")
            .and_then(|(_, v)| v.replace([',', ' '], "").parse::<f64>().ok());
        if let Some(n) = n {
            let (one, many) = s.split_once('|').unwrap_or((&s, &s));
            s = if n == 1.0 { one } else { many }.to_string();
        }
    }
    for (k, v) in vars {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// `t!("文言")` / `t!("{n} 曲", n = x)`
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::tr($key)
    };
    ($key:literal, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::i18n::tr_vars($key, &[$((stringify!($name), ($val).to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn plural_and_vars() {
        super::set_lang("en");
        assert_eq!(crate::t!("{n} 曲", n = 1), "1 song");
        assert_eq!(crate::t!("{n} 曲", n = "1,234"), "1,234 songs");
        assert_eq!(crate::t!("辞書に無い文言"), "辞書に無い文言");
        super::set_lang("ja");
        assert_eq!(crate::t!("{n} 曲", n = 3), "3 曲");
    }
}
