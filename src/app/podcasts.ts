// ポッドキャストの購読の状態 (番組の一覧・選んだ回) と、そこから決まる値 (今の番組・iPod に入っている回か・聞いたか)

import { state } from "./state";
import { podKey } from "../domain/podcast";
import type { PodEpisode, PodFeed } from "../domain/podcast";
import type { Snapshot, Track } from "../domain/types";

export const pod = {
  feeds: [] as PodFeed[],
  /** 番組の画像 (無ければ null)。読み込み中は undefined */
  covers: new Map<string, { small: string; large: string } | null>(),
  /** 取得中の回 (回の ID → 0〜1)。取得を待っている回は -1 */
  downloading: new Map<string, number>(),
  /** フィードを確かめている番組 */
  refreshing: new Set<string>(),
  /** 一覧でチェックした回 (表示中の番組の中だけ) */
  selected: new Set<string>(),
  /** 取得したファイルの場所 → 回の ID (転送中の印に使う) */
  paths: new Map<string, string>(),
  /** 送り終えたばかりの回 (緑の iPod をポンと出す。1 回描いたら消す) */
  arrived: new Set<string>(),
  /** 同期の途中 (取得・転送・削除) */
  syncing: false,
};

export function isPodView(): boolean {
  return state.view.startsWith("pod:");
}

export function currentFeed(): PodFeed | null {
  return isPodView() ? (pod.feeds.find((f) => f.id === state.view.slice(4)) ?? null) : null;
}

/** iPod のポッドキャストの曲を、アルバム (番組名) と曲名で引く表。iPod の曲データごとに 1 回だけ作る */
let podIndexCache: { snap: Snapshot | null; map: Map<string, Track> } | null = null;
function podIndex(): Map<string, Track> {
  if (podIndexCache?.snap === state.snap) return podIndexCache.map;
  const map = new Map<string, Track>();
  for (const t of state.snap?.tracks ?? []) if (t.kind === "podcast") map.set(podKey(t.album, t.title), t);
  podIndexCache = { snap: state.snap, map };
  return map;
}

/** iPod にある回 (分類がポッドキャストで、取得したときに書いた番組名と回の名前が同じ曲) */
export function epOnIpod(f: PodFeed, e: PodEpisode): Track | undefined {
  if (!state.snap || state.partialOpen) return undefined;
  return podIndex().get(podKey(e.tagAlbum ?? f.title, e.tagTitle ?? e.title));
}

/** 再生済みか。手で印を変えたあとは、iPod の再生回数がそれより増えたときだけ再生済みにする */
export function epPlayed(f: PodFeed, e: PodEpisode): boolean {
  return e.played || (epOnIpod(f, e)?.playCount ?? 0) > (e.playsSeen ?? 0);
}

/** 登録してから出た、聞いていない回の数 (サイドバーに出す) */
export function newEpisodeCount(f: PodFeed): number {
  return f.episodes.filter((e) => e.firstSeen > f.added && !epPlayed(f, e)).length;
}
