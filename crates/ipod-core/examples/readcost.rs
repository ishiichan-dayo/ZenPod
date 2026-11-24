//! 性能の確認用: 1 曲のタグを読むのに、ファイルをどれだけ読むか (LD_PRELOAD の数える仕掛けと使う)
fn main() {
    for f in std::env::args().skip(1) {
        let p = std::path::Path::new(&f);
        let size = std::fs::metadata(p).unwrap().len();
        let _ = std::fs::File::open("/__phase__/start");
        let _ = ipod_core::media::read_info(p);
        let _ = std::fs::File::open(format!("/__phase__/read_info {f}"));
        let _ = ipod_core::media::read_release_type(p);
        let _ = std::fs::File::open(format!("/__phase__/read_release_type {f}"));
        let _ = ipod_core::pc::folder::read_track(p, size);
        let _ = std::fs::File::open(format!("/__phase__/read_track (合計) {f}  大きさ {} KB", size / 1024));
        let _ = ipod_core::media::cover::find_cover(p);
        let _ = std::fs::File::open(format!("/__phase__/find_cover {f}"));
    }
}
