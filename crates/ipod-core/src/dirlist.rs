//! フォルダの中身を、ファイルの更新日時と大きさ付きで読む。
//! macOS は getattrlistbulk でまとめて読む (FAT の iPod は 1 ファイルずつ stat すると 1 回 0.1ms ほどかかり、フォルダを読む時間の 7 割になる)

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Dir,
    /// シンボリックリンクなど (たどらない)
    Other,
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub name: OsString,
    pub kind: Kind,
    /// `wants` が true のファイルだけ (ほかは UNIX_EPOCH と 0。macOS はすべてのファイル)
    pub modified: SystemTime,
    pub len: u64,
}

/// `dir` の中身 (順は決まっていない)。`wants` は更新日時と大きさが要るファイルの名前か
pub(crate) fn read(dir: &Path, wants: &dyn Fn(&OsStr) -> bool) -> io::Result<Vec<Entry>> {
    #[cfg(target_os = "macos")]
    {
        match bulk::read(dir) {
            // getattrlistbulk に対応していないファイルシステム
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTSUP) | Some(libc::EINVAL)) => read_portable(dir, wants),
            r => r,
        }
    }
    #[cfg(not(target_os = "macos"))]
    read_portable(dir, wants)
}

/// フォルダの中のファイルの大きさの合計 (中のフォルダも数える。リンクはたどらない)。読めなければ 0。
/// macOS は getattrlistbulk でまとめて読む (FAT の iPod の写真・動画のフォルダを 1 ファイルずつ stat しない)
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(items) = read(dir, &|_| true) else { return 0 };
    items
        .iter()
        .map(|e| match e.kind {
            Kind::File => e.len,
            Kind::Dir => dir_size(&dir.join(&e.name)),
            Kind::Other => 0,
        })
        .sum()
}

/// 1 ファイルずつ問い合わせる読み方。Windows はフォルダを読んだときの情報を使うので問い合わせない
fn read_portable(dir: &Path, wants: &dyn Fn(&OsStr) -> bool) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let kind = if ft.is_file() {
            Kind::File
        } else if ft.is_dir() {
            Kind::Dir
        } else {
            Kind::Other
        };
        let name = e.file_name();
        let (modified, len) = if kind == Kind::File && wants(&name) {
            let Ok(m) = e.metadata() else { continue };
            (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len())
        } else {
            (SystemTime::UNIX_EPOCH, 0)
        };
        out.push(Entry { name, kind, modified, len });
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
mod bulk {
    use super::{Entry, Kind};
    use std::ffi::OsString;
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    /// sys/vnode.h の enum vtype
    const VREG: u32 = 1;
    const VDIR: u32 = 2;
    const BUF_BYTES: usize = 256 * 1024;

    pub(super) fn read(dir: &Path) -> io::Result<Vec<Entry>> {
        let f = std::fs::File::open(dir)?;
        // SAFETY: attrlist は数値だけの構造体なので 0 で埋めてよい
        let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
        list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
        list.commonattr = libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_NAME | libc::ATTR_CMN_OBJTYPE | libc::ATTR_CMN_MODTIME;
        list.fileattr = libc::ATTR_FILE_DATALENGTH;
        let mut buf = vec![0u8; BUF_BYTES];
        let mut out = Vec::new();
        loop {
            // SAFETY: 開いているフォルダの fd と、BUF_BYTES の大きさのバッファを渡す
            let n = unsafe {
                libc::getattrlistbulk(f.as_raw_fd(), (&raw mut list).cast(), buf.as_mut_ptr().cast(), BUF_BYTES, 0)
            };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            if n == 0 {
                return Ok(out);
            }
            let mut at = 0;
            for _ in 0..n {
                let len = u32_at(&buf, at).ok_or_else(broken)? as usize;
                out.push(parse(buf.get(at..at + len).ok_or_else(broken)?).ok_or_else(broken)?);
                at += len;
            }
        }
    }

    fn broken() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, "getattrlistbulk")
    }

    fn u32_at(b: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?))
    }

    fn i64_at(b: &[u8], at: usize) -> Option<i64> {
        Some(i64::from_ne_bytes(b.get(at..at + 8)?.try_into().ok()?))
    }

    /// 1 項目: 長さ (u32)・返した項目の印 (attribute_set_t)・名前 (attrreference_t)・種類・更新日時 (timespec)・大きさ (off_t)。
    /// 返さなかった項目は詰めて並ぶので、印を見て順に読む
    fn parse(e: &[u8]) -> Option<Entry> {
        let (common, file) = (u32_at(e, 4)?, u32_at(e, 16)?);
        let mut at = 24;
        let name_off = u32_at(e, at)? as i32;
        let name_len = u32_at(e, at + 4)? as usize;
        let start = usize::try_from(at as i32 + name_off).ok()?;
        // 長さは終わりの \0 を含む
        let name = e.get(start..start + name_len.checked_sub(1)?)?.to_vec();
        at += 8;
        let kind = if common & libc::ATTR_CMN_OBJTYPE != 0 {
            let t = u32_at(e, at)?;
            at += 4;
            match t {
                VREG => Kind::File,
                VDIR => Kind::Dir,
                _ => Kind::Other,
            }
        } else {
            Kind::Other
        };
        let modified = if common & libc::ATTR_CMN_MODTIME != 0 {
            let (sec, nsec) = (i64_at(e, at)?, i64_at(e, at + 8)?);
            at += 16;
            u64::try_from(sec).ok().map(|s| SystemTime::UNIX_EPOCH + Duration::new(s, nsec.clamp(0, 999_999_999) as u32))
        } else {
            None
        };
        let len = if file & libc::ATTR_FILE_DATALENGTH != 0 { u64::try_from(i64_at(e, at)?).ok() } else { None };
        // 1 ファイルずつ読むときと同じく、更新日時と大きさはファイルだけ
        let (modified, len) = match kind {
            Kind::File => (modified.unwrap_or(SystemTime::UNIX_EPOCH), len.unwrap_or(0)),
            _ => (SystemTime::UNIX_EPOCH, 0),
        };
        Some(Entry { name: OsString::from_vec(name), kind, modified, len })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 名前・種類・更新日時・大きさが、1 ファイルずつ問い合わせたときと同じ
    #[test]
    fn same_as_portable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("フォルダ")).unwrap();
        std::fs::write(root.join("01 がく.mp3"), vec![0u8; 1234]).unwrap();
        std::fs::write(root.join("cover.jpg"), b"x").unwrap();
        let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options().write(true).open(root.join("01 がく.mp3")).unwrap().set_modified(when).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("フォルダ"), root.join("link")).unwrap();
        let all = |_: &OsStr| true;
        let sorted = |mut v: Vec<Entry>| {
            v.sort_by(|a, b| a.name.cmp(&b.name));
            v.into_iter().map(|e| (e.name, e.kind, e.modified, e.len)).collect::<Vec<_>>()
        };

        let got = sorted(read(root, &all).unwrap());

        assert_eq!(got, sorted(read_portable(root, &all).unwrap()));
        let mp3 = got.iter().find(|e| e.0 == "01 がく.mp3").unwrap();
        assert_eq!((mp3.1, mp3.2, mp3.3), (Kind::File, when, 1234));
        assert_eq!(got.iter().find(|e| e.0 == "フォルダ").unwrap().1, Kind::Dir);
    }

    #[test]
    fn dir_size_counts_nested_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("x"), [0u8; 3]).unwrap();
        std::fs::write(d.path().join("a/b/y"), [0u8; 5]).unwrap();
        // リンクの先は数えない (同じファイルを 2 回数えない)
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path().join("a"), d.path().join("link")).unwrap();
        assert_eq!(dir_size(d.path()), 8);
        assert_eq!(dir_size(&d.path().join("missing")), 0);
    }
}
