//! タグを読むときに読む量の確認: 画像まで読む・lofty の画像を読まない設定・fastread の比べ
//!   covercost <ファイル>...   (LD_PRELOAD の数える仕掛けと一緒に使う)
use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    for f in &files {
        let t = lofty::read_from_path(f).unwrap();
        let pics: usize = t.tags().iter().map(|x| x.pictures().len()).sum();
        let _ = std::fs::File::open(format!("/__phase__/full {f} pics={pics}"));
        let t = Probe::open(f).unwrap().options(ParseOptions::new().read_cover_art(false)).read().unwrap();
        let pics: usize = t.tags().iter().map(|x| x.pictures().len()).sum();
        let _ = std::fs::File::open(format!("/__phase__/nocover {f} pics={pics}"));
        let i = ipod_core::media::fastread::read_info(std::path::Path::new(f)).unwrap();
        let _ = std::fs::File::open(format!("/__phase__/fastread {f} pic={} {:?} {}ms", i.has_picture, i.title, i.duration_ms));
    }
}
