//! フォルダがネットワーク上 (NAS の共有フォルダなど) にあるか。
//!
//! ネットワーク上のフォルダは、ファイル 1 つごとの問い合わせ (一覧・日時・タグの読み取り) に通信の往復がかかる。
//! 同時にいくつも問い合わせれば待ち時間が重なるので、PC の音楽フォルダの読み込みは同時に進める数を増やす
//! (配布リポジトリの Issue #16)。分からなければ false (今までどおり)。

use std::path::Path;

/// ネットワークのファイルシステムの名前 (Linux の /proc/mounts・macOS の statfs)。
/// 9p は WSL の Windows のドライブ (/mnt/c) で、これもファイルごとに往復がかかる
#[cfg_attr(windows, allow(dead_code))]
const NETWORK_FS: &[&str] = &[
    "cifs", "smb3", "smbfs", "nfs", "nfs4", "afpfs", "webdav", "davfs", "9p", "fuse.sshfs", "fuse.rclone",
];

pub fn is_network_path(path: &Path) -> bool {
    imp(path)
}

#[cfg(windows)]
fn imp(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_REMOTE: u32 = 4;
    match path.components().next() {
        // \\NAS\share\... や \\?\UNC\NAS\share\...
        Some(Component::Prefix(p)) => match p.kind() {
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => true,
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                // ネットワークドライブ (Z: など)
                let root: Vec<u16> = std::ffi::OsStr::new(&format!("{}:\\", letter as char)).encode_wide().chain([0]).collect();
                unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
            }
            _ => false,
        },
        _ => false,
    }
}

#[cfg(target_os = "macos")]
fn imp(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    let name: Vec<u8> = st.f_fstypename.iter().take_while(|&&b| b != 0).map(|&b| b as u8).collect();
    NETWORK_FS.contains(&String::from_utf8_lossy(&name).as_ref())
}

#[cfg(target_os = "linux")]
fn imp(path: &Path) -> bool {
    let Ok(path) = std::fs::canonicalize(path) else { return false };
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else { return false };
    fs_type_of(&path, &mounts).is_some_and(|t| NETWORK_FS.contains(&t))
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn imp(_path: &Path) -> bool {
    false
}

/// `/proc/mounts` の形の一覧から、`path` を含むいちばん深いマウントポイントのファイルシステムの名前
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn fs_type_of<'a>(path: &Path, mounts: &'a str) -> Option<&'a str> {
    mounts
        .lines()
        .filter_map(|line| {
            let mut f = line.split(' ');
            let (_, point, fstype) = (f.next()?, f.next()?, f.next()?);
            Some((unescape(point), fstype))
        })
        .filter(|(point, _)| path.starts_with(point))
        .max_by_key(|(point, _)| point.len())
        .map(|(_, t)| t)
}

/// /proc/mounts の場所の空白などは \040 のような 8 進数で書かれている
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTS: &str = "/dev/sda1 / ext4 rw 0 0\n\
        //nas/music /mnt/nas cifs ro,vers=3.1.1 0 0\n\
        nas:/export /mnt/my\\040music nfs4 rw 0 0\n\
        /dev/sdb1 /mnt/nas/local ext4 rw 0 0\n\
        drvfs /mnt/c 9p rw 0 0\n";

    #[test]
    fn deepest_mount_wins() {
        assert_eq!(fs_type_of(Path::new("/home/u/Music"), MOUNTS), Some("ext4"));
        assert_eq!(fs_type_of(Path::new("/mnt/nas/Artist/Album"), MOUNTS), Some("cifs"));
        assert_eq!(fs_type_of(Path::new("/mnt/nas/local/x"), MOUNTS), Some("ext4"));
        assert_eq!(fs_type_of(Path::new("/mnt/my music/a"), MOUNTS), Some("nfs4"));
        // 名前が似ているだけの場所は別
        assert_eq!(fs_type_of(Path::new("/mnt/nasx"), MOUNTS), Some("ext4"));
        assert_eq!(fs_type_of(Path::new("/mnt/c/Users"), MOUNTS), Some("9p"));
    }
}
