// サイドバー (ライブラリ・プレイリスト・PC のフォルダ) と、表示の切り替え

import { t as tr } from "../i18n";
import { pod } from "../app/podcasts";
import { KIND_VIEWS, PC_ALL, currentPcFolders, isPcView, writable } from "../app/selectors";
import { $, state } from "../app/state";
import type { MediaKind } from "../domain/types";
import { esc } from "./dom";
import { ICON_ALL, ICON_BOOK, ICON_FOLDER, ICON_LIB, ICON_PHOTO, ICON_PL, ICON_POD, ICON_SMART } from "./icons";
import { PHOTO_VIEW, isVisualView, vis } from "../app/visual";
import { onVisualViewSelected, videoSideHtml } from "./visual";
import { folderName, scanPcFolder } from "./pc";
import { orderedPlaylists } from "./playlists";
import { renderPodSidebar } from "./podcasts";
import { renderAll } from "./render";

/** ライブラリの分類ごとの表示の名前 (サイドバーと見出し) */
// tr: 使う側で訳す
export const VIEW_TITLE: Record<string, string> = { music: "ミュージック", podcasts: "@side:ポッドキャスト", audiobooks: "@side:オーディオブック" };

export function renderSidebar() {
  // 開発版の「ドライブが出ない状態を真似る」の間は、開いている iPod が無いものとして描く (プレイリストも出さない)
  const s = state.devNoDrive ? null : state.snap;
  // iPod が無いとき (開いている途中を除く) は、プレイリストの欄を出さず、ライブラリは接続の案内を開く「すべて」だけにする
  const noIpod = !s && !state.opening;
  document.querySelector('.side-sec[data-sec="pl"]')?.classList.toggle("hidden", noIpod);
  const master = s?.playlists.find((p) => p.master);
  // すべて・ミュージック・ポッドキャスト・オーディオブック (曲が無くても出す)。数は iPod を開いているときだけ
  const counts: Record<MediaKind, number> = { music: 0, podcast: 0, audiobook: 0 };
  for (const t of s?.tracks ?? []) counts[t.kind ?? "music"]++;
  const kindItem = (v: string, icon: string) =>
    `<li class="side-item ${state.view === v ? "active" : ""}" data-view="${v}" data-lib="1">${icon}<span class="label">${tr(VIEW_TITLE[v])}</span><span class="count">${
      s ? counts[KIND_VIEWS[v]] : ""
    }</span></li>`;
  const lib = $("lib-list");
  const hovered = lib.querySelector<HTMLElement>(".side-item:hover");
  lib.innerHTML =
    `<li class="side-item ${state.view === "library" ? "active" : ""}" data-view="library" data-lib="1">${ICON_ALL}<span class="label">${tr("すべて")}</span>${
      state.transferring ? `<span class="spinner small" title="${esc(tr("転送中"))}"></span>` : ""
    }<span class="count">${master ? master.trackIds.length + state.transferAdded : ""}</span></li>` +
    (noIpod ? "" : kindItem("music", ICON_LIB) + kindItem("podcasts", ICON_POD) + kindItem("audiobooks", ICON_BOOK) + visualItems());
  keepHover(lib, hovered);
  // ポッドキャストのプレイリスト (iPod OS が「ポッドキャスト」のメニューに使う) は、ライブラリの「ポッドキャスト」で見るので出さない
  const pls = orderedPlaylists(s?.playlists.filter((p) => !p.master && !p.podcast) ?? []);
  $("pl-list").innerHTML = !s
    ? `<li class="side-note">${state.opening ? tr("読み込み中…") : tr("iPod を接続すると表示されます")}</li>`
    : pls
      .map((p) => {
        const icon = p.smart ? ICON_SMART : p.podcast ? ICON_POD : ICON_PL;
        // ポッドキャストのプレイリストの名前は iPod では使われない (「ポッドキャスト」のメニューになる)
        const name = p.podcast ? tr("ポッドキャスト") : p.name;
        return `<li class="side-item ${state.view === p.id ? "active" : ""}" data-view="${p.id}" data-pl="${p.id}" title="${esc(name)}">${icon}<span class="label">${esc(name)}</span><span class="count">${p.trackIds.length}</span></li>`;
      })
      .join("") +
      // プレイリストがあるときは見出しの「＋」から作れるので、項目は 1 つも無いときだけ出す
      (pls.length ? "" : `<li class="side-item new-pl" data-newpl="1">${ICON_PL}<span class="label">${tr("新規プレイリスト…")}</span></li>`);
  $<HTMLButtonElement>("btn-new-pl").disabled = !writable();
  const allCount = state.pc.folders.every((f) => state.pc.tracks.has(f))
    ? new Set(state.pc.folders.flatMap((f) => (state.pc.tracks.get(f) ?? []).map((t) => t.id))).size
    : null;
  const allItem = state.pc.folders.length
    ? `<li class="side-item ${state.view === PC_ALL ? "active" : ""}" data-view="${PC_ALL}">${ICON_ALL}<span class="label">${tr("すべて")}</span><span class="count">${allCount ?? ""}</span></li>`
    : "";
  $("pc-list").innerHTML = allItem + state.pc.folders
    .map((f) => {
      const n = state.pc.tracks.get(f)?.length;
      return `<li class="side-item ${state.view === `pc:${f}` ? "active" : ""}" data-view="pc:${esc(f)}" data-pc="${esc(f)}" title="${esc(f)}">${ICON_FOLDER}<span class="label">${esc(folderName(f))}</span><span class="count">${n ?? ""}</span></li>`;
    })
    .join("") + (state.pc.folders.length ? "" : `<li class="side-item new-pl" data-addpc="1">${ICON_FOLDER}<span class="label">${tr("音楽フォルダを追加…")}</span></li>`);
  $("pod-side-list").innerHTML = renderPodSidebar();
  const syncBtn = $<HTMLButtonElement>("btn-sync-pod");
  // 同期は iPod が要るので、iPod が無いときは出さない
  syncBtn.classList.toggle("hidden", !pod.feeds.length || !s);
  syncBtn.disabled = pod.syncing || state.transferring;
}

/**
 * 描き直す前にマウスが乗っていた行を、描き直した行に引き継ぐ (.hovering)。描き直した直後はマウスが乗っている
 * 扱いにならず、「ビデオ」の行のアイコンが一瞬戻ってから開け閉めの印に替わって見えていた
 */
function keepHover(list: HTMLElement, was: HTMLElement | null) {
  if (!was) return;
  const key = was.dataset.tkey ?? was.dataset.view;
  const now = [...list.querySelectorAll<HTMLElement>(".side-item")].find((li) => (li.dataset.tkey ?? li.dataset.view) === key);
  if (!now) return;
  now.classList.add("hovering");
  // マウスが動いたら、本当に乗っているかで決め直す (外れていれば外す)
  const off = () => {
    if (now.isConnected && now.matches(":hover")) return;
    now.classList.remove("hovering");
    document.removeEventListener("mousemove", off);
  };
  document.addEventListener("mousemove", off);
}

/** ライブラリの「ビデオ」(下に種類などを開け閉めして出す)「写真」。iPod OS で再生・表示できない機種では出さない。数は読み終えてから */
function visualItems(): string {
  const s = state.devNoDrive ? null : state.snap;
  const item = (v: string, icon: string, label: string, n: number | undefined) =>
    `<li class="side-item ${state.view === v ? "active" : ""}" data-view="${v}" data-lib="1">${icon}<span class="label">${label}</span><span class="count">${s && n !== undefined ? n : ""}</span></li>`;
  const ipodOs = s?.mode === "ipod";
  const video = !(ipodOs && s?.device.video === false);
  const photo = !(s && vis.photos && !vis.photos.supported);
  return (
    (video ? videoSideHtml(s && vis.videos ? String(vis.videos.length) : "") : "") +
    (photo ? item(PHOTO_VIEW, ICON_PHOTO, tr("@side:写真"), vis.photos?.photos.length) : "")
  );
}

export function selectView(v: string) {
  // プレイリストの並び替え中は、保存かキャンセルで終えるまでほかを開かない
  if (state.view === v || state.reorder) return;
  state.view = v;
  if (isVisualView()) onVisualViewSelected();
  pod.selected.clear();
  state.openAlbum = null;
  state.gridFocus = -1;
  state.selected.clear();
  state.anchor = -1;
  state.focused = null;
  state.sort = v === "library" || Object.prototype.hasOwnProperty.call(KIND_VIEWS, v) || isPcView() ? { key: "artist", asc: true } : { key: "order", asc: true };
  $("tbody").scrollTop = 0;
  renderAll();
  for (const f of currentPcFolders()) if (!state.pc.tracks.has(f)) scanPcFolder(f);
}
