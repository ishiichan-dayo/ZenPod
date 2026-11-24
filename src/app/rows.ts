// 今の表示で一覧に出す行 (絞り込み・並べ替え・アーティストごとのまとまり・アルバムのまとまり)

import { t as tr } from "../i18n";
import { isPodView } from "./podcasts";
import { isVisualView } from "./visual";
import { albumSort, artistSort, currentPlaylist, isPcView, isRockbox, onIpodIndex, pcPool, viewKind } from "./selectors";
import { ALBUM_GAP, ALBUM_HEAD_H, ARTIST_HEAD_H, ROW_H, state } from "./state";
import type { RowLayout } from "./state";
import { NUM_KEY, albumKey, artistKey, haystack, ranks, releaseGroups, releaseRank } from "../domain/track-order";
import type { RankKey } from "../domain/track-order";
import type { SortKey, Track } from "../domain/types";

/** 開いているアルバムの曲 (名前を変えてもそのアルバムを開いたままにするため) */
let openAlbumIds = new Set<string>();

export function computeRows() {
  if (isPodView() || isVisualView() || state.devSkeleton) {
    state.rows = [];
    state.rowLayout = null;
    return;
  }
  const s = state.snap;
  const pl = currentPlaylist();
  const pc = isPcView();
  const pool = pc ? pcPool() : (s?.tracks ?? []);
  const poolId = pc ? state.view : "ipod";
  let rows: Track[];
  if (pl && !pl.master) {
    // 並び替えのパネルで編集中なら下書きの順
    const ids = state.reorder?.id === pl.id ? state.reorder.ids : pl.trackIds;
    rows = ids.map((id) => state.trackMap.get(id)).filter((t): t is Track => !!t);
  } else if (viewKind()) {
    const kind = viewKind();
    rows = pool.filter((t) => (t.kind ?? "music") === kind);
  } else {
    rows = pool.slice();
  }
  if (state.layout === "albums" && state.openAlbum !== null) {
    let open = rows.filter((t) => albumKey(t) === state.openAlbum);
    // 開いているアルバムの名前などを変えたら、同じ曲の新しいアルバムを開いたままにする (空の一覧にしない)
    if (!open.length) {
      const t = rows.find((x) => openAlbumIds.has(x.id));
      if (t) {
        state.openAlbum = albumKey(t);
        open = rows.filter((x) => albumKey(x) === state.openAlbum);
      }
    }
    openAlbumIds = new Set(open.map((t) => t.id));
    rows = open;
  }
  const q = state.search.trim().toLowerCase();
  if (q) {
    const terms = q.split(/\s+/);
    rows = rows.filter((t) => {
      const hay = haystack(t);
      return terms.every((w) => hay.includes(w));
    });
  }
  state.rowLayout = null;
  if (state.layout === "artists") {
    layoutArtists(rows, pool, poolId);
    return;
  }
  const { key, asc } = state.sort;
  if (key !== "order") sortTracks(rows, key, asc, pool, poolId, pc);
  else if (!asc) rows.reverse();
  state.rows = rows;
}

/** 曲を並べ替える (その場で)。`pool` は順位を数える曲の全体 (iPod の曲か PC の曲)。
 * 同じ値の曲は元の順のまま (並べ直しても順が変わらない。プレイリストの順を並べ方に合わせ続けるときに使う) */
export function sortTracks(rows: Track[], key: SortKey, asc: boolean, pool: Track[], poolId: string, pc: boolean) {
  const num = NUM_KEY[key];
  const main = num || key === "onIpod" ? null : ranks(key as RankKey, pool, poolId);
  const album = key === "title" ? null : ranks("album", pool, poolId);
  const title = key === "title" ? null : ranks("title", pool, poolId);
  const onIpod = pc && key === "onIpod" ? onIpodIndex() : null;
  const release = pc ? releaseGroups(pool) : null;
  rows.sort((a, b) => {
    let r = 0;
    if (num) r = num(a) - num(b);
    // PC の曲: iPod に転送済みか。未転送の曲を先に並べる
    else if (onIpod) r = Number(onIpod(a)) - Number(onIpod(b));
    else if (main) r = main.get(a)! - main.get(b)!;
    if (r === 0 && album && title) {
      r =
        album.get(a)! - album.get(b)! ||
        (release ? release(a, b) : 0) ||
        a.discNr - b.discNr ||
        a.trackNr - b.trackNr ||
        title.get(a)! - title.get(b)!;
    }
    return asc ? r : -r;
  });
}

/**
 * アーティスト表示: `rows` (検索で絞り込んだあと) をアーティストでまとめて左の一覧にし、
 * 選んだアーティストの曲をアルバムごと (新しい順) に並べて state.rows にする
 */
function layoutArtists(rows: Track[], pool: Track[], poolId: string) {
  const by = new Map<string, Track[]>();
  for (const t of rows) {
    const k = artistKey(t);
    const list = by.get(k);
    if (list) list.push(t);
    else by.set(k, [t]);
  }
  // iPod と同じく読みの順。アーティスト名の無い曲は最後
  const rank = ranks("artist", pool, poolId);
  const first = (list: Track[]) => Math.min(...list.map((t) => rank.get(t) ?? 0));
  const as = artistSort();
  const adir = as.desc ? -1 : 1;
  state.artists = [...by.entries()]
    .map(([key, tracks]) => ({ key, tracks, r: first(tracks) }))
    .sort((a, b) => Number(!a.key) - Number(!b.key) || adir * (as.key === "count" ? a.tracks.length - b.tracks.length || a.r - b.r : a.r - b.r))
    .map(({ key, tracks }) => ({ key, tracks }));
  let cur = state.artists.find((a) => a.key === state.artist);
  if (!cur) {
    cur = state.artists[0];
    state.artist = cur?.key ?? null;
  }
  if (!cur) {
    state.rows = [];
    return;
  }

  // アルバムごとに分けて、新しいアルバムから。同じ年ならアルバム → EP → シングル、名前順
  const albumRank = ranks("album", pool, poolId);
  const albums = new Map<string, Track[]>();
  for (const t of cur.tracks) {
    const k = albumKey(t);
    const list = albums.get(k);
    if (list) list.push(t);
    else albums.set(k, [t]);
  }
  const groups = [...albums.values()].map((tracks) => {
    tracks.sort((a, b) => a.discNr - b.discNr || a.trackNr - b.trackNr);
    return {
      tracks,
      year: Math.max(...tracks.map((t) => t.year)),
      kind: Math.min(...tracks.map(releaseRank)),
      name: albumRank.get(tracks[0]) ?? 0,
    };
  });
  groups.sort(
    (a, b) =>
      Number(!a.tracks[0].album) - Number(!b.tracks[0].album) || b.year - a.year || a.kind - b.kind || a.name - b.name,
  );

  const out: Track[] = [];
  const tops: number[] = [];
  const blocks: RowLayout["blocks"] = [{ top: 0, h: ARTIST_HEAD_H, kind: "artist", tracks: cur.tracks }];
  let y = ARTIST_HEAD_H;
  for (const [n, g] of groups.entries()) {
    if (n > 0) y += ALBUM_GAP;
    blocks.push({ top: y, h: ALBUM_HEAD_H, kind: "album", tracks: g.tracks });
    y += ALBUM_HEAD_H;
    for (const t of g.tracks) {
      out.push(t);
      tops.push(y);
      y += ROW_H;
    }
  }
  state.rows = out;
  state.rowLayout = { tops, height: y + 12, blocks };
}

/** 行 i の top (アーティスト表示では見出しの分ずれる) */
export function rowTop(i: number): number {
  return state.rowLayout ? (state.rowLayout.tops[i] ?? 0) : i * ROW_H;
}

/** 高さ y の位置にある (か、その直前の) 行の番号 */
export function rowAtY(y: number): number {
  const tops = state.rowLayout?.tops;
  if (!tops) return Math.floor(y / ROW_H);
  let lo = 0;
  let hi = tops.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (tops[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

export interface AlbumGroup {
  key: string;
  album: string;
  artist: string;
  tracks: Track[];
  /** リリースの種類の順位 (releaseRank: 0 = アルバム、1 = EP、2 = シングル) */
  rank: number;
}

/** 表示中の曲 (検索で絞り込んだあと) をアルバムごとにまとめる。アーティスト → アルバム名の順 */
let albumGroupsCache: { rows: Track[]; sort: string; groups: AlbumGroup[] } | null = null;
export function albumGroups(): AlbumGroup[] {
  const sortSig = `${JSON.stringify(albumSort())}${isPcView() || isRockbox()}`;
  if (albumGroupsCache?.rows === state.rows && albumGroupsCache.sort === sortSig) return albumGroupsCache.groups;
  const byKey = new Map<string, AlbumGroup>();
  for (const t of state.rows) {
    const k = albumKey(t);
    let g = byKey.get(k);
    if (!g) {
      g = { key: k, album: t.album, artist: "", tracks: [], rank: 3 };
      byKey.set(k, g);
    }
    g.tracks.push(t);
  }
  const cmp = new Intl.Collator("ja", { numeric: true, sensitivity: "base" }).compare;
  const groups = [...byKey.values()];
  for (const g of groups) {
    g.tracks.sort((a, b) => a.discNr - b.discNr || a.trackNr - b.trackNr);
    const artists = new Set(g.tracks.map((t) => t.albumArtist || t.artist));
    g.artist = artists.size > 1 ? tr("複数のアーティスト") : ([...artists][0] ?? "");
    // 1 曲だけ MEDIATYPE が違うことがあるので、一番アルバムらしい曲の種類をアルバムの種類にする
    g.rank = Math.min(...g.tracks.map(releaseRank));
  }
  const sortName = (g: AlbumGroup) => {
    const t = g.tracks[0];
    return [t.sortAlbumArtist || t.albumArtist || t.sortArtist || t.artist, t.sortAlbum || t.album];
  };
  const { key: sortKey, desc } = albumSort();
  // 追加日の無い曲 (PC・Rockbox) はアーティスト順 (ボタンの表示と同じ)
  const sk = sortKey === "added" && (isPcView() || isRockbox()) ? "artist" : sortKey;
  const dir = desc ? -1 : 1;
  const year = (g: AlbumGroup) => Math.max(...g.tracks.map((t) => t.year));
  const added = (g: AlbumGroup) => Math.max(...g.tracks.map((t) => t.dateAdded));
  groups.sort((a, b) => {
    const [aa, al] = sortName(a);
    const [ba, bl] = sortName(b);
    // アルバム名の無い曲は、どの並べ方でも最後にまとめる
    // 同じ名前ならアルバム → EP → シングルの順。種類が同じ (分からない) なら曲の多い方を先に
    const main =
      sk === "album"
        ? cmp(al, bl) || cmp(aa, ba)
        : sk === "year"
          ? year(a) - year(b) || cmp(aa, ba) || cmp(al, bl)
          : sk === "added"
            ? added(a) - added(b) || cmp(aa, ba) || cmp(al, bl)
            : cmp(aa, ba) || cmp(al, bl);
    return Number(!a.album) - Number(!b.album) || dir * main || a.rank - b.rank || b.tracks.length - a.tracks.length;
  });
  albumGroupsCache = { rows: state.rows, sort: sortSig, groups };
  return groups;
}
