// 状態から決まる値: 今の表示の種類・今のプレイリスト・書き込めるか・Rockbox か

import { state } from "./state";
import type { MediaKind, Playlist, Snapshot, Track } from "../domain/types";
import { getStored } from "../infra/prefs";

/** サイドバーのライブラリの、分類ごとの表示 (iPod の曲のうちその分類のもの)。"library" は「すべて」(全曲) */
export const KIND_VIEWS: Record<string, MediaKind> = { music: "music", podcasts: "podcast", audiobooks: "audiobook" };

/** 分類ごとの表示なら、その分類 */
export function viewKind(): MediaKind | null {
  return Object.prototype.hasOwnProperty.call(KIND_VIEWS, state.view) ? KIND_VIEWS[state.view] : null;
}

/** iPod の全曲から選ぶ表示 (すべて・ミュージック・ポッドキャスト・オーディオブック)。プレイリストでも PC の曲でもない */
export function isLibraryView(): boolean {
  return state.view === "library" || viewKind() !== null;
}

export function currentPlaylist(): Playlist | null {
  if (!state.snap || isLibraryView()) return null;
  return state.snap.playlists.find((p) => p.id === state.view) ?? null;
}

export function editablePlaylist(id: string): Playlist | null {
  const p = state.snap?.playlists.find((x) => x.id === id);
  // ポッドキャストのプレイリストは分類から自動で作るので、手で曲を足したり外したりしない
  return p && !p.master && !p.smart && !p.podcast ? p : null;
}

export function writable(): boolean {
  return !!state.snap && !state.snap.writableError && !state.partialOpen;
}

/** Rockbox モードで開いているか (iTunesDB を使わず、ファイルをそのまま扱う) */
export function isRockbox(): boolean {
  return state.snap?.mode === "rockbox";
}

// ---------------- 並べ替え (アルバム表示・アーティスト表示) ----------------

export type AlbumSort = "artist" | "album" | "year" | "added";
export type ArtistSort = "name" | "count";
/** アルバム表示とアーティスト表示の並べ方 (次に開いたときも同じ)。desc は逆順 */
export function albumSort(): { key: AlbumSort; desc: boolean } {
  const k = getStored("albumSort") as AlbumSort | null;
  return { key: k && ["artist", "album", "year", "added"].includes(k) ? k : "artist", desc: getStored("albumSortDesc") === "1" };
}
export function artistSort(): { key: ArtistSort; desc: boolean } {
  const k = getStored("artistSort") as ArtistSort | null;
  return { key: k === "count" ? "count" : "name", desc: getStored("artistSortDesc") === "1" };
}

/** 「PC」の「すべて」(登録フォルダの曲を全部) の表示 */
export const PC_ALL = "pcall";

export function isPcView(): boolean {
  return state.view.startsWith("pc:") || state.view === PC_ALL;
}
export function isPcAll(): boolean {
  return state.view === PC_ALL;
}
/** 1 つのフォルダを表示中ならその場所 (「すべて」では null) */
export function currentPcFolder(): string | null {
  return state.view.startsWith("pc:") ? state.view.slice(3) : null;
}
/** 表示中の曲が入っているフォルダ (「すべて」なら登録フォルダ全部) */
export function currentPcFolders(): string[] {
  if (isPcAll()) return state.pc.folders;
  const f = currentPcFolder();
  return f ? [f] : [];
}
/** 表示中の PC の曲。フォルダの中に別の登録フォルダがあっても同じ曲は 1 回だけ */
export function pcPool(): Track[] {
  const seen = new Set<string>();
  const out: Track[] = [];
  for (const f of currentPcFolders())
    for (const t of state.pc.tracks.get(f) ?? []) {
      if (seen.has(t.id)) continue;
      seen.add(t.id);
      out.push(t);
    }
  return out;
}

/**
 * iPod に同じ曲があるか (転送時の重複チェックと同じ条件: 曲名・アーティスト・アルバム・ディスク/トラック番号・長さ ±2 秒)。
 * スクロールのたびに作り直さないよう、iPod の曲データ (スナップショット) ごとに 1 回だけ作る
 */
let onIpodCache: { snap: Snapshot | null; fn: (t: Track) => boolean } | null = null;
export function onIpodIndex(): (t: Track) => boolean {
  if (onIpodCache?.snap === state.snap) return onIpodCache.fn;
  const norm = (x: string) => x.trim().toLowerCase();
  const key = (t: Track) => `${norm(t.title)}\u0000${norm(t.artist)}\u0000${norm(t.album)}`;
  const byKey = new Map<string, Track[]>();
  for (const t of state.snap?.tracks ?? []) {
    const k = key(t);
    const list = byKey.get(k);
    if (list) list.push(t);
    else byKey.set(k, [t]);
  }
  // 番号がどちらにもあって違えば別の曲 (シングル版とアルバム版はタグが同じで番号だけ違うことがある)
  const sameNr = (a: number, b: number) => !a || !b || a === b;
  // 並べ替えの比べ方の中で何度も呼ばれるので、曲ごとに答えを覚えておく (iPod を読み直したら作り直す)
  const memo = new WeakMap<Track, boolean>();
  const fn = (t: Track) => {
    let v = memo.get(t);
    if (v === undefined) {
      v = (byKey.get(key(t)) ?? []).some(
        (x) => Math.abs(x.durationMs - t.durationMs) < 2000 && sameNr(x.discNr, t.discNr) && sameNr(x.trackNr, t.trackNr),
      );
      memo.set(t, v);
    }
    return v;
  };
  onIpodCache = { snap: state.snap, fn };
  return fn;
}
