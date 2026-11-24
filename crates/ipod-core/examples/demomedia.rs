//! README の撮影用: デモの iPod に動画と写真を入れる (アプリの転送と同じ API)。
//!
//!   cargo run -p ipod-core --example demomedia -- <iPod のフォルダ> <ipod|rockbox> <素材のフォルダ>
//!
//! 素材は `<素材>/videos/*.mp4` と `<素材>/photos/<アルバム>/*.jpg`。ffmpeg は PATH のもの

use std::path::{Path, PathBuf};

use ipod_core::domain::video::{VideoOptions, VideoOutcome};
use ipod_core::library::{Library, Mode};
use ipod_core::media::video::Progress;

const NO_PROGRESS: Progress = Progress { report: &|_| {}, cancelled: &|| false };

fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    v.sort();
    v
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, mode, src] = args.as_slice() else { panic!("使い方: demomedia <iPod> <ipod|rockbox> <素材>") };
    let mode = if mode == "rockbox" { Mode::Rockbox } else { Mode::Ipod };
    let ffmpeg = std::env::split_paths(&std::env::var_os("PATH").unwrap()).map(|d| d.join("ffmpeg")).find(|f| f.is_file());
    let src = Path::new(src);
    let mut lib = Library::open(Path::new(root), mode, None).unwrap();
    let opts = VideoOptions::default();
    for v in sorted(&src.join("videos")) {
        match lib.add_video(&v, ffmpeg.as_deref(), &opts, &NO_PROGRESS).unwrap() {
            VideoOutcome::Added { video, converted } => println!("動画 {} ({:?}, 変換 {converted})", video.title, video.kind),
            other => println!("動画 {}: {other:?}", v.display()),
        }
    }
    for album in sorted(&src.join("photos")) {
        let name = album.file_name().unwrap().to_string_lossy().into_owned();
        for p in sorted(&album) {
            lib.add_photo(&p, Some(&name), ffmpeg.as_deref()).unwrap();
        }
        println!("写真 {name}");
    }
    lib.save().unwrap();
}
