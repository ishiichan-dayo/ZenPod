//! ZenPod のアプリケーション層。Tauri にも画面にも依存しない (やり取りは src-tauri が受け持つ)。
//!
//! 依存の向きは外から内への一方向だけ:
//! - `domain`: 外の世界を知らない決まりごと (ID の形、書き出すファイル名、PC の曲を触ってよい範囲など)
//! - `application`: ユースケース。ファイル・通知・別のプロセスには `application::ports` の trait 越しにだけ触る
//! - `infrastructure`: ポートの実装のうち Tauri が要らないもの (設定のファイル、縮小画像のキャッシュなど)
//!
//! iTunesDB・Rockbox・タグ・ネットの検索は今は ipod-core を直接呼ぶ (ipod-core の整理は別の段で行う)。

pub mod application;
pub mod domain;
pub mod error;
pub mod infrastructure;

pub use error::{AppError, AppResult};
