// プレイリストの並び替え (配布リポジトリの Issue #22)。一覧の並べ替えは表示だけ (「デフォルト」が保存してある順)。
// プレイリストの順そのものを変えるのは、ツールバーの「並び替え」で開く右のパネルから: 並べ方を選ぶか、曲をドラッグして
// 下書きを作り、「保存」で iPod に書く。並べ替えたつもりがプレイリストの順まで変わっていた、とならないように分けている

import { t as tr } from "../i18n";
import { computeRows } from "../app/rows";
import { editablePlaylist, isRockbox, writable } from "../app/selectors";
import { $, state } from "../app/state";
import type { Snapshot, SortKey } from "../domain/types";
import { esc } from "./dom";
import { call, toast } from "./feedback";
import { busySave, confirmBox } from "./modal";
import { playlistSort, rememberPlaylistSort, sortedOrder } from "./playlist-sort";
import { applySnapshot, renderAll } from "./render";
import { animateRowMoves } from "./rows";
import { setLayout, trackSortLabel } from "./sort-menu";

/** パネルで選べる並べ方。追加日は Rockbox ではプレイリストに足した順。再生回数は iPod OS だけ */
function keys(): SortKey[] {
  return ["title", "artist", "album", "trackNr", "year", "duration", "dateAdded", ...(isRockbox() ? [] : (["plays"] as SortKey[])), "rating", "artwork"];
}

/** 初めて選んだときの向き: 新しいもの・多いものから */
function newestFirst(k: SortKey): boolean {
  return k === "dateAdded" || k === "plays" || k === "rating" || k === "year";
}

/** 開いているプレイリストの並び替えを始める */
export async function startReorder() {
  const pl = editablePlaylist(state.view);
  if (!pl || !writable() || state.reorder) return;
  // 下書きは曲の一覧で、保存してある順 (デフォルト)・検索なしで見せる
  if (state.layout !== "tracks") await setLayout("tracks");
  state.search = "";
  $<HTMLInputElement>("search").value = "";
  state.sort = { key: "order", asc: true };
  const sort = playlistSort(pl.id);
  state.reorder = { id: pl.id, ids: pl.trackIds.slice(), sort, keep: !!sort, dirty: false };
  state.selected.clear();
  document.body.classList.add("reordering");
  $("tbody").scrollTop = 0;
  renderAll();
}

/** 並び替えを終える (保存しないで閉じるときは下書きを捨てる) */
export function endReorder() {
  if (!state.reorder) return;
  state.reorder = null;
  document.body.classList.remove("reordering");
  renderAll();
}

/** キャンセル: 変えていれば確かめてから閉じる */
export async function cancelReorder() {
  const d = state.reorder;
  if (!d) return;
  if (d.dirty && !(await confirmBox(tr("並び替えをやめますか？"), tr("変えた順は保存されません。"), tr("やめる")))) return;
  endReorder();
}

/** 並べ方を選ぶ (今の並べ方を選び直したら向きを変える) */
export function chooseReorderKey(key: SortKey) {
  const d = state.reorder;
  if (!d) return;
  const asc = d.sort?.key === key ? !d.sort.asc : !newestFirst(key);
  setDraftSort({ key, asc });
}

function setDraftSort(sort: { key: SortKey; asc: boolean }) {
  const d = state.reorder;
  const pl = d && editablePlaylist(d.id);
  if (!d || !pl) return;
  const ids = sortedOrder(pl, sort);
  // 並べ方を選んだら、足した曲もその並べ方で入れるのを既定にする
  if (!d.sort) d.keep = true;
  d.sort = sort;
  d.dirty = true;
  applyDraft(ids);
}

/** ドラッグで動かした (並べ方は「カスタム」になる) */
export function moveDraft(ids: string[]) {
  const d = state.reorder;
  if (!d) return;
  d.sort = null;
  d.keep = false;
  d.dirty = true;
  applyDraft(ids);
}

/** 下書きの順を変えて、行を新しい位置へ動かす */
function applyDraft(ids: string[]) {
  const d = state.reorder!;
  animateRowMoves(() => {
    d.ids = ids;
    computeRows();
    renderAll();
  });
}

/** 保存: 下書きの順を iPod に書き、並べ方を覚える */
async function saveReorder(btn: HTMLButtonElement) {
  const d = state.reorder;
  const pl = d && editablePlaylist(d.id);
  if (!d || !pl) return;
  const sort = d.sort && d.keep ? d.sort : null;
  const changed = d.ids.join("\u0000") !== pl.trackIds.join("\u0000");
  let snap: Snapshot | null = null;
  if (changed) {
    const done = busySave(btn);
    snap = await call<Snapshot>("reorder_playlist", { id: pl.id, trackIds: d.ids });
    if (!snap) {
      done();
      return;
    }
    await done.settle();
  }
  rememberPlaylistSort(pl.id, sort);
  state.reorder = null;
  document.body.classList.remove("reordering");
  if (snap) applySnapshot(snap);
  else renderAll();
  toast(tr("「{a}」の並び順を保存しました", { a: pl.name }));
}

/** 右のパネルを描く (並び替え中だけ出す) */
export function renderReorderPanel() {
  const el = $("reorder-panel");
  const d = state.reorder;
  const pl = d && editablePlaylist(d.id);
  el.classList.toggle("hidden", !pl || state.view !== d?.id);
  if (!d || !pl) return;
  const keyItems = keys()
    .map((k) => {
      const on = d.sort?.key === k;
      const dir = on ? (d.sort!.asc ? "↑" : "↓") : "";
      return `<li><button class="rp-key${on ? " active" : ""}" data-key="${k}" aria-pressed="${on}"><span>${esc(tr(trackSortLabel(k)))}</span><span class="rp-dir">${dir}</span></button></li>`;
    })
    .join("");
  const sorted = !!d.sort;
  el.innerHTML = `
    <div class="rp-head">
      <h3>${esc(tr("並び替え"))}</h3>
      <div class="rp-name muted" title="${esc(pl.name)}">${esc(pl.name)}</div>
    </div>
    <div class="rp-body">
      <p class="rp-help muted">${esc(tr("並べ方を選ぶか、曲をドラッグして動かします。「保存」を押すまで iPod には書き込みません。"))}</p>
      <div class="rp-label">${esc(tr("並べ方"))}<span class="rp-current">${esc(d.sort ? "" : tr("カスタム"))}</span></div>
      <ul class="rp-keys">${keyItems}</ul>
      <div class="seg rp-order" role="radiogroup" aria-label="${esc(tr("向き"))}">
        <label class="seg-item${sorted ? "" : " disabled"}"><input type="radio" name="rp-order" value="asc"${d.sort?.asc ? " checked" : ""}${sorted ? "" : " disabled"} /><span>${esc(tr("昇順"))}</span></label>
        <label class="seg-item${sorted ? "" : " disabled"}"><input type="radio" name="rp-order" value="desc"${sorted && !d.sort!.asc ? " checked" : ""}${sorted ? "" : " disabled"} /><span>${esc(tr("降順"))}</span></label>
      </div>
      <label class="rp-keep${sorted ? "" : " disabled"}"><input type="checkbox"${d.keep && sorted ? " checked" : ""}${sorted ? "" : " disabled"} /><span>${esc(tr("あとから足した曲もこの並べ方で入れる"))}</span></label>
    </div>
    <div class="rp-foot">
      <button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button>
      <button class="btn primary" data-x="save">${esc(tr("保存"))}</button>
    </div>`;
}

export function bindReorderPanel() {
  $("btn-reorder").addEventListener("click", () => void startReorder());
  const el = $("reorder-panel");
  el.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    const key = t.closest<HTMLElement>(".rp-key")?.dataset.key;
    if (key) return chooseReorderKey(key as SortKey);
    const x = t.closest<HTMLElement>("[data-x]")?.dataset.x;
    if (x === "cancel") void cancelReorder();
    if (x === "save") void saveReorder(t.closest("button")!);
  });
  el.addEventListener("change", (e) => {
    const t = e.target as HTMLInputElement;
    const d = state.reorder;
    if (!d) return;
    if (t.name === "rp-order" && d.sort) setDraftSort({ key: d.sort.key, asc: t.value === "asc" });
    else if (t.type === "checkbox") d.keep = t.checked;
  });
}
