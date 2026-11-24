//! 性能の確認用: iTunesDB の読み込み・書き出し・署名の時間
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let data = std::fs::read(&path).unwrap();
    let t = std::time::Instant::now();
    let mut db = ipod_core::ipod::itunesdb::ITunesDb::parse(&data).unwrap();
    eprintln!("parse      {:6.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    // 2 つ目に場所を渡すと、書き出した結果をそこに保存する (直す前と後で同じか比べる)
    if let Some(out) = std::env::args().nth(2) {
        std::fs::write(out, db.serialize()).unwrap();
    }
    for _ in 0..3 {
        let t = std::time::Instant::now();
        let out = db.serialize();
        eprintln!("serialize  {:6.1} ms  ({} KB)", t.elapsed().as_secs_f64() * 1000.0, out.len() / 1024);
        let mut o = out.clone();
        let t = std::time::Instant::now();
        ipod_core::ipod::hash58::write_hash58(&mut o, &[0, 0x0A, 0x27, 0, 0x1A, 0x2B, 0x3C, 0x4D]);
        eprintln!("hash58     {:6.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    }
}
