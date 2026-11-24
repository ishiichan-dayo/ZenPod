//! 再生用の FLAC を ffmpeg で作る (パイプに書き出す。ディスクには書かない)。設定で選んだファイルが ffmpeg かも確かめる

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use ipod_core::media;

use crate::application::ports::Transcoder;

/// `-version` の答えを待つ時間
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

pub struct FfmpegTranscoder;

impl Transcoder for FfmpegTranscoder {
    fn to_flac(&self, ffmpeg: &Path, src: &Path) -> Option<Vec<u8>> {
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-v", "error", "-i"])
            .arg(src)
            .args(["-map", "0:a:0", "-f", "flac", "-compression_level", "0", "-"]);
        no_window(&mut cmd);
        match cmd.output() {
            Ok(o) if o.status.success() && !o.stdout.is_empty() => {
                let mut bytes = o.stdout;
                let duration_ms = media::read_info(src).map(|i| i.duration_ms).unwrap_or(0);
                set_flac_total_samples(&mut bytes, duration_ms);
                Some(bytes)
            }
            _ => None,
        }
    }

    fn is_ffmpeg(&self, path: &Path) -> bool {
        says_ffmpeg(path, VERSION_TIMEOUT)
    }
}

/// `path -version` の出力が "ffmpeg version" で始まるか。`timeout` までに答えなければ false
fn says_ffmpeg(path: &Path, timeout: Duration) -> bool {
    let mut cmd = Command::new(path);
    cmd.arg("-version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    no_window(&mut cmd);
    let Ok(mut child) = cmd.spawn() else { return false };
    let Some(mut out) = child.stdout.take() else { return false };
    // 読むのは別のスレッドで (答えない・閉じないプログラムでも待ち続けない)
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut head = Vec::new();
        let _ = out.by_ref().take(4096).read_to_end(&mut head);
        let _ = tx.send(head);
    });
    let head = rx.recv_timeout(timeout);
    let _ = child.kill();
    let _ = child.wait();
    matches!(head, Ok(h) if h.starts_with(b"ffmpeg version"))
}

/// 黒い窓を出さない (Windows の CREATE_NO_WINDOW)
fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// パイプに書き出した FLAC は曲全体のサンプル数が 0 のまま (後から書き戻せない) で、
/// WebView が長さを Infinity と扱いシークできない。元の曲の長さから計算して STREAMINFO に書き込む
fn set_flac_total_samples(flac: &mut [u8], duration_ms: u32) {
    // "fLaC" + ブロックヘッダ 4 バイトの後が STREAMINFO (34 バイト)
    if flac.len() < 42 || &flac[0..4] != b"fLaC" || flac[4] & 0x7F != 0 || duration_ms == 0 {
        return;
    }
    let si = &mut flac[8..42];
    let rate = ((si[10] as u64) << 12) | ((si[11] as u64) << 4) | ((si[12] as u64) >> 4);
    let current = (((si[13] & 0x0F) as u64) << 32) | u32::from_be_bytes([si[14], si[15], si[16], si[17]]) as u64;
    if rate == 0 || current != 0 {
        return;
    }
    let total = rate * duration_ms as u64 / 1000;
    si[13] = (si[13] & 0xF0) | ((total >> 32) as u8 & 0x0F);
    si[14..18].copy_from_slice(&(total as u32).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 44.1kHz・2ch・16bit で、全体のサンプル数が 0 の STREAMINFO
    fn header() -> Vec<u8> {
        let mut v = b"fLaC".to_vec();
        v.extend([0x00, 0x00, 0x00, 0x22]);
        let mut si = [0u8; 34];
        // 44100 = 0xAC44 → 20 ビット: 0x0AC44
        si[10] = 0x0A;
        si[11] = 0xC4;
        si[12] = 0x42; // 下位 4 ビット: チャンネル数 - 1 と bits の上位
        si[13] = 0xF0;
        v.extend(si);
        v
    }

    fn samples(flac: &[u8]) -> u64 {
        let si = &flac[8..42];
        (((si[13] & 0x0F) as u64) << 32) | u32::from_be_bytes([si[14], si[15], si[16], si[17]]) as u64
    }

    #[test]
    fn fills_total_samples_once() {
        let mut f = header();
        set_flac_total_samples(&mut f, 2_000);
        assert_eq!(samples(&f), 88_200);
        assert_eq!(f[8 + 13] & 0xF0, 0xF0);
        // 入っていれば書き換えない
        set_flac_total_samples(&mut f, 9_000);
        assert_eq!(samples(&f), 88_200);
    }

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn only_ffmpeg_passes() {
        let d = tempfile::tempdir().unwrap();
        let ok = script(d.path(), "ffmpeg", "echo 'ffmpeg version 7.1 Copyright (c) 2000-2024'");
        let other = script(d.path(), "other", "echo 'hello'");
        let silent = script(d.path(), "silent", "sleep 5");
        let short = Duration::from_millis(500);
        assert!(says_ffmpeg(&ok, short));
        assert!(!says_ffmpeg(&other, short));
        assert!(!says_ffmpeg(&silent, short));
        assert!(!says_ffmpeg(&d.path().join("missing"), short));
        // 実行できないファイル
        let text = d.path().join("text");
        std::fs::write(&text, "ffmpeg version").unwrap();
        assert!(!says_ffmpeg(&text, short));
    }

    #[test]
    fn ignores_other_data() {
        let mut short = b"fLaC".to_vec();
        set_flac_total_samples(&mut short, 1000);
        assert_eq!(short, b"fLaC");
        let mut f = header();
        set_flac_total_samples(&mut f, 0);
        assert_eq!(samples(&f), 0);
    }
}
