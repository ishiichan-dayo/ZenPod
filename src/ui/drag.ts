// 一覧の中のドラッグ (プレイリストへの追加・並べ替え)。WebView2 では HTML5 の DnD と競合するのでポインタイベントで作る

import { t as tr } from "../i18n";
import { albumGroups } from "../app/rows";
import { currentPlaylist, editablePlaylist, isPcView, writable } from "../app/selectors";
import { $, ROW_H, state } from "../app/state";
import { albumArtCache, albumArtSource } from "./album-grid";
import { artCache, pcArtCache } from "./artwork-cells";
import { showDetails } from "./dialogs/details";
import { esc } from "./dom";
import { sendPcTracks } from "./pc";
import { findTrack, playTrack, togglePlay } from "./player";
import { moveDraft } from "./playlist-reorder";
import { addToPlaylist, newPlaylist, selectedIds } from "./selection-actions";
import { clickSelect, rowIndexAt, toggleSelect } from "./table-select";

// 曲のドラッグ (サイドバーのプレイリストへ追加 / プレイリスト内の並び替え)
// Windows の WebView2 では dragDropEnabled と HTML5 DnD が競合するためポインタイベントで実装
export const drag = {
  /** 押した位置と、ドラッグを始めたときに運ぶ曲 (行なら押した行かチェックした曲、アルバムのカードならそのアルバム) */
  pending: null as { x: number; y: number; ids: () => string[]; art: () => string | null | undefined } | null,
  /** ドラッグを終えた時刻 (直後のクリックでアルバムを開かないように) */
  endedAt: 0,
  active: false,
  ids: [] as string[],
  target: null as { kind: "pl"; id: string } | { kind: "new" } | { kind: "lib" } | { kind: "reorder"; index: number } | null,
};

export function onRowMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;
  const i = rowIndexAt(e.target);
  if (i < 0) return;
  // テキスト選択を防ぎつつキーボード操作用にフォーカスだけ移す
  e.preventDefault();
  $("tbody").focus();
  const t = state.rows[i];
  // ジャケットの上の再生ボタン
  const playBtn = (e.target as HTMLElement).closest<HTMLElement>(".row-play");
  if (playBtn) {
    if (playBtn.dataset.toggle) togglePlay();
    else playTrack(t.id);
    return;
  }
  // 曲名の横の「タグを読めない」印: 詳細情報 (理由とタグの構造) を開く
  if ((e.target as HTMLElement).closest(".read-error")) {
    showDetails(t.id);
    return;
  }
  // チェックボックス: クリックで追加・解除 (Shift で範囲)。ドラッグは開始しない
  if ((e.target as HTMLElement).closest(".cell.chk")) {
    toggleSelect(i, e.shiftKey);
    return;
  }
  clickSelect(i, e);
  // ダブルクリック: 再生。押すたびに行を描き直すので、ブラウザの dblclick は同じ行の上と
  // みなされず届かない (一覧全体に出る)。押した行・時間・位置で自前で判定する
  const now = Date.now();
  const prev = lastRowDown;
  lastRowDown = { i, at: now, x: e.clientX, y: e.clientY };
  if (
    !e.shiftKey && !e.ctrlKey && !e.metaKey &&
    prev && prev.i === i && now - prev.at < 450 && Math.hypot(e.clientX - prev.x, e.clientY - prev.y) < 6
  ) {
    lastRowDown = null;
    playTrack(t.id);
    return;
  }
  drag.pending = {
    x: e.clientX,
    y: e.clientY,
    // チェックした行をつかんだらチェックした曲すべて、それ以外はつかんだ 1 曲
    ids: () => (state.selected.has(t.id) ? selectedIds() : [t.id]),
    // 1 曲目のジャケットを添える
    art: () => {
      const first = findTrack(state.selected.has(t.id) ? selectedIds()[0] : t.id);
      return first ? (first.pcPath ? pcArtCache.get(first.pcPath) : artCache.get(first.artworkId)) : null;
    },
  };
}

/** アルバムのカードを押した: ドラッグでサイドバーへ (PC の曲なら転送、iPod の曲ならプレイリストへ) */
export function onCardMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;
  const t = e.target as HTMLElement;
  const card = t.closest<HTMLElement>(".album-card");
  // チェックと再生のボタンはクリックだけ
  if (!card || t.closest(".album-chk, .album-play")) return;
  // ジャケットの画像そのものをブラウザがドラッグし始めないように (フォーカスは一覧に移す)
  e.preventDefault();
  $("album-grid").focus();
  const g = albumGroups()[Number(card.dataset.album)];
  if (!g) return;
  // チェックしたアルバムをつかんだらチェックした曲すべて、それ以外はそのアルバムの曲
  const picked = () => g.tracks.some((x) => state.selected.has(x.id));
  drag.pending = {
    x: e.clientX,
    y: e.clientY,
    ids: () => (picked() ? selectedIds() : g.tracks.map((x) => x.id)),
    art: () => {
      const src = albumArtSource(g);
      return src ? albumArtCache.get(src) : null;
    },
  };
}

/** アーティスト表示の左の一覧の名前を押した: ドラッグでサイドバーへ (アルバムのカードと同じ。配布リポジトリの Issue #25)。
 * チェックしたアーティストをつかんだらチェックした曲すべて、それ以外はそのアーティストの曲 */
export function onArtistMouseDown(e: MouseEvent) {
  if (e.button !== 0 || e.shiftKey || e.ctrlKey || e.metaKey) return;
  const t = e.target as HTMLElement;
  const li = t.closest<HTMLElement>("[data-artist]");
  if (!li || t.closest(".artist-chk")) return;
  const a = state.artists[Number(li.dataset.artist)];
  if (!a) return;
  // 名前の文字を選び始めないように (フォーカスは一覧に移す)
  e.preventDefault();
  $("artist-list").focus();
  const picked = () => a.tracks.some((x) => state.selected.has(x.id));
  drag.pending = {
    x: e.clientX,
    y: e.clientY,
    ids: () => (picked() ? selectedIds() : a.tracks.map((x) => x.id)),
    art: () => {
      const src = albumArtSource({ key: "", album: "", artist: "", tracks: a.tracks, rank: 0 });
      return src ? albumArtCache.get(src) : null;
    },
  };
}

/** 直前に行を押したときの行・時刻・位置 (ダブルクリックの判定用) */
let lastRowDown: { i: number; at: number; x: number; y: number } | null = null;

export function onMouseMove(e: MouseEvent) {
  if (drag.pending && !drag.active) {
    if (Math.hypot(e.clientX - drag.pending.x, e.clientY - drag.pending.y) < 6) return;
    drag.active = true;
    drag.ids = drag.pending.ids();
    const g = $("drag-ghost");
    const art = drag.pending.art();
    g.innerHTML = `${art ? `<img src="${art}" alt="">` : ""}<span>${drag.ids.length.toLocaleString()} ${esc(tr("曲"))}</span>`;
    g.classList.remove("hidden");
  }
  if (!drag.active) return;
  const g = $("drag-ghost");
  g.style.left = `${e.clientX}px`;
  g.style.top = `${e.clientY}px`;
  updateDragTarget(e.clientX, e.clientY);
}

export function clearDropMarks() {
  document.querySelectorAll(".drop-target").forEach((el) => el.classList.remove("drop-target"));
  document.querySelectorAll(".insert-before,.insert-after").forEach((el) => el.classList.remove("insert-before", "insert-after"));
}

function updateDragTarget(x: number, y: number) {
  clearDropMarks();
  drag.target = null;
  const el = document.elementFromPoint(x, y) as HTMLElement | null;
  const side = el?.closest<HTMLElement>(".side-item");
  // PC の曲: ライブラリ (そのまま転送。分類は転送のときに決まる) かプレイリスト (転送して追加) へ
  if (isPcView()) {
    if (side && writable() && !state.transferring) {
      if (side.dataset.lib) drag.target = { kind: "lib" };
      else if (side.dataset.pl && editablePlaylist(side.dataset.pl)) drag.target = { kind: "pl", id: side.dataset.pl };
      if (drag.target) side.classList.add("drop-target");
    }
    return;
  }
  if (side && writable()) {
    if (side.dataset.newpl) {
      drag.target = { kind: "new" };
      side.classList.add("drop-target");
    } else if (side.dataset.pl && editablePlaylist(side.dataset.pl) && side.dataset.pl !== state.view) {
      drag.target = { kind: "pl", id: side.dataset.pl };
      side.classList.add("drop-target");
    }
    return;
  }
  // 並び替え (並び替えのパネルで編集中のプレイリスト + 検索なし。Issue #22)。ふだんの一覧では動かさない
  // (並べ替えのつもりのドラッグでプレイリストの順が変わらないように。保存はパネルの「保存」)
  const pl = currentPlaylist();
  if (pl && state.reorder?.id === pl.id && state.layout === "tracks" && state.sort.key === "order" && state.sort.asc && !state.search && el?.closest("#tbody")) {
    const body = $("tbody");
    const rect = body.getBoundingClientRect();
    const pos = Math.max(0, Math.min(state.rows.length, Math.round((y - rect.top + body.scrollTop) / ROW_H)));
    drag.target = { kind: "reorder", index: pos };
    const before = document.querySelector<HTMLElement>(`.row[data-i="${pos}"]`);
    const after = document.querySelector<HTMLElement>(`.row[data-i="${pos - 1}"]`);
    if (before) before.classList.add("insert-before");
    else after?.classList.add("insert-after");
  }
}

export async function onMouseUp() {
  const wasActive = drag.active;
  const target = drag.target;
  const ids = drag.ids;
  drag.pending = null;
  drag.active = false;
  drag.target = null;
  $("drag-ghost").classList.add("hidden");
  clearDropMarks();
  if (wasActive) drag.endedAt = Date.now();
  if (!wasActive || !target) return;

  if (ids.some((id) => id.startsWith("pc:"))) {
    if (target.kind === "lib" || target.kind === "pl") sendPcTracks(ids, target.kind === "pl" ? target.id : null);
    return;
  }
  if (target.kind === "pl") await addToPlaylist(target.id, ids);
  else if (target.kind === "new") await newPlaylist(ids);
  else if (target.kind === "reorder") {
    const draft = state.reorder;
    if (!draft) return;
    const moving = new Set(ids);
    const order = draft.ids.filter((id) => !moving.has(id));
    const before = state.rows.slice(0, target.index).filter((t) => !moving.has(t.id)).length;
    order.splice(before, 0, ...ids);
    if (order.join() === draft.ids.join()) return;
    // 下書きだけを変える (保存はパネルの「保存」)。手で動かしたら並べ方は「カスタム」
    moveDraft(order);
  }
}
