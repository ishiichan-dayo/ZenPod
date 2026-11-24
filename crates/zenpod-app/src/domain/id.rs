//! 画面から文字列で渡される曲・プレイリストの ID (iTunesDB の dbid か、Rockbox の曲のパスのハッシュ。どちらも u64)

use crate::error::{AppError, AppResult};
use ipod_core::t;

pub fn parse_id(id: &str) -> AppResult<u64> {
    id.parse::<u64>().map_err(|_| AppError::msg(t!("不正な ID: {id}", id = id)))
}

pub fn parse_ids(ids: &[String]) -> AppResult<Vec<u64>> {
    ids.iter()
        .map(|s| s.parse::<u64>().map_err(|_| AppError::msg(t!("不正な ID: {s}", s = s))))
        .collect()
}

pub fn parse_opt_id(id: Option<&str>) -> AppResult<Option<u64>> {
    id.map(parse_id).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ids() {
        assert_eq!(parse_id("42").unwrap(), 42);
        assert_eq!(parse_ids(&["1".into(), "18446744073709551615".into()]).unwrap(), vec![1, u64::MAX]);
        assert_eq!(parse_opt_id(None).unwrap(), None);
        assert!(parse_id("-1").is_err());
        assert!(parse_ids(&["1".into(), "x".into()]).is_err());
    }
}
