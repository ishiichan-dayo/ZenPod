//! ポッドキャストの購読。購読の一覧は `podcasts.json` (設定と同じ場所)、
//! 落とした回はアプリのデータの場所の `podcasts/<番組>/` に置く。iPod への転送はふつうの転送 (`add_paths`) を使う

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use ipod_core::media;
use ipod_core::media::cover::{content_key, thumbnails_jpeg};
use ipod_core::online::podcast::{self, Episode, Feed, Store};
use ipod_core::t;
use serde::{Deserialize, Serialize};

use super::events::{AppEvent, PodcastDownloadProgress};
use super::pc_artwork::PcArtwork;
use super::ports::{AppDirs, EventSink, PodcastSource};
use crate::error::{AppError, AppResult};

/// 回の一覧の画像の大きさ (表示の 2 倍)
const EP_THUMB: u32 = 80;
/// 回に入れる番組の画像の大きさ (iPod の画面のジャケットは大きくても 320px。元の 3000px のままだとファイルが無駄に大きくなる)
const EP_COVER: u32 = 600;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResult {
    id: String,
    title: String,
    /// 新しく見つかった回の数
    added: usize,
    error: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EpRef {
    feed_id: String,
    ep_id: String,
    /// 印を変えたときの iPod の再生回数 (podcast_mark だけ。iPod に無い回は無し)
    #[serde(default)]
    plays: Option<u32>,
}

/// 番組の画像 (一覧用の小さいものと、見出し用の大きいもの)
#[derive(Serialize)]
pub struct Cover {
    small: String,
    large: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Downloaded {
    ep_id: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    /// 落とした (か、もう落としてあった) 回。頼まれた順
    done: Vec<Downloaded>,
    errors: Vec<String>,
    cancelled: bool,
}

pub struct Podcasts {
    dirs: Arc<dyn AppDirs>,
    source: Arc<dyn PodcastSource>,
    events: Arc<dyn EventSink>,
    pc_artwork: Arc<PcArtwork>,
    store: Mutex<Option<Store>>,
    /// 回を落としている間 (同時には 1 つだけ)
    busy: AtomicBool,
    cancel: AtomicBool,
}

fn now() -> i64 {
    ipod_core::ipod::itunesdb::now_unix()
}

impl Podcasts {
    pub fn new(dirs: Arc<dyn AppDirs>, source: Arc<dyn PodcastSource>, events: Arc<dyn EventSink>, pc_artwork: Arc<PcArtwork>) -> Self {
        Podcasts { dirs, source, events, pc_artwork, store: Mutex::default(), busy: AtomicBool::new(false), cancel: AtomicBool::new(false) }
    }

    /// 購読の一覧を読んで (最初の 1 回だけ) 操作する
    fn with_store<T>(&self, f: impl FnOnce(&mut Store) -> T) -> AppResult<T> {
        let mut guard = self.store.lock().unwrap();
        if guard.is_none() {
            let no_dir = || AppError::msg(t!("設定の保存先が決められません"));
            let list = self.dirs.config_dir().ok_or_else(no_dir)?.join("podcasts.json");
            let dir = self.dirs.local_data_dir().ok_or_else(no_dir)?.join("podcasts");
            *guard = Some(Store::load(list, dir));
        }
        Ok(f(guard.as_mut().unwrap()))
    }

    /// 画面に渡す番組の一覧。落としたファイルが無くなっていれば印を外しておく
    pub fn list(&self) -> AppResult<Vec<Feed>> {
        self.with_store(|store| {
            let mut changed = false;
            let ids: Vec<String> = store.feeds.iter().map(|f| f.id.clone()).collect();
            for id in ids {
                let missing: Vec<String> = store
                    .feed(&id)
                    .map(|f| f.episodes.iter().filter(|e| e.file.is_some() && store.episode_file(&id, e).is_none()).map(|e| e.id.clone()).collect())
                    .unwrap_or_default();
                if let Some(f) = store.feed_mut(&id) {
                    for e in f.episodes.iter_mut().filter(|e| missing.contains(&e.id)) {
                        e.file = None;
                        changed = true;
                    }
                }
            }
            if changed {
                let _ = store.save();
            }
            store.feeds.clone()
        })
    }

    /// 番組の画像を落として置いておく (失敗しても登録は続ける)
    fn save_cover(&self, feed_id: &str, url: &str) {
        let Ok(Some(path)) = self.with_store(|s| s.cover_path(feed_id)) else { return };
        let Some(bytes) = self.source.fetch_image(url) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, bytes);
    }

    /// 番組を登録する。フィードを読めたら番組の ID を返す (同じ URL がもうあればその ID)
    pub fn subscribe(&self, url: &str) -> AppResult<String> {
        let url = podcast::normalize_url(url)?;
        if let Some(id) = self.with_store(|s| s.feeds.iter().find(|f| f.url == url).map(|f| f.id.clone()))? {
            return Ok(id);
        }
        let fetched = self.source.fetch_feed(&url, None, None)?;
        let id = self.with_store(|s| {
            let id = s.add(&url, fetched, now());
            s.save().map(|_| id)
        })??;
        let image = self.with_store(|s| s.feed(&id).map(|f| f.image_url.clone()))?.unwrap_or_default();
        self.save_cover(&id, &image);
        Ok(id)
    }

    /// 番組の登録をやめる (落としたファイルも消す。iPod の回はそのまま)
    pub fn unsubscribe(&self, id: &str) -> AppResult<()> {
        self.with_store(|s| {
            s.remove(id);
            s.save()
        })??;
        Ok(())
    }

    /// フィードを読み直す (`ids` が無ければ全部)。`min_age` 秒より前に確かめた番組だけにする (起動時)
    pub fn refresh(&self, ids: Option<&[String]>, min_age: Option<i64>) -> AppResult<Vec<RefreshResult>> {
        // 確かめる番組 (ID, URL, 前回の ETag, 前回の Last-Modified, 画像がまだ無いか)
        type Target = (String, String, Option<String>, Option<String>, bool);
        let targets: Vec<Target> = self.with_store(|s| {
            s.feeds
                .iter()
                .filter(|f| ids.is_none_or(|ids| ids.contains(&f.id)))
                .filter(|f| min_age.is_none_or(|age| now() - f.checked >= age))
                .map(|f| {
                    // 保存の形が古い番組は、足した項目を入れるため前回の ETag を送らずに本文ごと取り直す
                    let fresh = f.meta_version >= podcast::META_VERSION;
                    let etag = f.etag.clone().filter(|_| fresh);
                    let lm = f.last_modified.clone().filter(|_| fresh);
                    (f.id.clone(), f.url.clone(), etag, lm, !s.cover_path(&f.id).is_some_and(|p| p.is_file()))
                })
                .collect()
        })?;
        let mut out = Vec::new();
        // 番組ごとに順に (同じ配信元へ同時に問い合わせない)
        for (id, url, etag, lm, no_cover) in targets {
            let r = self.source.fetch_feed(&url, etag.as_deref(), lm.as_deref());
            let (title, added, error, image) = self.with_store(|s| {
                let Some(f) = s.feed_mut(&id) else { return (String::new(), 0, None, None) };
                match r {
                    Ok(fetched) => {
                        let before = f.episodes.len();
                        f.apply(fetched, now());
                        let added = f.episodes.len() - before;
                        let (title, image) = (f.title.clone(), f.image_url.clone());
                        let _ = s.save();
                        (title, added, None, Some(image))
                    }
                    Err(e) => (f.title.clone(), 0, Some(e.to_string()), None),
                }
            })?;
            if let (true, Some(image)) = (no_cover, image) {
                self.save_cover(&id, &image);
            }
            out.push(RefreshResult { id, title, added, error });
        }
        Ok(out)
    }

    /// 同期で iPod に置いておく、聞いていない回の数 (0 = 自動では送らない) と、古い方から選ぶか
    pub fn set_keep(&self, id: &str, keep: u32, oldest: Option<bool>) -> AppResult<()> {
        self.with_store(|s| {
            if let Some(f) = s.feed_mut(id) {
                f.keep = keep;
                if let Some(o) = oldest {
                    f.keep_oldest = o;
                }
            }
            s.save()
        })??;
        Ok(())
    }

    /// 回に「聞いた」の印を付ける / 外す
    pub fn mark(&self, items: &[EpRef], played: bool) -> AppResult<()> {
        self.with_store(|s| {
            for it in items {
                if let Some(ep) = s.feed_mut(&it.feed_id).and_then(|f| f.episodes.iter_mut().find(|e| e.id == it.ep_id)) {
                    ep.played = played;
                    if let Some(p) = it.plays {
                        ep.plays_seen = p;
                    }
                }
            }
            s.save()
        })??;
        Ok(())
    }

    /// 落としたファイルを消す (iPod の回はそのまま)
    pub fn delete_files(&self, items: &[EpRef]) -> AppResult<()> {
        self.with_store(|s| {
            for it in items {
                s.delete_file(&it.feed_id, &it.ep_id);
            }
            s.save()
        })??;
        Ok(())
    }

    /// 落としたファイルの合計 (設定に出す)
    pub fn files_size(&self) -> AppResult<u64> {
        self.with_store(|s| s.files_size())
    }

    /// 番組の画像。登録している番組だけ (ID がパスになるので、ほかの ID では読まない)
    pub fn cover(&self, id: &str) -> Option<Cover> {
        let path = self.with_store(|s| s.cover_path(id)).ok()??;
        let bytes = std::fs::read(path).ok()?;
        let key = format!("pod-{}", content_key(&bytes));
        let t = self.pc_artwork.make_thumbs(&key, &bytes)?;
        Some(Cover { small: t.small, large: t.large })
    }

    /// 回を落としてタグを書く。もう落としてある回は落とし直さない
    pub fn download(&self, items: &[EpRef]) -> AppResult<DownloadResult> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(AppError::msg(t!("ほかの回を取得しています")));
        }
        self.cancel.store(false, Ordering::SeqCst);
        let r = self.download_all(items);
        self.busy.store(false, Ordering::SeqCst);
        Ok(r)
    }

    pub fn cancel_download(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn download_all(&self, items: &[EpRef]) -> DownloadResult {
        let mut res = DownloadResult { done: Vec::new(), errors: Vec::new(), cancelled: false };
        let total = items.len();
        for (index, it) in items.iter().enumerate() {
            if self.cancel.load(Ordering::SeqCst) {
                res.cancelled = true;
                break;
            }
            match self.download_one(it, index, total) {
                Ok(p) => res.done.push(Downloaded { ep_id: it.ep_id.clone(), path: p.to_string_lossy().into_owned() }),
                Err(e) => {
                    if self.cancel.load(Ordering::SeqCst) {
                        res.cancelled = true;
                        break;
                    }
                    res.errors.push(e);
                }
            }
        }
        res
    }

    fn download_one(&self, it: &EpRef, index: usize, total: usize) -> Result<PathBuf, String> {
        // 落とす間は一覧の lock を持たない (画面の操作を止めない)
        let found: Option<(Feed, Episode, PathBuf, Option<PathBuf>)> = self.with_store(|s| {
            let f = s.feed(&it.feed_id)?;
            let ep = f.episodes.iter().find(|e| e.id == it.ep_id)?.clone();
            let existing = s.episode_file(&f.id, &ep);
            let meta = Feed { episodes: Vec::new(), ..f.clone() };
            Some((meta, ep, s.feed_dir(&f.id)?, existing))
        })?;
        let Some((feed, ep, dir, existing)) = found else {
            return Err(t!("番組か回が見つかりません"));
        };
        if let Some(p) = existing {
            // 前の版で落としたファイルは、番組の画像を入れ直してから送る
            if !ep.cover_applied && apply_show_cover(&p, &dir) {
                self.with_store(|s| {
                    if let Some(e) = s.feed_mut(&it.feed_id).and_then(|f| f.episodes.iter_mut().find(|e| e.id == it.ep_id)) {
                        e.cover_applied = true;
                    }
                    s.save()
                })?
                .map_err(|e| e.to_string())?;
            }
            return Ok(p);
        }
        let label = |e: String| format!("{}: {e}", ep.title);
        if ep.is_video() {
            return Err(label(t!("動画の回には対応していません")));
        }
        let emit = |received: u64, size: u64| {
            self.events.emit(AppEvent::PodcastDownload(PodcastDownloadProgress {
                index,
                total,
                feed_id: feed.id.clone(),
                ep_id: ep.id.clone(),
                received,
                size,
            }));
        };
        emit(0, ep.length);
        let name = self.source.download_episode(&ep, &dir, &self.cancel, &mut |r, s| emit(r, s)).map_err(|e| label(e.to_string()))?;
        let path = dir.join(&name);
        let show = if feed.title.is_empty() { t!("ポッドキャスト") } else { feed.title.clone() };
        let tags = media::PodcastTags {
            title: &ep.title,
            show: &show,
            author: &feed.author,
            published: ep.published,
            description: &ep.description,
            feed_url: &feed.url,
            guid: &ep.guid,
        };
        if let Err(e) = media::tag_podcast_episode(&path, &tags) {
            let _ = std::fs::remove_file(&path);
            return Err(label(e.to_string()));
        }
        // 番組の画像を入れる (回に埋め込まれた画像は入れ替える。iPod のジャケットと Rockbox の cover.jpg になる)
        let cover_applied = apply_show_cover(&path, &dir);
        self.with_store(|s| {
            if let Some(e) = s.feed_mut(&it.feed_id).and_then(|f| f.episodes.iter_mut().find(|e| e.id == it.ep_id)) {
                e.cover_applied = cover_applied;
                e.file = Some(name.clone());
                e.tag_album = Some(show.clone());
                e.tag_title = Some(ep.title.clone());
            }
            s.save()
        })?
        .map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// 回の画像 (一覧用の小さいもの、data URL)。`url` が空か番組の画像と同じなら番組の画像。
    /// 小さくしたものを番組のフォルダの `img/` に置いて使い回す (同じ画像の回が多いので、URL ごとに 1 回だけ取る)。
    /// 取れなかった画像は印のファイルを置き、取り直さない
    pub fn image(&self, feed_id: &str, url: &str) -> Option<String> {
        let (dir, cover, show_image) = self
            .with_store(|s| {
                let f = s.feed(feed_id)?;
                Some((s.feed_dir(&f.id)?.join("img"), s.cover_path(&f.id)?, f.image_url.clone()))
            })
            .ok()??;
        let use_cover = url.is_empty() || url == show_image;
        let key = if use_cover { "cover".to_string() } else { podcast::short_id(url) };
        let path = dir.join(format!("{key}-{EP_THUMB}.jpg"));
        let none = dir.join(format!("{key}.none"));
        let url_of = |jpg: &[u8]| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg));
        if let Ok(jpg) = std::fs::read(&path) {
            return Some(url_of(&jpg));
        }
        if none.exists() && !use_cover {
            return None;
        }
        let bytes = if use_cover { std::fs::read(&cover).ok() } else { self.source.fetch_image(url) };
        let jpg = bytes.and_then(|b| thumbnails_jpeg(&b, &[EP_THUMB])).and_then(|mut v| v.pop());
        let _ = std::fs::create_dir_all(&dir);
        match jpg {
            Some(jpg) => {
                let _ = std::fs::write(&path, &jpg);
                Some(url_of(&jpg))
            }
            None => {
                if !use_cover {
                    let _ = std::fs::write(&none, b"");
                }
                None
            }
        }
    }

    /// 番組のサイト (フィードに書いてあるリンク)。登録している番組の http / https のリンクだけ。開くのは呼んだ側
    pub fn link(&self, id: &str) -> AppResult<String> {
        let link = self.with_store(|s| s.feed(id).map(|f| f.link.clone()))?.unwrap_or_default();
        if !is_web_link(&link) {
            return Err(AppError::msg(t!("開けないリンクです")));
        }
        Ok(link)
    }
}

/// ブラウザで開いてよい形のリンク (http / https で、空白や引用符を含まない)
fn is_web_link(link: &str) -> bool {
    (link.starts_with("https://") || link.starts_with("http://")) && !link.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"')
}

/// 落とした回のファイルに番組の画像を入れる (埋め込まれていた画像は入れ替える)。入れられたら true。
/// 番組の画像が無い・読めない・書けないときは何もしない (回は送れる)
fn apply_show_cover(path: &Path, dir: &Path) -> bool {
    let Ok(cover) = std::fs::read(dir.join("cover")) else { return false };
    let Some(jpg) = thumbnails_jpeg(&cover, &[EP_COVER]).and_then(|mut v| v.pop()) else { return false };
    media::write_cover(path, Some(&jpg)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_links_only() {
        assert!(is_web_link("https://example.com/show"));
        assert!(is_web_link("http://example.com"));
        assert!(!is_web_link("file:///C:/Windows"));
        assert!(!is_web_link("https://example.com/a b"));
        assert!(!is_web_link("https://example.com/\"x"));
        assert!(!is_web_link(""));
    }
}
