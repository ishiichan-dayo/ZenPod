// アーティスト表示の左の一覧

import { lang, t as tr } from "../i18n";
import { computeRows } from "../app/rows";
import { $, state } from "../app/state";
import type { RowLayout } from "../app/state";
import { fmtTotal } from "../domain/format";
import { albumKey, releaseRank } from "../domain/track-order";
import { albumArtCache, albumArtSource, artLoading, loadAlbumArt, skelStyle } from "./album-grid";
import { esc } from "./dom";
import { replayClass } from "./feedback";
import { ICON_LIB } from "./icons";
import { CHECK_MARK, ICON_PLAY } from "./player";
import { renderRows } from "./rows";
import { renderSelbar } from "./selbar";

/** アーティスト表示の見出し。アルバムの見出しはジャケット (大きめ)・名前・年と曲数・再生ボタン */
export function artistBlockHtml(b: RowLayout["blocks"][number]): string {
  const ms = b.tracks.reduce((a, t) => a + t.durationMs, 0);
  if (b.kind === "artist") {
    const albums = new Set(b.tracks.map(albumKey)).size;
    return `<div class="ab-artist" style="top:${b.top}px;height:${b.h}px">
      <div class="ab-artist-name">${esc(state.artist || tr("アーティストなし"))}</div>
      <div class="ab-sub">${esc(tr("{n} アルバム", { n: albums }))} · ${esc(tr("{n} 曲", { n: b.tracks.length }))} · ${fmtTotal(ms)}</div>
    </div>`;
  }
  const t = b.tracks[0];
  const src = albumArtSource({ key: "", album: "", artist: "", tracks: b.tracks, rank: 0 });
  const url = src ? albumArtCache.get(src) : null;
  const year = Math.max(...b.tracks.map((x) => x.year));
  const kind = Math.min(...b.tracks.map(releaseRank));
  const sub = [year ? String(year) : "", kind === 1 ? "EP" : kind === 2 ? tr("シングル") : "", tr("{n} 曲", { n: b.tracks.length }), fmtTotal(ms)]
    .filter(Boolean)
    .join(" · ");
  return `<div class="ab-album" style="top:${b.top}px;height:${b.h}px" data-first="${esc(t.id)}">
    <span class="ab-cover${!url && artLoading(src) ? " skel" : ""}"${!url && artLoading(src) ? skelStyle() : ""}${src && !albumArtCache.has(src) ? ` data-src="${esc(src)}"` : ""}>${url ? `<img src="${url}" alt="">` : artLoading(src) ? "" : ICON_LIB}
      <button class="ab-play" title="${esc(tr("このアルバムを再生"))}" aria-label="${esc(tr("このアルバムを再生"))}">${ICON_PLAY}</button></span>
    <div class="ab-text"><div class="ab-name">${esc(t.album || tr("アルバムなし"))}</div><div class="ab-sub">${esc(sub)}</div></div>
  </div>`;
}

/** アルバムの見出しのジャケットを読む (アルバム表示と同じ大きめの画像) */
export function loadBlockArt() {
  document.querySelectorAll<HTMLElement>("#rows .ab-cover[data-src]").forEach((el) => {
    const src = el.dataset.src!;
    delete el.dataset.src;
    loadAlbumArt(src).then((url) => {
      if (!el.isConnected) return;
      el.classList.remove("skel");
      if (url) el.prepend(Object.assign(document.createElement("img"), { src: url, alt: "", className: "fade" }));
      else el.insertAdjacentHTML("afterbegin", ICON_LIB);
    });
  });
}

/** アルバムの見出しの曲 (見出しの次の行から、次の見出しまで) */
export function blockTrackIds(firstId: string): string[] {
  const b = state.rowLayout?.blocks.find((x) => x.kind === "album" && x.tracks[0]?.id === firstId);
  return b ? b.tracks.map((t) => t.id) : [];
}

/** アーティストの選択の印: 全曲を選んでいれば " selected"、一部なら " some" (アルバム表示のカードと同じ) */
function artistSelClass(i: number): string {
  const tracks = state.artists[i]?.tracks ?? [];
  if (state.selected.size === 0) return "";
  let n = 0;
  for (const t of tracks) if (state.selected.has(t.id)) n++;
  return n === 0 ? "" : n === tracks.length ? " selected" : " some";
}

/** 選択が変わったかを安く見るための印 (数と最初・最後。チェックのたびに数が変わり、1 曲を選び直せば最初が変わる) */
function selSig(): string {
  if (state.selected.size === 0) return "";
  let first = "";
  let last = "";
  for (const id of state.selected) {
    if (!first) first = id;
    last = id;
  }
  return `${state.selected.size}\u0000${first}\u0000${last}`;
}

/** アーティスト表示の左の一覧 */
let artistListShown: { list: string; key: string | null; sel: string } | null = null;
export function renderArtistList() {
  const el = $("artist-list");
  // 行の描き直し (スクロールのたび) から呼ばれるので、一覧か選択が変わったときだけ作り直す
  // 一覧はアーティストを選ぶたびに作り直される (computeRows) ので、中身 (名前と曲数) で同じか比べる
  const sig = `${lang}\u0001${state.artists.map((a) => `${a.key}\u0000${a.tracks.length}`).join("\u0001")}`;
  const sel = selSig();
  if (artistListShown?.list === sig && artistListShown.key === state.artist && artistListShown.sel === sel) return;
  const ul = el.querySelector("ul")!;
  el.classList.toggle("selecting", state.selected.size > 0);
  // 一覧が同じで選んだアーティスト・チェックだけ変わったときは、印を付け替えるだけ (1000 人を作り直さない)
  if (artistListShown?.list === sig) {
    if (artistListShown.sel !== sel) {
      artistListShown.sel = sel;
      [...ul.children].forEach((li, i) => {
        const cls = artistSelClass(i);
        li.classList.toggle("selected", cls === " selected");
        li.classList.toggle("some", cls === " some");
      });
    }
    if (artistListShown.key !== state.artist) {
      artistListShown.key = state.artist;
      ul.querySelector(".active")?.classList.remove("active");
      const i = state.artists.findIndex((a) => a.key === state.artist);
      const li = i >= 0 ? ul.children[i] : undefined;
      li?.classList.add("active");
      if (li) revealArtist(ul, li as HTMLElement);
    }
    return;
  }
  artistListShown = { list: sig, key: state.artist, sel };
  const chk = `<button class="artist-chk" tabindex="-1" title="${esc(tr("このアーティストの曲を選択 / 解除"))}" aria-label="${esc(
    tr("このアーティストの曲を選択 / 解除"),
  )}"><span class="box">${CHECK_MARK}</span></button>`;
  const html = state.artists
    .map(
      (a, i) =>
        `<li class="side-item${a.key === state.artist ? " active" : ""}${artistSelClass(i)}" data-artist="${i}" title="${esc(
          a.key || tr("アーティストなし"),
        )}">${chk}<span class="label">${esc(a.key || tr("アーティストなし"))}</span><span class="count">${a.tracks.length}</span></li>`,
    )
    .join("");
  ul.innerHTML = html;
  const active = ul.querySelector<HTMLElement>(".active");
  if (active) revealArtist(ul, active);
  el.querySelector(".artist-empty")!.classList.toggle("hidden", state.artists.length > 0);
}

/** チェックの起点 (Shift で範囲を選ぶとき) */
let artistAnchor = -1;

/** アーティストの全曲を選ぶ (全部選んでいれば外す)。`range` なら前にチェックしたアーティストからここまでを選ぶ。
 * 開いているアーティストは変えない (いくつものアーティストをチェックして、まとめて転送などができるように) */
export function toggleArtistSelection(i: number, range = false) {
  const a = state.artists[i];
  if (!a) return;
  if (range && artistAnchor >= 0 && artistAnchor < state.artists.length) {
    const [from, to] = artistAnchor < i ? [artistAnchor, i] : [i, artistAnchor];
    for (let k = from; k <= to; k++) for (const t of state.artists[k].tracks) state.selected.add(t.id);
  } else {
    const all = a.tracks.every((t) => state.selected.has(t.id));
    for (const t of a.tracks) {
      if (all) state.selected.delete(t.id);
      else state.selected.add(t.id);
    }
    if (!all) {
      const li = $("artist-list").querySelector<HTMLElement>(`[data-artist="${i}"]`);
      if (li) replayClass(li, "chk-anim");
    }
  }
  artistAnchor = i;
  renderRows();
  renderArtistList();
  renderSelbar();
}

/** チェックしたアーティストの上か (右クリックでチェックした曲全部を対象にするか) */
export function artistChecked(i: number): boolean {
  return (state.artists[i]?.tracks ?? []).some((t) => state.selected.has(t.id));
}

/** 選んでいるアーティストが見えるように。もう見えていればスクロールしない (scrollIntoView は毎回レイアウトを測る) */
function revealArtist(ul: HTMLElement, li: HTMLElement) {
  const box = ul.closest<HTMLElement>(".artist-list") ?? ul;
  const top = li.offsetTop - box.offsetTop;
  if (top >= box.scrollTop && top + li.offsetHeight <= box.scrollTop + box.clientHeight) return;
  li.scrollIntoView({ block: "nearest" });
}

export function selectArtist(key: string) {
  if (state.artist === key) return;
  state.artist = key;
  // 一覧でチェックしたアーティストは、ほかのアーティストを開いても外さない (まとめて転送などができるように)
  state.anchor = -1;
  state.focused = null;
  $("tbody").scrollTop = 0;
  computeRows();
  renderRows();
  renderSelbar();
}
