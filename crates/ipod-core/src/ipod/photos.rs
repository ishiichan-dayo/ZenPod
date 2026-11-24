//! iPod OS の写真: /Photos/Photo Database と /Photos/Thumbs/F<形式>_<n>.ithmb (libgpod の写真の DB と同じ形)。
//!
//! - 写真の DB は ArtworkDB と同じ形 (`artwork::ArtworkDb`)。アルバムは mhsd 2 (mhla)。1 つ目のアルバム
//!   (album_type 1、「Photo Library」) に全部の写真が入る
//! - 画像は機種ごとの形式に縦横比を保って縮め、余白を付けて置く (`artwork::pack_fit`)。原寸の画像は置かない
//! - 写真とアルバムの id は保存のたびに libgpod と同じ決まりで振り直す (写真は 0x40 から、アルバムはその後ろ)
//! - 消した写真の画素は .ithmb に残るので、残りが 1/4 を超えたら保存のときに詰め直す
//! - 読むのは初めて写真を使うとき (iPod を開くときには読まない)

use crate::t;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::artwork::{self, ArtFormat, ArtworkDb, Image, PhotoAlbum, Thumb};
use super::itunesdb::{mac_to_unix, unix_to_mac};
use super::{Ipod, write_atomic};
use crate::domain::photo::{PhotoAlbumView, PhotoLibrary, PhotoOutcome, PhotoView};
use crate::error::{Error, Result};
use crate::media::photo as photo_file;

const PHOTO_DB: &str = "Photo Database";
const PHOTO_DB_BACKUP: &str = "Photo Database.zenpod-backup";
/// 写真の中身の印 → 写真の id (同じ写真を 2 回送らないため)
const PHOTO_INDEX: &str = "zenpod-photos.json";
/// 全部の写真のアルバムの名前 (libgpod と同じ。iPod の画面には出ない)
const MASTER_NAME: &str = "Photo Library";
const FIRST_PHOTO_ID: u32 = 0x40;
const FIRST_ALBUM_ID: u32 = 0x64;

pub(super) struct PhotoState {
    db: ArtworkDb,
    formats: Vec<ArtFormat>,
    /// 中身の印 → 写真の id
    index: HashMap<String, u32>,
    dirty: bool,
    /// 写真を消した (保存のときに .ithmb を詰め直すかを見る)
    removed: bool,
}

pub(super) fn photos_dir(root: &Path) -> PathBuf {
    root.join("Photos")
}

/// 写真の DB を読む。無ければ機種の形式で空の DB を作る。形式が分からなければ None
pub(super) fn load(root: &Path, model_formats: &[ArtFormat]) -> Option<PhotoState> {
    let dir = photos_dir(root);
    let db = match fs::read(dir.join(PHOTO_DB)) {
        Ok(data) => match ArtworkDb::parse(&data) {
            Ok(db) => {
                let backup = dir.join(PHOTO_DB_BACKUP);
                if !backup.exists() {
                    let _ = fs::write(&backup, &data);
                }
                db
            }
            Err(e) => {
                log::warn!("Photo Database: {e}");
                return None;
            }
        },
        Err(_) => ArtworkDb::new_empty(),
    };
    let formats = db.formats(model_formats);
    if formats.is_empty() {
        return None;
    }
    let index: HashMap<String, u32> = fs::read(dir.join(PHOTO_INDEX))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let ids: HashSet<u32> = db.images.iter().map(|i| i.id).collect();
    let index = index.into_iter().filter(|(_, id)| ids.contains(id)).collect();
    Some(PhotoState { db, formats, index, dirty: false, removed: false })
}

impl PhotoState {
    pub(super) fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn master_mut(&mut self) -> &mut PhotoAlbum {
        if !self.db.albums.first().is_some_and(|a| a.is_master()) {
            self.db.albums.insert(0, PhotoAlbum::new(MASTER_NAME, true));
        }
        &mut self.db.albums[0]
    }

    fn next_id(&self) -> u32 {
        let photos = self.db.images.iter().map(|i| i.id);
        let albums = self.db.albums.iter().map(|a| a.id);
        photos.chain(albums).max().map_or(FIRST_PHOTO_ID, |m| m + 1).max(FIRST_PHOTO_ID)
    }

    fn album_mut(&mut self, id: u32) -> Result<&mut PhotoAlbum> {
        self.db
            .albums
            .iter_mut()
            .find(|a| a.id == id && !a.is_master())
            .ok_or_else(|| Error::Other(t!("アルバムが見つかりません")))
    }

    /// 写真とアルバムの id を libgpod と同じ決まりで振り直す (写真は 0x40 から順に、アルバムはその後ろ)
    fn renumber(&mut self) {
        let mut map = HashMap::new();
        for (i, img) in self.db.images.iter_mut().enumerate() {
            let id = FIRST_PHOTO_ID + i as u32;
            map.insert(img.id, id);
            img.id = id;
            img.song_id = id as u64 + 2;
        }
        let mut prev = FIRST_ALBUM_ID;
        let first = FIRST_ALBUM_ID + self.db.images.len() as u32;
        for (i, a) in self.db.albums.iter_mut().enumerate() {
            a.items = a.items.iter().filter_map(|x| map.get(x).copied()).collect();
            a.id = first + i as u32;
            artwork_set_prev(a, prev);
            prev += 1;
            if i > 0 {
                prev += a.items.len() as u32;
            }
        }
        for v in self.index.values_mut() {
            *v = map.get(v).copied().unwrap_or(0);
        }
        self.index.retain(|_, v| *v != 0);
    }
}

fn artwork_set_prev(a: &mut PhotoAlbum, prev: u32) {
    a.hdr[0x3C..0x40].copy_from_slice(&prev.to_le_bytes());
}

fn photo_view(img: &Image) -> PhotoView {
    let big = img.thumbs.iter().max_by_key(|t| t.width as u32 * t.height as u32);
    PhotoView {
        id: img.id.to_string(),
        width: big.map_or(0, |t| (t.width - t.hpad) as u32),
        height: big.map_or(0, |t| (t.height - t.vpad) as u32),
        date: mac_to_unix(img.orig_date()),
        size: img.orig_size as u64,
        name: String::new(),
    }
}

/// .ithmb の中で使っていない画素の割合が多ければ、使っている画像だけを新しいファイルへ詰め直す。
/// 新しいファイルは今の番号の後ろに作り、DB を書いてから古いファイルを消す (途中で止まっても DB と画素が食い違わない)
fn compact(dir: &Path, db: &mut ArtworkDb) -> Result<Vec<PathBuf>> {
    let thumbs_dir = dir.join("Thumbs");
    let used: u64 = db.images.iter().flat_map(|i| &i.thumbs).map(|t| t.size as u64).sum();
    let mut files: Vec<PathBuf> = fs::read_dir(&thumbs_dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ithmb"))).collect())
        .unwrap_or_default();
    let total: u64 = files.iter().filter_map(|p| fs::metadata(p).ok()).map(|m| m.len()).sum();
    if total <= used + used / 3 {
        return Ok(Vec::new());
    }
    // 使っている番号より後ろに書く
    let max_idx = files
        .iter()
        .filter_map(|p| p.file_stem()?.to_str()?.rsplit('_').next()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    use std::io::{Read, Seek, SeekFrom, Write};
    // 形式・元のファイル・位置の順に並べて、元のファイルは 1 つずつ開いて読む
    let mut order: Vec<(usize, usize)> =
        db.images.iter().enumerate().flat_map(|(i, img)| (0..img.thumbs.len()).map(move |j| (i, j))).collect();
    order.sort_by_key(|&(i, j)| {
        let t = &db.images[i].thumbs[j];
        (t.format_id, t.file.clone(), t.offset)
    });
    let mut new_names: HashMap<u32, (u32, u64)> = HashMap::new(); // 形式 → (番号, 今の大きさ)
    let mut src: Option<(PathBuf, fs::File, u64)> = None;
    let mut dst: Option<(String, fs::File)> = None;
    for (i, j) in order {
        let t = &mut db.images[i].thumbs[j];
        // DB の場所が Photos の外を指していたり、位置と大きさがファイルに収まらなければ読まない
        let src_path = artwork::thumb_path(dir, &t.file).ok_or_else(|| Error::Corrupt(t!("写真の画像が足りません")))?;
        if src.as_ref().is_none_or(|(p, _, _)| *p != src_path) {
            let f = fs::File::open(&src_path)?;
            let len = f.metadata()?.len();
            src = Some((src_path, f, len));
        }
        let (_, f, flen) = src.as_mut().unwrap();
        if !artwork::thumb_fits(t, *flen) {
            return Err(Error::Corrupt(t!("写真の画像が足りません")));
        }
        let mut px = vec![0u8; t.size as usize];
        f.seek(SeekFrom::Start(t.offset as u64))?;
        f.read_exact(&mut px).map_err(|_| Error::Corrupt(t!("写真の画像が足りません")))?;
        let (idx, len) = new_names.entry(t.format_id).or_insert((max_idx.saturating_add(1), 0));
        if *len + px.len() as u64 > 256 * 1000 * 1000 {
            *idx = idx.saturating_add(1);
            *len = 0;
        }
        let name = format!("F{}_{}.ithmb", t.format_id, idx);
        if dst.as_ref().is_none_or(|(n, _)| *n != name) {
            if let Some((_, f)) = dst.take() {
                crate::device::sync_file(&f)?;
            }
            dst = Some((name.clone(), fs::OpenOptions::new().create(true).append(true).open(thumbs_dir.join(&name))?));
        }
        dst.as_mut().unwrap().1.write_all(&px)?;
        t.offset = *len as u32;
        t.file = format!(":Thumbs:{name}");
        *len += px.len() as u64;
    }
    if let Some((_, f)) = dst.take() {
        crate::device::sync_file(&f)?;
    }
    // 新しいファイルを書き終えたものに、古いファイルを入れ替える (消すのは DB を書いた後)
    let live: HashSet<String> = db.images.iter().flat_map(|i| &i.thumbs).map(|t| t.file.rsplit(':').next().unwrap_or("").to_string()).collect();
    files.retain(|p| !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| live.contains(n)));
    Ok(files)
}

impl Ipod {
    fn photo_state(&self) -> Option<&PhotoState> {
        self.photos.get_or_init(|| load(self.root(), &self.info.photo_formats)).as_ref()
    }

    fn photo_state_mut(&mut self) -> Result<&mut PhotoState> {
        if self.photos.get().is_none() {
            let st = load(self.root(), &self.info.photo_formats);
            let _ = self.photos.set(st);
        }
        self.photos
            .get_mut()
            .and_then(|s| s.as_mut())
            .ok_or_else(|| Error::Unsupported(t!("この iPod は写真を表示できません")))
    }

    /// 写真とアルバム (「フォトライブラリ」は除く)
    pub fn photo_library(&self) -> PhotoLibrary {
        let Some(st) = self.photo_state() else {
            return PhotoLibrary { supported: false, ..Default::default() };
        };
        PhotoLibrary {
            photos: st.db.images.iter().map(photo_view).collect(),
            albums: st
                .db
                .albums
                .iter()
                .filter(|a| !a.is_master())
                .map(|a| PhotoAlbumView { id: a.id.to_string(), name: a.name.clone(), photo_ids: a.items.iter().map(|i| i.to_string()).collect(), parent: None })
                .collect(),
            supported: true,
        }
    }

    /// 写真を 1 枚送る。`album` があればその名前のアルバムにも入れる (無ければ作る)
    pub fn add_photo(&mut self, src: &Path, album: Option<&str>, ffmpeg: Option<&Path>) -> Result<PhotoOutcome> {
        self.check_writable()?;
        let loaded = photo_file::load(src, ffmpeg)?;
        let dir = photos_dir(self.root());
        let st = self.photo_state_mut()?;
        let existing = st.index.get(&loaded.key).copied().filter(|id| st.db.images.iter().any(|i| i.id == *id));
        let id = match existing {
            Some(id) => id,
            None => {
                // 大きな写真は先に 1440px 程度まで縮める (形式ごとに縮めると遅い)
                let img = if loaded.image.width().max(loaded.image.height()) > 1440 {
                    loaded.image.resize(1440, 1440, image::imageops::FilterType::Triangle)
                } else {
                    loaded.image
                };
                let id = st.next_id();
                let thumbs_dir = dir.join("Thumbs");
                let mut thumbs = Vec::new();
                for fmt in &st.formats {
                    let p = artwork::pack_fit(&img, *fmt);
                    let (file, offset) = artwork::append_ithmb(&thumbs_dir, ":Thumbs:", fmt.id, &p.data)?;
                    thumbs.push(Thumb {
                        format_id: fmt.id,
                        file,
                        offset,
                        size: p.data.len() as u32,
                        vpad: p.vpad,
                        hpad: p.hpad,
                        height: p.height,
                        width: p.width,
                    });
                }
                let mut image = Image {
                    hdr: artwork::new_mhii_header(),
                    id,
                    song_id: id as u64 + 2,
                    orig_size: loaded.size.min(u32::MAX as u64) as u32,
                    thumbs,
                    extra: Vec::new(),
                };
                image.set_dates(unix_to_mac(loaded.date));
                st.db.images.push(image);
                st.master_mut().items.push(id);
                st.index.insert(loaded.key, id);
                st.dirty = true;
                id
            }
        };
        // iPod OS のアルバムは平らなので、階層 ("旅行/2024") は 1 つの名前にする ("旅行 / 2024")
        let album = album.map(|a| a.split('/').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" / "));
        if let Some(name) = album.as_deref().filter(|n| !n.is_empty()) {
            let pos = st.db.albums.iter().position(|a| !a.is_master() && a.name == name);
            let a = match pos {
                Some(i) => &mut st.db.albums[i],
                None => {
                    let mut a = PhotoAlbum::new(name, false);
                    a.id = st.next_id();
                    st.master_mut();
                    st.db.albums.push(a);
                    st.db.albums.last_mut().unwrap()
                }
            };
            if !a.items.contains(&id) {
                a.items.push(id);
                st.dirty = true;
            }
        }
        Ok(if existing.is_some() { PhotoOutcome::Duplicate } else { PhotoOutcome::Added { id: id.to_string() } })
    }

    /// 写真を消す (全部のアルバムから)。消した枚数
    pub fn remove_photos(&mut self, ids: &[u32]) -> Result<usize> {
        self.check_writable()?;
        let st = self.photo_state_mut()?;
        let ids: HashSet<u32> = ids.iter().copied().collect();
        let before = st.db.images.len();
        st.db.images.retain(|i| !ids.contains(&i.id));
        for a in st.db.albums.iter_mut() {
            a.items.retain(|i| !ids.contains(i));
        }
        st.index.retain(|_, v| !ids.contains(v));
        let n = before - st.db.images.len();
        st.dirty |= n > 0;
        st.removed |= n > 0;
        Ok(n)
    }

    /// アルバムを作る (写真を入れて)。新しいアルバムの id
    pub fn create_photo_album(&mut self, name: &str, photos: &[u32]) -> Result<String> {
        self.check_writable()?;
        let st = self.photo_state_mut()?;
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other(t!("名前を入れてください")));
        }
        st.master_mut();
        let have: HashSet<u32> = st.db.images.iter().map(|i| i.id).collect();
        let mut a = PhotoAlbum::new(name, false);
        a.id = st.next_id();
        a.items = photos.iter().copied().filter(|p| have.contains(p)).collect();
        let id = a.id;
        st.db.albums.push(a);
        st.dirty = true;
        Ok(id.to_string())
    }

    pub fn rename_photo_album(&mut self, id: u32, name: &str) -> Result<()> {
        self.check_writable()?;
        let st = self.photo_state_mut()?;
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(Error::Other(t!("名前を入れてください")));
        }
        let a = st.album_mut(id)?;
        if a.name != name {
            a.name = name;
            st.dirty = true;
        }
        Ok(())
    }

    /// アルバムを消す (写真はフォトライブラリに残す)
    pub fn delete_photo_album(&mut self, id: u32) -> Result<()> {
        self.check_writable()?;
        let st = self.photo_state_mut()?;
        st.album_mut(id)?;
        st.db.albums.retain(|a| a.id != id || a.is_master());
        st.dirty = true;
        Ok(())
    }

    pub fn add_to_photo_album(&mut self, id: u32, photos: &[u32]) -> Result<usize> {
        self.check_writable()?;
        let st = self.photo_state_mut()?;
        let have: HashSet<u32> = st.db.images.iter().map(|i| i.id).collect();
        let a = st.album_mut(id)?;
        let before = a.items.len();
        for p in photos {
            if have.contains(p) && !a.items.contains(p) {
                a.items.push(*p);
            }
        }
        let n = a.items.len() - before;
        st.dirty |= n > 0;
        Ok(n)
    }

    pub fn remove_from_photo_album(&mut self, id: u32, photos: &[u32]) -> Result<usize> {
        self.check_writable()?;
        let st = self.photo_state_mut()?;
        let set: HashSet<u32> = photos.iter().copied().collect();
        let a = st.album_mut(id)?;
        let before = a.items.len();
        a.items.retain(|p| !set.contains(p));
        let n = before - a.items.len();
        st.dirty |= n > 0;
        Ok(n)
    }

    /// 写真を JPEG で (大きい方 = 一番大きいサムネイル。小さい方 = 一覧に出す 100px 前後のもの)
    pub fn photo_jpeg(&self, id: u32, large: bool) -> Option<Vec<u8>> {
        let st = self.photo_state()?;
        let img = st.db.images.iter().find(|i| i.id == id)?;
        let area = |t: &&Thumb| t.width.saturating_sub(t.hpad) as u32 * t.height.saturating_sub(t.vpad) as u32;
        let thumb = if large {
            img.thumbs.iter().max_by_key(area)
        } else {
            // 一覧には 100px 以上のうち一番小さいもの (無ければ一番大きいもの)
            img.thumbs.iter().filter(|t| area(t) >= 100 * 70).min_by_key(area).or_else(|| img.thumbs.iter().max_by_key(area))
        }?;
        let rgb = artwork::read_thumb(&photos_dir(self.root()), thumb).ok()?;
        photo_file::to_jpeg(&image::DynamicImage::ImageRgb8(rgb), 90)
    }

    /// 写真の変更を書く (`save` から呼ぶ)
    pub(super) fn save_photos(&mut self) -> Result<()> {
        let root = self.root().to_path_buf();
        let Some(Some(st)) = self.photos.get_mut() else { return Ok(()) };
        if !st.dirty {
            return Ok(());
        }
        let dir = photos_dir(&root);
        fs::create_dir_all(dir.join("Thumbs"))?;
        st.renumber();
        let old_files = if st.removed { compact(&dir, &mut st.db)? } else { Vec::new() };
        // 写真が 1 枚も無くても、フォトライブラリのアルバムは置く (iPod が読めるように)
        st.master_mut();
        let mut next = st.next_id();
        if next < FIRST_ALBUM_ID {
            next = FIRST_ALBUM_ID;
        }
        st.db.files = st.formats.iter().map(|f| (f.id, f.width * f.height * 2)).collect();
        write_atomic(&dir.join(PHOTO_DB), &st.db.serialize_with_next_id(next))?;
        for f in old_files {
            let _ = fs::remove_file(f);
        }
        let _ = fs::write(dir.join(PHOTO_INDEX), serde_json::to_vec(&st.index).unwrap_or_default());
        st.dirty = false;
        st.removed = false;
        Ok(())
    }

    pub(super) fn photos_dirty(&self) -> bool {
        self.photos.get().and_then(|s| s.as_ref()).is_some_and(PhotoState::is_dirty)
    }

    /// 写真の DB を読み直す (別の作業が変えたとき)。今は使っていない
    #[allow(dead_code)]
    pub(super) fn forget_photos(&mut self) {
        self.photos.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renumbers_like_libgpod() {
        let mut db = ArtworkDb::new_empty();
        for id in [500u32, 501, 502] {
            db.images.push(Image { hdr: artwork::new_mhii_header(), id, song_id: 0, orig_size: 0, thumbs: Vec::new(), extra: Vec::new() });
        }
        let mut master = PhotoAlbum::new(MASTER_NAME, true);
        master.items = vec![500, 501, 502];
        let mut a = PhotoAlbum::new("旅行", false);
        a.items = vec![502, 500];
        db.albums = vec![master, a];
        let mut st = PhotoState { db, formats: Vec::new(), index: HashMap::from([("k".into(), 502)]), dirty: true, removed: false };
        st.renumber();
        assert_eq!(st.db.images.iter().map(|i| (i.id, i.song_id)).collect::<Vec<_>>(), vec![(0x40, 0x42), (0x41, 0x43), (0x42, 0x44)]);
        assert_eq!(st.db.albums[0].id, 0x64 + 3);
        assert_eq!(st.db.albums[1].id, 0x64 + 4);
        assert_eq!(st.db.albums[1].items, vec![0x42, 0x40]);
        assert_eq!(st.index["k"], 0x42);
        let prev = |a: &PhotoAlbum| u32::from_le_bytes(a.hdr[0x3C..0x40].try_into().unwrap());
        assert_eq!((prev(&st.db.albums[0]), prev(&st.db.albums[1])), (0x64, 0x65));
    }
}
