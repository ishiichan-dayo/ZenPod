//! 再生用に返す曲のデータの範囲 (HTTP の Range: bytes=start-[end])

/// 終わりが無い要求 (と Range の無い要求) には、この大きさずつ返す (シークしても全体を読まない)
pub const CHUNK: u64 = 2 * 1024 * 1024;

/// 返す範囲 (両端を含む)。None は範囲が合わない (416)。`total` は 1 以上
pub fn resolve(header: Option<&str>, total: u64) -> Option<(u64, u64)> {
    let last = total - 1;
    let (start, end) = match header.and_then(parse) {
        Some((a, b)) => (a, b.unwrap_or(a.saturating_add(CHUNK - 1)).min(last)),
        None => (0, (CHUNK - 1).min(last)),
    };
    (start <= end).then_some((start, end))
}

/// "bytes=start-[end]" を読む。読めなければ None (範囲の指定が無いものとして扱う)
fn parse(v: &str) -> Option<(u64, Option<u64>)> {
    let (a, b) = v.strip_prefix("bytes=")?.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse::<u64>().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        let total = 10 * CHUNK;
        assert_eq!(resolve(None, total), Some((0, CHUNK - 1)));
        assert_eq!(resolve(None, 100), Some((0, 99)));
        assert_eq!(resolve(Some("bytes=10-19"), total), Some((10, 19)));
        assert_eq!(resolve(Some("bytes=10-"), total), Some((10, 10 + CHUNK - 1)));
        assert_eq!(resolve(Some("bytes=90-200"), 100), Some((90, 99)));
        assert_eq!(resolve(Some("bytes=100-"), 100), None);
        assert_eq!(resolve(Some("bytes=-5"), 100), Some((0, 99)));
        assert_eq!(resolve(Some("items=1-2"), 100), Some((0, 99)));
        assert_eq!(resolve(Some(&format!("bytes={}-", u64::MAX)), 100), None);
    }
}
