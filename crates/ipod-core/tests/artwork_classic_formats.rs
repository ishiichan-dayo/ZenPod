//! iPod classic の小さいジャケット (1061) を、本体の形 (55×55 を 112 バイトの行で 55 行 = 6160 バイト) に直す
//! (配布リポジトリの Issue #12: アルバムの一覧と歌詞の画面にジャケットが出なかった)。
//! 以前の版が書いた 2 つの形の ArtworkDB を作り、開いて repair_artwork で直したあとの中身を確かめる

use std::path::Path;

use ipod_core::Ipod;
use ipod_core::ipod::artwork::{self, ArtFormat, ArtworkDb, CLASSIC, Pix, Thumb};

const SMALL: u32 = 1061;

/// 偽の iPod classic (DB 署名あり)
fn fake_classic(dir: &Path) {
    std::fs::create_dir_all(dir.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Device")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control/Music/F00")).unwrap();
    std::fs::write(dir.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xB029\nFirewireGuid: 0x000A27001A2B3C4D\n").unwrap();
}

fn jpeg(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(64, 64, image::Rgb(rgb));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

fn art_dir(root: &Path) -> std::path::PathBuf {
    root.join("iPod_Control/Artwork")
}

fn write_db(root: &Path, db: &ArtworkDb) {
    std::fs::write(art_dir(root).join("ArtworkDB"), db.serialize()).unwrap();
}

fn read_db(root: &Path) -> ArtworkDb {
    ArtworkDb::parse(&std::fs::read(art_dir(root).join("ArtworkDB")).unwrap()).unwrap()
}

fn small(img: &artwork::Image) -> Option<&Thumb> {
    img.thumbs.iter().find(|t| t.format_id == SMALL)
}

/// 1061 が本体の形で、色が元の画像と同じか
fn assert_small_ok(root: &Path, img: &artwork::Image, rgb: [u8; 3]) {
    let t = small(img).unwrap_or_else(|| panic!("画像 {} に 1061 がある", img.id));
    assert_eq!((t.size, t.width, t.height), (6160, 55, 55), "画像 {}", img.id);
    let px = artwork::read_thumb(&art_dir(root), t).unwrap();
    assert_eq!(px.dimensions(), (55, 55));
    for p in px.pixels() {
        let near = p.0.iter().zip(rgb).all(|(a, b)| (*a as i32 - b as i32).abs() <= 12);
        assert!(near, "画像 {} の色 {:?}", img.id, p.0);
    }
}

fn base(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("zenpod-classic-art-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// 以前の版が書いた 56×56 (6272 バイト) の 1061 は、新しいファイルに書き直して古いファイルを消す
#[test]
fn rewrites_old_56px_small_artwork() {
    let root = base("old").join("ipod");
    fake_classic(&root);
    let dir = art_dir(&root);
    std::fs::create_dir_all(&dir).unwrap();
    let old: Vec<ArtFormat> = std::iter::once(ArtFormat { id: SMALL, width: 56, height: 56, pix: Pix::Rgb565Le, row_align: 0 })
        .chain(CLASSIC[1..].iter().copied())
        .collect();
    let mut db = ArtworkDb::new_empty();
    db.files = old.iter().map(|f| (f.id, f.width * f.height * 2)).collect();
    let red = artwork::build_image(&dir, &jpeg([200, 30, 30]), &old, 100, 1).unwrap();
    // 同じジャケットの曲は画素データの場所を共有している
    let mut red2 = red.clone();
    red2.id = 101;
    red2.song_id = 2;
    let blue = artwork::build_image(&dir, &jpeg([30, 30, 200]), &old, 102, 3).unwrap();
    db.images = vec![red, red2, blue];
    write_db(&root, &db);

    let mut ipod = Ipod::open(&root).unwrap();
    assert_eq!(ipod.repair_artwork(), 3);
    ipod.save().unwrap();

    let db = read_db(&root);
    assert_small_ok(&root, &db.images[0], [200, 30, 30]);
    assert_small_ok(&root, &db.images[1], [200, 30, 30]);
    assert_small_ok(&root, &db.images[2], [30, 30, 200]);
    // 同じジャケットは書き直しても 1 回分 (場所を共有)
    assert_eq!(small(&db.images[0]).unwrap().offset, small(&db.images[1]).unwrap().offset);
    // 新しいファイルに 2 枚分だけ。古いファイルは消えている
    assert!(db.images.iter().all(|i| small(i).unwrap().file == ":F1061_2.ithmb"));
    assert_eq!(std::fs::metadata(dir.join("F1061_2.ithmb")).unwrap().len(), 2 * 6160);
    assert!(!dir.join("F1061_1.ithmb").exists());
    // mhlf も本体の大きさ。ほかの形式はそのまま
    assert!(db.files.contains(&(SMALL, 6160)) && db.files.contains(&(1060, 320 * 320 * 2)));
    assert!(db.images.iter().all(|i| i.thumbs.len() == 4));

    // もう一度開いても、直すものは無い
    let mut again = Ipod::open(&root).unwrap();
    assert_eq!(again.repair_artwork(), 0);
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

/// iTunes が書いた 6160 バイトの 1061 はそのまま。以前の版がそれを外して書いた画像にだけ 1061 を足す
#[test]
fn adds_missing_small_artwork_next_to_itunes_ones() {
    let root = base("itunes").join("ipod");
    fake_classic(&root);
    let dir = art_dir(&root);
    std::fs::create_dir_all(&dir).unwrap();
    let mut db = ArtworkDb::new_empty();
    db.files = vec![(SMALL, 6160), (1055, 32768), (1068, 32768), (1060, 204800)];
    let itunes = artwork::build_image(&dir, &jpeg([30, 160, 30]), CLASSIC, 100, 1).unwrap();
    let zenpod = artwork::build_image(&dir, &jpeg([200, 200, 30]), &CLASSIC[1..], 101, 2).unwrap();
    let before = small(&itunes).map(|t| (t.file.clone(), t.offset));
    db.images = vec![itunes, zenpod];
    write_db(&root, &db);

    let mut ipod = Ipod::open(&root).unwrap();
    assert_eq!(ipod.repair_artwork(), 1);
    ipod.save().unwrap();

    let db = read_db(&root);
    assert_eq!(small(&db.images[0]).map(|t| (t.file.clone(), t.offset)), before, "iTunes の画像は触らない");
    assert_small_ok(&root, &db.images[0], [30, 160, 30]);
    assert_small_ok(&root, &db.images[1], [200, 200, 30]);
    // 同じファイルの続きに、同じ大きさで足す
    let t = small(&db.images[1]).unwrap();
    assert_eq!((t.file.as_str(), t.offset), (":F1061_1.ithmb", 6160));
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}
