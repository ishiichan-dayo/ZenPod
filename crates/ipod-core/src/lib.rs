//! iPod classic などの曲とプレイリストを、iPod OS (iTunesDB) と Rockbox の両方の形で扱うライブラリ。
//!
//! - `domain`: 外の世界を知らない型と決まりごと (曲・プレイリスト・形式・表記ゆれ・置き場所の形・読み)
//! - `library`: 開いた iPod の窓口。iPod OS と Rockbox を同じ操作で扱う (`LibraryBackend` を両方が実装する)
//! - `ipod`: iPod OS (Apple のファームウェア) の iTunesDB・署名 (hash58)・ArtworkDB
//! - `rockbox`: Rockbox の曲 (iPod の中のファイルそのもの) とデータベース (tagcache)
//! - `smartpl`: スマートプレイリストの条件 (iTunesDB の書式と iTunes の XML の両方)
//! - `device`: iPod のドライブの検出・機種の判定・取り出し
//! - `media`: 音声ファイルのタグ・ジャケット・変換 (lofty / ffmpeg)
//! - `pc`: PC の音楽フォルダ・iTunes / ミュージックの XML と .m3u
//! - `online`: ネットのジャケットと曲情報の検索

pub mod device;
mod dirlist;
pub mod domain;
pub mod error;
pub mod i18n;
pub mod ipod;
pub mod library;
pub mod media;
pub mod online;
pub mod pc;
pub mod rockbox;
pub mod smartpl;

pub use error::{Error, Result};
pub use dirlist::dir_size;
pub use ipod::Ipod;
