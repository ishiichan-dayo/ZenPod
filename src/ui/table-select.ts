// 一覧の選択 (クリック・Shift・チェック)

import { rowAtY } from "../app/rows";
import { $, state } from "../app/state";
import { justChecked } from "./feedback";
import { renderRows } from "./rows";
import { renderSelbar } from "./selbar";

export function rowIndexAt(target: EventTarget | null): number {
  const row = (target as HTMLElement | null)?.closest<HTMLElement>(".row");
  return row ? Number(row.dataset.i) : -1;
}

/**
 * 行のクリック。チェックは変えずにカーソルを置くだけ (うっかりクリックしてチェックが消えないように)。
 * Shift: 直前にクリックした行からの範囲をチェック / Ctrl (Mac は ⌘): その行のチェックを切り替え
 */
export function clickSelect(i: number, e: MouseEvent) {
  const t = state.rows[i];
  if (!t) return;
  if (e.shiftKey && state.anchor >= 0) {
    checkRange(i);
  } else if (e.metaKey || e.ctrlKey) {
    toggleCheck(t.id);
    setAnchor(i);
  } else {
    setAnchor(i);
  }
  state.focused = t.id;
  renderRows();
  renderSelbar();
}

/** 範囲の起点を置く (Shift での範囲チェックは、この時点のチェックに範囲を足したものになる) */
export function setAnchor(i: number) {
  state.anchor = i;
  state.checkBase = new Set(state.selected);
}

function toggleCheck(id: string) {
  if (state.selected.has(id)) state.selected.delete(id);
  else {
    state.selected.add(id);
    justChecked.set(id, { until: Date.now() + 400, delay: 0 });
  }
}

/** 新しくチェックした行に、起点から順に少しずつ遅らせて動きを付ける (見えている行だけ) */
function markChecked(before: Set<string>, order: number[]) {
  const now = Date.now();
  let n = 0;
  for (const k of order) {
    const id = state.rows[k]?.id;
    if (!id || before.has(id) || !state.selected.has(id)) continue;
    const delay = Math.min(n++ * 18, 300);
    justChecked.set(id, { until: now + delay + 400, delay });
  }
}

/** 画面に見えている行 (前後に少し余裕) だけにする */
function visibleOnly(order: number[]): number[] {
  const body = $("tbody");
  const first = rowAtY(body.scrollTop) - 2;
  const last = rowAtY(body.scrollTop + body.clientHeight) + 2;
  return order.filter((k) => k >= first && k <= last);
}

/** 起点から i までをチェック。範囲を縮めたら、範囲から外れた行のチェックは起点を置いた時点に戻す */
export function checkRange(i: number) {
  const [a, b] = [Math.min(state.anchor, i), Math.max(state.anchor, i)];
  const before = state.selected;
  state.selected = new Set(state.checkBase);
  for (let k = a; k <= b; k++) state.selected.add(state.rows[k].id);
  // 起点の側から波のように
  const order = Array.from({ length: b - a + 1 }, (_, n) => (state.anchor <= i ? a + n : b - n));
  markChecked(before, visibleOnly(order));
}

/** チェックボックス操作: 1 行の追加・解除、Shift なら直前の行からの範囲をまとめてチェック */
export function toggleSelect(i: number, range: boolean) {
  const t = state.rows[i];
  if (!t) return;
  if (range && state.anchor >= 0) checkRange(i);
  else {
    toggleCheck(t.id);
    setAnchor(i);
  }
  state.focused = t.id;
  renderRows();
  renderSelbar();
}

export function selectAllVisible(on: boolean) {
  const before = new Set(state.selected);
  if (on) {
    state.rows.forEach((t) => state.selected.add(t.id));
    markChecked(before, visibleOnly(state.rows.map((_, k) => k)));
  } else state.rows.forEach((t) => state.selected.delete(t.id));
  renderRows();
  renderSelbar();
}
