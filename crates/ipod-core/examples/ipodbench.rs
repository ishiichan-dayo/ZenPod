//! 性能の確認用: iPod OS モードの大きなライブラリで、開く・編集・保存・snapshot 相当の時間を測る
//!   ipodbench <iPod のルート (無ければ作る)> <曲のフォルダ> <入れる曲の数>
use std::path::Path;
use std::time::Instant;

use ipod_core::library::{Library, Mode};
use ipod_core::domain::track::AddOptions;

fn t<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let s = Instant::now();
    let r = f();
    eprintln!("{label:<40} {:>8.1} ms", s.elapsed().as_secs_f64() * 1000.0);
    let _ = std::fs::File::open(format!("/__phase__/{label}"));
    r
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let root = Path::new(&a[0]);
    if !root.join("iPod_Control/iTunes/iTunesDB").is_file() {
        std::fs::create_dir_all(root.join("iPod_Control/iTunes")).unwrap();
        let n: usize = a[2].parse().unwrap();
        let files: Vec<_> = walkdir::WalkDir::new(&a[1]).into_iter().flatten().filter(|e| e.file_type().is_file()).map(|e| e.into_path()).filter(|p| p.extension().is_some_and(|e| e == "mp3")).take(n).collect();
        let mut lib = Library::open(root, Mode::Ipod, None).unwrap();
        let opts = AddOptions { skip_duplicates: false, auto_reading: false, artwork: false, ..Default::default() };
        let s = Instant::now();
        for f in &files {
            lib.add_file(f, None, &opts).unwrap();
        }
        eprintln!("add {} files: {:.1} s", files.len(), s.elapsed().as_secs_f64());
        t("save (初回)", || lib.save().unwrap());
    }
    let mut lib = t("open", || Library::open(root, Mode::Ipod, None).unwrap());
    eprintln!("tracks = {}", lib.tracks().len());
    let json = t("tracks()+playlists() JSON (snapshot 相当)", || serde_json::to_vec(&(lib.tracks(), lib.playlists())).unwrap());
    eprintln!("  JSON {} KB", json.len() / 1024);
    let ids: Vec<u64> = lib.tracks().iter().take(3).map(|t| t.id.parse().unwrap()).collect();
    let pl = t("create_playlist", || lib.create_playlist("bench", &ids).unwrap());
    t("save (プレイリストを作った後)", || lib.save().unwrap());
    let all: Vec<u64> = lib.tracks().iter().map(|t| t.id.parse().unwrap()).collect();
    let big = t("create_playlist (空)", || lib.create_playlist("all", &[]).unwrap());
    t("add_to_playlist (全曲)", || lib.add_to_playlist(big.parse().unwrap(), &all).unwrap());
    t("remove_from_playlist (全曲)", || lib.remove_from_playlist(big.parse().unwrap(), &all).unwrap());
    t("rename_playlist", || lib.rename_playlist(pl.parse().unwrap(), "bench2").unwrap());
    t("save (名前を変えた後)", || lib.save().unwrap());
}
