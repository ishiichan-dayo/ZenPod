//! 転送の前に、曲を一時ファイルへ変換しておく (配布リポジトリの Issue #29)。
//!
//! ffmpeg の AAC の変換は 1 つのスレッドでしか動かないので、1 曲ずつ変換すると CPU の 1 コアしか使わない。
//! 転送する側 (zenpod-app の transfer) は、先の曲をいくつか同時にここで変換しておき、iPod への書き込みは
//! 1 曲ずつ順に `Library::add_file_preconverted` で行う。どの形式にするかは `Library::conversion_plan` が決める
//! (重複で送らない曲・変換の要らない曲は None)。

use std::path::{Path, PathBuf};

use crate::domain::audio::ConvertTarget;
use crate::error::Result;

/// 変換のしかた (`Library::conversion_plan` が作る)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionPlan {
    pub target: ConvertTarget,
    pub(crate) sample_rate: u32,
    /// Mac の afconvert を使ってよいか (iPod OS だけ。Rockbox は ffmpeg だけで変換している)
    pub(crate) afconvert: bool,
}

impl ConversionPlan {
    /// 一時ファイルへ変換する。失敗したら一時ファイルは残さない
    pub fn run(&self, src: &Path, ffmpeg: Option<&Path>) -> Result<Preconverted> {
        let pre = Preconverted { path: temp_path(self.target.ext())?, plan: *self };
        crate::ipod::convert_for_transfer(src, &pre.path, self.target, self.sample_rate, self.afconvert, ffmpeg)?;
        Ok(pre)
    }
}

/// 変換しておいた一時ファイル。捨てるとファイルも消す
#[derive(Debug)]
pub struct Preconverted {
    path: PathBuf,
    plan: ConversionPlan,
}

impl Preconverted {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 書き込むときに決め直した変換と同じなら使える (間に設定や iPod の曲が変わったら使わない)
    pub(crate) fn matches(&self, plan: &ConversionPlan) -> bool {
        self.plan == *plan
    }
}

impl Drop for Preconverted {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 変換の一時ファイルの場所 (`<一時フォルダ>/zenpod/<乱数>.<拡張子>`)
pub(crate) fn temp_path(ext: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("zenpod");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{:016x}.{ext}", crate::ipod::itunesdb::random_id())))
}
