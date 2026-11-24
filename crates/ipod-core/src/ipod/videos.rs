//! iPod OS の動画: iTunesDB に動画 (ムービー・ミュージックビデオ・テレビ番組) として載せる。
//! ファイルは曲と同じ iPod_Control/Music/Fxx に .m4v で置く。一覧の画像は ArtworkDB に入れる (曲のジャケットと同じ)

use crate::t;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use super::itunesdb::*;
use super::{ArtIndex, FREE_MARGIN, Ipod, apply_play_options, apply_tag_edit, copy_file, filetype_marker};
use crate::domain::track::TagEdit;
use crate::domain::video::{VideoEdit, VideoKind, VideoOptions, VideoOutcome, VideoSize, VideoView};
use crate::domain::yomi;
use crate::error::{Error, Result};
use crate::media::video::{self, Prepared, Progress, VideoInfo, VideoTarget};

/// 動画の一覧の 1 本分。`custom_art` はアートワークを利用者が選んだ動画
fn video_view(t: &Track, kind: VideoKind, idx: &ArtIndex, custom_art: bool) -> VideoView {
    let s = |o: Option<&str>| o.unwrap_or("").to_string();
    let artwork_id = idx.image_id_of(t).unwrap_or(0);
    VideoView {
        id: t.dbid().to_string(),
        title: s(t.title()),
        kind,
        artist: s(t.artist()),
        album: s(t.album()),
        album_artist: s(t.album_artist()),
        genre: s(t.genre()),
        composer: s(t.composer()),
        grouping: s(t.string(MHOD_GROUPING)),
        comment: s(t.string(MHOD_COMMENT)),
        year: t.year(),
        track_nr: t.track_nr(),
        disc_nr: t.cd_nr(),
        released: mac_to_unix(t.time_released_mac()),
        sort_title: s(t.string(MHOD_SORT_TITLE)),
        sort_artist: s(t.string(MHOD_SORT_ARTIST)),
        sort_album: s(t.string(MHOD_SORT_ALBUM)),
        sort_album_artist: s(t.string(MHOD_SORT_ALBUM_ARTIST)),
        sort_composer: s(t.string(MHOD_SORT_COMPOSER)),
        sort_show: s(t.string(MHOD_SORT_TV_SHOW)),
        show: s(t.string(MHOD_TV_SHOW)),
        season: t.season(),
        episode: t.episode(),
        episode_id: s(t.string(MHOD_TV_EPISODE)),
        network: s(t.string(MHOD_TV_NETWORK)),
        description: s(t.string(MHOD_DESCRIPTION)),
        rating: t.rating(),
        skip_shuffle: t.skip_shuffle(),
        remember_position: t.remember_position(),
        volume: t.volume(),
        start_ms: t.start_ms(),
        stop_ms: t.stop_ms(),
        eq: t.eq_preset().unwrap_or(0),
        duration_ms: t.length_ms(),
        size: t.size() as u64,
        date_added: mac_to_unix(t.time_added_mac()),
        artwork_id,
        custom_art: custom_art && artwork_id != 0,
        play_count: t.playcount(),
        last_played: mac_to_unix(t.last_played_mac()),
        date_modified: mac_to_unix(t.time_modified_mac()),
        filetype: s(t.string(MHOD_FILETYPE)),
        bitrate: t.bitrate(),
        sample_rate: t.samplerate(),
        ipod_path: s(t.ipod_path()),
        rb_path: String::new(),
    }
}

/// 題名 (無ければファイル名)
fn title_of(info: &VideoInfo, src: &Path) -> String {
    info.tags
        .title
        .clone()
        .unwrap_or_else(|| src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
}

impl Ipod {
    /// 「ビデオ」に出す動画 (ビデオポッドキャストは曲の一覧に出す)
    pub fn videos(&self) -> Vec<VideoView> {
        let idx = ArtIndex::new(self.art.as_ref());
        let custom = |t: &Track| self.art.as_ref().is_some_and(|a| a.custom_video.contains(&t.dbid()));
        self.db.tracks.iter().filter_map(|t| Some(video_view(t, t.video_kind()?, &idx, custom(t)))).collect()
    }

    /// 同じ動画 (題名・番組・シーズン・話数が同じで、長さの差が 2 秒未満) の題名
    fn find_video_duplicate(&self, title: &str, info: &VideoInfo, length_ms: u32) -> Option<String> {
        let norm = |s: Option<&str>| s.unwrap_or("").trim().to_lowercase();
        let (title_n, show_n) = (norm(Some(title)), norm(info.tags.show.as_deref()));
        self.db
            .tracks
            .iter()
            .filter(|t| t.video_kind().is_some())
            .find(|t| {
                norm(t.title()) == title_n
                    && norm(t.string(MHOD_TV_SHOW)) == show_n
                    && t.season() == info.tags.season
                    && t.episode() == info.tags.episode
                    && (t.length_ms() as i64 - length_ms as i64).abs() < 2000
            })
            .map(|t| t.title().unwrap_or("").to_string())
    }

    /// 動画を送れるか (書き込める・動画を再生できる機種か) と、変換するときの形
    pub fn video_target(&self, size: VideoSize) -> Result<VideoTarget> {
        self.check_writable()?;
        if !self.info.video {
            return Err(Error::Unsupported(t!("この iPod は動画を再生できません")));
        }
        Ok(VideoTarget::Ipod(size))
    }

    /// 同じ動画があればその題名 (`info` は元のファイルの情報)
    pub fn video_duplicate(&self, src: &Path, info: &VideoInfo) -> Option<String> {
        self.find_video_duplicate(&title_of(info, src), info, info.duration_ms)
    }

    /// 準備した (変換した) 動画を載せる (DB の保存は呼び出し側で save())
    pub fn add_prepared_video(&mut self, src: &Path, p: &Prepared) -> Result<VideoView> {
        self.check_writable()?;
        let thumb = if self.art.is_some() { p.thumb.clone() } else { None };
        self.register_video(&p.file, src, &title_of(&p.info, src), &p.info, &p.props, thumb)
    }

    /// 動画を 1 本送る (DB の保存は呼び出し側で save())。iPod で再生できない形式は ffmpeg で変換する
    pub fn add_video(&mut self, src: &Path, ffmpeg: Option<&Path>, opts: &VideoOptions, progress: &Progress) -> Result<VideoOutcome> {
        let target = self.video_target(opts.size)?;
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
        let p = video::prepare(src, ffmpeg, target, info, opts.artwork, progress)?;
        let view = self.add_prepared_video(src, &p)?;
        Ok(VideoOutcome::Added { video: Box::new(view), converted: p.converted })
    }

    fn register_video(
        &mut self,
        source: &Path,
        original: &Path,
        title: &str,
        info: &VideoInfo,
        props: &VideoInfo,
        thumb: Option<Vec<u8>>,
    ) -> Result<VideoView> {
        let size = fs::metadata(source)?.len();
        if size > u32::MAX as u64 {
            return Err(Error::Unsupported(t!("4GB を超えるファイルは転送できません")));
        }
        let free = fs4::available_space(self.root()).unwrap_or(u64::MAX);
        if size + FREE_MARGIN > free {
            return Err(Error::NoSpace);
        }
        let (dest, ipod_path) = self.pick_destination("m4v")?;
        if let Err(e) = copy_file(source, &dest) {
            let _ = fs::remove_file(&dest);
            return Err(e.into());
        }

        let tags = &info.tags;
        let mut t = Track::new(self.db.new_dbid());
        t.set_string(MHOD_TITLE, Some(title));
        t.set_string(MHOD_ARTIST, tags.artist.as_deref());
        t.set_string(MHOD_ALBUM, tags.album.as_deref());
        t.set_string(MHOD_ALBUM_ARTIST, tags.album_artist.as_deref());
        t.set_string(MHOD_GENRE, tags.genre.as_deref());
        t.set_string(MHOD_DESCRIPTION, tags.description.as_deref());
        t.set_string(MHOD_TV_SHOW, tags.show.as_deref());
        t.set_string(MHOD_TV_EPISODE, tags.episode_id.as_deref());
        t.set_string(MHOD_TV_NETWORK, tags.network.as_deref());
        t.set_string(MHOD_SORT_TITLE, tags.sort_title.as_deref());
        t.set_string(MHOD_SORT_TV_SHOW, tags.sort_show.as_deref());
        t.set_string(MHOD_PATH, Some(&ipod_path));
        // libgpod と同じ (unk126 / unk144 は 0)
        t.set_string(MHOD_FILETYPE, Some("MPEG-4 video file"));
        t.set_filetype_marker(filetype_marker("m4v"));
        t.set_size(size as u32);
        t.set_length_ms(props.duration_ms.max(info.duration_ms));
        t.set_bitrate(props.bitrate_kbps);
        let rate = props.audio.as_ref().map(|a| a.sample_rate).filter(|&r| r > 0).unwrap_or(44100);
        t.set_samplerate(rate);
        t.set_samplecount(props.duration_ms as u64 * rate as u64 / 1000);
        t.set_year(tags.year);
        t.set_season(tags.season);
        t.set_episode(tags.episode);
        t.set_video_kind(VideoKind::from_tags(tags.media_kind, tags.show.is_some()));
        let now = now_unix();
        t.set_time_added_mac(unix_to_mac(now));
        let mtime = fs::metadata(original)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(now);
        t.set_time_modified_mac(unix_to_mac(mtime));
        if let Some(bytes) = thumb {
            // 画像の失敗で動画の転送は止めない
            match self.ensure_image(&bytes, t.dbid()) {
                Ok(id) => t.set_artwork(id, bytes.len() as u32),
                Err(e) => log::warn!("video artwork: {e}"),
            }
        }
        let kind = t.video_kind().unwrap_or_default();
        let view = video_view(&t, kind, &ArtIndex::new(self.art.as_ref()), false);
        self.db.tracks.push(t);
        self.dup_index = None;
        self.dirty = true;
        Ok(view)
    }

    /// 動画の情報を変える。変えた本数
    pub fn edit_videos(&mut self, ids: &[u64], e: &VideoEdit) -> Result<usize> {
        self.check_writable()?;
        let ids: HashSet<u64> = ids.iter().copied().collect();
        // 曲と同じ項目は曲の編集と同じ手順で (名前を変えたら読みを付け直す)
        let tags = TagEdit {
            title: e.title.clone(),
            artist: e.artist.clone(),
            album: e.album.clone(),
            album_artist: e.album_artist.clone(),
            genre: e.genre.clone(),
            composer: e.composer.clone(),
            year: e.year,
            track_nr: e.track_nr,
            disc_nr: e.disc_nr,
            sort_title: e.sort_title.clone(),
            sort_artist: e.sort_artist.clone(),
            sort_album: e.sort_album.clone(),
            sort_album_artist: e.sort_album_artist.clone(),
            rating: e.rating,
            volume: e.volume,
            start_ms: e.start_ms,
            stop_ms: e.stop_ms,
            eq: e.eq,
            ..Default::default()
        };
        // 「シャッフル時にスキップ」「再生位置を記憶」は種類を変えた後で書く
        let flags = TagEdit {
            skip_shuffle: e.skip_shuffle,
            remember_position: e.remember_position,
            ..Default::default()
        };
        let mut n = 0;
        for t in self.db.tracks.iter_mut().filter(|t| ids.contains(&t.dbid()) && t.video_kind().is_some()) {
            let text = |t: &mut Track, mhod: u32, v: &Option<String>| match v.as_deref().map(str::trim) {
                Some(v) if t.string(mhod).unwrap_or("") != v => {
                    t.set_string(mhod, Some(v));
                    true
                }
                _ => false,
            };
            let mut changed = apply_tag_edit(t, &tags);
            if text(t, MHOD_TV_SHOW, &e.show) {
                t.set_string(MHOD_SORT_TV_SHOW, yomi::reading(t.string(MHOD_TV_SHOW).unwrap_or("")).as_deref());
                changed = true;
            }
            changed |= text(t, MHOD_SORT_TV_SHOW, &e.sort_show);
            changed |= text(t, MHOD_SORT_COMPOSER, &e.sort_composer);
            changed |= text(t, MHOD_GROUPING, &e.grouping);
            changed |= text(t, MHOD_COMMENT, &e.comment);
            if let Some(r) = e.released.map(unix_to_mac).filter(|&r| r != t.time_released_mac()) {
                t.set_time_released_mac(r);
                changed = true;
            }
            if e.reset_play_count && (t.playcount() != 0 || t.playcount2() != 0 || t.last_played_mac() != 0) {
                t.set_playcount(0);
                t.set_playcount2(0);
                t.set_last_played_mac(0);
                changed = true;
            }
            changed |= text(t, MHOD_TV_EPISODE, &e.episode_id);
            changed |= text(t, MHOD_TV_NETWORK, &e.network);
            changed |= text(t, MHOD_DESCRIPTION, &e.description);
            if let Some(s) = e.season.filter(|&s| s != t.season()) {
                t.set_season(s);
                changed = true;
            }
            if let Some(ep) = e.episode.filter(|&ep| ep != t.episode()) {
                t.set_episode(ep);
                changed = true;
            }
            // 種類を変えると「シャッフルで飛ばす」「再生位置を覚える」は種類の既定に戻るので、指定はその後で
            if let Some(k) = e.kind {
                changed |= t.set_video_kind(k);
            }
            changed |= apply_play_options(t, &flags);
            if changed {
                n += 1;
            }
        }
        self.dirty |= n > 0;
        if n > 0 {
            self.dup_index = None;
        }
        Ok(n)
    }
}
