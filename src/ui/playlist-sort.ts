// プレイリストの並べ方 (配布リポジトリの Issue #22)。並び替えのパネル (playlist-reorder.ts) で並べ方を選んで保存すると、
// その並べ方をプレイリストごとに覚え (「あとから足した曲もこの並べ方で入れる」をオンにしたとき)、あとから曲を足したり
// 曲の情報を変えたりしても、その並べ方に保つ。ドラッグで動かして保存したら覚えた並べ方を外す (カスタム)。
// Rockbox の曲には確かな追加日が無い (ファイルの更新日時は元のファイルのものが写る) ので、Rockbox の「追加日」は
// プレイリストに足した順: 新しい順なら、足した曲を先頭に入れる

import { sortTracks } from "../app/rows";
import { editablePlaylist, isRockbox, writable } from "../app/selectors";
import { state } from "../app/state";
import type { Playlist, Snapshot, SortKey, Track } from "../domain/types";
import { getStored, setStored } from "../infra/prefs";
import { invoke } from "../infra/tauri";

export type PlaylistSort = { key: SortKey; asc: boolean };

const PREF = "playlistSort";

function stored(): Record<string, PlaylistSort> {
  try {
    const v = JSON.parse(getStored(PREF) ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

/** プレイリストの覚えた並べ方 (無ければカスタム = null) */
export function playlistSort(id: string): PlaylistSort | null {
  return stored()[id] ?? null;
}

/** プレイリストの並べ方を覚える (null ならカスタム = 足した曲は後ろに付けるだけ) */
export function rememberPlaylistSort(id: string, sort: PlaylistSort | null) {
  const all = stored();
  if (sort) all[id] = sort;
  else delete all[id];
  setStored(PREF, Object.keys(all).length ? JSON.stringify(all) : null);
}

/** Rockbox の「追加日」(プレイリストに足した順) か */
function byAddition(key: SortKey): boolean {
  return key === "dateAdded" && isRockbox();
}

/** 並べ方どおりの曲の ID の順。見つからない曲 (読み込み中など) は後ろに元の順のまま */
function sortedIds(ids: string[], sort: PlaylistSort): string[] {
  const s = state.snap;
  const tracks: Track[] = [];
  const missing: string[] = [];
  for (const id of ids) {
    const t = state.trackMap.get(id);
    if (t) tracks.push(t);
    else missing.push(id);
  }
  sortTracks(tracks, sort.key, sort.asc, s?.tracks ?? [], "ipod", false);
  return [...tracks.map((t) => t.id), ...missing];
}

/** `sort` で並べた曲の ID の順 (並び替えのパネルの下書き)。Rockbox の「追加日」は、保存してあるプレイリストの順を
 * 足した順とみなす (覚えた並べ方が「追加日 ↓」なら今の順が新しい順なので、その逆が古い順) */
export function sortedOrder(pl: Playlist, sort: PlaylistSort): string[] {
  if (byAddition(sort.key)) {
    const prev = playlistSort(pl.id);
    const savedDesc = prev?.key === "dateAdded" && !prev.asc;
    return savedDesc === !sort.asc ? pl.trackIds.slice() : pl.trackIds.slice().reverse();
  }
  return sortedIds(pl.trackIds, sort);
}

async function reorder(pl: Playlist, order: string[]): Promise<Snapshot | null> {
  if (order.join("\u0000") === pl.trackIds.join("\u0000")) return null;
  return invoke<Snapshot>("reorder_playlist", { id: pl.id, trackIds: order });
}

/** 動いている並べ直し (同時に 2 つ動かさない) */
let keeping = false;

/**
 * 覚えた並べ方があるプレイリストの順を、その並べ方に保つ (新しいスナップショットを受け取ったあと)。
 * 曲を足した・曲の情報を変えたときに並べ直す。`prev` は前のスナップショットのプレイリスト (Rockbox で足した曲を見分ける)。
 * 並べ直したスナップショットを `apply` に渡す (受け取った側でまた呼ばれるが、もう並んでいるので何もしない)
 */
export function keepPlaylistsSorted(prev: Playlist[] | null, apply: (s: Snapshot) => void) {
  const s = state.snap;
  if (keeping || !s || !writable()) return;
  const all = stored();
  const before = new Map((prev ?? []).map((p) => [p.id, p]));
  const work: [Playlist, string[]][] = [];
  for (const p of s.playlists) {
    const sort = all[p.id];
    // 並び替えのパネルで編集中のプレイリストは、保存するまで触らない
    if (!sort || !editablePlaylist(p.id) || state.reorder?.id === p.id) continue;
    let order: string[];
    if (byAddition(sort.key)) {
      // 古い順はそのまま (Rockbox は足した曲を後ろに付ける)。新しい順は、前に無かった曲を先頭へ
      const old = before.get(p.id);
      if (sort.asc || !old) continue;
      const had = new Set(old.trackIds);
      const added = p.trackIds.filter((id) => !had.has(id));
      if (!added.length) continue;
      order = [...added, ...p.trackIds.filter((id) => had.has(id))];
    } else {
      order = sortedIds(p.trackIds, sort);
    }
    if (order.join("\u0000") !== p.trackIds.join("\u0000")) work.push([p, order]);
  }
  if (!work.length) return;
  keeping = true;
  (async () => {
    let last: Snapshot | null = null;
    for (const [p, order] of work) {
      try {
        last = (await reorder(p, order)) ?? last;
      } catch (e) {
        // ほかの作業中などで書けなければ、次のスナップショットでまた並べ直す
        console.warn("playlist sort", e);
      }
    }
    keeping = false;
    if (last) apply(last);
  })();
}
