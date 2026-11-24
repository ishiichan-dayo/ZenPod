//! Rockbox の動画と写真。Rockbox は動画も写真もファイルを開くだけなので、DB は無い。
//!
//! - 動画: `Videos/` (テレビ番組は `Videos/番組名/`) に MPEG-2 の .mpg で置く (動画のプラグイン mpegplayer が開ける形)
//! - 写真: `Pictures/` (アルバムは `Pictures/アルバム名/`) に、640×480 に収めたベースラインの JPEG で置く
//!   (画像のビューアはプログレッシブ JPEG を読めず、大きい画像は開くのが遅い)
//! - 同じ写真を 2 回送らないよう、元の写真の中身の印を `Pictures/.zenpod-photos.json` に覚える
//! - どちらも曲の一覧を作るときには見ない (SKIP_DIRS)。初めて使うときに読み、変えたら読み直す

use crate::t;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::{FREE_MARGIN, RockboxLib, clean_name, id_of, rbdb, rel_key, unique_path};
use crate::domain::photo::{PhotoAlbumView, PhotoLibrary, PhotoOutcome, PhotoView};
use crate::domain::video::{VideoEdit, VideoKind, VideoOptions, VideoOutcome, VideoView};
use crate::error::{Error, Result};
use crate::media::photo as photo_file;
use crate::media::video::{self, Prepared, Progress, VideoInfo, VideoTarget};

pub(super) const VIDEO_DIR: &str = "Videos";
pub(super) const PICTURE_DIR: &str = "Pictures";
const PHOTO_INDEX: &str = ".zenpod-photos.json";
/// 写真を置く大きさ (iPod の画面の 2 倍。拡大して見られるように)
const PHOTO_MAX: (u32, u32) = (640, 480);

/// 置いてある写真 1 枚
#[derive(Debug, Clone)]
pub(super) struct RbPhoto {
    id: u64,
    path: PathBuf,
    /// アルバム (Pictures の下のフォルダ)。Pictures の直下なら None
    album: Option<PathBuf>,
    size: u64,
    mtime: i64,
}

/// 置いてある動画 1 本
#[derive(Debug, Clone)]
pub(super) struct RbVideo {
    view: VideoView,
    path: PathBuf,
}

fn mtime_of(m: &fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// フォルダの中を全部たどってファイルを集める (隠しファイルは除く)
fn walk(dir: &Path, keep: &dyn Fn(&Path) -> bool) -> Vec<(PathBuf, fs::Metadata)> {
    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .flatten()
        .filter(|e| e.file_type().is_file() && keep(e.path()))
        .filter_map(|e| Some((e.path().to_path_buf(), e.metadata().ok()?)))
        .collect()
}

/// 空になったフォルダを消す (`top` そのものは消さない)
fn remove_empty_up_to(dir: &Path, top: &Path) {
    let mut cur = Some(dir.to_path_buf());
    while let Some(d) = cur {
        if !d.starts_with(top) || d == top {
            break;
        }
        let empty = fs::read_dir(&d).map(|rd| {
            rd.flatten().all(|e| {
                let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                n.starts_with("._") || n == ".ds_store" || n == "thumbs.db"
            })
        });
        if !empty.unwrap_or(false) {
            break;
        }
        let _ = fs::remove_dir_all(&d);
        cur = d.parent().map(Path::to_path_buf);
    }
}

fn move_file(from: &Path, to_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(to_dir)?;
    let stem = from.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = from.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let to = unique_path(to_dir, &stem, &ext);
    fs::rename(from, &to)?;
    Ok(to)
}

impl RockboxLib {
    fn videos_dir(&self) -> PathBuf {
        self.root().join(VIDEO_DIR)
    }
    fn pictures_dir(&self) -> PathBuf {
        self.root().join(PICTURE_DIR)
    }

    fn rb_videos(&self) -> &[RbVideo] {
        self.video_cache.get_or_init(|| {
            let root = self.root().to_path_buf();
            let top = self.videos_dir();
            let mut out: Vec<RbVideo> = walk(&top, &|p| {
                p.extension().and_then(|e| e.to_str()).is_some_and(|e| video::ROCKBOX_EXTS.contains(&e.to_ascii_lowercase().as_str()))
            })
            .into_iter()
            .map(|(path, meta)| {
                let info = video::probe_mpeg_ps(&path).unwrap_or_default();
                let parent = path.parent().filter(|p| *p != top);
                let show = parent.and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let key = rel_key(&root, &path);
                let view = VideoView {
                    id: id_of(&key).to_string(),
                    title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    kind: if show.is_empty() { VideoKind::Movie } else { VideoKind::TvShow },
                    show,
                    duration_ms: info.duration_ms,
                    size: meta.len(),
                    date_added: mtime_of(&meta),
                    rb_path: rbdb::rel_path(&root, &path),
                    ..Default::default()
                };
                RbVideo { view, path }
            })
            .collect();
            out.sort_by_key(|v| v.view.title.to_lowercase());
            out
        })
    }

    fn rb_photos(&self) -> &[RbPhoto] {
        self.photo_cache.get_or_init(|| {
            let root = self.root().to_path_buf();
            let top = self.pictures_dir();
            let mut out: Vec<RbPhoto> = walk(&top, &|p| {
                p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "bmp" | "gif"))
            })
            .into_iter()
            .map(|(path, meta)| RbPhoto {
                id: id_of(&rel_key(&root, &path)),
                album: path.parent().filter(|p| *p != top).map(Path::to_path_buf),
                size: meta.len(),
                mtime: mtime_of(&meta),
                path,
            })
            .collect();
            out.sort_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.path.cmp(&b.path)));
            out
        })
    }

    fn forget_videos(&mut self) {
        self.video_cache.take();
    }
    fn forget_photos(&mut self) {
        self.photo_cache.take();
    }

    // ---------------- 動画 ----------------

    pub fn videos(&self) -> Vec<VideoView> {
        self.rb_videos().iter().map(|v| v.view.clone()).collect()
    }

    pub fn video_file(&self, id: u64) -> Option<PathBuf> {
        let id = id.to_string();
        self.rb_videos().iter().find(|v| v.view.id == id).map(|v| v.path.clone())
    }

    /// 置き場所: (フォルダ, 拡張子を除いた名前, 題名)。テレビ番組は Videos/番組名/S01E02 題名
    fn video_place(&self, src: &Path, info: &VideoInfo) -> (PathBuf, String, String) {
        let tags = &info.tags;
        let title = tags.title.clone().unwrap_or_else(|| src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        let stem = match (tags.season, tags.episode) {
            (s, e) if s > 0 && e > 0 => format!("S{s:02}E{e:02} {title}"),
            (_, e) if e > 0 => format!("{e:02} {title}"),
            _ => title.clone(),
        };
        let dir = match tags.show.as_deref().map(|s| clean_name(s, "")).filter(|s| !s.is_empty()) {
            Some(show) => self.videos_dir().join(show),
            None => self.videos_dir(),
        };
        (dir, clean_name(&stem, "Video"), title)
    }

    /// 同じ動画 (同じ置き場所に同じ名前の .mpg) があればその題名
    pub fn video_duplicate(&self, src: &Path, info: &VideoInfo) -> Option<String> {
        let (dir, stem, title) = self.video_place(src, info);
        dir.join(format!("{stem}.mpg")).is_file().then_some(title)
    }

    /// 準備した (変換した) 動画を置く
    pub fn add_prepared_video(&mut self, src: &Path, p: &Prepared) -> Result<VideoView> {
        let (dir, stem, _) = self.video_place(src, &p.info);
        let size = fs::metadata(&p.file)?.len();
        let free = fs4::available_space(self.root()).unwrap_or(u64::MAX);
        if size + FREE_MARGIN > free {
            return Err(Error::NoSpace);
        }
        fs::create_dir_all(&dir)?;
        let dest = unique_path(&dir, &stem, "mpg");
        if let Err(e) = fs::copy(&p.file, &dest).and_then(|_| crate::device::sync_file(&fs::OpenOptions::new().write(true).open(&dest)?)) {
            let _ = fs::remove_file(&dest);
            return Err(e.into());
        }
        self.forget_videos();
        self.info.free_bytes = fs4::available_space(self.root()).unwrap_or(self.info.free_bytes);
        let id = id_of(&rel_key(self.root(), &dest)).to_string();
        Ok(self.rb_videos().iter().find(|v| v.view.id == id).map(|v| v.view.clone()).unwrap_or_default())
    }

    /// 動画を 1 本送る。MPEG-1 / 2 の .mpg 以外は 320×240 の MPEG-2 に変換する
    pub fn add_video(&mut self, src: &Path, ffmpeg: Option<&Path>, opts: &VideoOptions, progress: &Progress) -> Result<VideoOutcome> {
        let mut info = video::probe(src, ffmpeg)?;
        video::apply_placement(&mut info.tags, &opts.placement);
        if !info.has_video() {
            return Err(Error::Media(t!("映像が見つかりません")));
        }
        if opts.skip_duplicates
            && let Some(title) = self.video_duplicate(src, &info)
        {
            return Ok(VideoOutcome::Duplicate { title });
        }
        let p = video::prepare(src, ffmpeg, VideoTarget::Rockbox, info, false, progress)?;
        let view = self.add_prepared_video(src, &p)?;
        Ok(VideoOutcome::Added { video: Box::new(view), converted: p.converted })
    }

    pub fn remove_videos(&mut self, ids: &[u64]) -> Result<usize> {
        let set: HashSet<String> = ids.iter().map(|i| i.to_string()).collect();
        let top = self.videos_dir();
        let mut n = 0;
        for v in self.rb_videos().iter().filter(|v| set.contains(&v.view.id)) {
            fs::remove_file(&v.path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })?;
            if let Some(d) = v.path.parent() {
                remove_empty_up_to(d, &top);
            }
            n += 1;
        }
        self.forget_videos();
        self.info.free_bytes = fs4::available_space(self.root()).unwrap_or(self.info.free_bytes);
        Ok(n)
    }

    /// 題名 (ファイル名) と番組 (フォルダ) を変える。ほかの項目は Rockbox では持てない
    pub fn edit_videos(&mut self, ids: &[u64], e: &VideoEdit) -> Result<usize> {
        let set: HashSet<String> = ids.iter().map(|i| i.to_string()).collect();
        let top = self.videos_dir();
        let targets: Vec<RbVideo> = self.rb_videos().iter().filter(|v| set.contains(&v.view.id)).cloned().collect();
        let mut n = 0;
        for v in targets {
            let show = e.show.as_deref().map(str::trim).unwrap_or(&v.view.show).to_string();
            let title = e.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(&v.view.title).to_string();
            let dir = if show.is_empty() { top.clone() } else { top.join(clean_name(&show, "Show")) };
            let stem = clean_name(&title, "Video");
            if dir == v.path.parent().unwrap_or(&top) && v.path.file_stem().is_some_and(|s| s.to_string_lossy() == stem) {
                continue;
            }
            fs::create_dir_all(&dir)?;
            let ext = v.path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "mpg".into());
            let to = unique_path(&dir, &stem, &ext);
            fs::rename(&v.path, &to)?;
            if let Some(d) = v.path.parent() {
                remove_empty_up_to(d, &top);
            }
            n += 1;
        }
        if n > 0 {
            self.forget_videos();
        }
        Ok(n)
    }

    // ---------------- 写真 ----------------

    /// Pictures の下のフォルダ (どの深さも。隠しフォルダは除く) = アルバム
    fn album_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(self.pictures_dir())
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .flatten()
            .filter(|e| e.file_type().is_dir())
            .map(|e| e.into_path())
            .collect();
        dirs.sort();
        dirs
    }

    /// 写真とアルバム。アルバムは Pictures の下のフォルダで、階層 (親) を持つ。写真を全部外したフォルダも出す
    pub fn photo_library(&self) -> PhotoLibrary {
        let photos = self.rb_photos();
        let root = self.root();
        let top = self.pictures_dir();
        let album_id = |d: &Path| id_of(&rel_key(root, d)).to_string();
        let albums = self
            .album_dirs()
            .into_iter()
            .map(|d| PhotoAlbumView {
                id: album_id(&d),
                name: d.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                photo_ids: photos.iter().filter(|p| p.album.as_deref() == Some(d.as_path())).map(|p| p.id.to_string()).collect(),
                parent: d.parent().filter(|p| *p != top).map(album_id),
            })
            .collect();
        PhotoLibrary {
            photos: photos
                .iter()
                .map(|p| PhotoView {
                    id: p.id.to_string(),
                    width: 0,
                    height: 0,
                    date: p.mtime,
                    size: p.size,
                    name: p.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                })
                .collect(),
            albums,
            supported: true,
        }
    }

    fn photo_index(&self) -> HashMap<String, String> {
        fs::read(self.pictures_dir().join(PHOTO_INDEX)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save_photo_index(&self, index: &HashMap<String, String>) {
        let _ = fs::write(self.pictures_dir().join(PHOTO_INDEX), serde_json::to_vec(index).unwrap_or_default());
    }

    fn album_dir(&self, id: u64) -> Result<PathBuf> {
        let root = self.root();
        self.album_dirs()
            .into_iter()
            .find(|p| id_of(&rel_key(root, p)) == id)
            .ok_or_else(|| Error::Other(t!("アルバムが見つかりません")))
    }

    /// iPod のルートからの場所 ("/Pictures/旅行/a.jpg")。覚えている写真の場所の形
    fn rel_of(&self, p: &Path) -> String {
        rbdb::rel_path(self.root(), p)
    }

    /// 覚えている写真の場所を、動かしたファイル・フォルダに合わせて直す
    fn rewrite_index(&self, moves: &[(PathBuf, PathBuf)]) {
        if moves.is_empty() {
            return;
        }
        let mut index = self.photo_index();
        for (from, to) in moves {
            let (old, new) = (self.rel_of(from), self.rel_of(to));
            for v in index.values_mut() {
                if *v == old {
                    *v = new.clone();
                } else if let Some(rest) = v.strip_prefix(&format!("{old}/")) {
                    *v = format!("{new}/{rest}");
                }
            }
        }
        self.save_photo_index(&index);
    }

    /// 写真を 1 枚送る。`album` があれば Pictures/アルバム名/ に置く ("旅行/2024" のように / で区切れば階層にする)
    pub fn add_photo(&mut self, src: &Path, album: Option<&str>, ffmpeg: Option<&Path>) -> Result<PhotoOutcome> {
        let loaded = photo_file::load(src, ffmpeg)?;
        let mut index = self.photo_index();
        let root = self.root().to_path_buf();
        if let Some(rel) = index.get(&loaded.key)
            && root.join(rel.trim_start_matches('/')).is_file()
        {
            return Ok(PhotoOutcome::Duplicate);
        }
        let img = &loaded.image;
        let img = if img.width() > PHOTO_MAX.0 || img.height() > PHOTO_MAX.1 {
            img.resize(PHOTO_MAX.0, PHOTO_MAX.1, image::imageops::FilterType::CatmullRom)
        } else {
            img.clone()
        };
        let jpeg = photo_file::to_jpeg(&img, 90).ok_or_else(|| Error::Media(t!("画像を書けません")))?;
        let free = fs4::available_space(&root).unwrap_or(u64::MAX);
        if jpeg.len() as u64 + FREE_MARGIN > free {
            return Err(Error::NoSpace);
        }
        let mut dir = self.pictures_dir();
        for part in album.unwrap_or("").split('/').map(|a| clean_name(a.trim(), "")).filter(|a| !a.is_empty()) {
            dir.push(part);
        }
        fs::create_dir_all(&dir)?;
        let stem = clean_name(&src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), "Photo");
        let dest = unique_path(&dir, &stem, "jpg");
        fs::write(&dest, &jpeg)?;
        // 撮った日時をファイルの更新日時にする (一覧を撮った順に並べるため)
        if loaded.date > 0
            && let Ok(f) = fs::OpenOptions::new().write(true).open(&dest)
        {
            let _ = f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(loaded.date as u64));
            let _ = crate::device::sync_file(&f);
        }
        index.insert(loaded.key, rbdb::rel_path(&root, &dest));
        self.save_photo_index(&index);
        self.forget_photos();
        self.info.free_bytes = fs4::available_space(&root).unwrap_or(self.info.free_bytes);
        Ok(PhotoOutcome::Added { id: id_of(&rel_key(&root, &dest)).to_string() })
    }

    pub fn remove_photos(&mut self, ids: &[u64]) -> Result<usize> {
        let set: HashSet<u64> = ids.iter().copied().collect();
        let root = self.root().to_path_buf();
        let mut removed = HashSet::new();
        for p in self.rb_photos().iter().filter(|p| set.contains(&p.id)) {
            fs::remove_file(&p.path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })?;
            removed.insert(rbdb::rel_path(&root, &p.path));
        }
        let mut index = self.photo_index();
        index.retain(|_, rel| !removed.contains(rel));
        self.save_photo_index(&index);
        self.forget_photos();
        self.info.free_bytes = fs4::available_space(&root).unwrap_or(self.info.free_bytes);
        Ok(removed.len())
    }

    /// 写真をフォルダへ動かす (覚えている場所も直す)
    fn move_photos(&mut self, ids: &[u64], to: &Path) -> Result<usize> {
        let set: HashSet<u64> = ids.iter().copied().collect();
        let root = self.root().to_path_buf();
        let rel = |p: &Path| rbdb::rel_path(&root, p);
        let mut index = self.photo_index();
        let mut n = 0;
        let targets: Vec<RbPhoto> = self.rb_photos().iter().filter(|p| set.contains(&p.id) && p.path.parent() != Some(to)).cloned().collect();
        for p in targets {
            let moved = move_file(&p.path, to)?;
            let (old, new) = (rel(&p.path), rel(&moved));
            for v in index.values_mut().filter(|v| **v == old) {
                *v = new.clone();
            }
            n += 1;
        }
        self.save_photo_index(&index);
        self.forget_photos();
        Ok(n)
    }

    /// アルバム (フォルダ) を作り、写真を入れる。`parent` があればその中に作る
    pub fn create_photo_album(&mut self, name: &str, parent: Option<u64>, photos: &[u64]) -> Result<String> {
        let name = clean_name(name.trim(), "");
        if name.is_empty() {
            return Err(Error::Other(t!("名前を入れてください")));
        }
        let base = match parent {
            Some(p) => self.album_dir(p)?,
            None => self.pictures_dir(),
        };
        let dir = base.join(&name);
        if dir.exists() {
            return Err(Error::Other(t!("同じ名前のアルバムがあります")));
        }
        fs::create_dir_all(&dir)?;
        self.move_photos(photos, &dir)?;
        self.forget_photos();
        Ok(id_of(&rel_key(self.root(), &dir)).to_string())
    }

    pub fn rename_photo_album(&mut self, id: u64, name: &str) -> Result<()> {
        let from = self.album_dir(id)?;
        let name = clean_name(name.trim(), "");
        if name.is_empty() {
            return Err(Error::Other(t!("名前を入れてください")));
        }
        let to = from.parent().unwrap_or(&from).join(&name);
        if to == from {
            return Ok(());
        }
        if to.exists() {
            return Err(Error::Other(t!("同じ名前のアルバムがあります")));
        }
        fs::rename(&from, &to)?;
        self.rewrite_index(&[(from, to)]);
        self.forget_photos();
        Ok(())
    }

    /// アルバムを消す。中の写真と子のアルバムは 1 つ上の階層に移す (写真は消さない)
    pub fn delete_photo_album(&mut self, id: u64) -> Result<()> {
        let dir = self.album_dir(id)?;
        let up = dir.parent().unwrap_or(&dir).to_path_buf();
        let mut moves = Vec::new();
        for e in fs::read_dir(&dir)?.flatten() {
            let from = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.eq_ignore_ascii_case("thumbs.db") {
                continue;
            }
            let to = if from.is_dir() {
                let mut to = up.join(&name);
                let mut n = 2;
                while to.exists() {
                    to = up.join(format!("{name} ({n})"));
                    n += 1;
                }
                fs::rename(&from, &to)?;
                to
            } else {
                move_file(&from, &up)?
            };
            moves.push((from, to));
        }
        self.rewrite_index(&moves);
        let _ = fs::remove_dir_all(&dir);
        self.forget_photos();
        Ok(())
    }

    pub fn add_to_photo_album(&mut self, id: u64, photos: &[u64]) -> Result<usize> {
        let dir = self.album_dir(id)?;
        self.move_photos(photos, &dir)
    }

    /// アルバムから外す (1 つ上の階層に移す)
    pub fn remove_from_photo_album(&mut self, id: u64, photos: &[u64]) -> Result<usize> {
        let dir = self.album_dir(id)?;
        let inside: Vec<u64> = self.rb_photos().iter().filter(|p| photos.contains(&p.id) && p.path.parent() == Some(&dir)).map(|p| p.id).collect();
        let up = dir.parent().unwrap_or(&dir).to_path_buf();
        self.move_photos(&inside, &up)
    }

    /// 写真を JPEG で (小さい方は一覧の 256px)
    pub fn photo_jpeg(&self, id: u64, large: bool) -> Option<Vec<u8>> {
        let p = self.rb_photos().iter().find(|p| p.id == id)?;
        let bytes = fs::read(&p.path).ok()?;
        if large && p.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg")) {
            return Some(bytes);
        }
        let img = image::load_from_memory(&bytes).ok()?;
        let img = if large { img } else { img.thumbnail(256, 256) };
        photo_file::to_jpeg(&img, 85)
    }

    pub fn photo_file(&self, id: u64) -> Option<PathBuf> {
        self.rb_photos().iter().find(|p| p.id == id).map(|p| p.path.clone())
    }
}
