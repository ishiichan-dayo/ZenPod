//! ユースケースの失敗。画面には `to_string()` (訳した文言) で渡す。
//! 文言は分ける前の src-tauri と同じにしてある (画面のエラー表示がそのまま使えるように)

use ipod_core::t;

#[derive(Debug)]
pub enum AppError {
    /// iPod を開いていない
    NotConnected,
    /// 転送などの長い処理が iPod を使っている
    Busy,
    Core(ipod_core::Error),
    /// その場で作った文言 (訳してあるもの)。I/O などの失敗はそのままの文言で入れる
    Message(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn msg(m: impl Into<String>) -> Self {
        AppError::Message(m.into())
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::NotConnected => f.write_str(&t!("iPod が接続されていません")),
            AppError::Busy => f.write_str(&t!("転送中です。完了までお待ちください")),
            AppError::Core(e) => write!(f, "{e}"),
            AppError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for AppError {}

impl From<ipod_core::Error> for AppError {
    fn from(e: ipod_core::Error) -> Self {
        AppError::Core(e)
    }
}

/// `std::fs` の失敗は ipod-core の `Error::Io` (「I/O エラー: 」が前に付く) にせず、OS の文言のまま出す
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Message(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Message(e.to_string())
    }
}

/// Tauri のコマンドは文字列でエラーを返す
impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_keep_os_message() {
        let e = std::io::Error::other("boom");
        assert_eq!(String::from(AppError::from(e)), "boom");
    }
}
