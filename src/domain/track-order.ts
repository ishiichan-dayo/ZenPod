// 曲の並べ替えと検索のキー。文字列の順位は Collator で 1 回だけ数えて覚える (strOrder)

import type { SortKey, Track } from "./types";

export type RankKey = "title" | "artist" | "album" | "albumArtist" | "genre" | "composer" | "filetype" | "fileName";

/** 並べ替えに使う文字列。iPod 本体と同じく、ソート用の読みがあればそれで並べる */
const RANK_STR: Record<RankKey, (t: Track) => string> = {
  title: (t) => t.sortTitle || t.title,
  artist: (t) => t.sortAlbumArtist || t.albumArtist || t.sortArtist || t.artist,
  album: (t) => t.sortAlbum || t.album,
  albumArtist: (t) => t.sortAlbumArtist || t.albumArtist || t.sortArtist || t.artist,
  genre: (t) => t.genre,
  composer: (t) => t.composer,
  filetype: (t) => fileKind(t),
  fileName: (t) => fileNameOf(t),
};

/**
 * 一覧の「ファイル名」の列: PC の曲と Rockbox モードの曲はファイル名 (拡張子込み)。
 * iPod OS の曲は iTunesDB が付けた名前 (F00/ABCD.m4a) で意味が無いので出さない
 */
export function fileNameOf(t: Track): string {
  const p = t.pcPath ?? t.rbPath ?? "";
  return p.slice(Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")) + 1);
}

/** 数値で並べる列 */
export const NUM_KEY: Partial<Record<SortKey, (t: Track) => number>> = {
  year: (t) => t.year,
  // 曲順 (ディスク番号 → トラック番号)。同じ番号の曲はアルバム → 曲名の順
  trackNr: (t) => t.discNr * 10000 + t.trackNr,
  duration: (t) => t.durationMs,
  size: (t) => t.size,
  bitrate: (t) => t.bitrate,
  dateAdded: (t) => t.dateAdded,
  plays: (t) => t.playCount,
  rating: (t) => t.rating,
  mediaKind: (t) => ({ music: 0, podcast: 1, audiobook: 2 })[t.kind ?? "music"],
  // 無い曲を先に並べる
  artwork: (t) => Number(hasArtwork(t)),
};

/** 曲にアートワークがあるか。PC の曲はファイルに埋め込まれているか (同じフォルダの cover.jpg などは数えない)、
 * iPod の曲は iPod に画像があるか */
export function hasArtwork(t: Track): boolean {
  return t.pcPath ? !!t.hasPicture : t.artworkId > 0;
}

/** 一覧の「種類」の列: iPod の曲は iTunesDB の種類名を短くしたもの、PC の曲は拡張子 */
export function fileKind(t: Track): string {
  if (t.pcPath) return (t.pcPath.match(/\.([^.\\/]+)$/)?.[1] ?? "").toUpperCase();
  const f = t.filetype;
  if (/lossless/i.test(f)) return "ALAC";
  if (/aac|mpeg-4/i.test(f)) return "AAC";
  if (/mpeg/i.test(f)) return "MP3";
  if (/wav/i.test(f)) return "WAV";
  if (/aiff/i.test(f)) return "AIFF";
  return f;
}

/**
 * 曲 → 並び順 (整数)。Collator での比較は重いので、スナップショットごとに
 * 使うキーの分だけ 1 回並べて順位にしておき、以降の並べ替えは整数の比較で済ませる。
 * Collator で等しい文字列は同じ順位にする
 */
export const rankCache = new Map<string, Map<Track, number>>();
let searchText = new WeakMap<Track, string>();

/** 検索用の文字列を作り直させる (iPod を開き直したとき) */
export function resetSearchText() {
  searchText = new WeakMap();
}

const rankCollator = new Intl.Collator("ja", { numeric: true, sensitivity: "base" });

/**
 * これまでに並べた文字列の順番 (項目ごと)。曲の情報は操作のたびに新しいオブジェクトで届くが、文字列の順番は変わらない。
 * 覚えておいて、新しい文字列が来たときだけ差し込む (2 万曲の曲名を毎回並べ直すと、操作のたびに 0.1 秒以上かかる)
 */
const strOrder = new Map<RankKey, { sorted: string[]; rank: Map<string, number> }>();

function stringRanks(key: RankKey, strings: Iterable<string>): Map<string, number> {
  const cmp = rankCollator.compare;
  let o = strOrder.get(key);
  const fresh = [...new Set(strings)].filter((k) => !o?.rank.has(k));
  if (o && !fresh.length) return o.rank;
  // 新しい文字列だけ並べて、並べ済みの列と合わせる
  fresh.sort(cmp);
  const old = o?.sorted ?? [];
  const sorted: string[] = [];
  let i = 0;
  let j = 0;
  while (i < old.length || j < fresh.length) {
    if (j >= fresh.length || (i < old.length && cmp(old[i], fresh[j]) <= 0)) sorted.push(old[i++]);
    else sorted.push(fresh[j++]);
  }
  // 並べ方で同じとみなす文字列 (大文字小文字・全角半角の違いなど) は同じ順位
  const rank = new Map<string, number>();
  let r = 0;
  sorted.forEach((k, n) => {
    if (n > 0 && cmp(sorted[n - 1], k) !== 0) r++;
    rank.set(k, r);
  });
  o = { sorted, rank };
  strOrder.set(key, o);
  return rank;
}

/** `pool` は並べる対象 (iPod の曲 or PC のフォルダの曲)。`poolId` でキャッシュを分ける */
export function ranks(key: RankKey, pool: Track[], poolId: string): Map<Track, number> {
  const cacheKey = `${poolId}\u0000${key}`;
  let m = rankCache.get(cacheKey);
  if (m) return m;
  const str = RANK_STR[key];
  const rank = stringRanks(key, pool.map(str));
  m = new Map();
  for (const t of pool) m.set(t, rank.get(str(t))!);
  rankCache.set(cacheKey, m);
  return m;
}

export function haystack(t: Track): string {
  let h = searchText.get(t);
  if (h === undefined) {
    h = `${t.title}\n${t.artist}\n${t.album}\n${t.albumArtist}\n${t.genre}`.toLowerCase();
    searchText.set(t, h);
  }
  return h;
}

export function artistKey(t: Track): string {
  return t.albumArtist || t.artist;
}

/**
 * PC の曲が属するリリース (アルバム・シングル) のフォルダ。同じフォルダの曲を 1 つのリリースとみなす。
 * 「Disc 1」「CD2」のようなディスクごとのフォルダは 1 つ上のフォルダにまとめる。iPod の曲は ""
 */
/** 並べ替えの比べ方の中で何十万回も呼ばれるので、曲ごとに覚えておく */
const releaseDirCache = new WeakMap<Track, string>();

export function releaseDir(t: Track): string {
  const hit = releaseDirCache.get(t);
  if (hit !== undefined) return hit;
  const parts = (t.pcPath ?? "").split(/[\\/]/);
  parts.pop();
  // tr: 訳さない (フォルダ名の「ディスク 1」などを見分ける)
  if (parts.length > 1 && /^(cd|disc|disk|ディスク)\s*\d+$/i.test(parts[parts.length - 1].trim())) parts.pop();
  const dir = parts.join("/");
  releaseDirCache.set(t, dir);
  return dir;
}

/**
 * PC の曲で、同じアルバム名の中の並び: フォルダ (= 1 つのリリース) ごとにまとめ、
 * アルバム → EP → シングル → その他のフォルダの順に並べる。
 * シングルのタグにアルバム名が入っていても、アルバムの曲順に混ざらないように。
 * MEDIATYPE は 1 曲ごとだと当てにならない (アルバムの中の 1 曲だけ single のことがある) ので、
 * フォルダの中で一番アルバムらしい曲の種類をそのフォルダの種類にする
 */
export function releaseGroups(pool: Track[]): (a: Track, b: Track) => number {
  const dir = releaseDir;
  const rankOf = new Map<string, number>();
  for (const t of pool) {
    const d = dir(t);
    rankOf.set(d, Math.min(rankOf.get(d) ?? 3, releaseRank(t)));
  }
  return (a, b) => {
    const da = dir(a);
    const db = dir(b);
    if (da === db) return 0;
    return rankOf.get(da)! - rankOf.get(db)! || (da < db ? -1 : 1);
  };
}

export function releaseRank(t: Track): number {
  switch (t.releaseType ?? "") {
    case "":
    case "album":
      return 0;
    case "ep":
      return 1;
    case "single":
      return 2;
    default:
      return 3;
  }
}

// ---------------- アルバム表示 ----------------

/**
 * アルバムの見分け方。iPod の曲はアルバム名 + アルバムアーティスト (無ければアーティスト)。
 * PC の曲はアルバム名 + フォルダ (シングルのタグに誤ってアルバム名が入っていても別のアルバムになる)
 */
export function albumKey(t: Track): string {
  return t.pcPath ? `${t.album}\u0000${releaseDir(t)}` : `${t.album}\u0000${t.albumArtist || t.artist}`;
}
