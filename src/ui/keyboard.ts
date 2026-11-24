// キーボードでの移動 (サイドバー・一覧・アルバム・アーティスト)

import { albumGroups, rowTop } from "../app/rows";
import { $, ALBUM_HEAD_H, ROW_H, state } from "../app/state";
import { GRID_GAP, GRID_PAD, gridRange, gridText, openAlbum, renderAlbumGrid, toggleAlbumSelection } from "./album-grid";
import { selectArtist, toggleArtistSelection } from "./artist-list";
import { renderRows } from "./rows";
import { renderSelbar } from "./selbar";
import { selectView } from "./sidebar";
import { openVideoNode } from "./visual";
import { checkRange, setAnchor } from "./table-select";

/** ↑↓ キー: カーソル移動。Shift で範囲をチェック */
// ---------------- キーボードでの移動 ----------------
// サイドバー → (アーティストの一覧) → 曲の一覧 / アルバムの一覧 を ← → で行き来し、その中は ↑ ↓ (アルバムは上下左右)。
// Enter で開く、Esc / Backspace で戻る。Home / End / PageUp / PageDown も使える

/** キーボードで操作しているか (選んでいるものの枠を出す)。マウスを押したら消す */
export function setKeyboardMode(on: boolean) {
  document.body.classList.toggle("kb", on);
}

function focusSidebar() {
  document.querySelector<HTMLElement>(".main-side")!.focus();
}

/** 今の見方の一覧へフォーカスを移す。`pick` (キーボードで移ったとき) ならアルバムの先頭のカードを選ぶ。
 * マウスで見方を切り替えたときは選ばない (枠は矢印キーを押してから出す) */
export function focusMain(pick = false) {
  if (state.layout === "artists") $("artist-list").focus();
  else if (state.layout === "albums" && state.openAlbum === null) {
    $("album-grid").focus();
    if (pick && state.gridFocus < 0 && albumGroups().length) setGridFocus(0);
  } else $("tbody").focus();
}

/** サイドバーのキー: ↑ ↓ で表示を切り替える (選ぶとすぐ開く)、→ / Enter で一覧へ */
export function sidebarKey(e: KeyboardEvent) {
  if (e.target !== e.currentTarget || e.metaKey || e.ctrlKey || e.altKey) return;
  // 閉じた塊の項目は飛ばす
  const items = [...document.querySelectorAll<HTMLElement>(".main-side .side-sec:not(.collapsed) .side-item[data-view]:not(.hidden)")];
  const cur = items.findIndex((x) => x.classList.contains("active"));
  let next = -1;
  if (e.key === "ArrowDown") next = Math.min(items.length - 1, cur + 1);
  else if (e.key === "ArrowUp") next = Math.max(0, cur - 1);
  else if (e.key === "Home") next = 0;
  else if (e.key === "End") next = items.length - 1;
  else if (e.key === "ArrowRight" || e.key === "Enter") {
    e.preventDefault();
    e.stopPropagation();
    setKeyboardMode(true);
    focusMain(true);
    return;
  } else return;
  e.preventDefault();
  e.stopPropagation();
  setKeyboardMode(true);
  if (next < 0 || next === cur) return;
  const it = items[next];
  if (it.dataset.vnode !== undefined) openVideoNode(it.dataset.vnode);
  else selectView(it.dataset.view!);
  document.querySelector<HTMLElement>(".main-side .side-item.active")?.scrollIntoView({ block: "nearest" });
}

/** アルバムの一覧で選ぶカードを変えて、見える位置までスクロールする */
function setGridFocus(i: number) {
  state.gridFocus = i;
  const el = $("album-grid");
  if (i >= 0 && gridRange.cols > 0) {
    const pitch = gridRange.colW + gridText + GRID_GAP;
    // カードは上下に 6px はみ出す
    const top = GRID_PAD + Math.floor(i / gridRange.cols) * pitch - 6;
    const bottom = top + pitch - GRID_GAP + 12;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (bottom > el.scrollTop + el.clientHeight) el.scrollTop = bottom - el.clientHeight;
  }
  renderAlbumGrid();
  el.querySelectorAll<HTMLElement>(".album-card").forEach((c) => c.classList.toggle("kb-focus", Number(c.dataset.album) === i));
}

/** アルバムの一覧のキー。扱ったら true */
function gridKey(e: KeyboardEvent): boolean {
  const groups = albumGroups();
  const n = groups.length;
  if (!n) return false;
  const cols = Math.max(1, gridRange.cols);
  const cur = Math.min(state.gridFocus, n - 1);
  const page = Math.max(1, Math.floor($("album-grid").clientHeight / (gridRange.colW + gridText + GRID_GAP))) * cols;
  let next: number;
  switch (e.key) {
    case "ArrowRight":
      next = cur < 0 ? 0 : Math.min(n - 1, cur + 1);
      break;
    case "ArrowLeft":
      // 左端からはサイドバーへ
      if (cur <= 0 || cur % cols === 0) {
        e.preventDefault();
        focusSidebar();
        return true;
      }
      next = cur - 1;
      break;
    case "ArrowDown":
      // 最後の行の 1 つ上で、真下にカードが無いときは最後のカードへ
      next = cur < 0 ? 0 : Math.floor(cur / cols) < Math.floor((n - 1) / cols) ? Math.min(n - 1, cur + cols) : cur;
      break;
    case "ArrowUp":
      next = cur < 0 ? 0 : cur >= cols ? cur - cols : cur;
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = n - 1;
      break;
    case "PageDown":
      next = cur < 0 ? 0 : Math.min(n - 1, cur + page);
      break;
    case "PageUp":
      next = cur < 0 ? 0 : Math.max(cur % cols, cur - page);
      break;
    case "Enter":
      if (cur < 0) return false;
      e.preventDefault();
      openAlbum(groups[cur].key).then(() => $("tbody").focus());
      return true;
    case " ":
      // スペースで選んでいるカードのアルバムを選ぶ (曲を再生中でも、カードを選んでいるときはこちら)
      if (cur < 0) return false;
      e.preventDefault();
      toggleAlbumSelection(cur);
      return true;
    default:
      return false;
  }
  e.preventDefault();
  setGridFocus(next);
  return true;
}

/** アーティスト表示の左の一覧のキー。扱ったら true */
function artistListKey(e: KeyboardEvent): boolean {
  const n = state.artists.length;
  const cur = state.artists.findIndex((a) => a.key === state.artist);
  const page = Math.max(1, Math.floor($("artist-list").clientHeight / 30) - 1);
  let next: number;
  switch (e.key) {
    case "ArrowDown":
      next = cur + 1;
      break;
    case "ArrowUp":
      next = cur - 1;
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = n - 1;
      break;
    case "PageDown":
      next = cur + page;
      break;
    case "PageUp":
      next = cur - page;
      break;
    case "ArrowLeft":
      e.preventDefault();
      focusSidebar();
      return true;
    case "ArrowRight":
    case "Enter":
      e.preventDefault();
      $("tbody").focus();
      return true;
    // スペースで選んでいるアーティストの全曲をチェック (曲の一覧と同じ)
    case " ":
      e.preventDefault();
      if (cur >= 0) toggleArtistSelection(cur);
      return true;
    default:
      return false;
  }
  e.preventDefault();
  if (n) selectArtist(state.artists[Math.max(0, Math.min(n - 1, next))].key);
  return true;
}

/** 一覧 (曲・アルバム・アーティスト) のキーでの移動。扱ったら true */
export function navKey(e: KeyboardEvent): boolean {
  if (e.metaKey || e.ctrlKey || e.altKey) return false;
  // ボタンの上の Enter / スペースはボタンを押す (アルバムの再生ボタン・チェックなど)
  if ((e.key === "Enter" || e.key === " ") && (e.target as HTMLElement).closest("button")) return false;
  if (state.layout === "albums" && state.openAlbum === null) return gridKey(e);
  if (state.layout === "artists" && document.activeElement?.closest("#artist-list")) return artistListKey(e);
  const albumOpen = state.layout === "albums" && state.openAlbum !== null;
  switch (e.key) {
    case "ArrowLeft":
      e.preventDefault();
      if (state.layout === "artists") $("artist-list").focus();
      else focusSidebar();
      return true;
    case "Backspace":
      if (!albumOpen) return false;
      e.preventDefault();
      backToAlbums();
      return true;
    case "Home":
    case "End":
      e.preventDefault();
      moveCursor(e.key === "Home" ? -state.rows.length : state.rows.length, e.shiftKey);
      return true;
    case "PageUp":
    case "PageDown": {
      e.preventDefault();
      const page = Math.max(1, Math.floor($("tbody").clientHeight / ROW_H) - 1);
      moveCursor(e.key === "PageDown" ? page : -page, e.shiftKey);
      return true;
    }
    default:
      return false;
  }
}

/** 開いたアルバムから一覧へ戻る (キーボードで戻ったときは、そのアルバムのカードを選んだままにする) */
export function backToAlbums() {
  const prev = state.openAlbum;
  const kb = document.body.classList.contains("kb");
  openAlbum(null).then(() => {
    if (!kb || prev === null) return;
    $("album-grid").focus();
    const i = albumGroups().findIndex((g) => g.key === prev);
    if (i >= 0) setGridFocus(i);
  });
}

export function moveCursor(delta: number, extend: boolean) {
  if (!state.rows.length) return;
  const cur = state.rows.findIndex((t) => t.id === state.focused);
  const focusIdx = cur >= 0 ? cur : state.anchor;
  const next = Math.max(0, Math.min(state.rows.length - 1, (focusIdx < 0 ? -1 : focusIdx) + delta));
  if (extend) {
    if (state.anchor < 0) setAnchor(focusIdx < 0 ? next : focusIdx);
    checkRange(next);
  } else {
    setAnchor(next);
  }
  state.focused = state.rows[next].id;
  // 見える位置までスクロール
  const body = $("tbody");
  const top = rowTop(next);
  // アーティスト表示で先頭の行なら、上のアルバムの見出しも見えるように
  const headAbove = state.rowLayout && (next === 0 || rowTop(next - 1) + ROW_H !== top) ? ALBUM_HEAD_H : 0;
  if (top - headAbove < body.scrollTop) body.scrollTop = top - headAbove;
  else if (top + ROW_H > body.scrollTop + body.clientHeight) body.scrollTop = top + ROW_H - body.clientHeight;
  renderRows();
  renderSelbar();
}
