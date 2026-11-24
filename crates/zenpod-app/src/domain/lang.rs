//! 画面が起動する前 (起動時の確認ダイアログなど) に使う言語

/// 設定で選んだ言語。無ければ OS の言語 (日本語なら "ja"、それ以外は "en")
pub fn initial_lang(pref: Option<String>, os_locale: &str) -> String {
    pref.unwrap_or_else(|| if os_locale.to_lowercase().starts_with("ja") { "ja".into() } else { "en".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pref_wins_then_os() {
        assert_eq!(initial_lang(Some("en".into()), "ja-JP"), "en");
        assert_eq!(initial_lang(None, "ja-JP"), "ja");
        assert_eq!(initial_lang(None, "JA"), "ja");
        assert_eq!(initial_lang(None, "fr-FR"), "en");
        assert_eq!(initial_lang(None, ""), "en");
    }
}
