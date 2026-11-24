//! 「このアプリについて」から既定のブラウザで開いてよいリンク。画面から任意の URL を開けないよう、決めた先だけにする

const ALLOWED: &[&str] = &[
    "https://github.com/ishiichan-dayo/ZenPod",
    "https://ko-fi.com/ishiichan_dayo",
];

pub fn is_allowed_link(url: &str) -> bool {
    ALLOWED.iter().any(|a| url == *a || url.starts_with(&format!("{a}/"))) && is_plain_link(url)
}

/// 空白・引用符・制御文字・\ を含まず、URL のパスに「..」「.」(%2e も) の区切りが無い。
/// ブラウザは「..」を解くので、決めた先の外へ出られるため (`ZenPod/../other`)
fn is_plain_link(url: &str) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '`' | '\\' | '<' | '>')) {
        return false;
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    !path.split('/').any(|seg| {
        let seg = seg.to_ascii_lowercase().replace("%2e", ".");
        seg == "." || seg == ".."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listed_sites() {
        assert!(is_allowed_link("https://github.com/ishiichan-dayo/ZenPod"));
        assert!(is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/releases"));
        assert!(is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/releases?q=a..b#x"));
        assert!(is_allowed_link("https://ko-fi.com/ishiichan_dayo"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod-other"));
        assert!(!is_allowed_link("https://example.com/"));
    }

    #[test]
    fn no_escape_from_listed_sites() {
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/../other"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/%2E%2E/other"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/.%2e/other"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/./x"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/\\..\\other"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/ x"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/\"x"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/'x"));
        assert!(!is_allowed_link("https://github.com/ishiichan-dayo/ZenPod/\nx"));
    }
}
