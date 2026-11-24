//! 箱 (atom) の並びが途中で壊れていて、lofty が moov を見つけられない M4A を読む。
//!
//! lofty は先頭から箱を順にたどり、大きさが合わない箱・名前が文字でない箱 (ゴミや 0 の並び) に当たると
//! そこで止まって「moov が無い」とする (ffmpeg も同じ)。壊れた部分の後ろに moov が残っていれば、
//! ファイルの中を探して見つけ、ftyp と moov だけをつないだ読み口で読む (配布リポジトリの Issue #7)。
//! 音声の場所 (mdat) は分からないままなので、タグと長さを読むだけで、書き換えはしない。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{FileType, TaggedFile};
use lofty::probe::Probe;

use crate::media::fastread::{Seg, Virtual};

/// 探す範囲の 1 回の読み取り
const CHUNK: usize = 1 << 20;

/// 先頭の ftyp の大きさ。先頭が ftyp でなければ None
pub(crate) fn ftyp_len(f: &mut File, len: u64) -> Option<u64> {
    let mut h = [0u8; 8];
    f.seek(SeekFrom::Start(0)).ok()?;
    f.read_exact(&mut h).ok()?;
    let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
    (&h[4..8] == b"ftyp" && (16..=len).contains(&size)).then_some(size)
}

/// `from` から後ろで、moov の箱らしいもの (大きさがファイルに収まり、最初の中身が mvhd) を探す。
/// 見つかれば (始まり, 大きさ)
pub(crate) fn find_moov(f: &mut File, from: u64, len: u64) -> Option<(u64, u64)> {
    // 箱の見出し (大きさ 4 + "moov") と、最初の中身の見出し (大きさ 4 + "mvhd") まで 16 バイトを見る
    let mut buf = vec![0u8; CHUNK + 16];
    let mut pos = from;
    while pos + 16 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let want = ((len - pos) as usize).min(buf.len());
        f.read_exact(&mut buf[..want]).ok()?;
        let hay = &buf[..want];
        for i in 0..hay.len().saturating_sub(15) {
            if &hay[i + 4..i + 8] != b"moov" || &hay[i + 12..i + 16] != b"mvhd" {
                continue;
            }
            let size = u32::from_be_bytes([hay[i], hay[i + 1], hay[i + 2], hay[i + 3]]) as u64;
            let start = pos + i as u64;
            if size >= 16 && start + size <= len {
                return Some((start, size));
            }
        }
        // 境目をまたぐ見出しを逃さないよう、少し重ねて次へ
        pos += (want as u64).saturating_sub(15).max(1);
    }
    None
}

/// lofty が moov を見つけられなかった M4A を、壊れた部分の後ろの moov で読む。
/// 先頭が ftyp でない・moov が見つからない・それでも読めないなら None
pub(crate) fn read(path: &Path) -> Option<TaggedFile> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let ftyp = ftyp_len(&mut f, len)?;
    let (start, size) = find_moov(&mut f, ftyp, len)?;
    let segs = vec![Seg::File { start: 0, len: ftyp }, Seg::File { start, len: size }];
    let r = BufReader::new(Virtual::new(f, segs));
    Probe::with_file_type(r, FileType::Mp4).options(ParseOptions::new()).read().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::prelude::*;

    fn atom(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = ((data.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend(ty);
        v.extend(data);
        v
    }

    /// ffmpeg で 3 秒の AAC (曲名 Title) を作り、その ftyp・mdat・moov を返す (ffmpeg の既定は ftyp・free・mdat・moov の順)。
    /// ffmpeg が無ければ None
    fn ffmpeg_atoms(dir: &Path) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let ff = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file()))?;
        let src = dir.join("src.m4a");
        let ok = std::process::Command::new(ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=3", "-c:a", "aac", "-metadata", "title=Title"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(ok.success());
        let b = std::fs::read(&src).unwrap();
        let mut atoms = Vec::new();
        let mut p = 0;
        while p < b.len() {
            let size = u32::from_be_bytes(b[p..p + 4].try_into().unwrap()) as usize;
            atoms.push((b[p + 4..p + 8].to_vec(), b[p..p + size].to_vec()));
            p += size;
        }
        let get = |t: &[u8]| atoms.iter().find(|(n, _)| n == t).unwrap().1.clone();
        Some((get(b"ftyp"), get(b"mdat"), get(b"moov")))
    }

    /// mdat と moov の間のゴミ・0 の並び・大きさの壊れた mdat: moov を探して曲名と長さを読む。
    /// moov がどこにも無いファイルは読めない。どれも書き換えない
    #[test]
    fn reads_moov_after_broken_part() {
        let dir = tempfile::tempdir().unwrap();
        let Some((ftyp, mdat, moov)) = ffmpeg_atoms(dir.path()) else {
            return;
        };
        let mut big = mdat.clone();
        big[..4].copy_from_slice(&(mdat.len() as u32 + 1000).to_be_bytes());
        let cases: Vec<(&str, Vec<u8>, bool)> = vec![
            ("junk", [ftyp.clone(), mdat.clone(), b"\x12\x34garbage!!".to_vec(), moov.clone()].concat(), true),
            ("zeros", [ftyp.clone(), mdat.clone(), vec![0; 16], moov.clone()].concat(), true),
            ("mdat too big", [ftyp.clone(), big, moov.clone()].concat(), true),
            ("no moov", [ftyp.clone(), mdat.clone()].concat(), false),
            ("free atom only", [ftyp.clone(), atom(b"free", &[0; 8]), mdat.clone()].concat(), false),
        ];
        for (name, bytes, ok) in cases {
            let p = dir.path().join("a.m4a");
            std::fs::write(&p, &bytes).unwrap();
            assert!(lofty::read_from_path(&p).is_err(), "{name}");
            let tf = read(&p);
            assert_eq!(tf.is_some(), ok, "{name}");
            if let Some(tf) = tf {
                assert_eq!(tf.primary_tag().and_then(|t| t.title()).as_deref(), Some("Title"), "{name}");
                let info = crate::media::read_info(&p).unwrap();
                assert_eq!(info.title.as_deref(), Some("Title"), "{name}");
                assert!((2900..3100).contains(&info.duration_ms), "{name}: {}", info.duration_ms);
                let s = crate::media::structure::tag_structure(&p).unwrap();
                assert!(s.contains("moov found by searching"), "{name}: {s}");
            } else {
                assert!(crate::media::structure::tag_structure(&p).unwrap().contains("no moov anywhere"), "{name}");
            }
            let edit = crate::domain::track::TagEdit { album: Some("X".into()), ..Default::default() };
            assert!(crate::media::write_tags(&p, &edit).is_err(), "{name}");
            assert_eq!(std::fs::read(&p).unwrap(), bytes, "{name}");
        }
    }

    /// 大きさを 64 ビットで書いた mdat が moov より前にある M4A (配布リポジトリの Issue #13。Music.app は読める)。
    /// lofty 0.25.4 はこの箱を飛ばすと 8 バイト行き過ぎて「moov が無い」にしていた (vendor/lofty で直した)。
    /// 壊れたファイルではないので、そのまま読めて、転送でき、タグも書ける
    #[test]
    fn reads_mdat_with_64bit_size() {
        let dir = tempfile::tempdir().unwrap();
        let Some((ftyp, mdat, moov)) = ffmpeg_atoms(dir.path()) else {
            return;
        };
        let mut wide = 1u32.to_be_bytes().to_vec();
        wide.extend(b"mdat");
        wide.extend((mdat.len() as u64 + 8).to_be_bytes());
        wide.extend(&mdat[8..]);
        let p = dir.path().join("a.m4a");
        std::fs::write(&p, [ftyp, wide, moov].concat()).unwrap();
        let tf = lofty::read_from_path(&p).unwrap();
        assert_eq!(tf.primary_tag().and_then(|t| t.title()).as_deref(), Some("Title"));
        assert!(matches!(crate::media::classify(&p).unwrap(), crate::domain::audio::Format::Aac));
        let info = crate::media::read_info(&p).unwrap();
        assert!((2900..3100).contains(&info.duration_ms), "{}", info.duration_ms);
        let s = crate::media::structure::tag_structure(&p).unwrap();
        assert!(s.contains("\nmdat ") && s.contains(" (64-bit size)\n") && !s.contains("moov found by searching"), "{s}");
        let edit = crate::domain::track::TagEdit { album: Some("X".into()), ..Default::default() };
        crate::media::write_tags(&p, &edit).unwrap();
        let info = crate::media::read_info(&p).unwrap();
        assert_eq!((info.title.as_deref(), info.album.as_deref()), (Some("Title"), Some("X")));
    }
}
