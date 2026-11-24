// サイドバーのプレイリストの操作 (並べ替え・名前の変更・削除・名前順)

import { t as tr } from "../i18n";
import { isRockbox, writable } from "../app/selectors";
import { $, state } from "../app/state";
import type { Playlist, Snapshot } from "../domain/types";
import { getStored, setStored } from "../infra/prefs";
import { call } from "./feedback";
import { confirmBox, promptText } from "./modal";
import { reorderPcFolders } from "./pc";
import { applySnapshot } from "./render";
import { renderSidebar } from "./sidebar";

export async function renamePlaylist(id: string) {
  const pl = state.snap?.playlists.find((p) => p.id === id);
  if (!pl) return;
  await promptText(tr("プレイリスト名を変更"), pl.name, tr("保存"), async (name) => {
    if (name === pl.name) return true;
    const s = await call<Snapshot>("rename_playlist", { id, name });
    if (s) applySnapshot(s);
    return !!s;
  });
}

export async function deletePlaylist(id: string) {
  const pl = state.snap?.playlists.find((p) => p.id === id);
  if (!pl) return;
  const ok = await confirmBox(tr("「{a}」を削除しますか？", { a: pl.name }), tr("プレイリストだけが削除され、曲は iPod に残ります。"), tr("削除"), true);
  if (!ok) return;
  const s = await call<Snapshot>("delete_playlist", { id });
  if (s) applySnapshot(s);
}

// ---------------- サイドバーの並べ替え ----------------
// プレイリストと PC のフォルダはドラッグで並べ替えられる。iPod OS のプレイリストは iTunesDB の中の順番を変え
// (iPod 本体のメニューもこの順)、Rockbox のプレイリストは本体がファイル名で並べるので、画面の表示の順だけを覚える

/** Rockbox のプレイリストの表示の順 (iPod の場所 → プレイリストの id の並び) */
function rbPlaylistOrder(root: string): string[] {
  try {
    const v = (JSON.parse(getStored("rbPlaylistOrder") ?? "{}") as Record<string, string[]>)[root];
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
}

function setRbPlaylistOrder(root: string, ids: string[]) {
  let map: Record<string, string[]> = {};
  try {
    map = JSON.parse(getStored("rbPlaylistOrder") ?? "{}");
  } catch {
    /* 壊れていたら作り直す */
  }
  map[root] = ids;
  setStored("rbPlaylistOrder", JSON.stringify(map));
}

/** サイドバーに出す順。Rockbox は覚えた順 (覚えていないものは後ろに元の順で) */
export function orderedPlaylists(pls: Playlist[]): Playlist[] {
  if (!isRockbox() || !state.snap) return pls;
  const order = rbPlaylistOrder(state.snap.device.root);
  if (!order.length) return pls;
  const pos = new Map(order.map((id, i) => [id, i]));
  return [...pls].sort((a, b) => (pos.get(a.id) ?? Infinity) - (pos.get(b.id) ?? Infinity));
}

export const sideDrag = {
  pending: null as { list: "pl" | "pc"; key: string; y: number; el: HTMLElement } | null,
  active: false,
  /** 入れる場所 (並びの中の位置。末尾は items.length) */
  index: -1,
  /** 並べ替えを終えた時刻 (直後のクリックで表示を切り替えないように) */
  endedAt: 0,
};

/** 並べ替えられる項目 (「新規プレイリスト…」「すべて」などは除く) */
function sideReorderItems(list: "pl" | "pc"): HTMLElement[] {
  return [...$(list === "pl" ? "pl-list" : "pc-list").querySelectorAll<HTMLElement>(list === "pl" ? ".side-item[data-pl]" : ".side-item[data-pc]")];
}

export function onSideMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;
  const el = (e.target as HTMLElement).closest<HTMLElement>(".side-item");
  if (!el) return;
  if (el.dataset.pl && (isRockbox() || writable())) sideDrag.pending = { list: "pl", key: el.dataset.pl, y: e.clientY, el };
  else if (el.dataset.pc) sideDrag.pending = { list: "pc", key: el.dataset.pc, y: e.clientY, el };
}

export function onSideMouseMove(e: MouseEvent) {
  const p = sideDrag.pending;
  if (!p) return;
  if (!sideDrag.active) {
    if (Math.abs(e.clientY - p.y) < 6) return;
    sideDrag.active = true;
    p.el.classList.add("side-dragging");
    document.body.classList.add("side-reordering");
  }
  const items = sideReorderItems(p.list);
  items.forEach((x) => x.classList.remove("insert-before", "insert-after"));
  const i = items.findIndex((x) => {
    const r = x.getBoundingClientRect();
    return e.clientY < r.top + r.height / 2;
  });
  sideDrag.index = i < 0 ? items.length : i;
  if (i >= 0) items[i].classList.add("insert-before");
  else items[items.length - 1]?.classList.add("insert-after");
}

export async function onSideMouseUp() {
  const p = sideDrag.pending;
  const active = sideDrag.active;
  sideDrag.pending = null;
  sideDrag.active = false;
  if (!p || !active) return;
  sideDrag.endedAt = Date.now();
  document.body.classList.remove("side-reordering");
  const items = sideReorderItems(p.list);
  items.forEach((x) => x.classList.remove("insert-before", "insert-after", "side-dragging"));
  const keys = items.map((x) => (p.list === "pl" ? x.dataset.pl! : x.dataset.pc!));
  const from = keys.indexOf(p.key);
  if (from < 0) return;
  const to = sideDrag.index > from ? sideDrag.index - 1 : sideDrag.index;
  if (to === from) return;
  keys.splice(from, 1);
  keys.splice(to, 0, p.key);
  if (p.list === "pc") {
    await reorderPcFolders(keys);
    renderSidebar();
    return;
  }
  await setPlaylistOrder(keys);
}

/** プレイリストを `keys` (id の並び) の順にする。iPod OS は iTunesDB に書き、Rockbox は表示の順だけ覚える */
async function setPlaylistOrder(keys: string[]) {
  const s = state.snap;
  if (!s) return;
  if (isRockbox()) {
    setRbPlaylistOrder(s.device.root, keys);
    renderSidebar();
    return;
  }
  // 先に画面の順を変えてから (待たせない) iPod に書く
  const pos = new Map(keys.map((id, i) => [id, i]));
  const master = s.playlists.filter((x) => x.master);
  const rest = s.playlists.filter((x) => !x.master).sort((a, b) => (pos.get(a.id) ?? Infinity) - (pos.get(b.id) ?? Infinity));
  s.playlists = [...master, ...rest];
  renderSidebar();
  const snap = await call<Snapshot>("reorder_playlists", { ids: keys });
  if (snap) applySnapshot(snap);
}

/** プレイリストを名前順に並べる (大文字小文字を区別せず、数字は数として比べる) */
export async function sortPlaylistsByName() {
  const s = state.snap;
  if (!s) return;
  const name = new Map(s.playlists.map((p) => [p.id, p.name]));
  const cmp = new Intl.Collator("ja", { numeric: true, sensitivity: "base" }).compare;
  const keys = sideReorderItems("pl").map((x) => x.dataset.pl!);
  const sorted = [...keys].sort((a, b) => cmp(name.get(a) ?? "", name.get(b) ?? ""));
  if (sorted.every((id, i) => id === keys[i])) return;
  await setPlaylistOrder(sorted);
}
