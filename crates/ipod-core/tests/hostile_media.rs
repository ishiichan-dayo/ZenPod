//! 壊れた・悪意のある入力 (動画・写真・iTunes の XML) で、止まる・落ちる・メモリを使い切ることがない

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use ipod_core::media::{cover, photo, video};
use ipod_core::pc::itunesxml::XmlLibrary;

fn tmp(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("zenpod-hostile-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

/// 別のスレッドで動かし、決めた時間で終わらなければ失敗にする (止まったままのテストにしない)
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs)).expect("終わらない (無限ループ)")
}

fn probe(path: &Path) -> Option<video::VideoInfo> {
    let p = path.to_path_buf();
    within(10, move || video::probe_mp4(&p))
}

/// ftyp の後ろに largesize が u64 の端に近い箱。以前は at + size があふれて先頭に戻り、読み続けた
#[test]
fn mp4_largesize_overflow_does_not_hang() {
    let dir = tmp("mp4-loop");
    for size in [0u64.wrapping_sub(16), u64::MAX, u64::MAX - 7] {
        let mut f = Vec::new();
        be32(&mut f, 16);
        f.extend_from_slice(b"ftypisom");
        be32(&mut f, 0);
        be32(&mut f, 1);
        f.extend_from_slice(b"free");
        f.extend_from_slice(&size.to_be_bytes());
        f.extend_from_slice(&[0u8; 16]);
        let p = dir.join("loop.mp4");
        std::fs::write(&p, &f).unwrap();
        assert!(probe(&p).is_none());
    }
}

/// moov の中の箱の largesize が u64 の端に近い。以前はあふれて範囲の外を切り出し、落ちた
#[test]
fn mp4_inner_box_overflow_does_not_panic() {
    let dir = tmp("mp4-inner");
    for size in [u64::MAX, u64::MAX - 7, u64::MAX - 15] {
        let mut body = Vec::new();
        // 正しい mvhd (版 0: timescale 1000、長さ 5000)
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&5000u32.to_be_bytes());
        be32(&mut body, 8 + mvhd.len() as u32);
        body.extend_from_slice(b"mvhd");
        body.extend_from_slice(&mvhd);
        be32(&mut body, 1);
        body.extend_from_slice(b"trak");
        body.extend_from_slice(&size.to_be_bytes());
        body.extend_from_slice(&[0u8; 32]);
        let mut f = Vec::new();
        be32(&mut f, 16);
        f.extend_from_slice(b"ftypisom");
        be32(&mut f, 0);
        be32(&mut f, 8 + body.len() as u32);
        f.extend_from_slice(b"moov");
        f.extend_from_slice(&body);
        let p = dir.join("inner.mp4");
        std::fs::write(&p, &f).unwrap();
        // 壊れた箱の手前 (mvhd) までは読める
        let info = probe(&p).expect("moov は読める");
        assert_eq!(info.duration_ms, 5000);
    }
}

/// 見出しだけで 65500×65500 を名乗る数百バイトの JPEG
fn jpeg_bomb() -> Vec<u8> {
    let mut j: Vec<u8> = vec![0xFF, 0xD8];
    j.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
    j.extend_from_slice(&[1u8; 64]);
    j.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0xFF, 0xDC, 0xFF, 0xDC, 0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00]);
    j.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x14, 0x00, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    j.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x14, 0x10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x0C, 0x03, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x00, 0x3F, 0x00]);
    j.extend_from_slice(&[0u8; 64]);
    j.extend_from_slice(&[0xFF, 0xD9]);
    j
}

#[test]
fn huge_dimension_jpeg_is_rejected() {
    let dir = tmp("jpeg-bomb");
    let p = dir.join("bomb.jpg");
    let bomb = jpeg_bomb();
    std::fs::write(&p, &bomb).unwrap();
    // ffmpeg に回さずにエラー (確保しようとして落ちない)
    assert!(within(30, move || photo::load(&p, None).is_err()));
    assert!(within(30, move || cover::thumbnail_jpeg(&bomb, 100).is_none()));
}

/// 普通の大きさの写真はこれまでどおり読める
#[test]
fn normal_photo_still_loads() {
    let dir = tmp("photo-ok");
    let p = dir.join("a.jpg");
    image::RgbImage::from_pixel(64, 48, image::Rgb([10, 200, 10])).save(&p).unwrap();
    let l = photo::load(&p, None).unwrap();
    assert_eq!((l.image.width(), l.image.height()), (64, 48));
}

fn nested_plist(depth: usize) -> String {
    let mut x = String::from("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Playlists</key>");
    x.push_str(&"<array>".repeat(depth));
    x.push_str(&"</array>".repeat(depth));
    x.push_str("</dict></plist>");
    x
}

/// 入れ子が深すぎる XML。以前は plist が再帰で読んでスタックがあふれ、アプリごと落ちた。
/// Windows の画面のスレッド (1MB) より小さいスタックで確かめる
#[test]
fn deeply_nested_plist_is_rejected() {
    let r = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            let deep = XmlLibrary::parse(nested_plist(200_000).as_bytes()).is_err();
            let medium = XmlLibrary::parse(nested_plist(3_000).as_bytes()).is_err();
            deep && medium
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(r);
    // 浅いものは今までどおり読める
    assert!(XmlLibrary::parse(nested_plist(3).as_bytes()).is_ok());
}

/// XML でない plist (バイナリ・テキスト) は読まない。テキストの plist も入れ子を再帰で読む
#[test]
fn non_xml_plist_is_rejected() {
    assert!(XmlLibrary::parse(b"bplist00\0\0\0\0").is_err());
    let ascii = format!("{}{}", "(".repeat(100_000), ")".repeat(100_000));
    assert!(XmlLibrary::parse(ascii.as_bytes()).is_err());
}

/// 曲の番号が u64 の端でも、足し合わせであふれない
#[test]
fn append_with_extreme_ids() {
    let xml = |id: u64| {
        format!(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Tracks</key><dict><key>{id}</key><dict><key>Track ID</key><integer>{id}</integer><key>Name</key><string>a</string></dict></dict>\
             <key>Playlists</key><array><dict><key>Name</key><string>p</string><key>Playlist Items</key><array><dict><key>Track ID</key><integer>{id}</integer></dict></array></dict></array></dict></plist>"
        )
    };
    let mut a = XmlLibrary::parse(xml(u64::MAX).as_bytes()).unwrap();
    let b = XmlLibrary::parse(xml(u64::MAX).as_bytes()).unwrap();
    a.append(b);
    assert_eq!(a.playlists.len(), 2);
}
