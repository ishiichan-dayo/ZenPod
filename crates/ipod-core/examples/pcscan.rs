//! PC の音楽フォルダの読み込みの時間を測る (配布リポジトリの Issue #16、NAS のフォルダが遅い)。
//! `cargo run --release -p ipod-core --example pcscan -- <フォルダ> <キャッシュの場所>`
//! キャッシュの場所のファイルを消してから動かすと初回、残したまま動かすと 2 回目以降の時間になる
use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, cache] = args.as_slice() else { panic!("使い方: pcscan <フォルダ> <キャッシュの場所>") };
    let started = Instant::now();
    let tracks = ipod_core::pc::folder::scan(Path::new(root), Some(Path::new(cache)), |_, _| {}).unwrap();
    println!("{} 曲 {:.2} 秒", tracks.len(), started.elapsed().as_secs_f64());
}
