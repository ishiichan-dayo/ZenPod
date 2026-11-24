// ポッドキャストの番組と回 (Rust の online/podcast.rs の Feed / Episode に合わせる)・決まった値・表示の書式

import { fmtDay, fmtDur } from "./format";

// ---------------- 転送 ----------------

/** 設定 (未設定なら ON) */
// ---------------- ポッドキャストの購読 ----------------

export interface PodEpisode {
  id: string;
  guid: string;
  title: string;
  /** 公開日時 (UNIX 秒、0 = 不明) */
  published: number;
  durationMs: number;
  url: string;
  length: number;
  mime: string;
  description: string;
  /** 回ごとの画像の URL (無ければ空。番組の画像を使う) */
  imageUrl: string;
  /** 再生済み (iPod で再生した・手で印を付けた) */
  played: boolean;
  /** 印を最後に変えたときの iPod の再生回数 (これより増えたら iPod で聞いたことにする) */
  playsSeen: number;
  /** 取得したファイルの名前 (無ければ null) */
  file: string | null;
  tagAlbum: string | null;
  tagTitle: string | null;
  firstSeen: number;
}

export interface PodFeed {
  id: string;
  url: string;
  title: string;
  author: string;
  description: string;
  imageUrl: string;
  link: string;
  added: number;
  checked: number;
  /** 同期で iPod に置いておく、聞いていない新しい回の数 (0 = 自動では送らない) */
  keep: number;
  /** 置いておく回を古い方から選ぶ (続きものの番組を #1 から順に聞くとき) */
  keepOldest: boolean;
  /** 保存したときの形の版 (Rust の META_VERSION より古ければ、次の更新で本文ごと取り直す) */
  metaVersion: number;
  /** 新しい順 */
  episodes: PodEpisode[];
}

export interface PodDownloadResult {
  /** 取得した (かもう取得してあった) 回。頼んだ順 */
  done: { epId: string; path: string }[];
  errors: string[];
  cancelled: boolean;
}

/** 同期で iPod に置いておく回の数の選択肢 */
export const POD_KEEP_CHOICES = [1, 3, 5, 10];
/** 保存の形の版 (Rust 側の podcast::META_VERSION と同じ) */
export const POD_META_VERSION = 1;
/** 「送らない」から戻したときの数 (Rust 側の DEFAULT_KEEP と同じ) */
export const POD_DEFAULT_KEEP = 3;
/** 起動時にフィードを確かめ直すのは、前回からこれだけたった番組だけ (秒) */
export const POD_REFRESH_AGE = 6 * 60 * 60;
export function podKey(album: string, title: string): string {
  return `${album.trim().toLowerCase()}\u0000${title.trim().toLowerCase()}`;
}

export function epVideo(e: PodEpisode): boolean {
  const ext = /\.([a-z0-9]+)(?:[?#]|$)/i.exec(e.url)?.[1]?.toLowerCase();
  return e.mime.startsWith("video/") || (!e.mime.startsWith("audio/") && (ext === "mp4" || ext === "m4v" || ext === "mov"));
}
/** iPod をつないだときに確かめる番組は、前回からこれだけたったものだけフィードを取り直す (秒) */
export const POD_CONNECT_REFRESH_AGE = 30 * 60;

/** 回の長さ (1 時間を超えたら "1:02:03") */
export function fmtEpDur(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 3600) return fmtDur(ms);
  return `${Math.floor(s / 3600)}:${String(Math.floor(s / 60) % 60).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

export function epDate(e: PodEpisode): string {
  return fmtDay(e.published);
}

export function epArtKey(f: PodFeed, e: PodEpisode): string {
  // 回の画像が番組の画像と同じなら番組の画像 (同じものを何度も取らない)
  const url = e.imageUrl && e.imageUrl !== f.imageUrl ? e.imageUrl : "";
  return `${f.id}\u0001${url}`;
}
