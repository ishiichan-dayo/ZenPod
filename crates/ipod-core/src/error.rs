//! エラーの文言は表示するときに訳す (言語は画面が起動時に決める)

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}", crate::t!("I/O エラー: {e}", e = .0))]
    Io(#[from] std::io::Error),
    #[error("{}", crate::t!("iTunesDB が壊れています: {e}", e = .0))]
    Corrupt(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{}", crate::t!("メタデータを読めません: {e}", e = .0))]
    Media(String),
    #[error("{}", crate::t!("変換に失敗しました: {e}", e = .0))]
    Convert(String),
    /// 転送の途中で iPod が見えなくなった (残りの転送を止める)
    #[error("{}", crate::t!("iPod が切断されました"))]
    Disconnected,
    /// iPod の空き容量が足りない (残りの転送を止める)
    #[error("{}", crate::t!("iPod の空き容量が足りません"))]
    NoSpace,
    /// 中止した (変換の途中など)
    #[error("{}", crate::t!("中止しました"))]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
