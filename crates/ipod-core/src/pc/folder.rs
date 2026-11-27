//! PC の音楽フォルダの曲を一覧用に読み取る。
//!
//! タグの読み取りは 1 曲数ミリ秒かかるので、ファイルの更新日時とサイズが前回と同じなら
//! キャッシュ (JSON) を使い、変わったファイルだけを 4 スレッドで読む。
//! ネットワーク上のフォルダ (NAS など。`netfs::is_network_path`) は 1 ファイルごとに通信の往復がかかるので、
//! フォルダをたどるのも 8 スレッド、タグの読み取りは 16 スレッドで同時に進める (配布リポジトリの Issue #16)。

use crate::t;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::media;

/// PC のフォルダ内の曲 (一覧表示用)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PcTrack {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    /// 以前のキャッシュには無いので既定値で読む
    #[serde(default)]
    pub composer: String,
    pub duration_ms: u32,
    pub track_nr: u32,
    pub disc_nr: u32,
    pub year: u32,
    pub size: u64,
    /// リリースの種類 ("album" / "single" など)。同じアルバム名のシングルとアルバムを分けて並べる
    #[serde(default)]
    pub release_type: String,
    /// 並べ替え用の読み (ファイルのソート用タグ)
    #[serde(default)]
    pub sort_title: String,
    #[serde(default)]
    pub sort_artist: String,
    #[serde(default)]
    pub sort_album: String,
    #[serde(default)]
    pub sort_album_artist: String,
    /// ここから下は Rockbox のデータベース (rbdb) 用
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub grouping: String,
    #[serde(default)]
    pub bitrate_kbps: u32,
    /// 曲名がタグにあったか (title はタグが無ければファイル名)
    #[serde(default)]
    pub title_tagged: bool,
    /// ジャケットが埋め込まれているか (Rockbox モードで、フォルダのジャケットを決めるのにファイルを開かずに済むように)
    #[serde(default)]
    pub has_picture: bool,
    /// ファイルではなく Rockbox のデータベースから埋めた曲 (並べ替え用のタグ・埋め込みのジャケットの有無は分からない)。
    /// あとでファイルから読み直す (`reread_device_files`)
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_db: bool,
    /// タグを読めなかった理由 (読めたら空)。一覧で印を付ける。読めなかった曲はキャッシュに残さない
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub read_error: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// 読み直す曲のうち、タグを読む代わりに使える曲を返す (`files` と同じ順。使えなければ None)。
/// Rockbox の iPod を初めて開くとき、本体のデータベースから埋める
pub type Seed<'a> = &'a dyn Fn(&[(PathBuf, std::time::SystemTime)]) -> Vec<Option<PcTrack>>;

/// キャッシュ 1 件 (更新日時とサイズが同じなら読み直さない)
#[derive(Serialize, Deserialize)]
struct CacheEntry {
    mtime: u64,
    size: u64,
    track: PcTrack,
    /// 読み取る項目を増やしたら上げる (古いキャッシュの曲は読み直す)
    #[serde(default)]
    ver: u32,
}

// 4: タグが複数あるファイルは空の項目をほかのタグから補う。読めなかった曲を覚えていたかもしれないので読み直す
const CACHE_VER: u32 = 4;

/// フォルダ内の音声ファイル (サブフォルダ含む、パス順)。
/// 隠しファイル・隠しフォルダは除く (Mac 形式でないドライブに macOS が作る `._曲.flac` は音声ではない)
pub fn audio_files(root: &Path) -> Vec<PathBuf> {
    audio_files_with_stat(root, if super::netfs::is_network_path(root) { 8 } else { 1 }).into_iter().map(|(f, _, _)| f).collect()
}

/// `audio_files` と同じ。更新日時と大きさも、フォルダを読んだときの情報から返す
/// (Windows はフォルダを読むときに一緒に返るので、ファイルごとに問い合わせない。2 万曲で約 0.66 秒 → ほぼ 0)
fn audio_files_with_stat(root: &Path, threads: usize) -> Vec<(PathBuf, std::time::SystemTime, u64)> {
    let mut files: Vec<_> = if threads > 1 {
        walk_parallel(root, threads)
    } else {
        walkdir::WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
            .flatten()
            .filter(|e| e.file_type().is_file() && media::is_audio_file(e.path()))
            .map(|e| {
                let m = e.metadata().ok();
                let modified = m.as_ref().and_then(|m| m.modified().ok()).unwrap_or(std::time::UNIX_EPOCH);
                let size = m.map(|m| m.len()).unwrap_or(0);
                (e.into_path(), modified, size)
            })
            .collect()
    };
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// `audio_files_with_stat` を、いくつかのフォルダを同時に読んで行う (ネットワーク上のフォルダ用)。
/// 隠しファイル・隠しフォルダは除く。リンクの先のフォルダもたどる (同じフォルダは 1 回だけ)。順は決まらない
fn walk_parallel(root: &Path, threads: usize) -> Vec<(PathBuf, std::time::SystemTime, u64)> {
    use std::collections::HashSet;
    use std::sync::{Condvar, Mutex};
    struct Queue {
        dirs: Vec<PathBuf>,
        /// 読んでいる途中のフォルダの数 (0 でキューも空なら終わり)
        busy: usize,
    }
    let queue = Mutex::new(Queue { dirs: vec![root.to_path_buf()], busy: 0 });
    let changed = Condvar::new();
    let out = Mutex::new(Vec::new());
    let linked = Mutex::new(HashSet::new());
    let stat = |m: &fs::Metadata| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len());
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let dir = {
                        let mut q = queue.lock().unwrap();
                        loop {
                            if let Some(d) = q.dirs.pop() {
                                q.busy += 1;
                                break d;
                            }
                            if q.busy == 0 {
                                return;
                            }
                            q = changed.wait(q).unwrap();
                        }
                    };
                    let (mut dirs, mut files) = (Vec::new(), Vec::new());
                    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
                        if e.file_name().to_string_lossy().starts_with('.') {
                            continue;
                        }
                        let Ok(ft) = e.file_type() else { continue };
                        let path = e.path();
                        if ft.is_symlink() {
                            match fs::metadata(&path) {
                                Ok(m) if m.is_dir() => {
                                    // リンクで輪になっていても止まるように、たどったリンク先は覚える
                                    if fs::canonicalize(&path).is_ok_and(|c| linked.lock().unwrap().insert(c)) {
                                        dirs.push(path);
                                    }
                                }
                                Ok(m) if m.is_file() && media::is_audio_file(&path) => {
                                    let (modified, size) = stat(&m);
                                    files.push((path, modified, size));
                                }
                                _ => {}
                            }
                        } else if ft.is_dir() {
                            dirs.push(path);
                        } else if ft.is_file() && media::is_audio_file(&path) {
                            let (modified, size) = e.metadata().map(|m| stat(&m)).unwrap_or((std::time::UNIX_EPOCH, 0));
                            files.push((path, modified, size));
                        }
                    }
                    out.lock().unwrap().extend(files);
                    let mut q = queue.lock().unwrap();
                    q.dirs.extend(dirs);
                    q.busy -= 1;
                    changed.notify_all();
                }
            });
        }
    });
    out.into_inner().unwrap()
}

/// フォルダの中のファイル全部 (隠しファイル・隠しフォルダは除く、名前の順)
pub fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    files.sort();
    files
}

pub fn read_track(path: &Path, size: u64) -> PcTrack {
    read_track_checked(path, size, true).0
}

/// `read_track` と同じ。2 つ目はタグを読めたか (読めなかった曲はキャッシュに残さず、次に開くときに読み直す)。
/// `release` が false ならリリースの種類 (MEDIATYPE) を返さない (iPod の中の曲では使わない)。
/// 読むときも、一覧のためのタグの読み取りと一緒に取り出す (読み直すと画像ごと読み、1 曲 1MB ほどになる)
fn read_track_checked(path: &Path, size: u64, release: bool) -> (PcTrack, bool) {
    // ファイルを読めなかったとき (つないだ直後にウイルス対策や検索のインデックスが開いているなど) は、少し待って読み直す。
    // 形式の問題で読めないファイルは待たない
    // 一覧のためなので、埋め込みの画像は読まない (画像があるかだけ調べる。fastread)
    let mut read = crate::media::fastread::read_info_release(path, release);
    for wait in [200, 800] {
        if !matches!(read, Err(crate::error::Error::Io(_))) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(wait));
        read = crate::media::fastread::read_info_release(path, release);
    }
    let ok = read.is_ok();
    let read_error = read.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
    if !ok {
        // tr: 訳さない (ログ)
        log::warn!("タグを読めませんでした: {}: {read_error}", path.display());
    }
    let (info, release_type) = read.unwrap_or_default();
    let s = |o: Option<String>| o.unwrap_or_default();
    let t = PcTrack {
        path: path.to_string_lossy().into_owned(),
        // タグが無ければファイル名を曲名にする
        title: info
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }),
        artist: s(info.artist),
        album: s(info.album),
        album_artist: s(info.album_artist),
        genre: s(info.genre),
        composer: s(info.composer),
        duration_ms: info.duration_ms,
        track_nr: info.track_nr,
        disc_nr: info.disc_nr,
        year: info.year,
        size,
        release_type: if release { release_type.unwrap_or_default() } else { String::new() },
        sort_title: s(info.sort_title),
        sort_artist: s(info.sort_artist),
        sort_album: s(info.sort_album),
        sort_album_artist: s(info.sort_album_artist),
        comment: s(info.comment),
        grouping: s(info.grouping),
        bitrate_kbps: info.bitrate_kbps,
        title_tagged: info.title_tagged,
        has_picture: info.has_picture,
        from_db: false,
        read_error,
    };
    (t, ok)
}

/// フォルダの曲を読み取る。`progress(読んだ数, 読む必要がある数)` は読み直すファイルがあるときだけ呼ばれる
pub fn scan(
    root: &Path,
    cache_path: Option<&Path>,
    progress: impl Fn(usize, usize) + Sync,
) -> Result<Vec<PcTrack>> {
    scan_streaming(root, cache_path, progress, |_| {})
}

/// `scan` と同じ。キャッシュに無い・変わった曲を読めるたびに `on_read` を呼ぶ (読み取りの裏の複数のスレッドから、一覧の順ではなく)。
/// キャッシュにあった曲では呼ばない (画面は先に `cached_folder_tracks` で出しておく)
pub fn scan_streaming(
    root: &Path,
    cache_path: Option<&Path>,
    progress: impl Fn(usize, usize) + Sync,
    on_read: impl Fn(&PcTrack) + Sync,
) -> Result<Vec<PcTrack>> {
    if !root.is_dir() {
        return Err(Error::Other(t!("フォルダが見つかりません: {p}", p = root.display())));
    }
    let remote = super::netfs::is_network_path(root);
    let (walkers, readers) = if remote { (8, 16) } else { (1, 4) };
    let files = audio_files_with_stat(root, walkers);
    Ok(scan_cached(files, cache_path, None, None, Reading { threads: readers, report_cached: false }, progress, on_read))
}

/// 前回読んだときのキャッシュにある、`root` の中の曲 (フォルダをたどらず、ファイルも開かない。場所の順)。
/// フォルダを開いたときにすぐ出す一覧で、消えた・増えた・変わった曲は続く `scan` で分かる
pub fn cached_folder_tracks(root: &Path, cache_path: &Path) -> Vec<PcTrack> {
    let root = root.to_string_lossy();
    let root = root.trim_end_matches(['/', '\\']);
    let mut v: Vec<PcTrack> = load_cache(Some(cache_path))
        .into_iter()
        .filter(|(k, c)| c.ver == CACHE_VER && k.strip_prefix(root).is_some_and(|rest| rest.starts_with(['/', '\\'])))
        .map(|(k, c)| {
            let mut t = c.track;
            t.path = k;
            t
        })
        .collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

/// タグの読み方
#[derive(Clone, Copy)]
struct Reading {
    /// 同時に読む数
    threads: usize,
    /// キャッシュにあった曲でも `on_track` を呼ぶか
    report_cached: bool,
}

const READ_LOCAL: Reading = Reading { threads: 4, report_cached: true };

/// 決めたファイルの曲を読み取る (キャッシュの扱いは `scan` と同じ)。Rockbox の iPod は、
/// iPod_Control などを除いたファイルを渡す
pub fn scan_files(
    files: Vec<PathBuf>,
    cache_path: Option<&Path>,
    progress: impl Fn(usize, usize) + Sync,
) -> Vec<PcTrack> {
    let files = files
        .into_iter()
        .map(|f| {
            let m = fs::metadata(&f).ok();
            let modified = m.as_ref().and_then(|m| m.modified().ok()).unwrap_or(std::time::UNIX_EPOCH);
            let size = m.map(|m| m.len()).unwrap_or(0);
            (f, modified, size)
        })
        .collect();
    scan_files_with_stat(files, cache_path, progress)
}

/// `scan_files` と同じ。更新日時と大きさを先に知っているとき (フォルダを読んだときの情報) はこちら。
/// ファイルごとに問い合わせないので、曲の多い iPod の HDD で速い
pub fn scan_files_with_stat(
    files: Vec<(PathBuf, std::time::SystemTime, u64)>,
    cache_path: Option<&Path>,
    progress: impl Fn(usize, usize) + Sync,
) -> Vec<PcTrack> {
    scan_cached(files, cache_path, None, None, READ_LOCAL, progress, |_| {})
}

/// iPod の中の曲を読む (キャッシュ付き)。キャッシュは `root` からの場所 (小文字、"/" 区切り) で覚えるので、
/// iPod のドライブ文字が変わっても (E: → F: など) 読み直さない
pub fn scan_device_files(
    files: Vec<(PathBuf, std::time::SystemTime, u64)>,
    cache_path: Option<&Path>,
    root: &Path,
    progress: impl Fn(usize, usize) + Sync,
) -> Vec<PcTrack> {
    scan_cached(files, cache_path, Some(root), None, READ_LOCAL, progress, |_| {})
}

/// `scan_device_files` と同じ。曲を読めるたびに (キャッシュにあった曲は最初にまとめて) `on_track` を呼ぶ。
/// 呼ぶ順は一覧の順ではなく、読み取りの裏の複数のスレッドから呼ぶ。
/// キャッシュに無い曲は、先に `seed` で埋められる分を埋め、残りだけタグを読む
pub fn scan_device_files_streaming(
    files: Vec<(PathBuf, std::time::SystemTime, u64)>,
    cache_path: Option<&Path>,
    root: &Path,
    seed: Seed,
    progress: impl Fn(usize, usize) + Sync,
    on_track: impl Fn(&PcTrack) + Sync,
) -> Vec<PcTrack> {
    scan_cached(files, cache_path, Some(root), Some(seed), READ_LOCAL, progress, on_track)
}

/// キャッシュの鍵: `root` があればそこからの場所 (小文字)、無ければフルパス
fn cache_key(f: &Path, root: Option<&Path>) -> String {
    match root.and_then(|r| f.strip_prefix(r).ok()) {
        Some(rel) => rel.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect::<Vec<_>>().join("/"),
        None => f.to_string_lossy().into_owned(),
    }
}

fn load_cache(cache_path: Option<&Path>) -> HashMap<String, CacheEntry> {
    cache_path
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_cache(cache_path: Option<&Path>, cache: &HashMap<String, CacheEntry>) {
    let Some(p) = cache_path else { return };
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(cache) {
        let _ = fs::write(p, json);
    }
}

/// `files` のうち、タグのキャッシュに覚えたときと更新日時・大きさが同じ曲 (タグは読まない・曲のファイルを開かない)。
/// `files` と同じ順で、キャッシュに無い・変わった曲は None。`root` は `scan_device_files_streaming` と同じ
pub fn cached_device_tracks(files: &[(PathBuf, std::time::SystemTime, u64)], cache_path: &Path, root: &Path) -> Vec<Option<PcTrack>> {
    let cache = load_cache(Some(cache_path));
    files
        .iter()
        .map(|(f, m, size)| {
            let mtime = m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let c = cache.get(&cache_key(f, Some(root)))?;
            (c.mtime == mtime && c.size == *size && c.ver == CACHE_VER).then(|| {
                let mut t = c.track.clone();
                t.path = f.to_string_lossy().into_owned();
                t
            })
        })
        .collect()
}

fn scan_cached(
    files: Vec<(PathBuf, std::time::SystemTime, u64)>,
    cache_path: Option<&Path>,
    root: Option<&Path>,
    seed: Option<Seed>,
    reading: Reading,
    progress: impl Fn(usize, usize) + Sync,
    on_track: impl Fn(&PcTrack) + Sync,
) -> Vec<PcTrack> {
    let mut cache = load_cache(cache_path);
    let stats: Vec<(u64, u64)> = files
        .iter()
        .map(|(_, m, size)| (m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0), *size))
        .collect();
    let files: Vec<PathBuf> = files.into_iter().map(|(f, _, _)| f).collect();

    let mut out: Vec<Option<PcTrack>> = vec![None; files.len()];
    let mut todo: Vec<(usize, u64, u64)> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let (mtime, size) = stats[i];
        match cache.get(&cache_key(f, root)) {
            Some(c) if c.mtime == mtime && c.size == size && c.ver == CACHE_VER => {
                // 覚えたときとドライブ文字が違ってもよいように、場所は今のものにする
                let mut t = c.track.clone();
                t.path = f.to_string_lossy().into_owned();
                if reading.report_cached {
                    on_track(&t);
                }
                out[i] = Some(t);
            }
            _ => todo.push((i, mtime, size)),
        }
    }

    // データベースなどから埋められる曲は、タグを読まずに済ませる
    let mut seeded = 0;
    if let (Some(seed), false) = (seed, todo.is_empty()) {
        let asked: Vec<(PathBuf, std::time::SystemTime)> = todo
            .iter()
            .map(|&(i, mtime, _)| (files[i].clone(), std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime)))
            .collect();
        let got = seed(&asked);
        let mut rest = Vec::with_capacity(todo.len());
        for (&(i, mtime, size), t) in todo.iter().zip(got.into_iter().chain(std::iter::repeat(None))) {
            let Some(mut t) = t else {
                rest.push((i, mtime, size));
                continue;
            };
            t.path = files[i].to_string_lossy().into_owned();
            t.size = size;
            t.from_db = true;
            on_track(&t);
            cache.insert(cache_key(&files[i], root), CacheEntry { mtime, size, track: t.clone(), ver: CACHE_VER });
            out[i] = Some(t);
            seeded += 1;
        }
        todo = rest;
    }

    let total = todo.len();
    let done = AtomicUsize::new(0);
    // 次に読む曲の番号。読み終えた係から順に取る (遅いファイルがあっても、ほかの係が先へ進める)
    let next = AtomicUsize::new(0);
    let threads = reading.threads.clamp(1, total.max(1));
    let read: Vec<(usize, u64, PcTrack, bool)> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (files, todo, done, next, progress, on_track) = (&files, &todo, &done, &next, &progress, &on_track);
                sc.spawn(move || {
                    let mut got = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(i, mtime, size)) = todo.get(k) else { break };
                        let (t, ok) = read_track_checked(&files[i], size, root.is_none());
                        // 開発版だけ: 遅い iPod を真似る (ZENPOD_DEV_READ_DELAY_MS に 1 曲あたりの待ち時間)
                        #[cfg(debug_assertions)]
                        if let Some(ms) = std::env::var("ZENPOD_DEV_READ_DELAY_MS").ok().and_then(|v| v.parse().ok()) {
                            std::thread::sleep(std::time::Duration::from_millis(ms));
                        }
                        on_track(&t);
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 25 == 0 || n == total {
                            progress(n, total);
                        }
                        got.push((i, mtime, t, ok));
                    }
                    got
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });

    for (i, mtime, t, ok) in read {
        // 読めなかった曲 (壊れたタグ・一時的な読み取りの失敗など) は覚えない。覚えると、ファイルが変わるまで空のままになる
        if !ok {
            out[i] = Some(t);
            continue;
        }
        cache.insert(
            cache_key(&files[i], root),
            CacheEntry {
                mtime,
                size: t.size,
                track: t.clone(),
                ver: CACHE_VER,
            },
        );
        out[i] = Some(t);
    }
    if total + seeded > 0 {
        save_cache(cache_path, &cache);
    }
    out.into_iter().flatten().collect()
}

/// データベースから埋めた曲 (`from_db`) のタグをファイルから読み直し、キャッシュを直す。iPod を開いたあとに裏で使う。
/// HDD の iPod でほかの操作の邪魔をしないよう、1 曲ずつ順に読む。`pause` が true の間は待ち、`stop` が true になったらやめる。
/// 戻り値は読み直せた曲 (場所は `files` のもの)
pub fn reread_device_files(
    files: &[PathBuf],
    cache_path: Option<&Path>,
    root: &Path,
    pause: &dyn Fn() -> bool,
    stop: &dyn Fn() -> bool,
) -> Vec<PcTrack> {
    let mut read = Vec::new();
    for f in files {
        while pause() && !stop() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        if stop() {
            break;
        }
        let Ok(m) = fs::metadata(f) else { continue };
        let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let (t, ok) = read_track_checked(f, m.len(), false);
        if ok {
            read.push((mtime, t));
        }
    }
    if read.is_empty() {
        return Vec::new();
    }
    // 読んでいる間に、ほかの処理がキャッシュを書いているかもしれないので、読み込み直してから足す
    let mut cache = load_cache(cache_path);
    for (mtime, t) in &read {
        let key = cache_key(Path::new(&t.path), Some(root));
        // その間にファイルが変わって読み直された曲 (データベースから埋めた印が無い) は、そちらを残す
        if cache.get(&key).is_some_and(|c| !c.track.from_db && c.ver == CACHE_VER) {
            continue;
        }
        cache.insert(key, CacheEntry { mtime: *mtime, size: t.size, track: t.clone(), ver: CACHE_VER });
    }
    save_cache(cache_path, &cache);
    read.into_iter().map(|(_, t)| t).collect()
}

/// ファイル名 (拡張子を除く部分) として使えるか。Windows で使えない文字・予約名を弾く
pub fn validate_stem(stem: &str) -> std::result::Result<(), String> {
    if stem.trim().is_empty() {
        return Err(t!("ファイル名が空です"));
    }
    if stem.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return Err(t!("ファイル名に \\ / : * ? \" < > | は使えません"));
    }
    if stem.ends_with('.') || stem.ends_with(' ') {
        return Err(t!("ファイル名の最後に「.」や空白は使えません"));
    }
    let upper = stem.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str())
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(t!("この名前は Windows では使えません"));
    }
    Ok(())
}

/// 同じフォルダの中で名前を変える (拡張子はそのまま)。戻り値は新しい場所
pub fn rename(path: &Path, new_stem: &str) -> Result<PathBuf> {
    let new_stem = new_stem.trim();
    validate_stem(new_stem).map_err(Error::Other)?;
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    let name = match ext {
        Some(e) => format!("{new_stem}.{e}"),
        None => new_stem.to_string(),
    };
    let dst = path.with_file_name(name);
    if dst == path {
        return Ok(dst);
    }
    // 大文字小文字だけの変更 (Windows では同じファイル) は許す
    let same_file = dst.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
    if dst.exists() && !same_file {
        return Err(Error::Other(t!("同じ名前のファイルが既にあります")));
    }
    fs::rename(path, &dst)?;
    Ok(dst)
}

/// まとめて名前を変える (拡張子はそのまま)。`renames` は (元の場所, 新しい名前 (拡張子なし))。
/// A→B と B→A のような入れ替えでもぶつからないよう、いったん仮の名前にしてから付け直す。
/// 途中で失敗したら、それまでに変えた分を元に戻してエラーを返す。戻り値は新しい場所
pub fn rename_many(renames: &[(PathBuf, String)]) -> Result<Vec<PathBuf>> {
    let key = |p: &Path| p.to_string_lossy().to_lowercase();
    let mut dsts = Vec::with_capacity(renames.len());
    for (src, stem) in renames {
        let stem = stem.trim();
        validate_stem(stem).map_err(|m| Error::Other(format!("{stem}: {m}")))?;
        let name = match src.extension() {
            Some(e) => format!("{stem}.{}", e.to_string_lossy()),
            None => stem.to_string(),
        };
        dsts.push(src.with_file_name(name));
    }
    // 変更後の名前どうしの重複と、変更しない既存ファイルとの重複を調べる (Windows は大文字小文字を区別しない)
    let sources: HashMap<String, ()> = renames.iter().map(|(s, _)| (key(s), ())).collect();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for d in &dsts {
        if seen.insert(key(d), ()).is_some() {
            return Err(Error::Other(t!("同じ名前になるファイルがあります: {p}", p = d.display())));
        }
        if d.exists() && !sources.contains_key(&key(d)) {
            return Err(Error::Other(t!("同じ名前のファイルが既にあります: {p}", p = d.display())));
        }
    }
    // 1 段目: 仮の名前へ
    let mut tmps: Vec<PathBuf> = Vec::new();
    for (i, (src, _)) in renames.iter().enumerate() {
        let tmp = src.with_file_name(format!(".zenpod-rename-{}-{i}.tmp", std::process::id()));
        if let Err(e) = fs::rename(src, &tmp) {
            for (j, t) in tmps.iter().enumerate() {
                let _ = fs::rename(t, &renames[j].0);
            }
            return Err(Error::Other(format!("{}: {e}", src.display())));
        }
        tmps.push(tmp);
    }
    // 2 段目: 新しい名前へ
    for (i, tmp) in tmps.iter().enumerate() {
        if let Err(e) = fs::rename(tmp, &dsts[i]) {
            for (j, t) in tmps.iter().enumerate() {
                let from = if j < i { &dsts[j] } else { t };
                let _ = fs::rename(from, &renames[j].0);
            }
            return Err(Error::Other(format!("{}: {e}", dsts[i].display())));
        }
    }
    Ok(dsts)
}

#[cfg(test)]
mod tests {

    /// 同時にたどっても、1 つずつたどったときと同じファイルになる (隠しフォルダは除く・リンクの輪で止まる)
    #[test]
    fn parallel_walk_matches_sequential() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("音楽");
        for (i, a) in ["A/1", "A/2", "B", "B/ディスク 2", ".hidden", "C/.git"].iter().enumerate() {
            let d = root.join(a);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("{i:02} 曲.flac")), b"x").unwrap();
            std::fs::write(d.join("cover.jpg"), b"x").unwrap();
            std::fs::write(d.join("._mac.flac"), b"x").unwrap();
        }
        #[cfg(unix)]
        {
            // リンクの先のフォルダもたどる。輪になっていても止まる
            std::os::unix::fs::symlink(root.join("A"), root.join("B/A へのリンク")).unwrap();
            std::os::unix::fs::symlink(&root, root.join("A/1/上へ")).unwrap();
        }
        let names = |v: Vec<(std::path::PathBuf, std::time::SystemTime, u64)>| {
            let mut n: Vec<String> =
                v.into_iter().map(|(p, _, size)| format!("{} {size}", p.file_name().unwrap().to_string_lossy())).collect();
            n.sort();
            n.dedup();
            n
        };
        let one = super::audio_files_with_stat(&root, 1);
        let many = super::audio_files_with_stat(&root, 8);
        assert!(many.windows(2).all(|w| w[0].0 <= w[1].0), "場所の順");
        assert_eq!(names(many), names(one));
        let got = names(super::audio_files_with_stat(&root, 8));
        assert_eq!(got, ["00 曲.flac 1", "01 曲.flac 1", "02 曲.flac 1", "03 曲.flac 1"]);
    }

    /// 前回のキャッシュから、そのフォルダの中の曲だけをすぐ出す (名前が似ている別のフォルダは含めない)
    #[test]
    fn cached_tracks_of_one_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Music");
        let other = dir.path().join("Music 2");
        for d in [&root, &other] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/audio/plain.mp3"), d.join("a.mp3")).unwrap();
        }
        let cache = dir.path().join("cache.json");
        assert!(super::cached_folder_tracks(&root, &cache).is_empty());
        super::scan(&root, Some(&cache), |_, _| {}).unwrap();
        super::scan(&other, Some(&cache), |_, _| {}).unwrap();
        let got = super::cached_folder_tracks(&root, &cache);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, root.join("a.mp3").to_string_lossy());
        assert_eq!(got, super::scan(&root, Some(&cache), |_, _| {}).unwrap());
    }

    /// macOS が作る `._` のファイルと隠しフォルダの中は曲にしない (Issue #6)
    #[test]
    fn audio_files_skip_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".ライブラリ");
        let album = root.join("Artist/Album");
        fs::create_dir_all(&album).unwrap();
        fs::create_dir_all(root.join(".Trashes/501")).unwrap();
        fs::write(album.join("05 Daria.flac"), b"").unwrap();
        fs::write(album.join("._05 Daria.flac"), b"").unwrap();
        fs::write(root.join(".Trashes/501/old.mp3"), b"").unwrap();
        assert_eq!(audio_files(&root), vec![album.join("05 Daria.flac")], "ルート自体が . で始まっても中は見る");
    }

    /// iPod の曲のキャッシュは iPod の中の場所で覚えるので、ドライブ文字 (ルートの場所) が変わっても読み直さない
    #[test]
    fn device_cache_survives_root_change() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, cache) = (dir.path().join("E"), dir.path().join("F"), dir.path().join("cache.json"));
        let file = |root: &Path| root.join("Music/X/01 曲.wav");
        fs::create_dir_all(file(&a).parent().unwrap()).unwrap();
        let wav = tiny_wav();
        fs::write(file(&a), &wav).unwrap();
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        fs::File::options().write(true).open(file(&a)).unwrap().set_modified(when).unwrap();
        let len = wav.len() as u64;
        let stat = |root: &Path| vec![(file(root), when, len)];
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let count = |_: usize, _: usize| {
            reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        };
        scan_device_files(stat(&a), Some(&cache), &a, count);
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 1, "1 回目は読む");
        // 同じ iPod が別のドライブ文字でつながった
        fs::rename(&a, &b).unwrap();
        let got = scan_device_files(stat(&b), Some(&cache), &b, count);
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 1, "読み直さない");
        assert_eq!(got[0].path, file(&b).to_string_lossy(), "場所は今のもの");
    }

    /// 短い無音の WAV (16 bit・モノラル・8 kHz)
    fn tiny_wav() -> Vec<u8> {
        let data = vec![0u8; 1600];
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&8000u32.to_le_bytes());
        w.extend_from_slice(&16000u32.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        w
    }

    /// タグを読めなかった曲は覚えず、次も読み直す (覚えると、ファイルが変わるまで曲名・アルバムが空のままになる)
    #[test]
    fn unreadable_files_are_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let (root, cache) = (dir.path().join("E"), dir.path().join("cache.json"));
        let f = root.join("Music/X/01 曲.mp3");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, b"not really audio").unwrap();
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let count = |_: usize, _: usize| {
            reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        };
        for n in 1..=2 {
            let got = scan_device_files(vec![(f.clone(), when, 16)], Some(&cache), &root, count);
            assert_eq!(got[0].title, "01 曲", "曲名はファイル名");
            assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), n, "毎回読み直す");
        }
    }
    use super::*;

    #[test]
    fn stem_rules() {
        assert!(validate_stem("01 - 夜行").is_ok());
        assert!(validate_stem("").is_err());
        assert!(validate_stem("a/b").is_err());
        assert!(validate_stem("なに?").is_err());
        assert!(validate_stem("曲名.").is_err());
        assert!(validate_stem("con").is_err());
        assert!(validate_stem("COM1").is_err());
        assert!(validate_stem("COMIC").is_ok());
    }

    #[test]
    fn rename_many_swaps_and_checks_conflicts() {
        let dir = std::env::temp_dir().join(format!("zenpod-rename-many-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a.flac"), dir.join("b.flac"), dir.join("keep.flac"));
        fs::write(&a, b"A").unwrap();
        fs::write(&b, b"B").unwrap();
        fs::write(&c, b"C").unwrap();
        // 入れ替え
        rename_many(&[(a.clone(), "b".into()), (b.clone(), "a".into())]).unwrap();
        assert_eq!(fs::read(&a).unwrap(), b"B");
        assert_eq!(fs::read(&b).unwrap(), b"A");
        // 変更しない既存ファイルとぶつかる / 変更後どうしがぶつかる → 何も変えない
        assert!(rename_many(&[(a.clone(), "keep".into())]).is_err());
        assert!(rename_many(&[(a.clone(), "x".into()), (b.clone(), "X".into())]).is_err());
        assert!(a.exists() && b.exists());
        let out = rename_many(&[(a.clone(), "01 - 夜行".into()), (b.clone(), "02 - 春泥棒".into())]).unwrap();
        assert_eq!(out[0].file_name().unwrap(), "01 - 夜行.flac");
        assert!(out.iter().all(|p| p.is_file()) && !a.exists() && !b.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_keeps_extension() {
        let dir = std::env::temp_dir().join(format!("zenpod-rename-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let a = dir.join("a.flac");
        fs::write(&a, b"x").unwrap();
        fs::write(dir.join("b.flac"), b"y").unwrap();
        assert!(rename(&a, "b").is_err(), "既存の名前には変えない");
        let c = rename(&a, "  新しい名前 ").unwrap();
        assert_eq!(c.file_name().unwrap(), "新しい名前.flac");
        assert!(c.is_file() && !a.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

