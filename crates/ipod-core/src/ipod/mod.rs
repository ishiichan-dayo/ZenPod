//! iPod OS モード (Apple のファームウェア): iTunesDB を読み書きして、曲の追加・削除・プレイリスト編集・保存を行う。

pub mod artwork;
pub mod cdb;
pub mod hash58;
pub mod hashab;
pub mod hash72;
pub mod itunesdb;
mod photos;
pub mod shuffle;
pub mod sqlitedb;
mod videos;

use crate::t;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use self::artwork::{ArtFormat, ArtworkDb, HashIndex};
use self::itunesdb::*;
use crate::device::{self, ChecksumKind, DeviceInfo, adopt_legacy_name};
use crate::domain::audio::{ConvertTarget, Format, MediaKind};
use crate::domain::playlist::{PlaylistView, SmartPreview};
use crate::domain::track::{
    AddOptions, AddOutcome, ArtistUnify, DbDetails, DupKey, ReadingChange, ReadingEdit, Readings, TagEdit, TrackOptions, TrackTagEdit, TrackView,
    dup_key, same_number,
};
use crate::domain::{devpath, variants, yomi};
use crate::error::{Error, Result};
use crate::media::{self, convert, preconvert};
use crate::smartpl::{FIELDS_NOT_ON_IPOD, Rules, SmartDef, SmartPrefs, now_mac_local};

const BACKUP_NAME: &str = "iTunesDB.zenpod-backup";
const CDB_BACKUP_NAME: &str = "iTunesCDB.zenpod-backup";
const ART_BACKUP_NAME: &str = "ArtworkDB.zenpod-backup";
/// 画像内容ハッシュ → mhii id (同じジャケットを共有するための索引)
const ART_INDEX_NAME: &str = "zenpod-art.json";
/// アートワークを利用者が選んだ動画の dbid。アプリの一覧には動画の 1 コマではなくそのアートワークを出す
/// (転送のときに作ったアートワークは、正方形に切り抜いた 1 コマのことがあるので一覧には使わない)
const VIDEO_ART_NAME: &str = "zenpod-video-art.json";
/// (元フィールド, ソート用フィールド)
const READING_FIELDS: [(u32, u32); 5] = [
    (MHOD_TITLE, MHOD_SORT_TITLE),
    (MHOD_ARTIST, MHOD_SORT_ARTIST),
    (MHOD_ALBUM, MHOD_SORT_ALBUM),
    (MHOD_ALBUM_ARTIST, 29),
    (MHOD_COMPOSER, MHOD_SORT_COMPOSER),
];
/// 転送時に残しておく空き容量
const FREE_MARGIN: u64 = 20 * 1024 * 1024;

pub struct Ipod {
    pub info: DeviceInfo,
    db: ITunesDb,
    /// 保存成功後に削除するファイル (Play Counts / OTG)
    pending_cleanup: Vec<PathBuf>,
    dirty: bool,
    /// None = アートワーク非対応機種 / ArtworkDB が読めない
    art: Option<ArtState>,
    /// 重複チェック用の索引。初回の転送時に作り、曲の追加・削除で更新する
    dup_index: Option<HashMap<DupKey, Vec<DupEntry>>>,
    /// 曲のファイルを足した・消した (Rockbox も入った iPod なら、保存のときに Rockbox のデータベースも作り直す)
    pub files_changed: bool,
    /// Rockbox のデータベースを作るときのタグの読み取りのキャッシュ
    pub rockbox_cache: Option<PathBuf>,
    /// 開くのにかかった時間の内訳
    pub open_times: device::OpenTimes,
    /// 写真の DB (初めて写真を使うときに読む。None = 写真に対応していない)
    photos: std::cell::OnceCell<Option<photos::PhotoState>>,
    /// nano 5G の hash72 の材料 (HashInfo か、iTunes が署名した iTunesCDB から)
    hash_info: Option<hash72::HashInfo>,
}

/// iPod に置くファイル (変換した場合は変換後のもの)
struct Converted<'a> {
    path: &'a Path,
    fmt: Format,
    /// 長さ・ビットレートなど (タグは元のファイルのものを使う)
    props: &'a media::MediaInfo,
}

/// 重複チェックの索引の 1 曲分: (長さ ms, (ディスク番号, トラック番号), 曲名, dbid)
type DupEntry = (u32, (u32, u32), String, u64);

struct ArtState {
    db: ArtworkDb,
    formats: Vec<ArtFormat>,
    index: HashIndex,
    /// アートワークを利用者が選んだ動画 (VIDEO_ART_NAME)
    custom_video: HashSet<u64>,
    dirty: bool,
}

fn reading_field_name(sort: u32) -> &'static str {
    match sort {
        MHOD_SORT_TITLE => "title",
        MHOD_SORT_ARTIST => "artist",
        MHOD_SORT_ALBUM => "album",
        29 => "albumArtist",
        _ => "composer",
    }
}

fn reading_field_id(name: &str) -> Option<u32> {
    Some(match name {
        "title" => MHOD_SORT_TITLE,
        "artist" => MHOD_SORT_ARTIST,
        "album" => MHOD_SORT_ALBUM,
        "albumArtist" => 29,
        "composer" => MHOD_SORT_COMPOSER,
        _ => return None,
    })
}

/// 条件の画面で曲名を見せる曲の数
const SMART_SAMPLE: usize = 5;

impl Ipod {
    pub fn open(root: &Path) -> Result<Self> {
        let itunes = itunes_dir(root);
        let db_path = itunes.join("iTunesDB");
        let mut pending_cleanup = Vec::new();
        let mut dirty = false;
        let mut times = device::OpenTimes::default();

        let db_started = std::time::Instant::now();
        // nano 5G 以降は縮めた iTunesCDB が本体で、iTunesDB は 0 バイト
        let cdb_raw = fs::read(itunes.join("iTunesCDB")).ok().filter(|d| !d.is_empty());
        let has_db = fs::metadata(&db_path).is_ok_and(|m| m.is_file() && m.len() > 0);
        let mut db = if let Some(raw) = &cdb_raw {
            let data = if cdb::is_compressed(raw) { cdb::decompress(raw)? } else { raw.clone() };
            let db = ITunesDb::parse(&data)?;
            let backup = itunes.join(CDB_BACKUP_NAME);
            if !backup.exists() {
                let _ = fs::write(&backup, raw);
            }
            db
        } else if has_db {
            let data = fs::read(&db_path)?;
            let db = ITunesDb::parse(&data)?;
            let backup = itunes.join(BACKUP_NAME);
            adopt_legacy_name(&backup, "iTunesDB.ipodsync-backup");
            if !backup.exists() {
                // 初回のみ元 DB を退避 (失敗しても続行)
                let _ = fs::write(&backup, &data);
            }
            db
        } else {
            let name = root
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "iPod".into());
            dirty = true;
            ITunesDb::new_empty(&name)
        };
        times.db_ms = db_started.elapsed().as_millis() as u64;

        // OTG プレイリストは Play Counts 取り込み前の並び (DB 順) で解決する
        let mut otg_index = 0;
        for i in 0..100 {
            let name = if i == 0 {
                "OTGPlaylistInfo".to_string()
            } else {
                format!("OTGPlaylistInfo_{i}")
            };
            let p = itunes.join(&name);
            let Ok(data) = fs::read(&p) else {
                if i > 0 {
                    break;
                }
                continue;
            };
            let items = parse_otg(&db, &data);
            if !items.is_empty() {
                otg_index += 1;
                let mut pl = Playlist::new(&format!("On-The-Go {otg_index}"), random_id(), false);
                pl.items = items;
                db.playlists.push(pl);
            }
            pending_cleanup.push(p);
            dirty = true;
        }
        let pc = itunes.join("Play Counts");
        if let Ok(data) = fs::read(&pc) {
            merge_play_counts(&mut db, &data);
            pending_cleanup.push(pc);
            dirty = true;
        }
        // shuffle の再生回数 (本体が前回の iTunesSD の順に書く)
        if let (Ok(stats), Ok(sd)) = (fs::read(itunes.join("iTunesStats")), fs::read(itunes.join("iTunesSD"))) {
            let n = shuffle::merge_stats(&mut db, &sd, &stats);
            if n > 0 {
                log::info!("iTunesStats: merged {n} tracks");
                dirty = true;
            }
        }
        // nano 5G 以降の再生回数 (曲は persistentID = dbid で指す)
        if let Ok(data) = fs::read(itunes.join("PlayCounts.plist")) {
            let n = merge_play_counts_plist(&mut db, &data);
            if n > 0 {
                log::info!("PlayCounts.plist: merged {n} tracks");
                dirty = true;
            }
        }
        for extra in ["iTunesStats", "PlayCounts.plist"] {
            let p = itunes.join(extra);
            if p.exists() {
                pending_cleanup.push(p);
            }
        }

        let scheme = (cdb_raw.is_some() || has_db).then(|| db.hashing_scheme());
        let info = device::probe_timed(root, scheme, &mut times);
        let hash_info = if info.checksum == ChecksumKind::Hash72 { load_hash_info(root, &info, cdb_raw.as_deref()) } else { None };
        if info.shuffle.is_some() {
            // シャッフルの順 (iTunesSD の曲の番号) は曲の並びが変わると合わなくなる (libgpod も保存で消す)
            let p = itunes.join("iTunesShuffle");
            if p.exists() {
                pending_cleanup.push(p);
            }
            // iTunesSD が無い・壊れている shuffle は、開いたら書き直す
            if fs::read(itunes.join("iTunesSD")).ok().and_then(|d| shuffle::sd_format_of(&d)) != info.shuffle {
                dirty = true;
            }
        }
        let art_started = std::time::Instant::now();
        let art = load_art(root, &info.art_formats);
        times.art_ms = art_started.elapsed().as_millis() as u64;
        Ok(Ipod {
            info,
            db,
            pending_cleanup,
            dirty,
            art,
            dup_index: None,
            files_changed: false,
            rockbox_cache: None,
            open_times: times,
            photos: std::cell::OnceCell::new(),
            hash_info,
        })
    }

    pub fn root(&self) -> &Path {
        &self.info.root
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty || self.art.as_ref().is_some_and(|a| a.dirty) || self.photos_dirty()
    }
    pub fn artwork_supported(&self) -> bool {
        self.art.is_some()
    }
    pub fn name(&self) -> &str {
        &self.db.master().name
    }
    pub fn db(&self) -> &ITunesDb {
        &self.db
    }

    /// 書き込み可能か (署名方式・FireWire GUID の確認)
    pub fn check_writable(&self) -> Result<()> {
        match self.info.checksum {
            ChecksumKind::Unsupported => Err(Error::Unsupported(
                t!("この iPod のデータベースの署名の方式に対応していないため書き込めません"),
            )),
            ChecksumKind::Hash58 | ChecksumKind::Hash72 | ChecksumKind::HashAb if self.info.firewire_id_bytes().is_none() => {
                Err(Error::Unsupported(
                    t!("FireWire GUID を取得できません。設定から手動で入力してください"),
                ))
            }
            ChecksumKind::Hash72 if self.hash_info.is_none() => Err(Error::Unsupported(t!(
                "この iPod nano (5G) に書き込むには、一度 iTunes (Mac は Finder) で同期してから開き直してください (署名に使う情報を iPod から取り出します)"
            ))),
            _ => Ok(()),
        }
    }

    /// FireWire GUID を手動設定 (USB 経由で取得できない環境向け)
    pub fn set_firewire_id(&mut self, hex: &str) -> Result<()> {
        hash58::parse_firewire_id(hex)
            .ok_or_else(|| Error::Other(t!("FireWire GUID は 16 桁の 16 進数です")))?;
        self.info.firewire_id = Some(hex.trim().trim_start_matches("0x").to_ascii_uppercase());
        self.info.firewire_id_source = Some(crate::device::FirewireSource::Manual);
        Ok(())
    }

    /// DB を iPod に書き込む (tmp → rename で原子的に置換)
    pub fn save(&mut self) -> Result<()> {
        self.check_writable()?;
        let itunes = itunes_dir(self.root());
        fs::create_dir_all(&itunes)?;
        // ArtworkDB を先に書く (iTunesDB の mhii_link が指す先を先に確定させる)
        let art_dir = artwork_dir(self.root());
        if let Some(art) = self.art.as_mut().filter(|a| a.dirty) {
            // mhlf の 1 枚の大きさは書き込みに使う形式に合わせる (repair_artwork で形を変えたときも)
            for f in &art.formats {
                let size = artwork::frame_size(f);
                match art.db.files.iter_mut().find(|(id, _)| *id == f.id) {
                    Some(e) => e.1 = size,
                    None => art.db.files.push((f.id, size)),
                }
            }
            fs::create_dir_all(&art_dir)?;
            write_atomic(&art_dir.join("ArtworkDB"), &art.db.serialize())?;
            let json = serde_json::to_vec(&art.index).unwrap_or_default();
            let _ = fs::write(art_dir.join(ART_INDEX_NAME), json);
            // 消した動画・アートワークを外した動画は忘れる
            let alive: HashSet<u64> = self.db.tracks.iter().filter(|t| t.is_video() && t.has_artwork()).map(|t| t.dbid()).collect();
            art.custom_video.retain(|id| alive.contains(id));
            let path = art_dir.join(VIDEO_ART_NAME);
            if art.custom_video.is_empty() {
                let _ = fs::remove_file(path);
            } else {
                let mut ids: Vec<u64> = art.custom_video.iter().copied().collect();
                ids.sort_unstable();
                let _ = fs::write(path, serde_json::to_vec(&ids).unwrap_or_default());
            }
            art.dirty = false;
        }
        self.save_photos()?;
        // 曲の追加・削除・編集をスマートプレイリストに反映する
        self.db.refresh_smart_playlists(now_mac_local());
        let mut bytes = self.db.serialize();
        if self.info.sqlite_db {
            // libgpod と同じ: 縮めた DB の機種は 0x0C を 2 に、0x70 に署名の種類 (hash72 は 2、hashAB は 4)
            bytes[0x0C..0x10].copy_from_slice(&2u32.to_le_bytes());
            let kind: u16 = match self.info.checksum {
                ChecksumKind::Hash72 => 2,
                ChecksumKind::HashAb => 4,
                _ => 0,
            };
            bytes[0x70..0x72].copy_from_slice(&kind.to_le_bytes());
            // 0x32〜0x45 (不明な 20 バイト) は 0 にする。hashAB の SHA1 にここを含めるか (libgpod は含め、iOpenPod は
            // Apple の署名から 0 にして計算すると確かめた) が食い違うが、0 なら同じになる。libgpod も新しく書く DB では 0
            bytes[0x32..0x46].fill(0);
            // 署名は縮めたあとのバイト列にかける
            bytes = cdb::compress(&bytes)?;
        }
        let fwid = self.info.firewire_id_bytes();
        match self.info.checksum {
            ChecksumKind::Hash58 => hash58::write_hash58(&mut bytes, &fwid.expect("checked")),
            ChecksumKind::Hash72 => {
                // iTunes と同じく hashing_scheme は 1 で、hash72 をかけてから (hash72 を含めて) hash58 をかける
                hash72::write_hash72(&mut bytes, self.hash_info.as_ref().expect("checked"), 1);
                hash58::write_hash58(&mut bytes, &fwid.expect("checked"));
            }
            ChecksumKind::HashAb => hashab::write_hashab(&mut bytes, &fwid.expect("checked")),
            ChecksumKind::None | ChecksumKind::Unsupported => {}
        }
        if self.info.sqlite_db {
            // 本体が曲の一覧に使うのは SQLite のデータベース (iTunes Library.itlp)。iTunesCDB より先に書く
            self.write_sqlite(&itunes)?;
            write_atomic(&itunes.join("iTunesCDB"), &bytes)?;
            // iTunesCDB と中身のある iTunesDB が両方あると本体が迷う (libgpod と同じく 0 バイトにする)
            write_atomic(&itunes.join("iTunesDB"), &[])?;
        } else {
            write_atomic(&itunes.join("iTunesDB"), &bytes)?;
        }
        // shuffle は iTunesDB ではなく iTunesSD で再生する (serialize で振ったアルバムの番号を使うので後に書く)
        if let Some(format) = self.info.shuffle {
            write_atomic(&itunes.join("iTunesSD"), &shuffle::write_itunes_sd(&self.db, format))?;
        }
        for p in self.pending_cleanup.drain(..) {
            let _ = fs::remove_file(p);
        }
        self.dirty = false;
        self.info.free_bytes = fs4::available_space(self.root()).unwrap_or(self.info.free_bytes);
        Ok(())
    }

    /// nano 5G 以降の SQLite のデータベースと、その Locations.itdb の署名 (cbk) を書く。
    /// 本体が求める後処理の SQL は SysInfoExtended にあれば流す (nano 5G は必須、6G / 7G には無いことがある)
    fn write_sqlite(&self, itunes: &Path) -> Result<()> {
        let fwid = self.info.firewire_id_bytes().expect("checked");
        let sysinfo_ext = fs::read_to_string(self.root().join("iPod_Control").join("Device").join("SysInfoExtended")).unwrap_or_default();
        let post_process = sqlitedb::post_process_commands(&sysinfo_ext);
        let uuid = self.info.firewire_id.clone().unwrap_or_default();
        let checksum = self.info.checksum;
        let hash_info = self.hash_info.clone();
        let sign = move |sha1: &[u8; 20]| -> Result<Vec<u8>> {
            match (checksum, &hash_info) {
                (ChecksumKind::Hash72, Some(h)) => Ok(h.sign(sha1).to_vec()),
                (ChecksumKind::HashAb, _) => Ok(hashab::calc_hashab(sha1, &fwid, hashab::HASHAB_RND).to_vec()),
                // tr: 訳さない (check_writable で止まるので起きない)
                _ => Err(Error::Other("no signature for Locations.itdb.cbk".into())),
            }
        };
        let opts = sqlitedb::ItlpOptions { uuid: &uuid, post_process: post_process.as_ref() };
        sqlitedb::write_itlp(itunes, &self.db, &opts, &sign)
    }

    // ---------------- views ----------------

    /// 曲 (動画は「ビデオ」に出すので除く。ビデオポッドキャストは曲に入れる)
    pub fn tracks(&self) -> Vec<TrackView> {
        let idx = ArtIndex::new(self.art.as_ref());
        self.db.tracks.iter().filter(|t| t.video_kind().is_none()).map(|t| self.view(t, &idx)).collect()
    }

    /// 「ビデオ」に出す動画の dbid
    fn video_ids(&self) -> HashSet<u64> {
        self.db.tracks.iter().filter(|t| t.video_kind().is_some()).map(|t| t.dbid()).collect()
    }

    fn view(&self, t: &Track, idx: &ArtIndex) -> TrackView {
        let mut v = track_view(t);
        v.artwork_id = idx.image_id_of(t).unwrap_or(0);
        v
    }

    /// プレイリスト。動画は曲の一覧に出さないので、中身からも除く
    pub fn playlists(&self) -> Vec<PlaylistView> {
        let videos = self.video_ids();
        self.db
            .playlists
            .iter()
            .map(|p| PlaylistView {
                id: p.id().to_string(),
                name: p.name.clone(),
                track_ids: p.items.iter().filter(|d| !videos.contains(d)).map(|d| d.to_string()).collect(),
                master: p.is_master(),
                smart: p.is_smart(),
                podcast: p.is_podcast(),
            })
            .collect()
    }

    // ---------------- tracks ----------------

    /// 1 曲転送する (DB 保存は呼び出し側で save())
    pub fn add_file(&mut self, src: &Path, ffmpeg: Option<&Path>, opts: &AddOptions) -> Result<AddOutcome> {
        self.add_file_preconverted(src, ffmpeg, opts, None)
    }

    /// 同じ曲 (iPod にあれば (dbid, 曲名))。重複を調べない設定なら None
    fn duplicate_for(&mut self, info: &media::MediaInfo, opts: &AddOptions) -> Option<(u64, String)> {
        if opts.skip_duplicates || opts.replace_duplicates { self.find_duplicate(info) } else { None }
    }

    /// 変換先 (「変換して転送」なら、選んだ形式にする曲も)。変換しないなら None
    fn conversion_target(&self, fmt: Format, info: &media::MediaInfo, opts: &AddOptions) -> Option<ConvertTarget> {
        let target = match opts.convert_all_to {
            Some(t) if t.should_convert(fmt, info.bitrate_kbps) => Some(t),
            _ => fmt.needs_conversion().then_some(if fmt == Format::ConvertLossless { opts.lossless_to } else { opts.lossy_to }),
        };
        shuffle_target(self.info.shuffle, fmt, target, opts)
    }

    /// 転送の前に変換しておけるか (`media::preconvert`)。重複で送らない曲・変換の要らない曲は None
    pub fn conversion_plan(&mut self, src: &Path, opts: &AddOptions) -> Result<Option<preconvert::ConversionPlan>> {
        let fmt = media::classify(src)?;
        let info = media::read_info(src)?;
        if self.duplicate_for(&info, opts).is_some() && !opts.replace_duplicates {
            return Ok(None);
        }
        Ok(self
            .conversion_target(fmt, &info, opts)
            .map(|target| preconvert::ConversionPlan { target, sample_rate: info.sample_rate, afconvert: true }))
    }

    /// 1 曲転送する。`pre` に同じ変換をしておいた一時ファイルがあれば、変換せずにそれを使う
    pub fn add_file_preconverted(
        &mut self,
        src: &Path,
        ffmpeg: Option<&Path>,
        opts: &AddOptions,
        pre: Option<&preconvert::Preconverted>,
    ) -> Result<AddOutcome> {
        self.check_writable()?;
        let fmt = media::classify(src)?;
        let info = media::read_info(src)?;

        let dup = self.duplicate_for(&info, opts);
        if let Some((dbid, title)) = &dup
            && !opts.replace_duplicates
        {
            return Ok(AddOutcome::Duplicate { title: title.clone(), id: dbid.to_string() });
        }
        let replace = dup.map(|(dbid, _)| dbid);

        // 変換が必要なら一時ファイルへ (「変換して転送」なら、選んだ形式にする曲も)
        let target = self.conversion_target(fmt, &info, opts);
        let plan = target.map(|target| preconvert::ConversionPlan { target, sample_rate: info.sample_rate, afconvert: true });
        let pre = pre.filter(|p| plan.as_ref().is_some_and(|plan| p.matches(plan)));
        let mut tmp_file = None;
        // 変換しておいた一時ファイル (持ち主は呼び出し側なので消さない。ジャケットはその場で縮めてよい)
        let (source, final_fmt, props) = if let (Some(pre), Some(target)) = (pre, target) {
            (pre.path().to_path_buf(), target.format(), media::read_info(pre.path())?)
        } else if let Some(target) = target {
            let out = preconvert::temp_path(target.ext())?;
            // 変換や読み取りに失敗しても一時ファイルを残さない
            let props = convert_for_transfer(src, &out, target, info.sample_rate, true, ffmpeg).and_then(|_| media::read_info(&out));
            let props = match props {
                Ok(p) => p,
                Err(e) => {
                    let _ = fs::remove_file(&out);
                    return Err(e);
                }
            };
            tmp_file = Some(out.clone());
            (out, target.format(), props)
        } else if matches!(fmt, Format::Aac | Format::Alac) && convert::moov_after_mdat(src) {
            // 目次が末尾にある m4a は、音声はそのままに目次を先頭へ並べ替えてから転送する
            // (末尾のままだと、大きい曲が続くと iPod が曲の切り替えで飛ばすことがある)
            let dir = std::env::temp_dir().join("zenpod");
            fs::create_dir_all(&dir)?;
            let out = dir.join(format!("{:016x}.m4a", random_id()));
            match faststart(ffmpeg, src, &out) {
                Ok(()) => {
                    tmp_file = Some(out.clone());
                    (out, fmt, info.clone())
                }
                Err(e) => {
                    // 並べ替えに失敗しても転送は続ける (元のファイルのまま)
                    log::warn!("faststart: {e}");
                    let _ = fs::remove_file(&out);
                    (src.to_path_buf(), fmt, info.clone())
                }
            }
        } else {
            (src.to_path_buf(), fmt, info.clone())
        };
        // 埋め込みのジャケットを縮める (iPod OS は使わないので容量のため)。一時ファイルはその場で、元のファイルは写してから
        let source = match opts.shrink_artwork {
            Some(max) if tmp_file.is_some() || pre.is_some() => {
                if let Err(e) = media::shrink_embedded_cover(&source, max) {
                    log::warn!("shrink artwork: {e}");
                }
                source
            }
            Some(max) => match media::shrunk_copy(&source, max) {
                Some(t) => {
                    tmp_file = Some(t.clone());
                    t
                }
                None => source,
            },
            None => source,
        };

        let file = Converted { path: &source, fmt: final_fmt, props: &props };
        let result = self.copy_and_register(file, &info, src, opts, replace);
        if let Some(t) = tmp_file {
            let _ = fs::remove_file(t);
        }
        let track = Box::new(result?);
        // shuffle で再生できない ALAC・AIFF を変換したときも数える (「変換して転送」で選んだ形式にしたものは数えない)
        let converted = fmt.needs_conversion() || (opts.convert_all_to.is_none() && target.is_some());
        Ok(if replace.is_some() {
            AddOutcome::Replaced { track, converted }
        } else {
            AddOutcome::Added { track, converted }
        })
    }

    fn copy_and_register(
        &mut self,
        file: Converted,
        info: &media::MediaInfo,
        original: &Path,
        opts: &AddOptions,
        replace: Option<u64>,
    ) -> Result<TrackView> {
        let Converted { path: source, fmt, props } = file;
        let size = fs::metadata(source)?.len();
        if size > u32::MAX as u64 {
            return Err(Error::Unsupported(
                t!("4GB を超えるファイルは転送できません"),
            ));
        }
        let free = fs4::available_space(self.root()).unwrap_or(u64::MAX);
        if size + FREE_MARGIN > free {
            return Err(Error::NoSpace);
        }

        let ext = match fmt {
            Format::Mp3 => "mp3",
            Format::Wav => "wav",
            Format::Aiff => "aif",
            _ => "m4a",
        };
        let (dest, ipod_path) = self.pick_destination(ext)?;
        if let Err(e) = copy_file(source, &dest) {
            let _ = fs::remove_file(&dest);
            return Err(e.into());
        }

        // 置き換えるときは同じ dbid を使う (プレイリストは dbid で曲を指すのでそのまま残る)
        let mut t = Track::new(replace.unwrap_or_else(|| self.db.new_dbid()));
        t.set_string(MHOD_TITLE, info.title.as_deref());
        t.set_string(MHOD_ARTIST, info.artist.as_deref());
        t.set_string(MHOD_ALBUM, info.album.as_deref());
        t.set_string(MHOD_ALBUM_ARTIST, info.album_artist.as_deref());
        t.set_string(MHOD_GENRE, info.genre.as_deref());
        t.set_string(MHOD_COMPOSER, info.composer.as_deref());
        t.set_string(MHOD_COMMENT, info.comment.as_deref());
        t.set_string(MHOD_SORT_TITLE, info.sort_title.as_deref());
        t.set_string(MHOD_SORT_ARTIST, info.sort_artist.as_deref());
        t.set_string(MHOD_SORT_ALBUM, info.sort_album.as_deref());
        t.set_string(29, info.sort_album_artist.as_deref());
        t.set_string(MHOD_SORT_COMPOSER, info.sort_composer.as_deref());
        t.set_string(MHOD_PATH, Some(&ipod_path));
        let (desc, type2, unk126, unk144) = match fmt {
            Format::Mp3 => ("MPEG audio file", 1, 0xFFFF, 0x000C),
            Format::Aac => ("AAC audio file", 0, 0xFFFF, 0x0033),
            Format::Alac => ("Apple Lossless audio file", 0, 0, 0),
            Format::Wav => ("WAV audio file", 0, 0, 0),
            _ => ("AIFF audio file", 0, 0, 0),
        };
        t.set_string(MHOD_FILETYPE, Some(desc));
        t.set_types(0, type2);
        t.set_unk126(unk126);
        t.set_unk144(unk144);
        t.set_filetype_marker(filetype_marker(ext));
        t.set_compilation(info.compilation);
        t.set_size(size as u32);
        t.set_length_ms(props.duration_ms);
        t.set_bitrate(props.bitrate_kbps);
        t.set_samplerate(props.sample_rate);
        t.set_samplecount(props.duration_ms as u64 * props.sample_rate as u64 / 1000);
        // 歌詞は iPod がファイルから読むが、この印が無いと出さない (変換したときは変換後のファイルで見る)
        t.set_lyrics_flag(props.has_lyrics);
        t.set_track_nr(info.track_nr);
        t.set_track_total(info.track_total);
        t.set_cd_nr(info.disc_nr);
        t.set_cd_total(info.disc_total);
        t.set_year(info.year);
        let now = now_unix();
        t.set_time_added_mac(unix_to_mac(now));
        let mtime = fs::metadata(original)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(now);
        t.set_time_modified_mac(unix_to_mac(mtime));

        t.set_kind(media::detect_kind(original, info));
        // ポッドキャストは公開日を入れる (iTunes と同じ。iPod の回の情報に出る)
        if t.kind() == MediaKind::Podcast && info.date > 0 {
            t.set_time_released_mac(unix_to_mac(info.date));
        }
        if opts.auto_reading {
            fill_readings(&mut t, false);
        }
        if opts.artwork && self.art.is_some() {
            // ジャケットの失敗で曲の転送自体は止めない
            if let Some(bytes) = crate::media::cover::find_cover(original) {
                match self.ensure_image(&bytes, t.dbid()) {
                    Ok(id) => t.set_artwork(id, bytes.len() as u32),
                    Err(e) => log::warn!("artwork: {e}"),
                }
            }
        }

        if let Some(i) = replace.and_then(|id| self.db.tracks.iter().position(|x| x.dbid() == id)) {
            // 再生回数・評価・追加日などは元の曲から引き継ぐ
            let old = &self.db.tracks[i];
            t.set_playcount(old.playcount());
            t.set_rating(old.rating());
            t.set_time_added_mac(old.time_added_mac());
            t.set_last_played_mac(old.last_played_mac());
            t.set_skipcount(old.skipcount());
            t.set_last_skipped_mac(old.last_skipped_mac());
            t.set_bookmark_ms(old.bookmark_ms());
            // 種類は手で変えたかもしれないので元の曲に合わせる
            t.set_kind(old.kind());
            let old_file = old.ipod_path().and_then(|p| ipod_to_fs(self.root(), p));
            let view = self.view(&t, &ArtIndex::new(self.art.as_ref()));
            self.db.tracks[i] = t;
            // 新しいファイルのコピーが終わってから古いファイルを消す
            if let Some(f) = old_file.filter(|f| *f != dest) {
                let _ = fs::remove_file(f);
            }
            self.dup_index = None;
            self.gc_images();
            self.dirty = true;
            return Ok(view);
        }
        let view = self.view(&t, &ArtIndex::new(self.art.as_ref()));
        if let Some(idx) = &mut self.dup_index {
            idx.entry(dup_key(t.title(), t.artist(), t.album()))
                .or_default()
                .push((t.length_ms(), (t.cd_nr(), t.track_nr()), t.title().unwrap_or("").to_string(), t.dbid()));
        }
        self.db.tracks.push(t);
        self.dirty = true;
        Ok(view)
    }

    /// 同じ曲 (曲名・アーティスト・アルバム・ディスク/トラック番号が同じで長さの差が 2 秒未満) の (dbid, 曲名)
    fn find_duplicate(&mut self, info: &media::MediaInfo) -> Option<(u64, String)> {
        let tracks = &self.db.tracks;
        let idx = self.dup_index.get_or_insert_with(|| {
            let mut m: HashMap<DupKey, Vec<DupEntry>> = HashMap::new();
            for t in tracks {
                m.entry(dup_key(t.title(), t.artist(), t.album()))
                    .or_default()
                    .push((t.length_ms(), (t.cd_nr(), t.track_nr()), t.title().unwrap_or("").to_string(), t.dbid()));
            }
            m
        });
        let found = idx.get(&dup_key(
            info.title.as_deref(),
            info.artist.as_deref(),
            info.album.as_deref(),
        ))?;
        found
            .iter()
            .find(|(len, nr, _, _)| {
                (*len as i64 - info.duration_ms as i64).abs() < 2000
                    && same_number(*nr, (info.disc_nr, info.track_nr))
            })
            .map(|(_, _, title, dbid)| (*dbid, title.clone()))
    }

    /// iPod_Control/Music/Fxx/XXXX.ext の空き名を決める
    fn pick_destination(&self, ext: &str) -> Result<(PathBuf, String)> {
        let music = self.root().join("iPod_Control").join("Music");
        let mut dirs: Vec<String> = fs::read_dir(&music)
            .map(|rd| {
                // 種類はフォルダを読んだときの情報で見る (曲ごとに Fxx を 1 つずつ stat すると、1 曲で 50 回問い合わせる)
                rd.flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| {
                        n.len() == 3
                            && n[..1].eq_ignore_ascii_case("f")
                            && n[1..].bytes().all(|b| b.is_ascii_digit())
                    })
                    .collect()
            })
            .unwrap_or_default();
        if dirs.is_empty() {
            // 機種ごとの数 (libgpod の表)。型番が分からなければ、署名のある機種は 50、それ以外は 20
            let n = self.info.music_dirs.unwrap_or(if self.info.checksum == ChecksumKind::Hash58 {
                50
            } else {
                20
            });
            for i in 0..n {
                let d = format!("F{i:02}");
                fs::create_dir_all(music.join(&d))?;
                dirs.push(d);
            }
        }
        dirs.sort();
        const CH: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        for _ in 0..1000 {
            let dir = &dirs[rand::random_range(0..dirs.len())];
            let name: String = (0..4)
                .map(|_| CH[rand::random_range(0..CH.len())] as char)
                .collect();
            let file = format!("{name}.{ext}");
            let dest = music.join(dir).join(&file);
            if !dest.exists() {
                return Ok((dest, format!(":iPod_Control:Music:{dir}:{file}")));
            }
        }
        Err(Error::Other(t!("転送先ファイル名を決められません")))
    }

    /// 曲を削除 (ファイルも消す)
    pub fn remove_tracks(&mut self, ids: &[u64]) -> Result<usize> {
        self.check_writable()?;
        let mut removed = 0;
        let root = self.root().to_path_buf();
        let ids: HashSet<u64> = ids.iter().copied().collect();
        self.db.tracks.retain(|t| {
            if !ids.contains(&t.dbid()) {
                return true;
            }
            if let Some(f) = t.ipod_path().and_then(|p| ipod_to_fs(&root, p)) {
                let _ = fs::remove_file(f);
            }
            removed += 1;
            false
        });
        for pl in &mut self.db.playlists {
            pl.items.retain(|d| !ids.contains(d));
        }
        if removed > 0 {
            self.dup_index = None;
        }
        self.dirty |= removed > 0;
        self.gc_images();
        Ok(removed)
    }

    // ---------------- artwork ----------------

    /// 曲 1 つ分の画像 (mhii) を ArtworkDB に登録して id を返す。
    ///
    /// mhii は曲ごとに作る。iPod video 5G などの古い機種は mhii の song_id (曲の dbid) でしか
    /// 画像を探さないので、1 つの mhii を複数の曲で共有すると最初の 1 曲にしか表示されない
    /// (共有して mhii_link で指す方式は iPod classic / nano 3G 以降だけ)。
    /// 同じ内容の画像が既にあれば、ithmb の画素データはそれを指すだけにして容量は増やさない
    fn ensure_image(&mut self, bytes: &[u8], song_id: u64) -> Result<u32> {
        let dir = artwork_dir(self.root());
        let art = self.art.as_mut().ok_or_else(|| {
            Error::Unsupported(t!("この iPod はアートワークに対応していません"))
        })?;
        let key = crate::media::cover::content_key(bytes);
        let id = art.db.next_id();
        let same = art
            .index
            .get(&key)
            .filter(|e| e.valid_in(&art.db))
            .and_then(|e| art.db.image(e.id))
            .cloned();
        let img = match same {
            Some(mut img) => {
                img.id = id;
                img.song_id = song_id;
                img
            }
            None => {
                let img = artwork::build_image(&dir, bytes, &art.formats, id, song_id)?;
                art.index.insert(
                    key,
                    artwork::IndexEntry {
                        id,
                        size: bytes.len() as u32,
                        song: song_id,
                    },
                );
                img
            }
        };
        art.db.images.push(img);
        art.dirty = true;
        Ok(id)
    }

    /// 1 つの mhii を複数の曲が指している状態 (以前の版が書いたもの) を、曲ごとの mhii に分ける。
    /// 画素データは同じ場所を指すので容量は増えない。分けた曲の数を返す (0 なら何もしていない)
    pub fn unshare_artwork(&mut self) -> usize {
        let Some(art) = self.art.as_mut() else { return 0 };
        let by_id: HashMap<u32, usize> = art.db.images.iter().enumerate().map(|(i, img)| (img.id, i)).collect();
        let mut next = art.db.next_id();
        let mut added = Vec::new();
        for t in self.db.tracks.iter_mut().filter(|t| t.has_artwork() && t.mhii_link() != 0) {
            let Some(&i) = by_id.get(&t.mhii_link()) else { continue };
            let img = &art.db.images[i];
            if img.song_id == t.dbid() {
                continue;
            }
            let mut own = img.clone();
            own.id = next;
            own.song_id = t.dbid();
            next += 1;
            t.set_artwork(own.id, own.orig_size);
            added.push(own);
        }
        let n = added.len();
        if n > 0 {
            art.db.images.extend(added);
            art.dirty = true;
            self.dirty = true;
        }
        n
    }

    /// どの画像にも、書き込みに使う形式 (art.formats) のサムネイルがその大きさでそろっているか確かめ、
    /// 欠けている・大きさが違うものを同じ画像のほかの大きさから作り直す。作り直した画像の数を返す (0 なら何もしていない)。
    /// 以前の版は iPod classic の 1061 を 56×56 で書くか (本体に出ない)、iTunes が書いた 6160 バイトの 1061 を外して
    /// 書いていなかった (Issue #12)。「上書きする」で送り直しても同じ画像のサムネイルを使い回すので、開いたときに直す
    pub fn repair_artwork(&mut self) -> usize {
        let dir = artwork_dir(self.root());
        let Some(art) = self.art.as_mut() else { return 0 };
        let mut changed = HashSet::new();
        for fmt in art.formats.clone() {
            let size = artwork::frame_size(&fmt);
            let ok = |t: &artwork::Thumb| t.format_id == fmt.id && t.size == size;
            let has = |t: &artwork::Thumb| t.format_id == fmt.id;
            // 大きさの違うものがあれば、この形式は全部を新しいファイルに書き直す (同じファイルに大きさの違う画像を混ぜない)
            let rewrite_all = art.db.images.iter().flat_map(|i| &i.thumbs).any(|t| has(t) && !ok(t));
            if !rewrite_all && art.db.images.iter().all(|i| i.thumbs.iter().any(ok)) {
                continue;
            }
            let old_files: HashSet<String> = art.db.images.iter().flat_map(|i| &i.thumbs).filter(|t| has(t)).map(|t| t.file.clone()).collect();
            let mut writer = artwork::IthmbWriter::new(&dir, ":", fmt.id);
            // 同じ画素データ (ほかの曲と同じジャケット) は 1 回だけ作る
            let mut made: HashMap<(String, u32), (String, u32)> = HashMap::new();
            for img in art.db.images.iter_mut() {
                if !rewrite_all && img.thumbs.iter().any(ok) {
                    continue;
                }
                // 元にするのは、この形式以外で、この大きさ以上の一番小さいもの (無ければ一番大きいもの)
                let area = |t: &artwork::Thumb| t.width as u32 * t.height as u32;
                let mut src: Vec<&artwork::Thumb> = img.thumbs.iter().filter(|t| !has(t)).collect();
                src.sort_by_key(|t| {
                    let big = t.width as u32 >= fmt.width && t.height as u32 >= fmt.height;
                    (!big, if big { area(t) } else { u32::MAX - area(t) })
                });
                let Some(from) = src.first().map(|t| (t.file.clone(), t.offset)) else { continue };
                let placed = match made.get(&from) {
                    Some(p) => Ok(p.clone()),
                    None => src
                        .iter()
                        .find_map(|t| artwork::read_thumb(&dir, t).ok())
                        .ok_or_else(|| Error::Corrupt(t!("mhli が不正です")))
                        .and_then(|rgb| {
                            let px = artwork::to_rgb565(&image::DynamicImage::ImageRgb8(rgb), fmt);
                            if rewrite_all { writer.push(&px) } else { artwork::append_ithmb(&dir, ":", fmt.id, &px) }
                        }),
                };
                let (file, offset) = match placed {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("artwork {} ({}): {e}", img.id, fmt.id);
                        continue;
                    }
                };
                made.insert(from, (file.clone(), offset));
                let thumb = artwork::Thumb {
                    format_id: fmt.id,
                    file,
                    offset,
                    size,
                    vpad: 0,
                    hpad: 0,
                    height: fmt.height as u16,
                    width: fmt.width as u16,
                };
                match img.thumbs.iter_mut().find(|t| has(t)) {
                    Some(t) => *t = thumb,
                    None => img.thumbs.push(thumb),
                }
                changed.insert(img.id);
            }
            if let Err(e) = writer.close() {
                log::warn!("artwork {}: {e}", fmt.id);
            }
            // 書き直した形式の古いファイルは、ArtworkDB を保存してから消す (どの画像からも指されていなければ)
            if rewrite_all {
                let still: HashSet<&String> = art.db.images.iter().flat_map(|i| &i.thumbs).map(|t| &t.file).collect();
                for f in old_files.iter().filter(|f| !still.contains(f)) {
                    if let Some(p) = artwork::thumb_path(&dir, f) {
                        self.pending_cleanup.push(p);
                    }
                }
            }
        }
        if !changed.is_empty() {
            art.dirty = true;
            self.dirty = true;
            log::info!("artwork: {} 枚のサムネイルを作り直した", changed.len());
        }
        changed.len()
    }

    /// どのトラックからも参照されなくなった画像を ArtworkDB から外す
    /// (ithmb 上の領域はそのまま残る)
    fn gc_images(&mut self) {
        let Some(art) = self.art.as_mut() else { return };
        let mut links = std::collections::HashSet::new();
        let mut legacy = std::collections::HashSet::new();
        for t in self.db.tracks.iter().filter(|t| t.has_artwork()) {
            // mhii_link のある曲はそれだけを見る (song_id で残すと、差し替え前の画像が残り、
            // song_id で画像を探す機種で古い画像が表示されてしまう)
            if t.mhii_link() != 0 {
                links.insert(t.mhii_link());
            } else {
                legacy.insert(t.dbid());
            }
        }
        let before = art.db.images.len();
        art.db.images.retain(|i| {
            links.contains(&i.id)
                || (legacy.contains(&i.song_id) && !links.contains(&i.id) && i.song_id != 0)
        });
        if art.db.images.len() != before {
            let alive: std::collections::HashSet<u32> =
                art.db.images.iter().map(|i| i.id).collect();
            art.index.retain(|_, e| alive.contains(&e.id));
            art.dirty = true;
        }
    }

    /// 画像ファイルを選択した曲のアートワークにする
    pub fn set_artwork_from_file(&mut self, ids: &[u64], image: &Path) -> Result<usize> {
        let bytes = fs::read(image)?;
        self.set_artwork_from_bytes(ids, &bytes)
    }

    /// 画像データ (JPEG / PNG 等) を選択した曲のアートワークにする
    pub fn set_artwork_from_bytes(&mut self, ids: &[u64], bytes: &[u8]) -> Result<usize> {
        self.check_writable()?;
        if ids.is_empty() {
            return Err(Error::Other(t!("曲が選択されていません")));
        }
        // 曲ごとに mhii を作る (ensure_image の説明を参照)
        // 全曲を選んだときに 1 曲ごとに全曲を探さないよう、集合と位置の表を使う
        let want: HashSet<u64> = ids.iter().copied().collect();
        let targets: Vec<(usize, u64)> =
            self.db.tracks.iter().enumerate().map(|(i, t)| (i, t.dbid())).filter(|(_, d)| want.contains(d)).collect();
        let mut n = 0;
        for (i, dbid) in targets {
            let id = self.ensure_image(bytes, dbid)?;
            if let Some(t) = self.db.tracks.get_mut(i).filter(|t| t.dbid() == dbid) {
                t.set_artwork(id, bytes.len() as u32);
                if t.is_video()
                    && let Some(art) = self.art.as_mut()
                {
                    art.custom_video.insert(dbid);
                }
                n += 1;
            }
        }
        self.dirty = true;
        self.gc_images();
        Ok(n)
    }

    /// iPod の中の曲のファイルの埋め込みのジャケットを縮める (`media::shrink_embedded_cover`)。iPod OS は埋め込みの画像を使わないので、
    /// 容量を減らすだけ (iPod が出すジャケットは ArtworkDB のまま)。戻り値は (縮めた曲, 縮められなかった曲)。
    /// `progress(済んだ数, 全体)` は 1 曲ごとに呼ぶ
    pub fn shrink_artwork(&mut self, ids: &[u64], max: u32, progress: &dyn Fn(usize, usize)) -> Result<(usize, usize)> {
        self.check_writable()?;
        let want: HashSet<u64> = ids.iter().copied().collect();
        let root = self.root().to_path_buf();
        let targets: Vec<(usize, PathBuf)> = self
            .db
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| want.contains(&t.dbid()))
            .filter_map(|(i, t)| Some((i, t.ipod_path().and_then(|p| ipod_to_fs(&root, p))?)))
            .collect();
        let (mut done, mut failed) = (0, 0);
        for (n, (i, path)) in targets.iter().enumerate() {
            progress(n, targets.len());
            match media::shrink_embedded_cover(path, max) {
                Ok(true) => {
                    // iTunesDB の大きさを今のファイルに合わせる (iPod は大きさで再生の終わりを見る)
                    if let Ok(m) = fs::metadata(path) {
                        self.db.tracks[*i].set_size(m.len() as u32);
                    }
                    done += 1;
                }
                Ok(false) => {}
                Err(e) => {
                    log::warn!("shrink artwork: {}: {e}", path.display());
                    failed += 1;
                }
            }
        }
        self.dirty |= done > 0;
        Ok((done, failed))
    }

    pub fn remove_artwork(&mut self, ids: &[u64]) -> Result<usize> {
        self.check_writable()?;
        let ids: HashSet<u64> = ids.iter().copied().collect();
        let mut n = 0;
        for t in self
            .db
            .tracks
            .iter_mut()
            .filter(|t| ids.contains(&t.dbid()))
        {
            t.clear_artwork();
            n += 1;
        }
        if let Some(art) = self.art.as_mut()
            && art.custom_video.iter().any(|id| ids.contains(id))
        {
            art.custom_video.retain(|id| !ids.contains(id));
            art.dirty = true;
        }
        self.dirty |= n > 0;
        self.gc_images();
        Ok(n)
    }

    /// アートワークの PNG (一覧表示用に一番小さいサイズ)
    /// 画面表示用の JPEG (PNG より小さく、作るのも速い)
    pub fn artwork_jpeg(&self, image_id: u32, prefer_large: bool) -> Option<Vec<u8>> {
        let art = self.art.as_ref()?;
        let img = art.db.image(image_id)?;
        let area = |t: &&artwork::Thumb| t.width as u32 * t.height as u32;
        let thumb = if prefer_large {
            img.thumbs.iter().max_by_key(area)
        } else {
            img.thumbs.iter().min_by_key(area)
        }?;
        let rgb = artwork::read_thumb(&artwork_dir(self.root()), thumb).ok()?;
        let mut out = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&rgb)
            .ok()?;
        Some(out.into_inner())
    }

    pub fn artwork_png(&self, image_id: u32, prefer_large: bool) -> Option<Vec<u8>> {
        let art = self.art.as_ref()?;
        let img = art.db.image(image_id)?;
        let area = |t: &&artwork::Thumb| t.width as u32 * t.height as u32;
        let thumb = if prefer_large {
            img.thumbs.iter().max_by_key(area)
        } else {
            img.thumbs.iter().min_by_key(area)
        }?;
        let rgb = artwork::read_thumb(&artwork_dir(self.root()), thumb).ok()?;
        let mut out = std::io::Cursor::new(Vec::new());
        rgb.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some(out.into_inner())
    }

    // ---------------- tags ----------------

    /// 曲情報を書き換える (複数曲に同じ値も可)。表記を変えた項目は読みを付け直す。
    /// iPod 内の情報だけを変え、PC の元ファイルは変更しない。戻り値は変更した曲数
    pub fn edit_tags(&mut self, ids: &[u64], e: &TagEdit) -> Result<usize> {
        let edits: Vec<TrackTagEdit> = ids
            .iter()
            .map(|&id| TrackTagEdit {
                id: id.to_string(),
                edit: e.clone(),
            })
            .collect();
        self.apply_tag_edits(&edits)
    }

    /// 曲ごとに違う内容で書き換える (ネットで取得した曲情報の反映など)。保存は 1 回で済む
    pub fn apply_tag_edits(&mut self, edits: &[TrackTagEdit]) -> Result<usize> {
        self.check_writable()?;
        let by_id: HashMap<u64, &TagEdit> = edits
            .iter()
            .filter_map(|e| Some((e.id.parse::<u64>().ok()?, &e.edit)))
            .collect();
        let mut n = 0;
        let root = self.root().to_path_buf();
        // 歌詞を書けなかった曲があっても、ほかの曲の変更は残して最後に知らせる
        let mut failed = None;
        for t in self.db.tracks.iter_mut() {
            if let Some(e) = by_id.get(&t.dbid()) {
                let mut changed = apply_tag_edit(t, e);
                if let Some(lyrics) = &e.lyrics {
                    match write_lyrics(&root, t, lyrics) {
                        Ok(()) => changed = true,
                        Err(e) => _ = failed.get_or_insert(e),
                    }
                }
                n += changed as usize;
            }
        }
        if n > 0 {
            self.dup_index = None;
            self.dirty = true;
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(n),
        }
    }

    // ---------------- artist variants ----------------

    /// アーティスト名 (アルバムアーティスト含む) の一覧。曲数の多い順
    pub fn artist_names(&self) -> Vec<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for t in &self.db.tracks {
            for n in [t.artist(), t.album_artist()].into_iter().flatten() {
                let n = n.trim();
                if !n.is_empty() {
                    *counts.entry(n).or_default() += 1;
                }
            }
        }
        let mut v: Vec<(&str, usize)> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v.into_iter().map(|(n, _)| n.to_string()).collect()
    }

    /// 表記ゆれの候補。`aliases` は表記 → MusicBrainz の別名 (空でも可)
    pub fn artist_variants(&self, aliases: &HashMap<String, Vec<String>>) -> Vec<variants::VariantGroup> {
        let entries: Vec<variants::Entry> = self
            .db
            .tracks
            .iter()
            .map(|t| variants::Entry {
                artist: t.artist().unwrap_or(""),
                album_artist: t.album_artist().unwrap_or(""),
                album: t.album().unwrap_or(""),
                disc_nr: t.cd_nr(),
                track_nr: t.track_nr(),
                compilation: t.compilation(),
            })
            .collect();
        variants::find_groups(&entries, aliases)
    }

    /// 表記を統一する。読み (ソート用) は統一先の表記に合わせて付け直す。戻り値は更新した曲数
    pub fn unify_artists(&mut self, edits: &[ArtistUnify]) -> Result<usize> {
        self.check_writable()?;
        let mut map: HashMap<&str, &str> = HashMap::new();
        for e in edits {
            let to = e.to.trim();
            if to.is_empty() {
                continue;
            }
            for f in &e.from {
                if f.trim() != to {
                    map.insert(f.trim(), to);
                }
            }
        }
        // 統一先に既に付いている読みがあれば揃える (手で直した読みを優先)
        let mut sort_of: HashMap<&str, String> = HashMap::new();
        for t in &self.db.tracks {
            for (main, sort) in [(MHOD_ARTIST, MHOD_SORT_ARTIST), (MHOD_ALBUM_ARTIST, 29)] {
                if let (Some(n), Some(s)) = (t.string(main), t.string(sort))
                    && let Some(to) = map.values().find(|to| **to == n.trim())
                {
                    sort_of.entry(to).or_insert_with(|| s.to_string());
                }
            }
        }
        let mut n = 0;
        for t in self.db.tracks.iter_mut() {
            let mut changed = false;
            for (main, sort) in [(MHOD_ARTIST, MHOD_SORT_ARTIST), (MHOD_ALBUM_ARTIST, 29)] {
                let Some(&to) = t.string(main).and_then(|s| map.get(s.trim())) else {
                    continue;
                };
                let reading = sort_of.get(to).cloned().or_else(|| yomi::reading(to));
                t.set_string(main, Some(to));
                t.set_string(sort, reading.as_deref());
                changed = true;
            }
            n += changed as usize;
        }
        if n > 0 {
            self.dup_index = None;
            self.dirty = true;
        }
        Ok(n)
    }

    // ---------------- readings ----------------

    /// 既存の曲に読みを付ける。`ids` が None なら全曲。戻り値は更新した曲数
    pub fn generate_readings(&mut self, ids: Option<&[u64]>, overwrite: bool) -> Result<usize> {
        self.check_writable()?;
        let ids: Option<HashSet<u64>> = ids.map(|i| i.iter().copied().collect());
        let ids = ids.as_ref();
        let mut n = 0;
        for t in self.db.tracks.iter_mut() {
            if ids.is_some_and(|ids| !ids.contains(&t.dbid())) {
                continue;
            }
            if fill_readings(t, overwrite) {
                n += 1;
            }
        }
        self.dirty |= n > 0;
        Ok(n)
    }

    /// 読みの自動生成で何が変わるかを返す (DB は変更しない)
    pub fn preview_readings(&self, ids: Option<&[u64]>, overwrite: bool) -> Vec<ReadingChange> {
        let ids: Option<HashSet<u64>> = ids.map(|i| i.iter().copied().collect());
        let ids = ids.as_ref();
        let mut out = Vec::new();
        for t in &self.db.tracks {
            if ids.is_some_and(|ids| !ids.contains(&t.dbid())) {
                continue;
            }
            for (main, sort, after) in reading_changes(t, overwrite) {
                out.push(ReadingChange {
                    id: t.dbid().to_string(),
                    track_title: t.title().unwrap_or("").to_string(),
                    field: reading_field_name(sort).to_string(),
                    source: t.string(main).unwrap_or("").to_string(),
                    before: t.string(sort).unwrap_or("").to_string(),
                    after,
                });
            }
        }
        out
    }

    /// プレビューで選んだ変更だけを反映する。戻り値は更新した曲数
    pub fn apply_reading_edits(&mut self, edits: &[ReadingEdit]) -> Result<usize> {
        self.check_writable()?;
        let mut touched = std::collections::HashSet::new();
        for e in edits {
            let (Ok(id), Some(field)) = (e.id.parse::<u64>(), reading_field_id(&e.field)) else {
                continue;
            };
            if let Some(t) = self.db.tracks.iter_mut().find(|t| t.dbid() == id) {
                t.set_string(field, Some(e.value.as_str()));
                touched.insert(id);
            }
        }
        self.dirty |= !touched.is_empty();
        Ok(touched.len())
    }

    pub fn set_readings(&mut self, id: u64, r: &Readings) -> Result<()> {
        self.check_writable()?;
        let t = self
            .db
            .tracks
            .iter_mut()
            .find(|t| t.dbid() == id)
            .ok_or_else(|| Error::Other(t!("曲が見つかりません")))?;
        for (v, field) in [
            (&r.title, MHOD_SORT_TITLE),
            (&r.artist, MHOD_SORT_ARTIST),
            (&r.album, MHOD_SORT_ALBUM),
            (&r.album_artist, 29),
        ] {
            if let Some(v) = v {
                t.set_string(field, Some(v.as_str()));
            }
        }
        self.dirty = true;
        Ok(())
    }

    /// iPod 上のファイルの実パス (再生・書き出し用)
    pub fn track_file(&self, id: u64) -> Option<PathBuf> {
        let t = self.db.track_by_dbid(id)?;
        ipod_to_fs(self.root(), t.ipod_path()?)
    }

    /// iTunes の「オプション」の今の値
    pub fn track_options(&self, id: u64) -> Option<TrackOptions> {
        let t = self.db.track_by_dbid(id)?;
        Some(TrackOptions {
            skip_shuffle: t.skip_shuffle(),
            remember_position: t.remember_position(),
            volume: t.volume(),
            start_ms: t.start_ms(),
            stop_ms: t.stop_ms(),
            eq: t.eq_preset().unwrap_or(0),
        })
    }

    /// iTunesDB に記録されている値 (詳細情報の画面用)
    pub fn track_db_details(&self, id: u64) -> Option<DbDetails> {
        let t = self.db.track_by_dbid(id)?;
        Some(DbDetails {
            dbid: format!("{:016X}", t.dbid()),
            ipod_path: t.ipod_path().unwrap_or("").to_string(),
            filetype: t.string(MHOD_FILETYPE).unwrap_or("").to_string(),
            size: t.size(),
            length_ms: t.length_ms(),
            bitrate: t.bitrate(),
            sample_rate: t.samplerate(),
            mediatype: t.mediatype(),
            compilation: t.compilation(),
            has_artwork: t.has_artwork(),
            rating: t.rating(),
            play_count: t.playcount(),
            skip_count: t.skipcount(),
            date_added: mac_to_unix(t.time_added_mac()),
            date_modified: mac_to_unix(t.time_modified_mac()),
            last_played: mac_to_unix(t.last_played_mac()),
            last_skipped: mac_to_unix(t.last_skipped_mac()),
        })
    }

    // ---------------- playlists ----------------

    fn playlist_mut(&mut self, id: u64) -> Result<&mut Playlist> {
        self.db
            .playlists
            .iter_mut()
            .find(|p| p.id() == id)
            .ok_or_else(|| Error::Other(t!("プレイリストが見つかりません")))
    }

    fn editable_playlist(&mut self, id: u64) -> Result<&mut Playlist> {
        let pl = self.playlist_mut(id)?;
        if pl.is_master() {
            return Err(Error::Other(t!("ライブラリは編集できません")));
        }
        if pl.is_smart() {
            return Err(Error::Other(t!("スマートプレイリストは編集できません")));
        }
        Ok(pl)
    }

    pub fn create_playlist(&mut self, name: &str, tracks: &[u64]) -> Result<String> {
        let mut pl = Playlist::new(name, random_id(), false);
        pl.items = tracks
            .iter()
            .copied()
            .filter(|d| self.db.track_by_dbid(*d).is_some())
            .collect();
        let id = pl.id().to_string();
        self.db.playlists.push(pl);
        self.dirty = true;
        Ok(id)
    }

    /// スマートプレイリストの条件 (画面で編集する形)
    pub fn smart_playlist(&self, id: u64) -> Result<SmartDef> {
        let pl = self
            .db
            .playlists
            .iter()
            .find(|p| p.id() == id)
            .ok_or_else(|| Error::Other(t!("プレイリストが見つかりません")))?;
        let (prefs, rules) = pl
            .smart()
            .ok_or_else(|| Error::Other(t!("スマートプレイリストの条件を読めません")))?;
        Ok(SmartDef::from_parts(prefs, rules))
    }

    /// スマートプレイリストを作る (`id` が None) か、条件と名前を変える。中身はすぐ条件から決める
    pub fn save_smart_playlist(&mut self, id: Option<u64>, name: &str, def: SmartDef) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other(t!("名前が空です")));
        }
        let (prefs, rules) = def.into_parts();
        let now = now_mac_local();
        let idx = match id {
            Some(id) => self
                .db
                .playlists
                .iter()
                .position(|p| p.id() == id && p.is_smart() && !p.is_master())
                .ok_or_else(|| Error::Other(t!("プレイリストが見つかりません")))?,
            None => {
                self.db.playlists.push(Playlist::new(name, random_id(), false));
                self.db.playlists.len() - 1
            }
        };
        let previous = self.db.playlists[idx].items.clone();
        let items = self.db.evaluate_smart(&prefs, &rules, &previous, now);
        let pl = &mut self.db.playlists[idx];
        pl.name = name.to_string();
        pl.set_smart(&prefs, &rules);
        pl.items = items;
        self.dirty = true;
        Ok(pl.id().to_string())
    }

    /// iTunes / ミュージックのプレイリストを取り込む。`pid` は iTunes の Persistent ID で、iPod のプレイリストの id にも使う
    /// (取り込み直すと同じプレイリストを上書きし、「プレイリストが…」の条件もそのままつながる)。
    /// 同じ id が無ければ同じ名前のもの (スマートかどうかが同じもの) を上書きする。`smart` があれば条件ごと、無ければ `items` の曲で作る
    pub fn import_playlist(&mut self, pid: u64, name: &str, items: &[u64], smart: Option<(&SmartPrefs, &Rules)>) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other(t!("名前が空です")));
        }
        let found = self
            .db
            .playlists
            .iter()
            .position(|p| !p.is_master() && !p.is_podcast() && pid != 0 && p.id() == pid)
            .or_else(|| {
                self.db
                    .playlists
                    .iter()
                    .position(|p| !p.is_master() && !p.is_podcast() && p.name == name && p.is_smart() == smart.is_some())
            });
        let idx = match found {
            Some(i) => i,
            None => {
                let used = pid == 0 || self.db.playlists.iter().any(|p| p.id() == pid);
                self.db
                    .playlists
                    .push(Playlist::new(name, if used { random_id() } else { pid }, false));
                self.db.playlists.len() - 1
            }
        };
        let items = match smart {
            Some((prefs, rules)) => {
                let previous = self.db.playlists[idx].items.clone();
                let items = self.db.evaluate_smart(prefs, rules, &previous, now_mac_local());
                self.db.playlists[idx].set_smart(prefs, rules);
                items
            }
            None => {
                // 普通のプレイリストとして上書きするときは、条件を外す
                self.db.playlists[idx].spl_mhods.clear();
                let known: HashSet<u64> = self.db.tracks.iter().map(|t| t.dbid()).collect();
                items.iter().copied().filter(|d| known.contains(d)).collect()
            }
        };
        let pl = &mut self.db.playlists[idx];
        pl.name = name.to_string();
        pl.items = items;
        self.dirty = true;
        Ok(pl.id().to_string())
    }

    /// スマートプレイリストの名前・設定・条件 (書き出し用)
    pub fn smart_parts(&self, id: u64) -> Option<(String, SmartPrefs, Rules)> {
        let pl = self.db.playlists.iter().find(|p| p.id() == id)?;
        let (prefs, rules) = pl.smart()?;
        Some((pl.name.clone(), prefs, rules))
    }

    /// 条件の画面で、今の条件に合う曲の数と合計の長さ・大きさを出す
    pub fn preview_smart(&self, def: SmartDef, current: Option<u64>) -> SmartPreview {
        let (prefs, rules) = def.into_parts();
        let previous = current
            .and_then(|id| self.db.playlists.iter().find(|p| p.id() == id))
            .map(|p| p.items.clone())
            .unwrap_or_default();
        let ids = self
            .db
            .evaluate_smart(&prefs, &rules, &previous, now_mac_local());
        let set: HashSet<u64> = ids.iter().copied().collect();
        let (ms, bytes) = self
            .db
            .tracks
            .iter()
            .filter(|t| set.contains(&t.dbid()))
            .fold((0u64, 0u64), |(ms, b), t| (ms + t.length_ms() as u64, b + t.size() as u64));
        SmartPreview {
            sample: ids.iter().take(SMART_SAMPLE).map(|d| d.to_string()).collect(),
            count: ids.len(),
            length_ms: ms,
            size: bytes,
            not_on_ipod: rules.uses_any_field(FIELDS_NOT_ON_IPOD),
        }
    }

    /// プレイリストの並びを変える (iTunesDB の中の順番。iPod 本体のプレイリストのメニューもこの順)。
    /// `order` に無いプレイリスト (ポッドキャストなど) は、元の順のまま後ろに置く。ライブラリはいつも先頭
    pub fn reorder_playlists(&mut self, order: &[u64]) {
        let pos: HashMap<u64, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let before: Vec<u64> = self.db.playlists.iter().map(|p| p.id()).collect();
        self.db.playlists[1..].sort_by_key(|p| pos.get(&p.id()).copied().unwrap_or(usize::MAX));
        if self.db.playlists.iter().map(|p| p.id()).ne(before) {
            self.dirty = true;
        }
    }

    pub fn rename_playlist(&mut self, id: u64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other(t!("名前が空です")));
        }
        self.playlist_mut(id)?.name = name.to_string();
        self.dirty = true;
        Ok(())
    }

    pub fn delete_playlist(&mut self, id: u64) -> Result<()> {
        let idx = self
            .db
            .playlists
            .iter()
            .position(|p| p.id() == id)
            .ok_or_else(|| Error::Other(t!("プレイリストが見つかりません")))?;
        if idx == 0 {
            return Err(Error::Other(t!("ライブラリは削除できません")));
        }
        self.db.playlists.remove(idx);
        self.dirty = true;
        Ok(())
    }

    pub fn add_to_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<usize> {
        let known: HashSet<u64> = self.db.tracks.iter().map(|t| t.dbid()).collect();
        let valid: Vec<u64> = tracks.iter().copied().filter(|d| known.contains(d)).collect();
        let pl = self.editable_playlist(id)?;
        let before = pl.items.len();
        let mut have: HashSet<u64> = pl.items.iter().copied().collect();
        for d in valid {
            if have.insert(d) {
                pl.items.push(d);
            }
        }
        let added = pl.items.len() - before;
        self.dirty |= added > 0;
        Ok(added)
    }

    pub fn remove_from_playlist(&mut self, id: u64, tracks: &[u64]) -> Result<()> {
        let tracks: HashSet<u64> = tracks.iter().copied().collect();
        let pl = self.editable_playlist(id)?;
        pl.items.retain(|d| !tracks.contains(d));
        self.dirty = true;
        Ok(())
    }

    /// 並び替え: `order` は新しい順序のトラック dbid 列 (既存メンバーの並べ替えのみ)
    /// プレイリストの曲を並べ替える。画面に出さない動画は、並びを変えずに後ろに置く
    pub fn reorder_playlist(&mut self, id: u64, order: &[u64]) -> Result<()> {
        let videos = self.video_ids();
        let pl = self.editable_playlist(id)?;
        let (hidden, mut sorted_old): (Vec<u64>, Vec<u64>) = pl.items.iter().partition(|d| videos.contains(d));
        let mut sorted_new = order.to_vec();
        sorted_old.sort_unstable();
        sorted_new.sort_unstable();
        if sorted_old != sorted_new {
            return Err(Error::Other(
                t!("並び替え対象がプレイリストの内容と一致しません"),
            ));
        }
        pl.items = order.iter().copied().chain(hidden).collect();
        self.dirty = true;
        Ok(())
    }

    pub fn rename_ipod(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other(t!("名前が空です")));
        }
        self.db.master_mut().name = name.to_string();
        self.dirty = true;
        Ok(())
    }
}

fn itunes_dir(root: &Path) -> PathBuf {
    root.join("iPod_Control").join("iTunes")
}

/// ":iPod_Control:Music:F00:ABCD.mp3" → <root>/iPod_Control/Music/F00/ABCD.mp3。
/// iPod の外を指す場所 ("..:" など) や空の場所は None (消す・開くをしない)
pub fn ipod_to_fs(root: &Path, ipod_path: &str) -> Option<PathBuf> {
    devpath::join_inside(root, ipod_path, devpath::IPOD_SEPS).filter(|p| p != root)
}

fn filetype_marker(ext: &str) -> u32 {
    let mut b = [b' '; 4];
    for (i, c) in ext.bytes().take(4).enumerate() {
        b[i] = c.to_ascii_uppercase();
    }
    u32::from_be_bytes(b)
}

/// 1 曲に編集内容を当てる。変更があれば true
/// 歌詞は iPod が曲のファイルから読むので、iPod の中のファイルに書き、大きさと歌詞の印を直す
fn write_lyrics(root: &Path, t: &mut Track, lyrics: &str) -> Result<()> {
    let path = t
        .ipod_path()
        .and_then(|p| ipod_to_fs(root, p))
        .ok_or_else(|| Error::Other(t!("曲のファイルが見つかりません")))?;
    media::write_tags(&path, &TagEdit { lyrics: Some(lyrics.to_string()), ..Default::default() })?;
    t.set_size(fs::metadata(&path)?.len().min(u32::MAX as u64) as u32);
    t.set_lyrics_flag(media::read_lyrics(&path)?.is_some());
    Ok(())
}

fn apply_tag_edit(t: &mut Track, e: &TagEdit) -> bool {
    let mut changed = false;
    for (v, main, sort) in [
        (&e.title, MHOD_TITLE, Some(MHOD_SORT_TITLE)),
        (&e.artist, MHOD_ARTIST, Some(MHOD_SORT_ARTIST)),
        (&e.album, MHOD_ALBUM, Some(MHOD_SORT_ALBUM)),
        (&e.album_artist, MHOD_ALBUM_ARTIST, Some(29)),
        (&e.genre, MHOD_GENRE, None),
        (&e.composer, MHOD_COMPOSER, Some(MHOD_SORT_COMPOSER)),
    ] {
        let Some(v) = v.as_deref().map(str::trim) else {
            continue;
        };
        if t.string(main).unwrap_or("") == v {
            continue;
        }
        t.set_string(main, Some(v));
        if let Some(sort) = sort {
            t.set_string(sort, yomi::reading(v).as_deref());
        }
        changed = true;
    }
    if let Some(v) = e.year
        && t.year() != v
    {
        t.set_year(v);
        changed = true;
    }
    if let Some(v) = e.track_nr
        && t.track_nr() != v
    {
        t.set_track_nr(v);
        changed = true;
    }
    if let Some(v) = e.disc_nr
        && t.cd_nr() != v
    {
        t.set_cd_nr(v);
        changed = true;
    }
    if let Some(v) = e.rating.map(|v| v.min(100) / 20 * 20)
        && t.rating() != v
    {
        t.set_rating(v);
        changed = true;
    }
    if let Some(k) = e.kind {
        changed |= t.set_kind(k);
    }
    changed |= apply_play_options(t, e);
    // 読みを指定されたら、自動で付け直した読みより優先する
    for (v, sort) in [
        (&e.sort_title, MHOD_SORT_TITLE),
        (&e.sort_artist, MHOD_SORT_ARTIST),
        (&e.sort_album, MHOD_SORT_ALBUM),
        (&e.sort_album_artist, 29),
    ] {
        let Some(v) = v.as_deref().map(str::trim) else {
            continue;
        };
        if t.string(sort).unwrap_or("") != v {
            t.set_string(sort, Some(v));
            changed = true;
        }
    }
    changed
}

/// iTunes の「オプション」。種類を変えると「シャッフル時にスキップ」「再生位置を記憶」は種類の既定に戻るので、その後で呼ぶ
pub(crate) fn apply_play_options(t: &mut Track, e: &TagEdit) -> bool {
    let mut changed = false;
    if let Some(v) = e.volume.map(|v| v.clamp(-255, 255)).filter(|&v| v != t.volume()) {
        t.set_volume(v);
        changed = true;
    }
    if let Some(v) = e.start_ms.filter(|&v| v != t.start_ms()) {
        t.set_start_ms(v);
        changed = true;
    }
    if let Some(v) = e.stop_ms.filter(|&v| v != t.stop_ms()) {
        t.set_stop_ms(v);
        changed = true;
    }
    if let Some(v) = e.eq {
        changed |= t.set_eq_preset((v != 0).then_some(v));
    }
    if let Some(v) = e.skip_shuffle.filter(|&v| v != t.skip_shuffle()) {
        t.set_skip_shuffle(v);
        changed = true;
    }
    if let Some(v) = e.remember_position.filter(|&v| v != t.remember_position()) {
        t.set_remember_position(v);
        changed = true;
    }
    changed
}

/// ソート用フィールドが空なら (overwrite 時は常に) 読みを入れる。変更があれば true
fn fill_readings(t: &mut Track, overwrite: bool) -> bool {
    let changes = reading_changes(t, overwrite);
    for (_, sort, r) in &changes {
        t.set_string(*sort, Some(r));
    }
    !changes.is_empty()
}

/// 自動生成で変わるフィールド: (元フィールド, ソート用フィールド, 新しい読み)
fn reading_changes(t: &Track, overwrite: bool) -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    for (main, sort) in READING_FIELDS {
        if !overwrite && t.string(sort).is_some() {
            continue;
        }
        let Some(r) = t.string(main).and_then(yomi::reading) else {
            continue;
        };
        if t.string(sort) != Some(r.as_str()) {
            out.push((main, sort, r));
        }
    }
    out
}

/// トラック → ArtworkDB の画像 id の索引。曲ごとに画像一覧を線形に探すと
/// 1 万曲 × 1 万画像で重くなるので、一覧を作る前に 1 回だけ組み立てる
struct ArtIndex {
    ids: HashSet<u32>,
    /// 古い形式: mhii.song_id がトラックの dbid
    by_song: HashMap<u64, u32>,
}

impl ArtIndex {
    fn new(art: Option<&ArtState>) -> Self {
        let images = art.map(|a| a.db.images.as_slice()).unwrap_or_default();
        let mut by_song = HashMap::new();
        for i in images {
            by_song.entry(i.song_id).or_insert(i.id);
        }
        ArtIndex {
            ids: images.iter().map(|i| i.id).collect(),
            by_song,
        }
    }

    fn image_id_of(&self, t: &Track) -> Option<u32> {
        if !t.has_artwork() {
            return None;
        }
        let link = t.mhii_link();
        if link != 0 && self.ids.contains(&link) {
            return Some(link);
        }
        self.by_song.get(&t.dbid()).copied()
    }
}

fn artwork_dir(root: &Path) -> PathBuf {
    root.join("iPod_Control").join("Artwork")
}

fn load_art(root: &Path, model_formats: &[ArtFormat]) -> Option<ArtState> {
    let dir = artwork_dir(root);
    let db = match fs::read(dir.join("ArtworkDB")) {
        Ok(data) => {
            let db = match ArtworkDb::parse(&data) {
                Ok(db) => db,
                Err(e) => {
                    // 壊れた ArtworkDB は上書きせずアートワーク機能だけ無効にする
                    log::warn!("{e}");
                    return None;
                }
            };
            let backup = dir.join(ART_BACKUP_NAME);
            adopt_legacy_name(&backup, "ArtworkDB.ipodsync-backup");
            if !backup.exists() {
                let _ = fs::write(&backup, &data);
            }
            db
        }
        Err(_) => ArtworkDb::new_empty(),
    };
    let formats = db.formats(model_formats);
    if formats.is_empty() {
        return None;
    }
    adopt_legacy_name(&dir.join(ART_INDEX_NAME), "ipodsync-art.json");
    let mut index: HashIndex = fs::read(dir.join(ART_INDEX_NAME))
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default();
    index.retain(|_, e| e.valid_in(&db));
    let custom_video: HashSet<u64> = fs::read(dir.join(VIDEO_ART_NAME))
        .ok()
        .and_then(|d| serde_json::from_slice::<Vec<u64>>(&d).ok())
        .unwrap_or_default()
        .into_iter()
        .collect();
    Some(ArtState {
        db,
        formats,
        index,
        custom_video,
        dirty: false,
    })
}

/// 一時ファイルに書いてから置き換える
/// nano 5G 以降の本体が書く PlayCounts.plist (`tracks` の配列。1 曲は persistentID・playCount・playMacOSDate・
/// skipCount・skipMacOSDate・userRating・bookmarkTimeInMS) を iTunesDB の曲に足す。読み方は libgpod 0.8.3 の
/// playcounts_plist_read と同じ。戻り値は反映した曲の数
fn merge_play_counts_plist(db: &mut ITunesDb, data: &[u8]) -> usize {
    let Ok(v) = plist::Value::from_reader(std::io::Cursor::new(data)) else { return 0 };
    let Some(list) = v.as_dictionary().and_then(|d| d.get("tracks")).and_then(|t| t.as_array()) else { return 0 };
    let index: HashMap<u64, usize> = db.tracks.iter().enumerate().map(|(i, t)| (t.dbid(), i)).collect();
    let int = |d: &plist::Dictionary, k: &str| -> u64 {
        d.get(k).and_then(|v| v.as_signed_integer().map(|i| i as u64).or_else(|| v.as_unsigned_integer())).unwrap_or(0)
    };
    let mut merged = 0;
    for d in list.iter().filter_map(|v| v.as_dictionary()) {
        let Some(&i) = d.get("persistentID").and_then(|_| index.get(&int(d, "persistentID"))) else { continue };
        let t = &mut db.tracks[i];
        let plays = int(d, "playCount") as u32;
        if plays != 0 {
            t.set_playcount(t.playcount().saturating_add(plays));
            t.set_mark_unplayed(1);
        }
        let played = int(d, "playMacOSDate") as u32;
        if played != 0 {
            t.set_last_played_mac(played);
        }
        let skips = int(d, "skipCount") as u32;
        if skips != 0 {
            t.set_skipcount(t.skipcount().saturating_add(skips));
        }
        let skipped = int(d, "skipMacOSDate") as u32;
        if skipped != 0 {
            t.set_last_skipped_mac(skipped);
        }
        let rating = int(d, "userRating");
        if (1..=100).contains(&rating) {
            t.set_rating(rating as u8);
        }
        let bookmark = int(d, "bookmarkTimeInMS") as u32;
        if bookmark != 0 {
            t.set_bookmark_ms(bookmark);
        }
        merged += 1;
    }
    merged
}

/// nano 5G の hash72 の材料。`iPod_Control/Device/HashInfo` (libgpod と同じ形) が無ければ、iTunes が署名した
/// iTunesCDB から取り出して HashInfo に書いておく (ZenPod が書き直したあとでも、同じ材料なので取り出せる)
fn load_hash_info(root: &Path, info: &DeviceInfo, cdb_raw: Option<&[u8]>) -> Option<hash72::HashInfo> {
    let fwid = info.firewire_id_bytes()?;
    let path = root.join("iPod_Control").join("Device").join("HashInfo");
    if let Some(h) = fs::read(&path).ok().and_then(|d| hash72::HashInfo::parse(&d, &fwid)) {
        return Some(h);
    }
    let h = hash72::extract_from_db(cdb_raw?, &fwid)?;
    if let Err(e) = fs::write(&path, h.to_bytes()) {
        log::warn!("HashInfo: {e}");
    }
    Some(h)
}

/// `src` を転送する形式 (`target`) にして `out` に書く。Mac の FLAC は、`afconvert` なら OS 標準の afconvert で
/// 変換できる (ffmpeg が無くてもよい。失敗したら ffmpeg があればそちらでやり直す)
pub(crate) fn convert_for_transfer(
    src: &Path,
    out: &Path,
    target: ConvertTarget,
    sample_rate: u32,
    afconvert: bool,
    ffmpeg: Option<&Path>,
) -> Result<()> {
    let af = (afconvert && convert::afconvert_supports(src, target)).then(convert::find_afconvert).flatten();
    match (af, ffmpeg) {
        (Some(af), ff) => match convert::convert_afconvert(&af, src, out, target, sample_rate) {
            Ok(()) => {
                fix_moov(ff, out);
                Ok(())
            }
            Err(e) => match ff {
                Some(ff) => {
                    log::warn!("afconvert: {e}");
                    convert::convert(ff, src, out, target, sample_rate)
                }
                None => Err(e),
            },
        },
        (None, Some(ff)) => convert::convert(ff, src, out, target, sample_rate),
        (None, None) => Err(Error::Unsupported(t!(
            "{ext} の変換には ffmpeg が必要です (設定 → 変換 から自動で入れられます)",
            ext = src.extension().and_then(|e| e.to_str()).unwrap_or("?")
        ))),
    }
}

/// shuffle で再生できない形式の変換先。shuffle は AIFF を再生できず (iTunesSD に AIFF の種類が無い)、
/// 1G / 2G は ALAC も再生できないので、ALAC にする曲は非可逆の変換先 (ALAC なら AAC 256kbps) にする
fn shuffle_target(shuffle: Option<shuffle::SdFormat>, fmt: Format, target: Option<ConvertTarget>, opts: &AddOptions) -> Option<ConvertTarget> {
    let Some(sd) = shuffle else { return target };
    let alac_ok = sd == shuffle::SdFormat::V2;
    let lossy = if opts.lossy_to == ConvertTarget::Alac { ConvertTarget::LOSSY_DEFAULT } else { opts.lossy_to };
    let fit = |t: ConvertTarget| if t == ConvertTarget::Alac && !alac_ok { lossy } else { t };
    match target {
        Some(t) => Some(fit(t)),
        None => match fmt {
            Format::Aiff => Some(fit(opts.lossless_to)),
            Format::Alac if !alac_ok => Some(lossy),
            _ => None,
        },
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("zenpod-tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        // macOS でも F_FULLFSYNC のまま。それまでに `sync_file` で書いた曲・画像も、ここでディスクのキャッシュから書き出される
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// m4a の目次 (moov) を先頭へ並べ替えて `dst` に書く。ffmpeg があれば ffmpeg で (実機で確かめた方法)、
/// 無ければ自前の並べ替えで
fn faststart(ffmpeg: Option<&Path>, src: &Path, dst: &Path) -> Result<()> {
    match ffmpeg {
        Some(ff) => convert::faststart(ff, src, dst),
        None => convert::faststart_native(src, dst),
    }
}

/// 変換してできた m4a の目次が末尾にあれば、その場で先頭へ並べ替える。失敗しても変換結果はそのまま使う
fn fix_moov(ffmpeg: Option<&Path>, path: &Path) {
    if !convert::moov_after_mdat(path) {
        return;
    }
    let tmp = path.with_extension("faststart.m4a");
    if let Err(e) = faststart(ffmpeg, path, &tmp).and_then(|_| Ok(fs::rename(&tmp, path)?)) {
        log::warn!("faststart: {e}");
        let _ = fs::remove_file(&tmp);
    }
}

fn copy_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::copy(src, dst)?;
    // 書き込み権限付きで開き直す。Windows の FlushFileBuffers は読み取り専用のハンドルだと
    // 「アクセスが拒否されました (os error 5)」で失敗する (Linux / macOS の fsync は通るので気づきにくい)
    crate::device::sync_file(&fs::OpenOptions::new().write(true).open(dst)?)
}

fn track_view(t: &Track) -> TrackView {
    let s = |o: Option<&str>| o.unwrap_or("").to_string();
    TrackView {
        id: t.dbid().to_string(),
        rb_path: String::new(),
        title: s(t.title()),
        artist: s(t.artist()),
        album: s(t.album()),
        album_artist: s(t.album_artist()),
        genre: s(t.genre()),
        duration_ms: t.length_ms(),
        track_nr: t.track_nr(),
        disc_nr: t.cd_nr(),
        year: t.year(),
        size: t.size(),
        bitrate: t.bitrate(),
        play_count: t.playcount(),
        rating: t.rating(),
        filetype: s(t.string(MHOD_FILETYPE)),
        date_added: mac_to_unix(t.time_added_mac()),
        artwork_id: 0,
        sort_title: s(t.string(MHOD_SORT_TITLE)),
        sort_artist: s(t.string(MHOD_SORT_ARTIST)),
        sort_album: s(t.string(MHOD_SORT_ALBUM)),
        sort_album_artist: s(t.string(29)),
        composer: s(t.composer()),
        kind: t.kind(),
    }
}
