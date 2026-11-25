//! ffmpeg による iPod 非対応形式の変換。

use crate::t;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::domain::audio::ConvertTarget;

/// ffmpeg 実行ファイルを探す (明示パス → 同梱・自動インストール先 → PATH → winget 等 → よくある場所)
pub fn find_ffmpeg(explicit: Option<&Path>, app_dirs: &[PathBuf]) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(p) = explicit {
        cands.push(p.to_path_buf());
    }
    // アプリに同梱・アプリが自動で入れた場所
    cands.extend(app_dirs.iter().map(|d| d.join(exe)));
    if let Some(path) = std::env::var_os("PATH") {
        cands.extend(std::env::split_paths(&path).map(|d| d.join(exe)));
    }
    if cfg!(windows) {
        cands.extend(windows_package_locations());
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        cands.push(Path::new(d).join(exe));
    }
    cands.into_iter().find(|p| p.is_file())
}

/// winget / Chocolatey / Scoop で入れた ffmpeg の場所。
/// 入れた直後は起動中のアプリの PATH に反映されないので、決まった場所を直接探す
fn windows_package_locations() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut v = Vec::new();
    if let Some(local) = env("LOCALAPPDATA") {
        let winget = local.join("Microsoft").join("WinGet");
        v.push(winget.join("Links").join("ffmpeg.exe"));
        // Packages\Gyan.FFmpeg_xxx\ffmpeg-7.1-full_build\bin\ffmpeg.exe
        if let Ok(rd) = std::fs::read_dir(winget.join("Packages")) {
            for pkg in rd.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase().contains("ffmpeg")) {
                if let Ok(inner) = std::fs::read_dir(pkg.path()) {
                    v.extend(inner.flatten().map(|d| d.path().join("bin").join("ffmpeg.exe")));
                }
            }
        }
    }
    if let Some(pd) = env("ProgramData") {
        v.push(pd.join("chocolatey").join("bin").join("ffmpeg.exe"));
    }
    if let Some(home) = env("USERPROFILE") {
        v.push(home.join("scoop").join("shims").join("ffmpeg.exe"));
    }
    v.push(PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"));
    v
}

/// `src` を iPod で再生できる形式 (`target`) に変換して `dst` に書く
pub fn convert(ffmpeg: &Path, src: &Path, dst: &Path, target: ConvertTarget, src_rate: u32) -> Result<()> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(["-map", "0:a:0", "-map_metadata", metadata_source(src), "-vn"])
        .args(codec_args(target, src_rate));
    if target.ext() == "m4a" {
        cmd.args(FASTSTART);
    }
    run(cmd.arg(dst), "ffmpeg")?;
    // ジャケットや歌詞が写らなくても曲は転送できるので、変換は失敗にしない
    if let Err(e) = crate::media::copy_pictures_and_lyrics(src, dst) {
        log::warn!("copy_pictures_and_lyrics: {e}");
    }
    Ok(())
}

/// ffmpeg に渡す、符号化の引数 (入力と出力のあいだ)
fn codec_args(target: ConvertTarget, src_rate: u32) -> Vec<String> {
    let rate = (src_rate > 48000).then(|| out_rate(src_rate));
    let mut args: Vec<String> = Vec::new();
    match target {
        ConvertTarget::Alac => {
            // iPod classic の ALAC は 16bit / 48kHz まで。24bit の曲をただ切り捨てて 16bit にすると、
            // 小さい音 (フェードアウトなど) にざらつきが出る。ffmpeg の既定はディザなしなので、
            // aresample でディザをかけて 16bit にし (サンプリング周波数の変換も同時に)、そのまま ALAC にする。
            // 元が 16bit の曲は何も変わらない (ビットが一致する)
            let mut filter = String::from("aresample=osf=s16p:dither_method=triangular");
            if let Some(r) = rate {
                filter.push_str(&format!(":osr={r}"));
            }
            args.extend(["-af".into(), filter]);
            args.extend(["-c:a", "alac", "-sample_fmt", "s16p"].map(String::from));
        }
        ConvertTarget::Aac { kbps } => {
            args.extend(IPOD_AAC.map(String::from));
            args.extend(["-b:a".into(), format!("{kbps}k")]);
        }
        ConvertTarget::Mp3 { kbps } => {
            // iPod は ID3v2.4 のタグを読めないことがあるので v2.3 で書く
            args.extend(["-c:a".into(), "libmp3lame".into(), "-b:a".into(), format!("{kbps}k")]);
            args.extend(["-id3v2_version", "3"].map(String::from));
        }
    }
    // ALAC はディザと同時に aresample で変換済み
    if let (Some(r), false) = (rate, target == ConvertTarget::Alac) {
        args.extend(["-ar".into(), r.to_string()]);
    }
    args
}

/// iPod は 48kHz まで。それより高いサンプリング周波数は、割り切れる方 (44.1kHz 系 / 48kHz 系) に落とす
fn out_rate(src_rate: u32) -> u32 {
    if src_rate <= 48000 {
        src_rate
    } else if src_rate.is_multiple_of(44100) {
        44100
    } else {
        48000
    }
}

/// macOS に標準で入っている変換ツール (Core Audio)。ffmpeg が無くても FLAC を変換できる
pub fn find_afconvert() -> Option<PathBuf> {
    let p = Path::new("/usr/bin/afconvert");
    (cfg!(target_os = "macos") && p.is_file()).then(|| p.to_path_buf())
}

/// ffmpeg の -map_metadata に渡すタグの写し元。Ogg / Opus のタグは ffmpeg ではファイル全体ではなく音声ストリームに付くため
fn metadata_source(src: &Path) -> &'static str {
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ["ogg", "oga", "opus"].iter().any(|e| ext.eq_ignore_ascii_case(e)) {
        "0:s:a:0"
    } else {
        "0"
    }
}

/// afconvert で変換できる組み合わせか。読めるのは FLAC まで (Ogg / Opus / WMA は読めない)、書けるのは ALAC / AAC だけ
pub fn afconvert_supports(src: &Path, target: ConvertTarget) -> bool {
    let flac = src.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("flac"));
    flac && matches!(target, ConvertTarget::Alac | ConvertTarget::Aac { .. })
}

/// afconvert に渡す引数 (2 回に分けて実行する)。
/// 1 回目で CAF の PCM (ALAC は 16bit、AAC は 32bit float) に、必要ならサンプリング周波数も落として書き出し、
/// 2 回目でそれを ALAC / AAC に符号化する。ALAC の 16bit 化とサンプリング周波数の変換を
/// 1 回の呼び出しでは確実に指定できないため
fn afconvert_args(src: &Path, pcm: &Path, dst: &Path, target: ConvertTarget, src_rate: u32) -> [Vec<std::ffi::OsString>; 2] {
    let os = |s: &str| std::ffi::OsString::from(s);
    let sample = if target == ConvertTarget::Alac { "LEI16" } else { "LEF32" };
    let mut first = vec![os("-f"), os("caff"), os("-d"), os(&format!("{sample}@{}", out_rate(src_rate)))];
    if src_rate > 48000 {
        // 既定より丁寧なサンプリング周波数の変換
        first.extend([os("--src-complexity"), os("bats")]);
    }
    first.extend([src.into(), pcm.into()]);
    let mut second = vec![os("-f"), os("m4af")];
    match target {
        ConvertTarget::Aac { kbps } => {
            // -q 127: 符号化の品質を最高に、-s 2: iTunes Plus と同じ「制約付き VBR」
            let bps = (kbps * 1000).to_string();
            second.extend([os("-d"), os("aac"), os("-b"), os(&bps), os("-q"), os("127"), os("-s"), os("2")]);
        }
        _ => second.extend([os("-d"), os("alac")]),
    }
    second.extend([pcm.into(), dst.into()]);
    [first, second]
}

/// afconvert で変換する (macOS)。タグとジャケットは afconvert が書かないので、変換後に元のファイルから写す
pub fn convert_afconvert(afconvert: &Path, src: &Path, dst: &Path, target: ConvertTarget, src_rate: u32) -> Result<()> {
    let pcm = dst.with_extension("caf");
    let result = (|| {
        for args in afconvert_args(src, &pcm, dst, target, src_rate) {
            run(Command::new(afconvert).args(args), "afconvert")?;
        }
        crate::media::copy_tags_to_m4a(src, dst).map_err(|e| Error::Convert(t!("タグを書き込めません: {e}", e = e)))
    })();
    let _ = std::fs::remove_file(&pcm);
    if result.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    result
}

/// m4a の目次 (moov) をファイルの先頭に置く。iPod は曲を続けて再生するとき次の曲を
/// メモリに入るぶんだけ先読みするので、大きい曲どうしが続くと末尾の moov まで届かず飛ばされる
const FASTSTART: [&str; 2] = ["-movflags", "+faststart"];

/// iPod 向けの AAC の ffmpeg の引数 (曲と動画で共用)。ffmpeg の AAC エンコーダは intensity stereo・PNS・TNS を
/// 既定で使う。iPod のデコーダはこれらをうまく扱えず、短い「ザッ」というノイズ (片方のチャンネルだけのことが多い)
/// になるので、すべて切って素の AAC-LC にする (Issue #26)
pub(crate) const IPOD_AAC: [&str; 10] = ["-c:a", "aac", "-profile:a", "aac_low", "-aac_is", "0", "-aac_pns", "0", "-aac_tns", "0"];

/// m4a の目次 (moov) が音声本体 (mdat) より後ろにあるか。先頭の箱だけを読む
pub fn moov_after_mdat(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut off = 0u64;
    let mut seen_mdat = false;
    while off + 8 <= len {
        let mut h = [0u8; 16];
        if f.seek(SeekFrom::Start(off)).is_err() || f.read_exact(&mut h[..8]).is_err() {
            return false;
        }
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        if size == 1 {
            if f.read_exact(&mut h[8..16]).is_err() {
                return false;
            }
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
        } else if size == 0 {
            size = len - off;
        }
        match &h[4..8] {
            b"mdat" => seen_mdat = true,
            b"moov" => return seen_mdat,
            _ => {}
        }
        if size < 8 {
            return false;
        }
        off += size;
    }
    false
}

/// 音声はそのまま (再エンコードせず) に、m4a の目次を先頭へ並べ替える
pub fn faststart(ffmpeg: &Path, src: &Path, dst: &Path) -> Result<()> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(["-map", "0", "-c", "copy", "-map_metadata", "0"])
        .args(FASTSTART);
    run(cmd.arg(dst), "ffmpeg")
}

/// ffmpeg を使わずに、m4a の目次 (moov) を音声本体 (mdat) の前へ移して `dst` に書く。
/// 音声のデータはそのまま。目次にある各チャンクの位置 (stco / co64) を、ずれる分だけ足して直す
pub fn faststart_native(src: &Path, dst: &Path) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let bad = |m: &str| Error::Convert(m.to_string());
    let mut f = std::fs::File::open(src)?;
    let len = f.metadata()?.len();
    // 先頭の階層の箱: (種類, 位置, 大きさ)
    let mut atoms: Vec<([u8; 4], u64, u64)> = Vec::new();
    let mut off = 0u64;
    while off + 8 <= len {
        let mut h = [0u8; 16];
        f.seek(SeekFrom::Start(off))?;
        f.read_exact(&mut h[..8])?;
        let mut size = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
        if size == 1 {
            f.read_exact(&mut h[8..16])?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
        } else if size == 0 {
            size = len - off;
        }
        if size < 8 || off + size > len {
            return Err(bad("broken mp4 atom"));
        }
        atoms.push((h[4..8].try_into().unwrap(), off, size));
        off += size;
    }
    let moov_i = atoms.iter().position(|a| &a.0 == b"moov").ok_or_else(|| bad("no moov"))?;
    let mdat_i = atoms.iter().position(|a| &a.0 == b"mdat").ok_or_else(|| bad("no mdat"))?;
    if moov_i < mdat_i {
        return Err(bad("moov is already before mdat"));
    }
    let (_, moov_off, moov_size) = atoms[moov_i];
    let mut moov = vec![0u8; moov_size as usize];
    f.seek(SeekFrom::Start(moov_off))?;
    f.read_exact(&mut moov)?;
    // moov より前 (mdat の先頭から moov まで) にあるデータは、moov の大きさの分だけ後ろへずれる
    let (from, to) = (atoms[mdat_i].1, moov_off);
    patch_chunk_offsets(&mut moov, &|o| if (from..to).contains(&o) { o + moov_size } else { o })?;

    let tmp = dst.with_extension("part");
    let result = (|| {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let copy = |f: &mut std::fs::File, off: u64, size: u64, out: &mut std::io::BufWriter<std::fs::File>| -> Result<()> {
            f.seek(SeekFrom::Start(off))?;
            std::io::copy(&mut f.take(size), out)?;
            Ok(())
        };
        for &(_, off, size) in &atoms[..mdat_i] {
            copy(&mut f, off, size, &mut out)?;
        }
        out.write_all(&moov)?;
        for (i, &(_, off, size)) in atoms.iter().enumerate().skip(mdat_i) {
            if i != moov_i {
                copy(&mut f, off, size, &mut out)?;
            }
        }
        crate::device::sync_file(&out.into_inner().map_err(|e| e.into_error())?)?;
        std::fs::rename(&tmp, dst)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// moov の中の stco (32bit) / co64 (64bit) の各位置を `map` で書き換える。32bit に収まらなければエラー
pub(crate) fn patch_chunk_offsets(buf: &mut [u8], map: &dyn Fn(u64) -> u64) -> Result<()> {
    let bad = |m: &str| Error::Convert(m.to_string());
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let mut size = u32::from_be_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
        let mut head = 8;
        if size == 1 {
            if off + 16 > buf.len() {
                return Err(bad("broken mp4 atom"));
            }
            size = u64::from_be_bytes(buf[off + 8..off + 16].try_into().unwrap()) as usize;
            head = 16;
        } else if size == 0 {
            size = buf.len() - off;
        }
        if size < head || off + size > buf.len() {
            return Err(bad("broken mp4 atom"));
        }
        let kind: [u8; 4] = buf[off + 4..off + 8].try_into().unwrap();
        let body = &mut buf[off + head..off + size];
        match &kind {
            // 子の箱を持つもの (チャンクの位置表まで降りる道筋だけ)
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => patch_chunk_offsets(body, map)?,
            b"stco" | b"co64" => {
                let wide = &kind == b"co64";
                let width = if wide { 8 } else { 4 };
                if body.len() < 8 {
                    return Err(bad("broken stco"));
                }
                let count = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
                if body.len() < 8 + count * width {
                    return Err(bad("broken stco"));
                }
                for e in body[8..8 + count * width].chunks_exact_mut(width) {
                    if wide {
                        let v = map(u64::from_be_bytes(e.try_into().unwrap()));
                        e.copy_from_slice(&v.to_be_bytes());
                    } else {
                        let v = map(u32::from_be_bytes(e.try_into().unwrap()) as u64);
                        let v = u32::try_from(v).map_err(|_| bad("chunk offset over 4GB"))?;
                        e.copy_from_slice(&v.to_be_bytes());
                    }
                }
            }
            _ => {}
        }
        off += size;
    }
    Ok(())
}

fn run(cmd: &mut Command, tool: &str) -> Result<()> {
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd
        .output()
        .map_err(|e| Error::Convert(t!("{tool} を起動できません: {e}", tool = tool, e = e)))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Convert(
            msg.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string).unwrap_or_else(|| t!("{tool} エラー", tool = tool)),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_moov_position() {
        let dir = std::env::temp_dir().join(format!("zenpod-moov-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let atom = |t: &[u8; 4], body: usize| {
            let mut v = ((8 + body) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(t);
            v.extend(std::iter::repeat_n(0u8, body));
            v
        };
        let end = dir.join("end.m4a");
        std::fs::write(&end, [atom(b"ftyp", 16), atom(b"mdat", 100), atom(b"moov", 20)].concat()).unwrap();
        let start = dir.join("start.m4a");
        std::fs::write(&start, [atom(b"ftyp", 16), atom(b"moov", 20), atom(b"mdat", 100)].concat()).unwrap();
        assert!(moov_after_mdat(&end));
        assert!(!moov_after_mdat(&start));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 箱を作る (中身は body)
    fn atom(t: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((8 + body.len()) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(t);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn native_faststart_moves_moov_and_fixes_offsets() {
        let dir = std::env::temp_dir().join(format!("zenpod-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ftyp = atom(b"ftyp", &[b'M', b'4', b'A', b' ', 0, 0, 0, 0]);
        let audio: Vec<u8> = (0..200u8).collect();
        let mdat = atom(b"mdat", &audio);
        // チャンクは mdat の中身の先頭と 100 バイト目
        let first = ftyp.len() as u32 + 8;
        let mut stco_body = vec![0, 0, 0, 0];
        stco_body.extend(2u32.to_be_bytes());
        stco_body.extend(first.to_be_bytes());
        stco_body.extend((first + 100).to_be_bytes());
        let stco = atom(b"stco", &stco_body);
        let moov = atom(b"moov", &atom(b"trak", &atom(b"mdia", &atom(b"minf", &atom(b"stbl", &stco)))));
        let free = atom(b"free", &[0; 4]);
        let src = dir.join("end.m4a");
        std::fs::write(&src, [ftyp.clone(), mdat.clone(), moov.clone(), free.clone()].concat()).unwrap();
        let dst = dir.join("start.m4a");
        faststart_native(&src, &dst).unwrap();

        let out = std::fs::read(&dst).unwrap();
        assert_eq!(out.len(), ftyp.len() + mdat.len() + moov.len() + free.len());
        assert!(!moov_after_mdat(&dst));
        assert_eq!(&out[ftyp.len() + 4..ftyp.len() + 8], b"moov");
        // stco の位置は moov の大きさの分だけ後ろへずれ、同じ音声データを指す
        let pos = out.windows(4).position(|w| w == b"stco").unwrap() + 4;
        let entry = |i: usize| u32::from_be_bytes(out[pos + 8 + i * 4..pos + 12 + i * 4].try_into().unwrap()) as usize;
        assert_eq!(entry(0), first as usize + moov.len());
        assert_eq!(out[entry(0)], 0);
        assert_eq!(out[entry(1)], 100);
        // 目次が先頭にあるファイルはそのままにする (エラー)
        assert!(faststart_native(&dst, &dir.join("again.m4a")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ffmpeg がある環境だけ: 本物の m4a (目次が末尾) を並べ替えても、音声が同じに復号できる
    #[test]
    fn native_faststart_keeps_audio() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("zenpod-fs2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("end.m4a");
        let ok = Command::new(&ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-c:a", "alac"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert!(moov_after_mdat(&src));
        let dst = dir.join("start.m4a");
        faststart_native(&src, &dst).unwrap();
        assert!(!moov_after_mdat(&dst));
        let md5 = |p: &Path| {
            let o = Command::new(&ff).args(["-v", "error", "-i"]).arg(p).args(["-f", "md5", "-"]).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8(o.stdout).unwrap()
        };
        assert_eq!(md5(&src), md5(&dst));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metadata_source_by_extension() {
        assert_eq!(metadata_source(Path::new("a.OGG")), "0:s:a:0");
        assert_eq!(metadata_source(Path::new("a.oga")), "0:s:a:0");
        assert_eq!(metadata_source(Path::new("a.opus")), "0:s:a:0");
        assert_eq!(metadata_source(Path::new("a.flac")), "0");
        assert_eq!(metadata_source(Path::new("a.wma")), "0");
    }

    /// ffmpeg がある環境だけ: m4a に変換しても曲名・アーティストとジャケットが残る
    #[test]
    fn tags_and_cover_survive_conversion() {
        use lofty::prelude::*;
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("zenpod-conv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        for (ext, codec) in [("ogg", "libvorbis"), ("opus", "libopus"), ("flac", "flac")] {
            let src = dir.join(format!("src.{ext}"));
            let ok = Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", codec])
                .args(["-metadata", "title=曲名", "-metadata", "artist=歌手"])
                .arg(&src)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            crate::media::write_cover(&src, Some(&png)).unwrap();
            let dst = dir.join(format!("{ext}.m4a"));
            convert(&ff, &src, &dst, ConvertTarget::Aac { kbps: 256 }, 48000).unwrap();
            let info = crate::media::read_info(&dst).unwrap();
            assert_eq!(info.title.as_deref(), Some("曲名"), "{ext}");
            assert_eq!(info.artist.as_deref(), Some("歌手"), "{ext}");
            let tf = lofty::read_from_path(&dst).unwrap();
            let pictures = tf.primary_tag().unwrap().pictures();
            assert_eq!(pictures.len(), 1, "{ext}");
            assert_eq!(pictures[0].data(), png.as_slice(), "{ext}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ffmpeg がある環境だけ: 変換しても歌詞が残る (ffmpeg だけでは UNSYNCEDLYRICS は落ち、MP3 では USLT にならない)
    #[test]
    fn lyrics_survive_conversion() {
        use lofty::file::TaggedFileExt;
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("zenpod-lyrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for key in ["LYRICS", "UNSYNCEDLYRICS"] {
            let src = dir.join(format!("{key}.flac"));
            let ok = Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "flac"])
                .args(["-metadata", &format!("{key}=一行目\n二行目")])
                .arg(&src)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            assert!(crate::media::read_info(&src).unwrap().has_lyrics, "{key}");
            for (target, ext) in [(ConvertTarget::Alac, "m4a"), (ConvertTarget::Mp3 { kbps: 256 }, "mp3")] {
                let dst = dir.join(format!("{key}.{ext}"));
                convert(&ff, &src, &dst, target, 48000).unwrap();
                assert!(crate::media::read_info(&dst).unwrap().has_lyrics, "{key} → {ext}");
                let tf = lofty::read_from_path(&dst).unwrap();
                assert_eq!(crate::media::lyrics_of(tf.primary_tag().unwrap()).as_deref(), Some("一行目\n二行目"), "{key} → {ext}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn afconvert_arguments() {
        let (src, pcm, dst) = (Path::new("a.flac"), Path::new("t.caf"), Path::new("o.m4a"));
        let join = |v: &[std::ffi::OsString]| v.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ");
        let [a, b] = afconvert_args(src, pcm, dst, ConvertTarget::Alac, 96000);
        assert_eq!(join(&a), "-f caff -d LEI16@48000 --src-complexity bats a.flac t.caf");
        assert_eq!(join(&b), "-f m4af -d alac t.caf o.m4a");
        let [a, b] = afconvert_args(src, pcm, dst, ConvertTarget::Aac { kbps: 256 }, 44100);
        assert_eq!(join(&a), "-f caff -d LEF32@44100 a.flac t.caf");
        assert_eq!(join(&b), "-f m4af -d aac -b 256000 -q 127 -s 2 t.caf o.m4a");
        assert_eq!(out_rate(88200), 44100);
        assert!(afconvert_supports(Path::new("x.FLAC"), ConvertTarget::Alac));
        assert!(!afconvert_supports(Path::new("x.flac"), ConvertTarget::Mp3 { kbps: 320 }));
        assert!(!afconvert_supports(Path::new("x.ogg"), ConvertTarget::Aac { kbps: 256 }));
    }

    #[test]
    fn ffmpeg_arguments() {
        let join = |t, rate| codec_args(t, rate).join(" ");
        // ALAC: ディザをかけて 16bit に。48kHz を超える曲はサンプリング周波数もここで落とす
        assert_eq!(join(ConvertTarget::Alac, 44100), "-af aresample=osf=s16p:dither_method=triangular -c:a alac -sample_fmt s16p");
        assert_eq!(join(ConvertTarget::Alac, 96000), "-af aresample=osf=s16p:dither_method=triangular:osr=48000 -c:a alac -sample_fmt s16p");
        // AAC: iPod がうまく扱えない符号化の道具は使わない
        assert_eq!(
            join(ConvertTarget::Aac { kbps: 256 }, 44100),
            "-c:a aac -profile:a aac_low -aac_is 0 -aac_pns 0 -aac_tns 0 -b:a 256k"
        );
        assert_eq!(
            join(ConvertTarget::Aac { kbps: 256 }, 88200),
            "-c:a aac -profile:a aac_low -aac_is 0 -aac_pns 0 -aac_tns 0 -b:a 256k -ar 44100"
        );
        assert_eq!(join(ConvertTarget::Mp3 { kbps: 320 }, 192000), "-c:a libmp3lame -b:a 320k -id3v2_version 3 -ar 48000");
    }

    /// ffmpeg がある環境だけ: 24bit / 96kHz の FLAC が、iPod の再生できる 16bit / 48kHz の ALAC と 48kHz の AAC になる。
    /// 16bit の FLAC は ALAC にしても音声が一致する (ディザがかからない)
    #[test]
    fn high_resolution_flac_converts_for_ipod() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("zenpod-hires-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let make = |name: &str, extra: &[&str]| {
            let p = dir.join(name);
            let ok = Command::new(&ff)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-ac", "2", "-c:a", "flac"])
                .args(extra)
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok, "{name}");
            p
        };
        let stream = |p: &Path| {
            let o = Command::new(&ff).arg("-hide_banner").arg("-i").arg(p).output().unwrap();
            String::from_utf8_lossy(&o.stderr).lines().find(|l| l.contains("Audio:")).unwrap_or("").to_string()
        };
        let md5 = |p: &Path| {
            let o = Command::new(&ff).args(["-v", "error", "-i"]).arg(p).args(["-f", "md5", "-"]).output().unwrap();
            String::from_utf8(o.stdout).unwrap()
        };
        let hires = make("hires.flac", &["-sample_fmt", "s32", "-ar", "96000"]);
        let alac = dir.join("hires-alac.m4a");
        convert(&ff, &hires, &alac, ConvertTarget::Alac, 96000).unwrap();
        let info = stream(&alac);
        assert!(info.contains("48000 Hz") && info.contains("s16p"), "{info}");
        let aac = dir.join("hires-aac.m4a");
        convert(&ff, &hires, &aac, ConvertTarget::Aac { kbps: 256 }, 96000).unwrap();
        assert!(stream(&aac).contains("48000 Hz"), "{}", stream(&aac));
        let plain = make("plain.flac", &["-ar", "44100"]);
        let same = dir.join("plain-alac.m4a");
        convert(&ff, &plain, &same, ConvertTarget::Alac, 44100).unwrap();
        assert_eq!(md5(&plain), md5(&same));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_targets() {
        assert_eq!(ConvertTarget::parse("alac"), Some(ConvertTarget::Alac));
        assert_eq!(ConvertTarget::parse("aac:256"), Some(ConvertTarget::Aac { kbps: 256 }));
        assert_eq!(ConvertTarget::parse("mp3:320"), Some(ConvertTarget::Mp3 { kbps: 320 }));
        assert_eq!(ConvertTarget::parse("aac:999"), None);
        assert_eq!(ConvertTarget::parse("opus:128"), None);
        assert_eq!(ConvertTarget::parse("mp3:320").map(|t| t.ext()), Some("mp3"));
    }
}
