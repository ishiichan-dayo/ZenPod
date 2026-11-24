//! 性能の確認用: Rockbox モードの大きなライブラリで、開く・転送・編集・保存・DB の作り直しの時間を測る
//!   rbbench <iPod のルート> <キャッシュのフォルダ> <転送する曲>...
use std::path::Path;
use std::time::Instant;

use ipod_core::domain::track::{AddOptions, TagEdit};
use ipod_core::rockbox::RockboxLib;

fn t<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let s = Instant::now();
    let r = f();
    eprintln!("{label:<40} {:>8.1} ms", s.elapsed().as_secs_f64() * 1000.0);
    // LD_PRELOAD の数える仕掛けに区切りを知らせる (無ければ何もしない)
    let _ = std::fs::File::open(format!("/__phase__/{label}"));
    r
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (root, cache) = (Path::new(&a[0]), Path::new(&a[1]).to_path_buf());
    // アプリと同じく、キャッシュはフォルダの中の iPod ごとのファイル (open_in)
    let mut lib = t("open (キャッシュの状態のまま)", || RockboxLib::open_in(root, Some(cache.clone()), &|_, _| {}).unwrap());
    let _ = t("open (2 回目)", || RockboxLib::open_in(root, Some(cache.clone()), &|_, _| {}).unwrap());
    let cache = lib.cache_path().map(Path::to_path_buf).unwrap();
    eprintln!("tracks = {}", lib.tracks().len());
    let json = t("tracks() + JSON (snapshot 相当)", || serde_json::to_vec(&lib.tracks()).unwrap());
    eprintln!("  JSON {} KB", json.len() / 1024);
    t("rbdb::rebuild (単体)", || ipod_core::rockbox::rbdb::rebuild(root, Some(&cache)).unwrap());
    let opts = AddOptions { skip_duplicates: std::env::var_os("DUP").is_some(), ..Default::default() };
    for f in &a[2..] {
        t(&format!("add_file {}", Path::new(f).file_name().unwrap().to_string_lossy()), || {
            lib.add_file(Path::new(f), None, &opts).unwrap()
        });
    }
    t("save (転送の後: DB の作り直しを含む)", || lib.save().unwrap());
    let id: u64 = lib.tracks()[0].id.parse().unwrap();
    t("edit_tags 1 曲", || lib.edit_tags(&[id], &TagEdit { genre: Some("Bench".into()), ..Default::default() }).unwrap());
    t("save (1 曲の編集の後)", || lib.save().unwrap());
    t("rockbox status (snapshot ごと)", || lib.status());
    // 歌詞 (.lrc) を探すようになった操作: 名前の変更・種類のフォルダへの移動・削除 (1000 曲ずつ)
    let ids: Vec<u64> = lib.tracks().iter().take(1000).map(|t| t.id.parse().unwrap()).collect();
    let renames: Vec<(u64, String)> = ids.iter().enumerate().map(|(i, id)| (*id, format!("renamed {i}"))).collect();
    t("rename_files 1000 曲", || lib.rename_files(&renames).unwrap());
    let ids: Vec<u64> = lib.tracks().iter().take(1000).map(|t| t.id.parse().unwrap()).collect();
    let kind = TagEdit { kind: Some(ipod_core::domain::audio::MediaKind::Podcast), ..Default::default() };
    t("edit_tags 種類の変更 1000 曲", || lib.edit_tags(&ids, &kind).unwrap());
    let ids: Vec<u64> = lib.tracks().iter().take(1000).map(|t| t.id.parse().unwrap()).collect();
    t("remove_tracks 1000 曲", || lib.remove_tracks(&ids).unwrap());
    t("save (削除の後)", || lib.save().unwrap());
}
