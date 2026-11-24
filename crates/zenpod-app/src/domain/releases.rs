//! 配布リポジトリのリリース。先行版は GitHub の「プレリリース」で `releases/latest` に出ないので
//! (リリースの一覧で普通の版と同じに見えないように。2.8.10 までは普通のリリース + latest.json の `early`)、
//! 設定の「先行版」の「確認」は、リリースの一覧で一番新しい版 (プレリリースを含む) の latest.json を読む

const REPO: &str = "ishiichan-dayo/ZenPod";

/// リリースの一覧 (GitHub API)。新しい順に返る
pub fn releases_api_url() -> String {
    format!("https://api.github.com/repos/{REPO}/releases?per_page=10")
}

/// その版の latest.json (updater が読む)
pub fn latest_json_url(tag: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/{tag}/latest.json")
}

/// リリースの一覧 (API の JSON) から、下書きでない一番新しい版のタグ ("v2.8.12")。
/// URL に入れるので "v数字.数字.数字" の形のものだけ見る
pub fn newest_release_tag(releases: &serde_json::Value) -> Option<String> {
    releases
        .as_array()?
        .iter()
        .filter(|r| r["draft"] != true)
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            Some((parse_version(tag)?, tag.to_string()))
        })
        .max()
        .map(|(_, tag)| tag)
}

fn parse_version(tag: &str) -> Option<(u32, u32, u32)> {
    let mut parts = tag.strip_prefix('v')?.split('.');
    let mut num = || parts.next()?.parse::<u32>().ok();
    let v = (num()?, num()?, num()?);
    parts.next().is_none().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_newest_version_including_prereleases() {
        let list = json!([
            {"tag_name": "v2.8.13", "draft": true, "prerelease": false},
            {"tag_name": "v2.8.12", "draft": false, "prerelease": true},
            {"tag_name": "v2.8.11", "draft": false, "prerelease": false},
            {"tag_name": "v2.8.9", "draft": false, "prerelease": false},
        ]);
        assert_eq!(newest_release_tag(&list).as_deref(), Some("v2.8.12"));
        // 並びではなく版で比べる (2.8.10 > 2.8.9)
        let list = json!([{"tag_name": "v2.8.9", "draft": false}, {"tag_name": "v2.8.10", "draft": false}]);
        assert_eq!(newest_release_tag(&list).as_deref(), Some("v2.8.10"));
    }

    #[test]
    fn ignores_odd_tags() {
        let list = json!([{"tag_name": "v9.9.9/../x", "draft": false}, {"tag_name": "early", "draft": false}, {"tag_name": "v1.2", "draft": false}]);
        assert_eq!(newest_release_tag(&list), None);
        assert_eq!(newest_release_tag(&json!({"message": "rate limited"})), None);
        assert_eq!(latest_json_url("v2.8.12"), "https://github.com/ishiichan-dayo/ZenPod/releases/download/v2.8.12/latest.json");
    }
}
