//! iPod OS (iTunesDB) と Rockbox のどちらのモードで開いた iPod も、同じ操作で扱う。
//! 画面とコマンドはこの型だけを見る。iPod OS だけの機能 (読み・FireWire GUID・DB の詳細など) は、
//! Rockbox では「使えません」を返すか、空を返す

use crate::t;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::device::DeviceInfo;
use crate::domain::playlist::{PlaylistView, SmartPreview};
use crate::domain::track::{
    AddOptions, AddOutcome, ArtistUnify, DbDetails, ReadingChange, ReadingEdit, Readings, TagEdit, TrackOptions, TrackTagEdit, TrackView,
};
use crate::domain::photo::{PhotoLibrary, PhotoOutcome};
use crate::domain::variants::VariantGroup;
use crate::domain::video::{VideoEdit, VideoOptions, VideoOutcome, VideoView};
use crate::media::video::{Prepared, Progress, VideoInfo, VideoTarget};
use crate::media::preconvert::{ConversionPlan, Preconverted};
use crate::error::{Error, Result};
use crate::ipod::Ipod;
use crate::pc::itunesxml::{ExportSmart, ImportCandidate, TrackMatcher, XmlLibrary, XmlPlaylist, XmlTrack, export_smart_playlists, m3u8_text};
use crate::rockbox::RockboxLib;
use crate::smartpl::{FIELDS_NOT_IN_ROCKBOX, FIELDS_NOT_ON_IPOD, Rules, SmartDef, SmartPrefs};

/// 取り込みの結果
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub playlists: usize,
    /// 普通のプレイリストに入れた曲 (スマートプレイリストは条件で決まるので数えない)
    pub tracks: usize,
    /// 見つからなかった曲
    pub missing: usize,
}

impl ImportSummary {
    fn add(&mut self, tracks: usize, missing: usize) {
        self.playlists += 1;
        self.tracks += tracks;
        self.missing += missing;
    }
}

/// どちらの形式で曲を管理するか
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Apple のファームウェア (iTunesDB)
    #[default]
    Ipod,
    /// Rockbox (ファイルそのもの)
    Rockbox,
}

pub enum Library {
    Ipod(Box<Ipod>),
    Rockbox(Box<RockboxLib>),
}

/// 書き出す写真
pub enum PhotoExport {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// iPod OS の写真の id (32 ビット)。範囲外は捨てる
fn photo_ids(ids: &[u64]) -> Vec<u32> {
    ids.iter().filter_map(|&i| u32::try_from(i).ok()).collect()
}

fn not_in_rockbox() -> Error {
    Error::Unsupported(t!("Rockbox モードでは使えません"))
}

fn not_in_ipod_os() -> Error {
    Error::Unsupported(t!("iPod OS モードでは使えません"))
}

/// 両方のモードが同じ意味で持つ操作。`Library` はこれを通して呼び分ける
/// (どちらか一方にしか無い操作と、モードで手順が違う操作 (曲の追加・削除など) は `Library` が直接呼び分ける)。
/// iPod OS (`Ipod`) と Rockbox (`RockboxLib`) はこの trait を知らない。実装はこのファイルの `backend!` で、
/// 同じ名前のメソッドへ渡すだけ (名前や型が食い違えばコンパイルで分かる)
trait Backend {
    fn info(&self) -> &DeviceInfo;
    fn is_dirty(&self) -> bool;
    fn name(&self) -> &str;
    fn save(&mut self) -> Result<()>;
    fn tracks(&self) -> Vec<TrackView>;
    fn playlists(&self) -> Vec<PlaylistView>;
    fn set_artwork_from_file(&mut self, ids: &[u64], image: &Path) -> Result<usize>;
    fn set_artwork_from_bytes(&mut self, ids: &[u64], bytes: &[u8]) -> Result<usize>;
    fn remove_artwork(&mut self, ids: &[u64]) -> Result<usize>;
    fn shrink_artwork(&mut self, ids: &[u64], max: u32, progress: &dyn Fn(usize, usize)) -> Result<(usize, usize)>;
    fn artwork_jpeg(&self, image_id: u32, prefer_large: bool) -> Option<Vec<u8>>;
    fn edit_tags(&mut self, ids: &[u64], e: &TagEdit) -> Result<usize>;
    fn apply_tag_edits(&mut self, edits: &[TrackTagEdit]) -> Result<usize>;
    fn artist_names(&self) -> Vec<String>;
    fn artist_variants(&self, aliases: &HashMap<String, Vec<String>>) -> Vec<VariantGroup>;
    fn unify_artists(&mut self, edits: &[ArtistUnify]) -> Result<usize>;
    fn track_file(&self, id: u64) -> Option<PathBuf>;
    fn create_playlist(&mut self, name: &str, tracks: &[u64]) -> Result<String>;
    fn rename_playlist(&mut self, id: u64, name: &str) -> Result<()>;
    fn delete_playlist(&mut self, id: u64) -> Result<()>;
    fn add_to_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<usize>;
    fn remove_from_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<()>;
    fn reorder_playlist(&mut self, id: u64, order: &[u64]) -> Result<()>;
    fn rename_ipod(&mut self, name: &str) -> Result<()>;
    fn smart_playlist(&self, id: u64) -> Result<SmartDef>;
    fn save_smart_playlist(&mut self, id: Option<u64>, name: &str, def: SmartDef) -> Result<String>;
    fn preview_smart(&self, def: SmartDef, current: Option<u64>) -> SmartPreview;
    fn smart_parts(&self, id: u64) -> Option<(String, SmartPrefs, Rules)>;
}

/// `Backend` を、その型の同じ名前のメソッドに渡して実装する
macro_rules! backend {
    ($t:ty) => {
        impl Backend for $t {
            fn info(&self) -> &DeviceInfo { &self.info }
            fn is_dirty(&self) -> bool { <$t>::is_dirty(self) }
            fn name(&self) -> &str { <$t>::name(self) }
            fn save(&mut self) -> Result<()> { <$t>::save(self) }
            fn tracks(&self) -> Vec<TrackView> { <$t>::tracks(self) }
            fn playlists(&self) -> Vec<PlaylistView> { <$t>::playlists(self) }
            fn set_artwork_from_file(&mut self, ids: &[u64], image: &Path) -> Result<usize> { <$t>::set_artwork_from_file(self, ids, image) }
            fn set_artwork_from_bytes(&mut self, ids: &[u64], bytes: &[u8]) -> Result<usize> { <$t>::set_artwork_from_bytes(self, ids, bytes) }
            fn remove_artwork(&mut self, ids: &[u64]) -> Result<usize> { <$t>::remove_artwork(self, ids) }
            fn shrink_artwork(&mut self, ids: &[u64], max: u32, progress: &dyn Fn(usize, usize)) -> Result<(usize, usize)> { <$t>::shrink_artwork(self, ids, max, progress) }
            fn artwork_jpeg(&self, image_id: u32, prefer_large: bool) -> Option<Vec<u8>> { <$t>::artwork_jpeg(self, image_id, prefer_large) }
            fn edit_tags(&mut self, ids: &[u64], e: &TagEdit) -> Result<usize> { <$t>::edit_tags(self, ids, e) }
            fn apply_tag_edits(&mut self, edits: &[TrackTagEdit]) -> Result<usize> { <$t>::apply_tag_edits(self, edits) }
            fn artist_names(&self) -> Vec<String> { <$t>::artist_names(self) }
            fn artist_variants(&self, aliases: &HashMap<String, Vec<String>>) -> Vec<VariantGroup> { <$t>::artist_variants(self, aliases) }
            fn unify_artists(&mut self, edits: &[ArtistUnify]) -> Result<usize> { <$t>::unify_artists(self, edits) }
            fn track_file(&self, id: u64) -> Option<PathBuf> { <$t>::track_file(self, id) }
            fn create_playlist(&mut self, name: &str, tracks: &[u64]) -> Result<String> { <$t>::create_playlist(self, name, tracks) }
            fn rename_playlist(&mut self, id: u64, name: &str) -> Result<()> { <$t>::rename_playlist(self, id, name) }
            fn delete_playlist(&mut self, id: u64) -> Result<()> { <$t>::delete_playlist(self, id) }
            fn add_to_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<usize> { <$t>::add_to_playlist(self, id, tracks) }
            fn remove_from_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<()> { <$t>::remove_from_playlist(self, id, tracks) }
            fn reorder_playlist(&mut self, id: u64, order: &[u64]) -> Result<()> { <$t>::reorder_playlist(self, id, order) }
            fn rename_ipod(&mut self, name: &str) -> Result<()> { <$t>::rename_ipod(self, name) }
            fn smart_playlist(&self, id: u64) -> Result<SmartDef> { <$t>::smart_playlist(self, id) }
            fn save_smart_playlist(&mut self, id: Option<u64>, name: &str, def: SmartDef) -> Result<String> { <$t>::save_smart_playlist(self, id, name, def) }
            fn preview_smart(&self, def: SmartDef, current: Option<u64>) -> SmartPreview { <$t>::preview_smart(self, def, current) }
            fn smart_parts(&self, id: u64) -> Option<(String, SmartPrefs, Rules)> { <$t>::smart_parts(self, id) }
        }
    };
}

backend!(Ipod);
backend!(RockboxLib);

impl Library {
    fn backend(&self) -> &dyn Backend {
        match self {
            Library::Ipod(l) => l.as_ref(),
            Library::Rockbox(l) => l.as_ref(),
        }
    }

    fn backend_mut(&mut self) -> &mut dyn Backend {
        match self {
            Library::Ipod(l) => l.as_mut(),
            Library::Rockbox(l) => l.as_mut(),
        }
    }

    /// `cache_dir` は Rockbox のタグの読み取りのキャッシュを置くフォルダ (iPod ごとにファイルを分ける)
    pub fn open(root: &Path, mode: Mode, cache_dir: Option<PathBuf>) -> Result<Self> {
        Self::open_with_progress(root, mode, cache_dir, &|_, _| {}, &|_, _, _, _| {})
    }

    /// `open` と同じ。Rockbox モードでタグを読み直す曲があるときは `progress(読んだ数, 読む数)` を呼び、
    /// 読み終わる前から一覧を出せるよう、読めた曲を `partial` でまとめて知らせる
    pub fn open_with_progress(
        root: &Path,
        mode: Mode,
        cache_dir: Option<PathBuf>,
        progress: &(dyn Fn(usize, usize) + Sync),
        partial: crate::rockbox::Partial,
    ) -> Result<Self> {
        Ok(match mode {
            Mode::Ipod => {
                let mut ipod = Ipod::open(root)?;
                ipod.rockbox_cache = cache_dir.map(|d| crate::rockbox::cache_file(&d, &ipod.info));
                Library::Ipod(Box::new(ipod))
            }
            Mode::Rockbox => Library::Rockbox(Box::new(RockboxLib::open_streaming(root, cache_dir, progress, partial)?)),
        })
    }

    pub fn mode(&self) -> Mode {
        match self {
            Library::Ipod(_) => Mode::Ipod,
            Library::Rockbox(_) => Mode::Rockbox,
        }
    }

    pub fn info(&self) -> &DeviceInfo {
        self.backend().info()
    }
    pub fn root(&self) -> &Path {
        &self.info().root
    }
    pub fn is_dirty(&self) -> bool {
        self.backend().is_dirty()
    }
    pub fn name(&self) -> &str {
        self.backend().name()
    }
    /// Rockbox モードのときだけ: 曲のファイル名を変える ((曲 ID, 新しい名前 (拡張子を除く)))
    pub fn rename_files(&mut self, renames: &[(u64, String)]) -> Result<usize> {
        match self {
            Library::Rockbox(l) => l.rename_files(renames),
            Library::Ipod(_) => Err(Error::Unsupported(t!("iPod OS モードでは曲のファイル名を変えられません"))),
        }
    }
    /// Rockbox モードのときだけ: データベースの様子
    /// Rockbox モードで曲を転送するときの置き場所の形 (iPod OS モードでは None)
    pub fn rockbox_layout(&self) -> Option<String> {
        match self {
            Library::Rockbox(l) => Some(l.layout().to_string()),
            Library::Ipod(_) => None,
        }
    }
    pub fn set_rockbox_layout(&mut self, template: &str) -> Result<()> {
        match self {
            Library::Rockbox(l) => l.set_layout(template),
            Library::Ipod(_) => Err(not_in_ipod_os()),
        }
    }
    pub fn guess_rockbox_layout(&self) -> Option<crate::domain::layout::Guess> {
        match self {
            Library::Rockbox(l) => l.guess_layout(),
            Library::Ipod(_) => None,
        }
    }
    pub fn preview_rockbox_layout(&self, template: &str) -> Result<String> {
        match self {
            Library::Rockbox(l) => l.preview_layout(template),
            Library::Ipod(_) => Err(not_in_ipod_os()),
        }
    }
    pub fn rockbox_status(&self) -> Option<crate::rockbox::Status> {
        match self {
            Library::Rockbox(l) => Some(l.status()),
            Library::Ipod(_) => None,
        }
    }
    /// 開くのにかかった時間の内訳 (Rockbox モードの曲の一覧の読み直しは `scan_stats`)
    pub fn open_times(&self) -> crate::device::OpenTimes {
        match self {
            Library::Rockbox(l) => l.open_times(),
            Library::Ipod(l) => l.open_times,
        }
    }
    /// Rockbox モードで最後に曲の一覧を読み直したときの内訳
    pub fn scan_stats(&self) -> Option<crate::rockbox::ScanStats> {
        match self {
            Library::Rockbox(l) => Some(l.scan_stats()),
            Library::Ipod(_) => None,
        }
    }
    /// Rockbox モードで、データベースから埋めて裏で読み直す曲があれば (iPod の場所, タグのキャッシュ, 曲のファイル)
    pub fn rockbox_reread_job(&self) -> Option<(PathBuf, Option<PathBuf>, Vec<PathBuf>)> {
        match self {
            Library::Rockbox(l) => {
                let files = l.db_filled_files();
                (!files.is_empty()).then(|| (l.info.root.clone(), l.cache_path().map(Path::to_path_buf), files))
            }
            Library::Ipod(_) => None,
        }
    }

    /// 裏で読み直した曲を一覧に反映する。戻り値は替えた曲の数
    pub fn apply_rockbox_reread(&mut self, read: &[crate::pc::folder::PcTrack]) -> usize {
        match self {
            Library::Rockbox(l) => l.apply_reread(read),
            Library::Ipod(_) => 0,
        }
    }
    pub fn save(&mut self) -> Result<()> {
        self.backend_mut().save()
    }

    /// Rockbox のデータベースを作り直したいときは、その iPod の場所とタグのキャッシュの場所を返す (1 回だけ)。
    /// - Rockbox モード: 曲を足した・消した・編集した
    /// - iPod OS モード: 曲のファイルを足した・消した。Rockbox のデータベースがある iPod だけ
    ///   (古いままだと、消した曲が Rockbox に残って再生できない)
    ///
    /// 作り直しは曲が多いと時間がかかるので、呼び出し側が保存の後に裏でまとめて行う
    pub fn take_rockbox_db_request(&mut self) -> Option<(PathBuf, Option<PathBuf>)> {
        match self {
            Library::Rockbox(l) => l.take_db_request().then(|| (l.info.root.clone(), l.cache_path().map(Path::to_path_buf))),
            Library::Ipod(l) => (std::mem::take(&mut l.files_changed)
                && crate::rockbox::rbdb::entry_count(l.root()).is_some()
                && crate::rockbox::rbdb::supported(l.root()))
            .then(|| (l.info.root.clone(), l.rockbox_cache.clone())),
        }
    }

    /// Rockbox のデータベースを作るときのタグのキャッシュの場所
    pub fn rockbox_cache_path(&self) -> Option<PathBuf> {
        match self {
            Library::Rockbox(l) => l.cache_path().map(Path::to_path_buf),
            Library::Ipod(l) => l.rockbox_cache.clone(),
        }
    }

    /// 裏で作り直した Rockbox のデータベースの結果 (Rockbox モードなら、失敗の理由を画面に出す)
    pub fn set_rockbox_db_error(&mut self, e: Option<String>) {
        if let Library::Rockbox(l) = self {
            l.set_db_error(e);
        }
    }
    pub fn tracks(&self) -> Vec<TrackView> {
        self.backend().tracks()
    }
    pub fn playlists(&self) -> Vec<PlaylistView> {
        self.backend().playlists()
    }
    pub fn add_file(&mut self, src: &Path, ffmpeg: Option<&Path>, opts: &AddOptions) -> Result<AddOutcome> {
        self.add_file_preconverted(src, ffmpeg, opts, None)
    }
    /// 転送の前に変換しておけるか (`media::preconvert`)。重複で送らない曲・変換の要らない曲は None
    pub fn conversion_plan(&mut self, src: &Path, opts: &AddOptions) -> Result<Option<ConversionPlan>> {
        match self {
            Library::Ipod(l) => l.conversion_plan(src, opts),
            Library::Rockbox(l) => l.conversion_plan(src, opts),
        }
    }
    /// 1 曲転送する。`pre` に同じ変換をしておいた一時ファイルがあれば、変換せずにそれを使う
    pub fn add_file_preconverted(
        &mut self,
        src: &Path,
        ffmpeg: Option<&Path>,
        opts: &AddOptions,
        pre: Option<&Preconverted>,
    ) -> Result<AddOutcome> {
        match self {
            Library::Ipod(l) => {
                let r = l.add_file_preconverted(src, ffmpeg, opts, pre);
                l.files_changed |= matches!(r, Ok(AddOutcome::Added { .. } | AddOutcome::Replaced { .. }));
                r
            }
            Library::Rockbox(l) => l.add_file_preconverted(src, ffmpeg, opts, pre),
        }
    }
    pub fn remove_tracks(&mut self, ids: &[u64]) -> Result<usize> {
        match self {
            Library::Ipod(l) => {
                let r = l.remove_tracks(ids);
                l.files_changed |= matches!(r, Ok(n) if n > 0);
                r
            }
            Library::Rockbox(l) => l.remove_tracks(ids),
        }
    }
    // ---------------- 動画 ----------------

    pub fn videos(&self) -> Vec<VideoView> {
        match self {
            Library::Ipod(l) => l.videos(),
            Library::Rockbox(l) => l.videos(),
        }
    }
    /// 動画を 1 本送る (iPod OS は DB の保存は呼び出し側で)。変換が要れば ffmpeg で変換する
    pub fn add_video(&mut self, src: &Path, ffmpeg: Option<&Path>, opts: &VideoOptions, progress: &Progress) -> Result<VideoOutcome> {
        match self {
            Library::Ipod(l) => l.add_video(src, ffmpeg, opts, progress),
            Library::Rockbox(l) => l.add_video(src, ffmpeg, opts, progress),
        }
    }
    /// 動画を送れるか (iPod OS で書き込めない・動画を再生できない機種はエラー) と変換するときの形
    pub fn video_target(&self, size: crate::domain::video::VideoSize) -> Result<VideoTarget> {
        match self {
            Library::Ipod(l) => l.video_target(size),
            Library::Rockbox(_) => Ok(VideoTarget::Rockbox),
        }
    }
    /// 同じ動画があればその題名
    pub fn video_duplicate(&self, src: &Path, info: &VideoInfo) -> Option<String> {
        match self {
            Library::Ipod(l) => l.video_duplicate(src, info),
            Library::Rockbox(l) => l.video_duplicate(src, info),
        }
    }
    /// 準備した (変換した) 動画を置く。変換は `media::video::prepare` で、iPod の lock を持たずに済ませておく
    pub fn add_prepared_video(&mut self, src: &Path, p: &Prepared) -> Result<VideoView> {
        match self {
            Library::Ipod(l) => l.add_prepared_video(src, p),
            Library::Rockbox(l) => l.add_prepared_video(src, p),
        }
    }
    pub fn remove_videos(&mut self, ids: &[u64]) -> Result<usize> {
        match self {
            Library::Ipod(l) => {
                let videos: std::collections::HashSet<u64> = l.videos().iter().filter_map(|v| v.id.parse().ok()).collect();
                let ids: Vec<u64> = ids.iter().copied().filter(|i| videos.contains(i)).collect();
                l.remove_tracks(&ids)
            }
            Library::Rockbox(l) => l.remove_videos(ids),
        }
    }
    pub fn edit_videos(&mut self, ids: &[u64], e: &VideoEdit) -> Result<usize> {
        match self {
            Library::Ipod(l) => l.edit_videos(ids, e),
            Library::Rockbox(l) => l.edit_videos(ids, e),
        }
    }
    /// 動画のファイル (PC への書き出し・一覧の画像を作るとき)
    pub fn video_file(&self, id: u64) -> Option<PathBuf> {
        match self {
            Library::Ipod(l) => l.track_file(id),
            Library::Rockbox(l) => l.video_file(id),
        }
    }

    // ---------------- 写真 ----------------

    pub fn photo_library(&self) -> PhotoLibrary {
        match self {
            Library::Ipod(l) => l.photo_library(),
            Library::Rockbox(l) => l.photo_library(),
        }
    }
    /// 写真を 1 枚送る。`album` があればそのアルバムにも入れる (無ければ作る)
    pub fn add_photo(&mut self, src: &Path, album: Option<&str>, ffmpeg: Option<&Path>) -> Result<PhotoOutcome> {
        match self {
            Library::Ipod(l) => l.add_photo(src, album, ffmpeg),
            Library::Rockbox(l) => l.add_photo(src, album, ffmpeg),
        }
    }
    pub fn remove_photos(&mut self, ids: &[u64]) -> Result<usize> {
        match self {
            Library::Ipod(l) => l.remove_photos(&photo_ids(ids)),
            Library::Rockbox(l) => l.remove_photos(ids),
        }
    }
    /// アルバムを作る。`parent` (その中に作る) は Rockbox だけ (iPod OS のアルバムは平ら)
    pub fn create_photo_album(&mut self, name: &str, parent: Option<u64>, photos: &[u64]) -> Result<String> {
        match self {
            Library::Ipod(l) => l.create_photo_album(name, &photo_ids(photos)),
            Library::Rockbox(l) => l.create_photo_album(name, parent, photos),
        }
    }
    pub fn rename_photo_album(&mut self, id: u64, name: &str) -> Result<()> {
        match self {
            Library::Ipod(l) => l.rename_photo_album(id as u32, name),
            Library::Rockbox(l) => l.rename_photo_album(id, name),
        }
    }
    /// アルバムを消す (写真は残す)
    pub fn delete_photo_album(&mut self, id: u64) -> Result<()> {
        match self {
            Library::Ipod(l) => l.delete_photo_album(id as u32),
            Library::Rockbox(l) => l.delete_photo_album(id),
        }
    }
    pub fn add_to_photo_album(&mut self, id: u64, photos: &[u64]) -> Result<usize> {
        match self {
            Library::Ipod(l) => l.add_to_photo_album(id as u32, &photo_ids(photos)),
            Library::Rockbox(l) => l.add_to_photo_album(id, photos),
        }
    }
    pub fn remove_from_photo_album(&mut self, id: u64, photos: &[u64]) -> Result<usize> {
        match self {
            Library::Ipod(l) => l.remove_from_photo_album(id as u32, &photo_ids(photos)),
            Library::Rockbox(l) => l.remove_from_photo_album(id, photos),
        }
    }
    /// 写真を JPEG で (`large` は見るとき・書き出すときの大きい方、そうでなければ一覧の小さい方)
    pub fn photo_jpeg(&self, id: u64, large: bool) -> Option<Vec<u8>> {
        match self {
            Library::Ipod(l) => l.photo_jpeg(id as u32, large),
            Library::Rockbox(l) => l.photo_jpeg(id, large),
        }
    }
    /// 書き出す写真: Rockbox は置いたファイル、iPod OS は (原寸を置かないので) 一番大きいサムネイルの JPEG
    pub fn photo_export(&self, id: u64) -> Option<PhotoExport> {
        match self {
            Library::Ipod(l) => l.photo_jpeg(id as u32, true).map(PhotoExport::Bytes),
            Library::Rockbox(l) => l.photo_file(id).map(PhotoExport::File),
        }
    }

    pub fn set_artwork_from_file(&mut self, ids: &[u64], image: &Path) -> Result<usize> {
        self.backend_mut().set_artwork_from_file(ids, image)
    }
    pub fn set_artwork_from_bytes(&mut self, ids: &[u64], bytes: &[u8]) -> Result<usize> {
        self.backend_mut().set_artwork_from_bytes(ids, bytes)
    }
    pub fn remove_artwork(&mut self, ids: &[u64]) -> Result<usize> {
        self.backend_mut().remove_artwork(ids)
    }
    /// iPod の中の曲の埋め込みのジャケットを縮める。戻り値は (縮めた曲, 縮められなかった曲)
    pub fn shrink_artwork(&mut self, ids: &[u64], max: u32, progress: &dyn Fn(usize, usize)) -> Result<(usize, usize)> {
        self.backend_mut().shrink_artwork(ids, max, progress)
    }
    pub fn artwork_jpeg(&self, image_id: u32, prefer_large: bool) -> Option<Vec<u8>> {
        self.backend().artwork_jpeg(image_id, prefer_large)
    }
    pub fn edit_tags(&mut self, ids: &[u64], e: &TagEdit) -> Result<usize> {
        self.backend_mut().edit_tags(ids, e)
    }
    pub fn apply_tag_edits(&mut self, edits: &[TrackTagEdit]) -> Result<usize> {
        self.backend_mut().apply_tag_edits(edits)
    }
    pub fn artist_names(&self) -> Vec<String> {
        self.backend().artist_names()
    }
    pub fn artist_variants(&self, aliases: &HashMap<String, Vec<String>>) -> Vec<VariantGroup> {
        self.backend().artist_variants(aliases)
    }
    pub fn unify_artists(&mut self, edits: &[ArtistUnify]) -> Result<usize> {
        self.backend_mut().unify_artists(edits)
    }
    pub fn track_file(&self, id: u64) -> Option<PathBuf> {
        self.backend().track_file(id)
    }
    pub fn create_playlist(&mut self, name: &str, tracks: &[u64]) -> Result<String> {
        self.backend_mut().create_playlist(name, tracks)
    }
    pub fn rename_playlist(&mut self, id: u64, name: &str) -> Result<()> {
        self.backend_mut().rename_playlist(id, name)
    }
    pub fn delete_playlist(&mut self, id: u64) -> Result<()> {
        self.backend_mut().delete_playlist(id)
    }
    pub fn add_to_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<usize> {
        self.backend_mut().add_to_playlist(id, tracks)
    }
    pub fn remove_from_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<()> {
        self.backend_mut().remove_from_playlist(id, tracks)
    }
    pub fn reorder_playlist(&mut self, id: u64, order: &[u64]) -> Result<()> {
        self.backend_mut().reorder_playlist(id, order)
    }
    /// プレイリストの並び (iPod OS だけ。Rockbox は本体がファイル名で並べるので、画面の表示の順だけを覚える)
    pub fn reorder_playlists(&mut self, order: &[u64]) -> Result<()> {
        match self {
            Library::Ipod(l) => {
                l.reorder_playlists(order);
                Ok(())
            }
            Library::Rockbox(_) => Err(not_in_rockbox()),
        }
    }
    pub fn rename_ipod(&mut self, name: &str) -> Result<()> {
        self.backend_mut().rename_ipod(name)
    }

    // ---------------- iTunes / ミュージックの XML ----------------

    /// 取り込む前の一覧 (プレイリストごとの、結び付いた曲・見つからない曲・上書きになるか)
    pub fn import_candidates(&self, xml: &XmlLibrary) -> Vec<ImportCandidate> {
        let m = TrackMatcher::new(&self.tracks());
        let existing: Vec<(u64, String)> = self
            .playlists()
            .into_iter()
            .filter(|p| !p.master && !p.podcast)
            .filter_map(|p| Some((p.id.parse().ok()?, p.name)))
            .collect();
        xml.candidates(&m, &existing, true, &self.unavailable_fields())
    }

    /// 開いている iPod で判定できない項目 (取り込むスマートプレイリストの条件から外す)
    fn unavailable_fields(&self) -> Vec<u32> {
        match self {
            Library::Ipod(_) => FIELDS_NOT_ON_IPOD.to_vec(),
            Library::Rockbox(_) => [FIELDS_NOT_ON_IPOD, FIELDS_NOT_IN_ROCKBOX].concat(),
        }
    }

    /// 選んだプレイリスト (`XmlLibrary::playlists` の番号) を取り込む。スマートプレイリストは条件ごと取り込む
    /// (iPod OS は iTunesDB、Rockbox は条件を書いた .m3u8)
    pub fn import_itunes(&mut self, xml: &XmlLibrary, indexes: &[usize]) -> Result<ImportSummary> {
        let m = TrackMatcher::new(&self.tracks());
        let mut sum = ImportSummary::default();
        // iPod で判定できない条件は外して取り込む (外さないと、どの曲も合わないことがある)
        let unavailable = self.unavailable_fields();
        let owned: Vec<XmlPlaylist> = indexes
            .iter()
            .filter_map(|&i| xml.playlists.get(i))
            .map(|p| XmlPlaylist {
                smart: p.smart.as_ref().map(|(prefs, rules)| (prefs.clone(), rules.without_fields(&unavailable))),
                ..p.clone()
            })
            .collect();
        let picked: Vec<&XmlPlaylist> = owned.iter().collect();
        match self {
            Library::Ipod(l) => {
                // 普通のプレイリストを先に作る (スマートプレイリストの「プレイリストが…」の条件がそれを指すことがある)
                for pl in picked.iter().filter(|p| p.smart.is_none()) {
                    let (ids, missing) = xml.resolve(pl, &m);
                    l.import_playlist(pl.pid, &pl.name, &ids, None)?;
                    sum.add(ids.len(), missing.len());
                }
                // スマートプレイリスト同士で指し合うことがあるので 2 回
                for round in 0..2 {
                    for pl in picked.iter() {
                        let Some((prefs, rules)) = &pl.smart else { continue };
                        l.import_playlist(pl.pid, &pl.name, &[], Some((prefs, rules)))?;
                        if round == 0 {
                            sum.playlists += 1;
                        }
                    }
                }
            }
            Library::Rockbox(l) => {
                for pl in picked.iter().filter(|p| p.smart.is_none()) {
                    let (ids, missing) = xml.resolve(pl, &m);
                    match l.playlists().into_iter().find(|p| !p.master && !p.smart && p.name == pl.name.trim()) {
                        // 同じ名前があれば中身を入れ替える
                        Some(p) => {
                            let id: u64 = p.id.parse().unwrap_or(0);
                            let old: Vec<u64> = p.track_ids.iter().filter_map(|t| t.parse().ok()).collect();
                            l.remove_from_playlist(id, &old)?;
                            l.add_to_playlist(id, &ids)?;
                        }
                        None => {
                            l.create_playlist(&pl.name, &ids)?;
                        }
                    }
                    sum.add(ids.len(), missing.len());
                }
                // スマートプレイリスト: 同じ名前のスマートプレイリストは上書き、普通のプレイリストなら置き換える。
                // 同士で指し合うことがあるので 2 回
                for round in 0..2 {
                    for pl in picked.iter() {
                        let Some((prefs, rules)) = &pl.smart else { continue };
                        let same = l.playlists().into_iter().find(|p| !p.master && p.name == pl.name.trim());
                        let id = match same {
                            Some(p) if p.smart => p.id.parse().ok(),
                            Some(p) => {
                                l.delete_playlist(p.id.parse().unwrap_or(0))?;
                                None
                            }
                            None => None,
                        };
                        l.save_smart_playlist(id, &pl.name, SmartDef::from_parts(prefs.clone(), rules.clone()))?;
                        if round == 0 {
                            sum.playlists += 1;
                        }
                    }
                }
            }
        }
        Ok(sum)
    }

    /// プレイリストを .m3u8 にする (名前と中身、`ids` の順)。曲の場所は iPod のルートからの "/..."
    /// (Rockbox の Playlists と同じ形。iPod OS は "/iPod_Control/Music/F00/ABCD.mp3")
    pub fn export_m3u8(&self, ids: &[u64]) -> Result<Vec<(String, String)>> {
        let pls: HashMap<String, PlaylistView> = self.playlists().into_iter().map(|p| (p.id.clone(), p)).collect();
        let tracks: HashMap<String, TrackView> = self.tracks().into_iter().map(|t| (t.id.clone(), t)).collect();
        let root = self.root();
        let out: Vec<(String, String)> = ids
            .iter()
            .filter_map(|id| pls.get(&id.to_string()))
            .map(|pl| {
                let list: Vec<XmlTrack> = pl
                    .track_ids
                    .iter()
                    .filter_map(|tid| {
                        let t = tracks.get(tid)?;
                        let file = self.track_file(tid.parse().ok()?)?;
                        let rel = file.strip_prefix(root).ok()?;
                        let location = rel.components().map(|c| format!("/{}", c.as_os_str().to_string_lossy())).collect();
                        Some(XmlTrack {
                            name: t.title.clone(),
                            artist: t.artist.clone(),
                            album: t.album.clone(),
                            total_time_ms: t.duration_ms,
                            track_number: t.track_nr,
                            disc_number: t.disc_nr,
                            location: Some(location),
                        })
                    })
                    .collect();
                (pl.name.clone(), m3u8_text(&pl.name, &list))
            })
            .collect();
        if out.is_empty() {
            return Err(Error::Other(t!("書き出せるプレイリストがありません")));
        }
        Ok(out)
    }

    /// スマートプレイリストを iTunes / ミュージックで読み込める XML にする
    pub fn export_smart_playlists(&self, ids: &[u64]) -> Result<Vec<u8>> {
        let parts: Vec<(String, SmartPrefs, Rules)> = ids.iter().filter_map(|&id| self.backend().smart_parts(id)).collect();
        if parts.is_empty() {
            return Err(Error::Other(t!("書き出せるスマートプレイリストがありません")));
        }
        let list: Vec<ExportSmart> = parts
            .iter()
            .map(|(name, prefs, rules)| ExportSmart { name, prefs, rules })
            .collect();
        Ok(export_smart_playlists(&list))
    }

    // ---------------- スマートプレイリスト ----------------

    pub fn smart_playlist(&self, id: u64) -> Result<SmartDef> {
        self.backend().smart_playlist(id)
    }
    pub fn save_smart_playlist(&mut self, id: Option<u64>, name: &str, def: SmartDef) -> Result<String> {
        self.backend_mut().save_smart_playlist(id, name, def)
    }
    pub fn preview_smart(&self, def: SmartDef, current: Option<u64>) -> Result<SmartPreview> {
        Ok(self.backend().preview_smart(def, current))
    }

    // ---------------- iPod OS だけ ----------------

    /// 書き込めるか (iPod OS は署名方式と FireWire GUID の確認。Rockbox はファイルを書くだけなので常に可)
    pub fn check_writable(&self) -> Result<()> {
        match self {
            Library::Ipod(l) => l.check_writable(),
            Library::Rockbox(_) => Ok(()),
        }
    }
    pub fn artwork_supported(&self) -> bool {
        match self {
            Library::Ipod(l) => l.artwork_supported(),
            Library::Rockbox(_) => true,
        }
    }
    pub fn set_firewire_id(&mut self, hex: &str) -> Result<()> {
        match self {
            Library::Ipod(l) => l.set_firewire_id(hex),
            Library::Rockbox(_) => Err(not_in_rockbox()),
        }
    }
    /// 以前の版が書いたアートワークの共有を分ける (iPod OS だけ)
    pub fn unshare_artwork(&mut self) -> usize {
        match self {
            Library::Ipod(l) => l.unshare_artwork(),
            Library::Rockbox(_) => 0,
        }
    }
    /// 欠けている・大きさの違うジャケットのサムネイルを作り直す (iPod OS だけ)
    pub fn repair_artwork(&mut self) -> usize {
        match self {
            Library::Ipod(l) => l.repair_artwork(),
            Library::Rockbox(_) => 0,
        }
    }
    pub fn generate_readings(&mut self, ids: Option<&[u64]>, overwrite: bool) -> Result<usize> {
        match self {
            Library::Ipod(l) => l.generate_readings(ids, overwrite),
            Library::Rockbox(_) => Err(not_in_rockbox()),
        }
    }
    pub fn preview_readings(&self, ids: Option<&[u64]>, overwrite: bool) -> Vec<ReadingChange> {
        match self {
            Library::Ipod(l) => l.preview_readings(ids, overwrite),
            Library::Rockbox(_) => Vec::new(),
        }
    }
    pub fn apply_reading_edits(&mut self, edits: &[ReadingEdit]) -> Result<usize> {
        match self {
            Library::Ipod(l) => l.apply_reading_edits(edits),
            Library::Rockbox(_) => Err(not_in_rockbox()),
        }
    }
    pub fn set_readings(&mut self, id: u64, r: &Readings) -> Result<()> {
        match self {
            Library::Ipod(l) => l.set_readings(id, r),
            Library::Rockbox(_) => Err(not_in_rockbox()),
        }
    }
    pub fn track_options(&self, id: u64) -> Option<TrackOptions> {
        match self {
            Library::Ipod(l) => l.track_options(id),
            Library::Rockbox(_) => None,
        }
    }
    pub fn track_db_details(&self, id: u64) -> Option<DbDetails> {
        match self {
            Library::Ipod(l) => l.track_db_details(id),
            Library::Rockbox(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// iPod OS モードで曲を足す・消すと、Rockbox のデータベースがある iPod ならそれも作り直す
    #[test]
    fn ipod_mode_updates_rockbox_db() {
        let Some(ff) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).find(|f| f.is_file())) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (root, src) = (dir.path().join("ipod"), dir.path().join("a.mp3"));
        fs::create_dir_all(root.join("iPod_Control/iTunes")).unwrap();
        fs::create_dir_all(root.join(".rockbox")).unwrap();
        fs::write(root.join(".rockbox/rockbox-info.txt"), "Target: ipod6g\nVersion: v4.0\n").unwrap();
        let ok = std::process::Command::new(&ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-metadata", "title=曲 A", "-metadata", "artist=X"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let titles = |root: &Path| -> Vec<String> {
            crate::rockbox::rbdb::read(&root.join(".rockbox")).map(|d| d.entries.iter().map(|e| e.strings[&3].clone()).collect()).unwrap_or_default()
        };

        // Rockbox のデータベースがまだ無い iPod では作らない (Rockbox を使っていないかもしれないので)
        let mut lib = Library::open(&root, Mode::Ipod, None).unwrap();
        let opts = AddOptions { auto_reading: false, ..Default::default() };
        let AddOutcome::Added { track, .. } = lib.add_file(&src, None, &opts).unwrap() else { panic!() };
        lib.save().unwrap();
        assert!(crate::rockbox::rbdb::read(&root.join(".rockbox")).is_none());

        assert!(lib.take_rockbox_db_request().is_none());
        // データベースがあれば、足した曲が入り、消せば消える (作り直しは呼び出し側が裏で行う)
        crate::rockbox::rbdb::rebuild(&root, None).unwrap();
        assert_eq!(titles(&root), ["曲 A"]);
        let save = |lib: &mut Library| {
            lib.save().unwrap();
            if let Some((r, c)) = lib.take_rockbox_db_request() {
                crate::rockbox::rbdb::rebuild(&r, c.as_deref()).unwrap();
            }
        };
        let AddOutcome::Added { track: t2, .. } = lib.add_file(&src, None, &AddOptions { skip_duplicates: false, ..opts }).unwrap() else {
            panic!()
        };
        save(&mut lib);
        assert_eq!(titles(&root), ["曲 A", "曲 A"]);
        lib.remove_tracks(&[track.id.parse().unwrap(), t2.id.parse().unwrap()]).unwrap();
        save(&mut lib);
        assert!(titles(&root).is_empty());
    }
}
