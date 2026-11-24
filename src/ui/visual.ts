// ライブラリの「ビデオ」と「写真」: 曲の一覧の代わりに、カードの一覧を出す。
// 選ぶ (クリック・Ctrl / ⌘・Shift・チェック)・開く (ダブルクリック: 動画は情報の編集、写真は大きく表示)・
// 右クリックのメニュー・選択の帯・アルバム (写真)

import { t as tr } from "../i18n";
import { invoke } from "../infra/tauri";
import { writable, isRockbox } from "../app/selectors";
import { $, state } from "../app/state";
import {
  PHOTO_VIEW,
  VIDEO_VIEW,
  isPhotoView,
  isVideoView,
  isVisualView,
  loadPhotos,
  loadVideos,
  albumLabel,
  childAlbums,
  photoLayout,
  photosShown,
  resetVisualIfChanged,
  setPhotos,
  setVideos,
  vis,
} from "../app/visual";
import { fmtBytes, fmtDay, fmtDur, fmtPhotoDate, fmtTotal } from "../domain/format";
import type { Snapshot } from "../domain/types";
import { VIDEO_KINDS, sortVideos, videoGroups, videoInNode, videoMatches } from "../domain/visual";
import type { PhotoLibrary, PhotoView, VideoKind, VideoNode, VideoView } from "../domain/visual";
import { dataUrlToObjectUrl, skelStyle } from "./album-grid";
import { esc, isEnter } from "./dom";
import { call, reduceMotion, toast } from "./feedback";
import { renderHeader } from "./header";
import { ICON_CHEVRON_LEFT, ICON_CHEVRON_RIGHT, ICON_CLOSE, ICON_COLUMNS, ICON_FOLDER, ICON_PHOTO, ICON_VIDEO } from "./icons";
import { showMenu } from "./menu";
import type { MenuItem } from "./menu";
import { confirmBox, promptText } from "./modal";
import { CHECK_MARK } from "./player";
import { applySnapshot } from "./render";
import { renderSidebar, selectView } from "./sidebar";
import { placeSegThumb } from "./segments";
import { ARTIST_MAX, ARTIST_MIN, bindResizer } from "./resizer";
import { getStored, setStored } from "../infra/prefs";
import { storedList } from "./side-sections";
import { startTransfer } from "./transfer";
import { openVideoPlayer } from "./video-player";
import { editVideos, videoArtEditable, videoArtMenuItems } from "./dialogs/edit-video";

// ---------------- 読み込み ----------------

/** iPod の状態が届いたら: iPod が替わっていれば一覧を捨て、まだ読んでいなければ裏で読む (サイドバーの数のため) */
export function syncVisual() {
  if (resetVisualIfChanged()) clearThumbs();
  if (!state.snap || state.partialOpen) return;
  const jobs: Promise<void>[] = [];
  if (!vis.videos) jobs.push(loadVideos());
  if (!vis.photos) jobs.push(loadPhotos());
  if (!jobs.length) return;
  Promise.all(jobs).then(() => {
    renderSidebar();
    if (isVisualView() && !state.settingsOpen) renderVisualView();
  });
}

/** 転送のあと: 送った分を読み直す */
export async function reloadVisual(videos: boolean, photos: boolean) {
  const jobs: Promise<void>[] = [];
  if (videos) jobs.push(loadVideos());
  if (photos) {
    clearPhotoThumbs();
    jobs.push(loadPhotos());
  }
  await Promise.all(jobs);
  renderSidebar();
  if (isVisualView() && !state.settingsOpen) renderVisualView();
}

// ---------------- 一覧の画像 ----------------

/** 読んだ画像 (キー → Blob の URL。null = 画像が無い) */
const thumbs = new Map<string, string | null>();
const thumbWaiting = new Set<string>();
let thumbQueue: { key: string; load: () => Promise<string | null> }[] = [];
let thumbRunning = 0;
const THUMB_PARALLEL = 4;

function clearThumbs() {
  for (const u of thumbs.values()) if (u) URL.revokeObjectURL(u);
  thumbs.clear();
  thumbQueue = [];
  thumbWaiting.clear();
}

/** iPod OS の写真の ID は保存のたびに振り直されるので、写真の画像は変えるたびに捨てる */
function clearPhotoThumbs() {
  for (const [k, u] of [...thumbs]) {
    if (!k.startsWith("p:")) continue;
    if (u) URL.revokeObjectURL(u);
    thumbs.delete(k);
  }
  thumbQueue = thumbQueue.filter((j) => !j.key.startsWith("p:"));
}

const videoKey = (v: VideoView) => `v:${v.id}:${v.artworkId}`;
const photoKey = (id: string) => `p:${id}`;

/** 動画の再生中は一覧の画像を作り始めない (Rockbox の動画は 1 枚ごとに ffmpeg で iPod を読むので、再生と取り合う) */
let thumbsPaused = false;

function pumpThumbs() {
  while (!thumbsPaused && thumbRunning < THUMB_PARALLEL && thumbQueue.length) {
    const job = thumbQueue.shift()!;
    thumbRunning++;
    job
      .load()
      .catch(() => null)
      .then((url) => {
        thumbRunning--;
        thumbWaiting.delete(job.key);
        const blob = url ? dataUrlToObjectUrl(url) : null;
        thumbs.set(job.key, blob);
        for (const el of document.querySelectorAll<HTMLElement>(`#vis-grid .vis-thumb[data-key="${CSS.escape(job.key)}"]`)) fillThumb(el, blob);
        pumpThumbs();
      });
  }
}

function fillThumb(el: HTMLElement, url: string | null) {
  el.classList.remove("skel", "wait");
  el.style.removeProperty("--skel-at");
  el.innerHTML = url ? `<img src="${url}" alt="" draggable="false">` : isPhotoView() ? ICON_PHOTO : ICON_VIDEO;
}

/** 見えるところに来た枠の画像を頼む (画面の外の枠は頼まない) */
const thumbObserver = new IntersectionObserver(
  (entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      const el = e.target as HTMLElement;
      thumbObserver.unobserve(el);
      const key = el.dataset.key!;
      if (thumbs.has(key)) {
        fillThumb(el, thumbs.get(key)!);
        continue;
      }
      el.classList.remove("wait");
      if (thumbWaiting.has(key)) continue;
      thumbWaiting.add(key);
      const id = el.dataset.id!;
      thumbQueue.push({
        key,
        load: () =>
          key.startsWith("p:") ? invoke<string | null>("photo_thumb", { id, large: false }) : invoke<string | null>("video_thumb", { id }),
      });
    }
    pumpThumbs();
  },
  { rootMargin: "200px" },
);

function thumbHtml(key: string, id: string): string {
  const url = thumbs.get(key);
  if (url !== undefined) return `<div class="vis-thumb">${url ? `<img src="${url}" alt="" draggable="false">` : isPhotoView() ? ICON_PHOTO : ICON_VIDEO}</div>`;
  return `<div class="vis-thumb skel wait" data-key="${esc(key)}" data-id="${esc(id)}"${skelStyle()}></div>`;
}

// ---------------- 描画 ----------------

/** 左の一覧で選んでいるところの動画 (検索の前) */
function nodeVideos(): VideoView[] {
  return (vis.videos ?? []).filter((v) => videoInNode(v, vis.videoNode));
}

function shownVideos(): VideoView[] {
  return sortVideos(nodeVideos().filter((v) => videoMatches(v, state.search)));
}

function shownPhotos(): PhotoView[] {
  const q = state.search.trim().toLowerCase();
  const list = photosShown();
  return q ? list.filter((p) => (p.name ?? "").toLowerCase().includes(q)) : list;
}

/** 表示中の並び (範囲の選択と全選択に使う) */
function shownIds(): string[] {
  return isVideoView() ? shownVideos().map((v) => v.id) : shownPhotos().map((p) => p.id);
}

/** 動画の長さ: 1 時間を超えたら "1:42:00"、それより短ければ "24:00" */
function fmtVideoDur(ms: number): string {
  const s = Math.round(ms / 1000);
  const h = Math.floor(s / 3600);
  return h ? `${h}:${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}` : fmtDur(ms);
}

function videoSub(v: VideoView, withDuration = true): string {
  const parts: string[] = [];
  // 番組を選んでいるときはシーズンを見出しに出すので、話数だけ
  const bySeason = vis.videoNode.startsWith("s:");
  if (v.kind === "tvshow") {
    if (v.season && v.episode && !bySeason) parts.push(tr("シーズン {s} · 第 {e} 話", { s: v.season, e: v.episode }));
    else if (v.episode) parts.push(tr("第 {e} 話", { e: v.episode }));
  } else if (v.kind === "musicvideo" && v.artist && !(vis.videoNode === "k:musicvideo" || vis.videoNode.startsWith("a:"))) parts.push(v.artist);
  if (v.durationMs && withDuration) parts.push(fmtVideoDur(v.durationMs));
  return parts.join(" · ");
}

/** ビデオの見方: サムネイル (カード) / リスト (1 行ずつ)。設定に覚える */
function videoLayout(): "grid" | "list" {
  return getStored("videoLayout") === "list" ? "list" : "grid";
}

/** 動画の題名。テレビ番組は番組名を前に付ける (番組ごとの見出しの下では付けない) */
function videoTitle(v: VideoView): string {
  const underShow = vis.videoNode === "k:tvshow" || vis.videoNode.startsWith("s:");
  return v.kind === "tvshow" && v.show && !underShow ? `${v.show} — ${v.title}` : v.title;
}

/** リストの 1 行: チェック・小さい画像・題名と補足・選んだ列 (標準は種類・長さ・大きさ) */
function videoRow(v: VideoView): string {
  const sel = vis.selected.has(v.id);
  const title = videoTitle(v);
  const sub = videoSub(v, false);
  return `<div class="vis-card vis-row ${sel ? "selected" : ""} ${vis.focused === v.id ? "focused" : ""}" data-id="${esc(v.id)}">
    <button class="vis-chk" tabindex="-1" aria-pressed="${sel}" aria-label="${esc(tr("選択"))}"><span class="box">${CHECK_MARK}</span></button>
    ${thumbHtml(videoKey(v), v.id)}
    <div class="vis-row-main"><div class="vis-title" title="${esc(title)}">${esc(title)}</div>${sub ? `<div class="vis-sub muted">${esc(sub)}</div>` : ""}</div>
    ${visCells(v)}
  </div>`;
}

function card(id: string, key: string, title: string, sub: string, extra = ""): string {
  const sel = vis.selected.has(id);
  return `<div class="vis-card ${sel ? "selected" : ""} ${vis.focused === id ? "focused" : ""}" data-id="${esc(id)}" ${extra}>
    ${thumbHtml(key, id)}
    <button class="vis-chk" tabindex="-1" aria-pressed="${sel}" aria-label="${esc(tr("選択"))}"><span class="box">${CHECK_MARK}</span></button>
    ${title ? `<div class="vis-title" title="${esc(title)}">${esc(title)}</div>` : ""}
    ${sub ? `<div class="vis-sub muted">${esc(sub)}</div>` : ""}
  </div>`;
}

/** 空のときの表示 (曲の一覧の空の表示と同じ形: アイコン・見出しの 1 行・小さい補足) */
function emptyHtml(icon: string, text: string, ...subs: string[]): string {
  return `<div class="vis-empty"><div class="drop-hint">${icon}<div>${esc(text)}</div>${subs.map((s) => `<div class="drop-hint-sub">${esc(s)}</div>`).join("")}</div></div>`;
}

function skeletonGrid(photo: boolean): string {
  const n = photo ? 12 : 6;
  return `<div class="vis-cards ${photo ? "photo" : "video"}">${Array.from(
    { length: n },
    () => `<div class="vis-card"><div class="vis-thumb skel"${skelStyle()}></div>${photo ? "" : `<div class="vis-title skel-line skel"${skelStyle()}></div>`}</div>`,
  ).join("")}</div>`;
}

function renderVideoGrid(): string {
  if (!vis.videos) return skeletonGrid(false);
  if (state.snap?.mode === "ipod" && state.snap.device.video === false)
    return emptyHtml(ICON_VIDEO, tr("この iPod は動画を再生できません"), tr("動画を再生できるのは iPod video (5G) 以降と iPod nano (3G) 以降です"));
  const list = shownVideos();
  if (!vis.videos.length)
    return emptyHtml(
      ICON_VIDEO,
      tr("ここに動画をドラッグ＆ドロップ"),
      isRockbox()
        ? tr("320×240 の MPEG-2 に変換して Videos フォルダに置きます")
        : tr("iPod で再生できない形式は H.264 に変換して送ります"),
    );
  if (!list.length && !state.search && vis.videoNode.startsWith("k:"))
    return emptyHtml(
      ICON_VIDEO,
      { movie: tr("ムービーはありません"), tvshow: tr("テレビ番組はありません"), musicvideo: tr("ミュージックビデオはありません") }[vis.videoNode.slice(2) as VideoKind],
    );
  if (!list.length) return emptyHtml(ICON_VIDEO, tr("一致する動画がありません"));
  const groups = videoSections(list);
  return groups
    .map(
      ([label, l]) =>
        `${groups.length > 1 || label !== null ? `<h3 class="vis-sec">${esc(label ?? "")}<span class="muted">${l.length}</span></h3>` : ""}
        ${
          videoLayout() === "list"
            ? `<div class="vis-cards video list">${l.map(videoRow).join("")}</div>`
            : `<div class="vis-cards video">${l.map((v) => card(v.id, videoKey(v), videoTitle(v), videoSub(v))).join("")}</div>`
        }`,
    )
    .join("");
}

/**
 * 右の見出し (並べ替えたあとの list を区切る)。すべてのビデオは種類ごと、ミュージックビデオはアーティストごと、
 * テレビ番組は番組ごと、番組はシーズンごと。見出しが 1 つにしかならないときは出さない (シーズンは 1 つでも出す)。
 * Rockbox は種類を持たない (番組のフォルダに入っているものだけテレビ番組)
 */
function videoSections(list: VideoView[]): [string | null, VideoView[]][] {
  const node = vis.videoNode;
  let key: (v: VideoView) => string;
  let label: (v: VideoView) => string;
  if (!node) {
    key = (v) => v.kind;
    label = (v) => tr(VIDEO_KINDS.find(([k]) => k === v.kind)?.[1] ?? "");
  } else if (node === "k:musicvideo") {
    key = label = (v) => v.artist || tr("アーティストなし");
  } else if (node === "k:tvshow") {
    key = label = (v) => v.show || tr("番組名なし");
  } else if (node.startsWith("s:")) {
    key = (v) => String(v.season);
    label = (v) => (v.season ? tr("シーズン {n}", { n: v.season }) : tr("シーズンなし"));
  } else {
    return [[null, list]];
  }
  // list は並べ替えてあるので、続いている間を 1 つの見出しにまとめる
  const out: [string, VideoView[]][] = [];
  let last: string | null = null;
  for (const v of list) {
    const k = key(v);
    if (k !== last) out.push([label(v), []]);
    out[out.length - 1][1].push(v);
    last = k;
  }
  if (out.length === 1 && !(node.startsWith("s:") && list[0]?.season)) return [[null, list]];
  return out;
}

/** 写真の一覧の行 (リスト): チェック・小さい画像・名前・選んだ列 (標準は撮った日・大きさ) */
function photoDate(p: PhotoView): string {
  return fmtPhotoDate(p.date);
}

/** 写真にファイル名があるか (Rockbox だけ。iPod OS の写真の DB には名前が無いので、リストは撮影日を主にする) */
function photosNamed(): boolean {
  return isRockbox();
}

function photoRow(p: PhotoView): string {
  const sel = vis.selected.has(p.id);
  const named = photosNamed();
  const name = named ? p.name || "" : photoDate(p);
  return `<div class="vis-card vis-row photo-row ${named ? "" : "noname"} ${sel ? "selected" : ""} ${vis.focused === p.id ? "focused" : ""}" data-id="${esc(p.id)}">
    <button class="vis-chk" tabindex="-1" aria-pressed="${sel}" aria-label="${esc(tr("選択"))}"><span class="box">${CHECK_MARK}</span></button>
    ${thumbHtml(photoKey(p.id), p.id)}
    <div class="vis-row-main"><div class="vis-title" title="${esc(name)}">${esc(name)}</div></div>
    ${visCells(p)}
  </div>`;
}

function renderPhotoGrid(): string {
  const lib = vis.photos;
  if (!lib) return skeletonGrid(true);
  if (!lib.supported) return emptyHtml(ICON_PHOTO, tr("この iPod は写真を表示できません"), tr("写真を表示できるのはカラー画面の iPod (iPod photo 以降) です"));
  if (!lib.photos.length && !lib.albums.length)
    return emptyHtml(
      ICON_PHOTO,
      tr("ここに写真やフォルダをドラッグ＆ドロップ"),
      isRockbox() ? tr("640×480 の JPEG にして Pictures フォルダに置きます") : tr("フォルダごと入れると、フォルダ名のアルバムになります"),
      tr("ブラウザの画像はコピーして {k} で貼り付けられます", { k: navigator.userAgent.includes("Mac") ? "⌘V" : "Ctrl+V" }),
    );
  const list = shownPhotos();
  const emptyAlbum = () =>
    emptyHtml(ICON_PHOTO, tr("このアルバムには写真がありません"), tr("ここに写真をドラッグ＆ドロップすると、このアルバムに入ります"));
  if (photoLayout() === "list") {
    if (!list.length) return state.search ? emptyHtml(ICON_PHOTO, tr("一致する写真がありません")) : emptyAlbum();
    return `<div class="vis-cards list">${list.map(photoRow).join("")}</div>`;
  }
  // サムネイル: 左で選んだアルバムの写真を並べる (アルバムの切り替えは左の一覧)
  if (!list.length) return state.search ? emptyHtml(ICON_PHOTO, tr("一致する写真がありません")) : emptyAlbum();
  return `<div class="vis-cards photo">${list.map((p) => card(p.id, photoKey(p.id), "", "")).join("")}</div>`;
}

// ---------------- 左の木 (ビデオの種類・写真のアルバム) の開け閉め ----------------

/** 閉じている行 (ビデオは "v:<選ぶところ>"、写真は "p:<アルバムの道筋>")。設定に覚える */
const treeCollapsed = new Set(storedList("visTreeCollapsed"));

const CHEVRON = '<svg class="icon-line" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg>';

interface TreeRow {
  /** data-vnode / data-album などの属性 */
  attrs: string;
  active: boolean;
  name: string;
  n: number;
  depth: number;
  icon: string;
  /** 開け閉めの印のキー */
  key: string;
  /** 親の行のキー (どれかが閉じていれば出さない) */
  parents: string[];
  /** 子がある (開け閉めの印を出す) */
  parent: boolean;
}

/** 左の木の 1 行。子のある行はサイドバーの塊と同じ ⌄ で開け閉めする。木に子のある行が 1 つでもあれば、子の無い行も同じ幅を空けてそろえる */
function treeHtml(rows: TreeRow[]): string {
  const slots = rows.some((r) => r.parent);
  return rows
    .map((r) => {
      const closed = r.parent && treeCollapsed.has(r.key);
      const hidden = r.parents.some((k) => treeCollapsed.has(k));
      const chev = !slots
        ? ""
        : r.parent
          ? `<button class="tree-chev" tabindex="-1" aria-expanded="${!closed}" aria-label="${esc(closed ? tr("開く") : tr("閉じる"))}">${CHEVRON}</button>`
          : `<span class="tree-chev none"></span>`;
      return `<li class="side-item ${r.active ? "active" : ""}${slots ? " slot" : ""}${closed ? " collapsed" : ""}${hidden ? " hidden" : ""}" ${r.attrs} data-tkey="${esc(r.key)}" data-tparents="${esc(r.parents.join("\u0001"))}" style="--depth:${r.depth}" title="${esc(r.name)}">${chev}${r.icon}<span class="label">${esc(r.name)}</span><span class="count">${r.n}</span></li>`;
    })
    .join("");
}

/** 開け閉めする。描き直さずに印と子の行だけを変える (印が回るのが見えるように)。選んでいる行が隠れたら、閉じた行を選ぶ */
function toggleTree(li: HTMLElement) {
  const key = li.dataset.tkey!;
  const closed = !treeCollapsed.has(key);
  if (closed) treeCollapsed.add(key);
  else treeCollapsed.delete(key);
  setStored("visTreeCollapsed", JSON.stringify([...treeCollapsed]));
  li.classList.toggle("collapsed", closed);
  const chev = li.querySelector(".tree-chev");
  chev?.setAttribute("aria-expanded", String(!closed));
  chev?.setAttribute("aria-label", closed ? tr("開く") : tr("閉じる"));
  let activeHidden = false;
  for (const x of li.parentElement!.querySelectorAll<HTMLElement>("li[data-tparents]")) {
    const hidden = (x.dataset.tparents ?? "").split("\u0001").some((k) => k && treeCollapsed.has(k));
    x.classList.toggle("hidden", hidden);
    if (hidden && x.classList.contains("active")) activeHidden = true;
  }
  if (!activeHidden) return;
  if (li.dataset.vnode !== undefined) openVideoNode(li.dataset.vnode);
  else if (li.dataset.album !== undefined) openAlbum(li.dataset.album || null);
}

/** 選んでいるところ (番組・アーティストならその種類) */
const nodeKind = (node: VideoNode): VideoKind | null =>
  node.startsWith("k:") ? (node.slice(2) as VideoKind) : node.startsWith("s:") ? "tvshow" : node.startsWith("a:") ? "musicvideo" : null;

/**
 * サイドバーの「ビデオ」と、その下の種類 (字下げ)。「ビデオ」の行がすべてのビデオで、開け閉めできる
 * (マウスを乗せるとアイコンが開け閉めの印に替わる)。番組・アーティストはビデオの画面の左 (`renderVideoSide`)。
 * 種類は動画が無くても出す (iPod のメニューと同じ)。Rockbox は種類を持たずミュージックビデオにならないので出さない
 */
export function videoSideHtml(count: string): string {
  const list = vis.videos ?? [];
  const on = state.view === VIDEO_VIEW;
  const closed = treeCollapsed.has("v:side");
  const row = (node: VideoNode, name: string, n: string | number, depth: number, ico: string, cls: string) =>
    `<li class="side-item${on && sideActive(node) ? " active" : ""}${cls}" data-view="${VIDEO_VIEW}" data-vnode="${esc(node)}" data-lib="1" data-tkey="${node ? `v:${esc(node)}` : "v:side"}" data-tparents="${node ? "v:side" : ""}" style="--depth:${depth}" title="${esc(name)}">${ico}<span class="label">${esc(name)}</span><span class="count">${n}</span></li>`;
  let html = row(
    "",
    tr("@side:ビデオ"),
    count,
    0,
    `<span class="side-ico">${ICON_VIDEO}<button class="tree-chev" tabindex="-1" aria-expanded="${!closed}" aria-label="${esc(closed ? tr("開く") : tr("閉じる"))}">${CHEVRON}</button></span>`,
    ` has-kids${closed ? " collapsed" : ""}`,
  );
  for (const [kind] of VIDEO_KINDS) {
    if (kind === "musicvideo" && isRockbox()) continue;
    const n = vis.videos ? list.filter((v) => v.kind === kind).length : "";
    // 英語は iPod のメニューと同じく複数形 (Movies など)
    const name = { movie: tr("@video:ムービー"), tvshow: tr("@video:テレビ番組"), musicvideo: tr("@video:ミュージックビデオ") }[kind];
    html += row(`k:${kind}`, name, n, 1, ICON_VIDEO, closed ? " hidden" : "");
  }
  return html;
}

/** サイドバーのその行を選んだ状態に見せるか (番組・アーティストを選んでいるときは、その種類の行。
 * 「ビデオ」を閉じているときは「ビデオ」の行) */
function sideActive(row: VideoNode): boolean {
  const cur = vis.videoNode;
  if (treeCollapsed.has("v:side")) return row === "";
  return row === cur || (row.startsWith("k:") && nodeKind(cur) === row.slice(2));
}

/** サイドバーの「ビデオ」の開け閉め。選んでいるところは変えない (閉じている間は「ビデオ」の行を選んだ状態に見せる) */
export function toggleVideoSide(li: HTMLElement) {
  const closed = !treeCollapsed.has("v:side");
  if (closed) treeCollapsed.add("v:side");
  else treeCollapsed.delete("v:side");
  setStored("visTreeCollapsed", JSON.stringify([...treeCollapsed]));
  li.classList.toggle("collapsed", closed);
  const chev = li.querySelector(".tree-chev");
  chev?.setAttribute("aria-expanded", String(!closed));
  chev?.setAttribute("aria-label", closed ? tr("開く") : tr("閉じる"));
  const on = state.view === VIDEO_VIEW;
  for (const x of $("lib-list").querySelectorAll<HTMLElement>("[data-vnode]")) {
    if (x.dataset.vnode) x.classList.toggle("hidden", closed);
    x.classList.toggle("active", on && sideActive(x.dataset.vnode!));
  }
}

/** ビデオの選ぶところを変える (サイドバーのビデオの行)。ビデオの画面を開いていなければ開く */
export function openVideoNode(node: VideoNode) {
  if (state.view !== VIDEO_VIEW) {
    vis.videoNode = node;
    return selectView(VIDEO_VIEW);
  }
  if (node === vis.videoNode) return;
  vis.videoNode = node;
  $("lib-list").querySelectorAll<HTMLElement>("[data-vnode]").forEach((li) => li.classList.toggle("active", sideActive(li.dataset.vnode!)));
  vis.selected.clear();
  vis.anchor = null;
  vis.focused = null;
  $("vis-grid").scrollTop = 0;
  renderVisualView();
}

/**
 * ビデオの左: テレビ番組なら番組、ミュージックビデオならアーティストの一覧 (先頭は「すべて」= その種類のすべて)。
 * 種類はサイドバーで選ぶ。すべてのビデオ・ムービー・番組やアーティストの無い種類では出さない
 */
function renderVideoSide() {
  const side = $("vis-side");
  const list = vis.videos ?? [];
  const kind = nodeKind(vis.videoNode);
  const groups = kind && kind !== "movie" ? videoGroups(list, kind) : [];
  const show = isVideoView() && groups.length > 0 && !(state.snap?.mode === "ipod" && state.snap.device.video === false);
  side.classList.toggle("hidden", !show);
  $("vis-side-resizer").classList.toggle("hidden", !show);
  if (!show || !kind) return;
  const row = (node: VideoNode, name: string, n: number, icon: string): TreeRow => ({
    attrs: `data-vnode="${esc(node)}"`,
    active: vis.videoNode === node,
    name,
    n,
    depth: 0,
    icon,
    key: `v:${node}`,
    parents: [],
    parent: false,
  });
  const tv = kind === "tvshow";
  const rows = [row(`k:${kind}`, tr("すべて"), list.filter((v) => v.kind === kind).length, ICON_VIDEO)];
  for (const g of groups) rows.push(row(`${tv ? "s:" : "a:"}${g.name}`, g.name || (tv ? tr("番組名なし") : tr("アーティストなし")), g.count, ICON_FOLDER));
  side.querySelector("ul")!.innerHTML = treeHtml(rows);
}

/** 写真の左: すべて (すべての写真)・アルバムの木 (字下げで階層)・新しいアルバム (リストとサムネイルのどちらでも出す) */
function renderPhotoSide() {
  if (isVideoView()) return renderVideoSide();
  const side = $("vis-side");
  const lib = vis.photos;
  const show = isPhotoView() && !!lib?.supported && (lib.photos.length > 0 || lib.albums.length > 0);
  side.classList.toggle("hidden", !show);
  $("vis-side-resizer").classList.toggle("hidden", !show);
  if (!show || !lib) return;
  // 開け閉めはアルバムの道筋で覚える (iPod OS は保存のたびに ID が振り直され、Rockbox は名前を変えると ID が変わる)
  const rows: TreeRow[] = [
    { attrs: `data-album=""`, active: !vis.album, name: tr("すべて"), n: lib.photos.length, depth: 0, icon: ICON_PHOTO, key: "p:", parents: [], parent: false },
  ];
  const tree = (parent: string | null, depth: number, parents: string[]) => {
    for (const a of childAlbums(parent)) {
      const key = `p:${albumLabel(a.id)}`;
      rows.push({
        attrs: `data-album="${esc(a.id)}"`,
        active: vis.album === a.id,
        name: a.name,
        n: a.photoIds.length,
        depth,
        icon: ICON_FOLDER,
        key,
        parents,
        parent: childAlbums(a.id).length > 0,
      });
      tree(a.id, depth + 1, [...parents, key]);
    }
  };
  tree(null, 0, []);
  // 開け閉めの印があるときは、「新しいアルバム…」の印の欄も空けてそろえる
  const slot = rows.some((r) => r.parent) ? `<span class="tree-chev none"></span>` : "";
  side.querySelector("ul")!.innerHTML =
    treeHtml(rows) + (writable() ? `<li class="side-item new-pl${slot ? " slot" : ""}" data-newalbum="1">${slot}${ICON_FOLDER}<span class="label">${esc(tr("新しいアルバム…"))}</span></li>` : "");
}

/** アルバムを開く (null = 一番上 / すべての写真) */
function openAlbum(id: string | null) {
  if (id === vis.album) return;
  vis.album = id;
  vis.selected.clear();
  vis.anchor = null;
  vis.focused = null;
  $("vis-grid").scrollTop = 0;
  renderVisualView();
}

function renderVisualHead() {
  for (const id of ["btn-edit-rules", "btn-reorder", "btn-album-back", "album-head-art"]) $(id).classList.add("hidden");
  document.querySelector(".view-head")!.classList.remove("album-open");
  const add = $("btn-add");
  add.classList.remove("hidden");
  if (isVideoView()) {
    $("view-title").textContent = tr("@side:ビデオ");
    const list = nodeVideos();
    $("view-count").textContent = vis.videos ? tr("{n} 本", { n: list.length.toLocaleString() }) : "";
    const ms = list.reduce((a, v) => a + v.durationMs, 0);
    const bytes = list.reduce((a, v) => a + v.size, 0);
    $("view-stats").textContent = list.length ? `${fmtTotal(ms)} · ${fmtBytes(bytes)}` : "";
    add.textContent = tr("＋ 動画を追加");
  } else {
    $("view-title").textContent = tr("@side:写真");
    const lib = vis.photos;
    const n = lib ? (vis.album ? photosShown().length : lib.photos.length) : 0;
    $("view-count").textContent = lib?.supported ? tr("{n} 枚", { n: n.toLocaleString() }) : "";
    $("view-stats").textContent = vis.album ? albumLabel(vis.album) : "";
    add.textContent = tr("＋ 写真を追加");
  }
}

export function renderVisualView() {
  const view = $("vis-view");
  view.classList.toggle("hidden", !isVisualView());
  if (!isVisualView()) return;
  for (const id of ["list-area", "album-grid", "connect-view", "view-switch", "selbar", "pod-view"]) $(id).classList.add("hidden");
  if (isVideoView() ? !vis.videos : !vis.photos) syncVisual();
  renderVisualHead();
  renderPhotoSide();
  // 見方の切り替えは、動画・写真 (かアルバム) があるときだけ
  const lib = vis.photos;
  const sw = isVideoView()
    ? !!vis.videos?.length && !(state.snap?.mode === "ipod" && state.snap.device.video === false)
    : !!lib?.supported && (lib.photos.length > 0 || lib.albums.length > 0);
  $("vis-switch").classList.toggle("hidden", !sw);
  const list = sw && layoutOf() === "list";
  renderListHead(list && shownIds().length > 0);
  if (sw) {
    for (const r of document.querySelectorAll<HTMLInputElement>('input[name="vis-layout"]')) r.checked = r.value === layoutOf();
    placeSegThumb($("vis-layout"));
  }
  const grid = $("vis-grid");
  grid.classList.toggle("photo", isPhotoView());
  grid.classList.toggle("list", list);
  grid.classList.toggle("selecting", vis.selected.size > 0);
  grid.innerHTML = isVideoView() ? renderVideoGrid() : renderPhotoGrid();
  for (const el of grid.querySelectorAll<HTMLElement>(".vis-thumb[data-key]")) thumbObserver.observe(el);
  renderVisualSelbar();
}

/** 今の表示の見方 (ビデオと写真で別々に覚える) */
function layoutOf(): "grid" | "list" {
  return isPhotoView() ? photoLayout() : videoLayout();
}

// ---------------- リストの列 ----------------
// 題名の右に並ぶ列。出す列は「表示する項目」(項目名の右端の歯車 / 項目名の右クリック) で選び、
// 幅は境目のドラッグで変える。どちらもビデオと写真で別々に覚える。題名の列が残りを埋めるので、
// 境目は列の左端に置き、左へ引くとその列が広がる

interface VisColumn<T> {
  key: string;
  label: string;
  /** 表示する項目のメニューでの名前 (項目名と違うとき) */
  menuLabel?: string;
  /** 標準の幅 (px) */
  width: number;
  num?: boolean;
  /** iPod OS だけ / Rockbox だけに出す列 (Rockbox の動画はファイルなので、題名・番組・長さ・大きさ・日付しか無い) */
  only?: "ipod" | "rockbox";
  cell: (x: T) => string;
}

// tr: label / menuLabel は使う側で訳す
const VIDEO_COLUMNS: VisColumn<VideoView>[] = [
  { key: "kind", label: "種類", width: 128, cell: (v) => tr(VIDEO_KINDS.find(([k]) => k === v.kind)?.[1] ?? "") },
  { key: "artist", label: "アーティスト", width: 160, only: "ipod", cell: (v) => v.artist },
  { key: "album", label: "アルバム", width: 160, only: "ipod", cell: (v) => v.album },
  { key: "show", label: "番組", width: 160, cell: (v) => v.show },
  { key: "season", label: "シーズン", width: 72, num: true, only: "ipod", cell: (v) => (v.season ? String(v.season) : "") },
  { key: "episode", label: "話数", width: 56, num: true, only: "ipod", cell: (v) => (v.episode ? String(v.episode) : "") },
  { key: "genre", label: "ジャンル", width: 112, only: "ipod", cell: (v) => v.genre },
  { key: "year", label: "年", width: 56, num: true, only: "ipod", cell: (v) => (v.year ? String(v.year) : "") },
  { key: "fileName", label: "ファイル名", width: 200, only: "rockbox", cell: (v) => v.rbPath?.split("/").pop() ?? "" },
  { key: "duration", label: "時間", width: 72, num: true, cell: (v) => (v.durationMs ? fmtVideoDur(v.durationMs) : "") },
  { key: "size", label: "サイズ", width: 80, num: true, cell: (v) => (v.size ? fmtBytes(v.size) : "") },
  { key: "dateAdded", label: "追加日", width: 104, num: true, only: "ipod", cell: (v) => fmtDay(v.dateAdded) },
  // Rockbox はファイルの更新日時
  { key: "modified", label: "更新日", width: 104, num: true, only: "rockbox", cell: (v) => fmtDay(v.dateAdded) },
  { key: "plays", label: "再生", menuLabel: "再生回数", width: 56, num: true, only: "ipod", cell: (v) => (v.playCount ? String(v.playCount) : "") },
];
const VIDEO_DEFAULT = ["kind", "duration", "size"];

const PHOTO_COLUMNS: VisColumn<PhotoView>[] = [
  // 名前の無い写真 (iPod OS) は撮影日が題名の列なので出さない
  { key: "date", label: "撮影日", width: 176, cell: (p) => photoDate(p) },
  { key: "resolution", label: "解像度", width: 112, num: true, cell: (p) => (p.width && p.height ? `${p.width} × ${p.height}` : "") },
  { key: "size", label: "サイズ", width: 80, num: true, cell: (p) => (p.size ? fmtBytes(p.size) : "") },
];
const PHOTO_DEFAULT = ["date", "size"];

/** 列を広げても題名の列はこれより細くしない (px) */
const VIS_TITLE_MIN = 120;

function visColumnsKey(): string {
  return isPhotoView() ? "photoColumns" : "videoColumns";
}
function visWidthsKey(): string {
  return isPhotoView() ? "photoColumnWidths" : "videoColumnWidths";
}

/** 今の見方 (ビデオ / 写真、iPod OS / Rockbox) で選べる列 */
function availableVisColumns(): VisColumn<VideoView>[] | VisColumn<PhotoView>[] {
  if (isPhotoView()) return photosNamed() ? PHOTO_COLUMNS : PHOTO_COLUMNS.filter((c) => c.key !== "date");
  const rb = isRockbox();
  return VIDEO_COLUMNS.filter((c) => !c.only || (c.only === "rockbox") === rb);
}

function shownVisKeys(): string[] {
  try {
    const v = JSON.parse(getStored(visColumnsKey()) ?? "null");
    if (Array.isArray(v)) return v as string[];
  } catch {
    /* 壊れていたら標準に戻す */
  }
  return isPhotoView() ? PHOTO_DEFAULT : VIDEO_DEFAULT;
}

/** 今出す列 (並びは表の順) */
function shownVisColumns<T extends VideoView | PhotoView>(): VisColumn<T>[] {
  const shown = new Set(shownVisKeys());
  return (availableVisColumns() as VisColumn<T>[]).filter((c) => shown.has(c.key));
}

function storedVisWidths(): Record<string, number> {
  try {
    const v = JSON.parse(getStored(visWidthsKey()) ?? "null");
    if (v && typeof v === "object" && !Array.isArray(v)) return v as Record<string, number>;
  } catch {
    /* 壊れていたら標準の幅に戻す */
  }
  return {};
}

/**
 * 列の並びを --vis-cols に入れる (項目名とリストの行で同じ)。チェック・画像・題名・選んだ列・歯車の分の空き。
 * 題名の列は残りを埋め、VIS_TITLE_MIN より細くならない
 * widths はドラッグ中の幅 (省くと覚えた幅)
 */
function applyVisColumns(widths: Record<string, number> = storedVisWidths()) {
  const thumb = isPhotoView() ? 40 : 80;
  const cols = shownVisColumns().map((c) => widths[c.key] ?? c.width);
  const root = document.documentElement.style;
  root.setProperty("--vis-cols", `24px ${thumb}px minmax(${VIS_TITLE_MIN}px, 1fr) ${cols.map((w) => `${w}px`).join(" ")} 8px`);
  // 列の最小の幅の合計 (列の間 16px と行の左右の余白 8px を含む)。狭いときはリストをこの幅にして横にスクロールさせる
  const tracks = [24, thumb, VIS_TITLE_MIN, ...cols, 8];
  root.setProperty("--vis-min", `${tracks.reduce((n, w) => n + w, 0) + 16 * (tracks.length - 1) + 16}px`);
}

/** 行の、題名より右の列 */
function visCells<T extends VideoView | PhotoView>(x: T): string {
  return shownVisColumns<T>()
    .map((c) => {
      const text = c.cell(x);
      return `<div class="${c.num ? "vis-row-num" : "vis-row-kind"} muted"${text && !c.num ? ` title="${esc(text)}"` : ""}>${esc(text)}</div>`;
    })
    .join("");
}

/** 項目名の 1 つ。key があれば幅を変えられる列 (左端に境目を付ける) */
function visTh(label: string, key?: string, num = false): string {
  const grip = key ? `<span class="col-grip at-left" data-col="${key}" title="${esc(tr("ドラッグで幅を変更 (ダブルクリックで元に戻す)"))}"></span>` : "";
  return `<div class="vis-th${num ? " num" : ""}"><span>${esc(label)}</span>${grip}</div>`;
}

/** 境目のドラッグ: 左へ引くとその列が広がり、題名の列が縮む */
function startVisResize(grip: HTMLElement, e: PointerEvent) {
  const key = grip.dataset.col!;
  const cell = grip.parentElement!;
  const title = $("vis-head").querySelector<HTMLElement>(".vis-th")!;
  e.preventDefault();
  // 項目名の文字が切れない幅より細くしない
  const min = Math.max(40, Math.ceil(cell.querySelector("span")!.getBoundingClientRect().width));
  const startW = cell.getBoundingClientRect().width;
  const max = Math.max(startW, startW + title.getBoundingClientRect().width - VIS_TITLE_MIN);
  const startX = e.clientX;
  const widths = storedVisWidths();
  let moved = false;
  grip.setPointerCapture(e.pointerId);
  grip.classList.add("dragging");
  document.body.classList.add("col-resizing");
  const move = (ev: PointerEvent) => {
    moved = true;
    widths[key] = Math.round(Math.min(max, Math.max(min, startW - (ev.clientX - startX))));
    applyVisColumns(widths);
  };
  const up = () => {
    grip.removeEventListener("pointermove", move);
    grip.removeEventListener("pointerup", up);
    grip.removeEventListener("pointercancel", up);
    grip.classList.remove("dragging");
    document.body.classList.remove("col-resizing");
    // 押して離しただけ (ダブルクリックの 1 回目など) では覚えない
    if (moved) setStored(visWidthsKey(), JSON.stringify(widths));
  };
  grip.addEventListener("pointermove", move);
  grip.addEventListener("pointerup", up);
  grip.addEventListener("pointercancel", up);
}

/** 境目のダブルクリック: その列を標準の幅に戻す */
function resetVisWidth(grip: HTMLElement) {
  const w = storedVisWidths();
  delete w[grip.dataset.col!];
  setStored(visWidthsKey(), Object.keys(w).length ? JSON.stringify(w) : null);
  applyVisColumns();
}

/** 表示する項目のメニュー (項目名の右端の歯車 / 項目名の右クリック) */
function openVisColumnMenu(x: number, y: number) {
  const items: MenuItem[] = [{ header: tr("表示する項目") }];
  const title = isPhotoView() ? (photosNamed() ? tr("名前") : tr("撮影日")) : tr("タイトル");
  items.push({ label: title, checked: true, disabled: true, keepOpen: true, action: () => undefined });
  for (const c of availableVisColumns()) {
    items.push({
      label: tr(c.menuLabel ?? c.label),
      checked: shownVisKeys().includes(c.key),
      keepOpen: true,
      action: () => {
        const shown = new Set(shownVisKeys());
        if (shown.has(c.key)) shown.delete(c.key);
        else shown.add(c.key);
        // 表の順で覚える (今のモードに無い列も、選んであれば残す)
        const order = (isPhotoView() ? PHOTO_COLUMNS : VIDEO_COLUMNS).map((x) => x.key);
        setStored(visColumnsKey(), JSON.stringify(order.filter((k) => shown.has(k))));
        renderVisualView();
      },
    });
  }
  items.push(
    { sep: true },
    {
      label: tr("標準に戻す"),
      action: () => {
        setStored(visColumnsKey(), null);
        renderVisualView();
      },
    },
    {
      label: tr("列の幅を標準に戻す"),
      disabled: Object.keys(storedVisWidths()).length === 0,
      action: () => {
        setStored(visWidthsKey(), null);
        applyVisColumns();
      },
    },
  );
  showMenu(x, y, items);
}

/** リストの項目名: 全選択のチェック・題名 (ビデオはタイトル、写真は名前か撮影日)・選んだ列・表示する項目の歯車 */
function renderListHead(show: boolean) {
  const head = $("vis-head");
  head.classList.toggle("hidden", !show);
  if (!show) return;
  applyVisColumns();
  const chk = `<div class="th chk" data-selall="1" title="${esc(isPhotoView() ? tr("表示中の写真をすべて選択 / 解除") : tr("表示中の動画をすべて選択 / 解除"))}"><span class="box"></span></div><div></div>`;
  const title = isPhotoView() ? (photosNamed() ? tr("名前") : tr("撮影日")) : tr("タイトル");
  const cols = shownVisColumns()
    .map((c) => visTh(tr(c.label), c.key, c.num))
    .join("");
  const colset = `<div class="vis-colset" data-colset="1" title="${esc(tr("表示する項目"))}"><button class="btn icon ghost mini" aria-label="${esc(tr("表示する項目"))}">${ICON_COLUMNS}</button></div>`;
  head.innerHTML = `${chk}${visTh(title)}${cols}${colset}`;
  updateHeadCheck();
}

/** 項目名のチェック: 全部選んだ / 一部 / なし */
function updateHeadCheck() {
  const box = document.querySelector<HTMLElement>("#vis-head [data-selall]");
  if (!box) return;
  const ids = shownIds();
  const n = ids.filter((id) => vis.selected.has(id)).length;
  box.classList.toggle("all", n > 0 && n === ids.length);
  box.classList.toggle("some", n > 0 && n < ids.length);
}

/** 選択だけを描き直す (カードは作り直さない) */
function renderSelection(anim?: Set<string>) {
  for (const el of $("vis-grid").querySelectorAll<HTMLElement>(".vis-card[data-id]")) {
    const on = vis.selected.has(el.dataset.id!);
    el.classList.toggle("selected", on);
    el.classList.toggle("focused", vis.focused === el.dataset.id);
    el.querySelector(".vis-chk")?.setAttribute("aria-pressed", String(on));
    if (on && anim?.has(el.dataset.id!) && !reduceMotion) {
      el.classList.remove("chk-anim");
      void el.offsetWidth;
      el.classList.add("chk-anim");
    }
  }
  $("vis-grid").classList.toggle("selecting", vis.selected.size > 0);
  updateHeadCheck();
  renderVisualSelbar();
}

function renderVisualSelbar() {
  const bar = $("vis-selbar");
  const n = vis.selected.size;
  bar.classList.toggle("hidden", !isVisualView() || n === 0);
  if (!isVisualView() || !n) return;
  const ro = writable() ? "" : "disabled";
  const count = isVideoView() ? tr("{n} 本を選択中", { n }) : tr("{n} 枚を選択中", { n });
  const inAlbum = isPhotoView() && vis.album;
  bar.innerHTML = `<span class="sel-count">${esc(count)}</span><div class="spacer"></div>
    ${isVideoView() ? `<button class="btn" data-act="edit" ${ro}>${esc(tr("情報を編集"))}</button>` : `<button class="btn" data-act="album" ${ro}>${esc(tr("アルバムに追加"))}</button>`}
    ${inAlbum ? `<button class="btn" data-act="unalbum" ${ro}>${esc(tr("アルバムから外す"))}</button>` : ""}
    <button class="btn" data-act="export">${esc(tr("PC に書き出す"))}</button>
    <button class="btn danger" data-act="remove" ${ro}>${esc(tr("iPod から削除"))}</button>
    <button class="btn ghost" data-act="clear">${esc(tr("選択を解除"))}</button>`;
}

// ---------------- 選ぶ ----------------

/** 範囲の起点を置く (Shift での範囲チェックは、この時点のチェックに範囲を足したもの) */
function setVisAnchor(id: string) {
  vis.anchor = id;
  vis.checkBase = new Set(vis.selected);
}

/**
 * クリック (曲の一覧と同じ): 普通のクリックはカーソルを置くだけでチェックは変えない。Shift は起点からの範囲をチェック、
 * Ctrl / ⌘ とチェックの欄はその 1 つを付け外す (チェックの欄で Shift なら範囲)
 */
function select(id: string, e: MouseEvent | KeyboardEvent, fromCheck: boolean) {
  const ids = shownIds();
  const before = new Set(vis.selected);
  if (e.shiftKey && vis.anchor && ids.includes(vis.anchor)) {
    const [a, b] = [ids.indexOf(vis.anchor), ids.indexOf(id)];
    vis.selected.clear();
    for (const x of vis.checkBase) vis.selected.add(x);
    for (const x of ids.slice(Math.min(a, b), Math.max(a, b) + 1)) vis.selected.add(x);
  } else if (fromCheck || e.ctrlKey || e.metaKey) {
    if (vis.selected.has(id)) vis.selected.delete(id);
    else vis.selected.add(id);
    setVisAnchor(id);
  } else {
    setVisAnchor(id);
  }
  vis.focused = id;
  renderSelection(new Set([...vis.selected].filter((x) => !before.has(x))));
}

/** 操作の対象: チェックしたもの。無ければカーソルのもの */
function targetIds(): string[] {
  const checked = selectedIds();
  if (checked.length) return checked;
  return vis.focused && shownIds().includes(vis.focused) ? [vis.focused] : [];
}

function selectedIds(): string[] {
  const order = shownIds();
  return order.filter((id) => vis.selected.has(id));
}

// ---------------- 操作 ----------------

/** 書き出し先のフォルダのダイアログは Rust が開く (やめたら null が返る) */
async function exportSelected(ids = selectedIds()) {
  if (!ids.length) return;
  if (isVideoView()) {
    const n = await call<number | null>("export_videos", { ids });
    if (n != null) toast(tr("{n} 本の動画を書き出しました", { n }));
  } else {
    const n = await call<number | null>("export_photos", { ids });
    if (n != null) toast(isRockbox() ? tr("{n} 枚の写真を書き出しました", { n }) : tr("{n} 枚の写真を書き出しました (iPod に置いた縮小した画像です)", { n }));
  }
}

async function removeSelected(ids = selectedIds()) {
  if (!ids.length || !writable()) return;
  const video = isVideoView();
  const ok = await confirmBox(
    video ? tr("{n} 本の動画を iPod から削除しますか？", { n: ids.length }) : tr("{n} 枚の写真を iPod から削除しますか？", { n: ids.length }),
    video ? tr("iPod の動画のファイルも消します。元に戻せません。") : tr("すべてのアルバムから消えます。元に戻せません。"),
    tr("削除"),
    true,
  );
  if (!ok) return;
  if (video) {
    const r = await call<{ videos: VideoView[]; snapshot: Snapshot }>("remove_videos", { ids });
    if (!r) return;
    vis.selected.clear();
    setVideos(r.videos);
    applySnapshot(r.snapshot);
  } else {
    const r = await call<{ library: PhotoLibrary; snapshot: Snapshot }>("remove_photos", { ids });
    if (!r) return;
    vis.selected.clear();
    applyPhotoEdit(r);
  }
}

function applyPhotoEdit(r: { library: PhotoLibrary; snapshot: Snapshot }) {
  clearPhotoThumbs();
  setPhotos(r.library);
  applySnapshot(r.snapshot);
}

/** 写真の操作 (アルバムなど) を呼んで、一覧を描き直す。選択は ID が振り直されるので外す */
async function photoOp(cmd: string, args: Record<string, unknown>): Promise<{ library: PhotoLibrary; snapshot: Snapshot; albumId?: string } | null> {
  const r = await call<{ library: PhotoLibrary; snapshot: Snapshot; albumId?: string }>(cmd, args);
  if (!r) return null;
  vis.selected.clear();
  applyPhotoEdit(r);
  return r;
}

/** アルバムを作る。`parent` の中に作る (Rockbox だけ。iPod OS のアルバムは平ら) */
async function newAlbum(ids: string[], parent: string | null = isRockbox() ? vis.album : null) {
  const name = await promptText(tr("新しいアルバム"), "", tr("作成"));
  if (!name) return;
  const r = await photoOp("create_photo_album", { name, parent, ids });
  if (r?.albumId && ids.length) toast(tr("「{a}」に {n} 枚を入れました", { a: name, n: ids.length }));
}

function albumMenuItems(ids: string[]): MenuItem[] {
  const albums = vis.photos?.albums ?? [];
  return [
    ...albums
      .filter((a) => a.id !== vis.album)
      .sort((a, b) => albumLabel(a.id).localeCompare(albumLabel(b.id)))
      .map((a) => ({
        label: albumLabel(a.id),
        action: async () => {
          if (await photoOp("add_to_photo_album", { id: a.id, ids })) toast(tr("「{a}」に {n} 枚を入れました", { a: a.name, n: ids.length }));
        },
      })),
    ...(albums.length ? [{ sep: true } as MenuItem] : []),
    { label: tr("新しいアルバム…"), action: () => newAlbum(ids) },
  ];
}

async function removeFromAlbum(ids = selectedIds()) {
  if (!vis.album) return;
  await photoOp("remove_from_photo_album", { id: vis.album, ids });
}

function albumContextMenu(x: number, y: number, id: string) {
  const a = vis.photos?.albums.find((x) => x.id === id);
  if (!a) return;
  showMenu(x, y, [
    { header: albumLabel(a.id) },
    ...(isRockbox() ? [{ label: tr("この中に新しいアルバム…"), disabled: !writable(), action: () => void newAlbum([], id) }] : []),
    {
      label: tr("名前を変更…"),
      disabled: !writable(),
      action: async () => {
        const name = await promptText(tr("アルバムの名前"), a.name, tr("変更"));
        if (name && name !== a.name) await photoOp("rename_photo_album", { id, name });
      },
    },
    { sep: true },
    {
      label: tr("アルバムを削除"),
      danger: true,
      disabled: !writable(),
      action: async () => {
        const ok = await confirmBox(
          tr("「{a}」を削除しますか？", { a: a.name }),
          isRockbox() ? tr("写真と中のアルバムは削除せず、1 つ上の階層に移します。") : tr("写真は削除しません (「すべて」に残ります)。"),
          tr("削除"),
          true,
        );
        if (ok) await photoOp("delete_photo_album", { id });
      },
    },
  ]);
}

// ---------------- 動画の再生 ----------------

/** 表示中の動画の並びで、その動画から再生する (前後の動画へも移れる) */
function playVideo(id: string) {
  const list = shownVideos();
  thumbsPaused = true;
  openVideoPlayer(list, list.findIndex((v) => v.id === id), videoTitle, () => {
    thumbsPaused = false;
    pumpThumbs();
  });
}

// ---------------- 写真を大きく見る ----------------

function openViewer(startId: string) {
  const list = shownPhotos();
  let i = Math.max(0, list.findIndex((p) => p.id === startId));
  const back = document.createElement("div");
  back.className = "vis-viewer";
  back.innerHTML = `<button class="vis-viewer-nav btn icon ghost prev" aria-label="${esc(tr("前の写真"))}">${ICON_CHEVRON_LEFT}</button>
    <div class="vis-viewer-stage"><span class="spinner"></span><img alt=""></div>
    <button class="vis-viewer-nav btn icon ghost next" aria-label="${esc(tr("次の写真"))}">${ICON_CHEVRON_RIGHT}</button>
    <div class="vis-viewer-info"></div>
    <button class="vis-viewer-close btn icon ghost" aria-label="${esc(tr("閉じる"))}" title="${esc(tr("閉じる"))} (Esc)">${ICON_CLOSE}</button>`;
  const img = back.querySelector("img")!;
  const info = back.querySelector<HTMLElement>(".vis-viewer-info")!;
  let shown = "";
  const show = async () => {
    const p = list[i];
    if (!p) return;
    shown = p.id;
    img.classList.add("loading");
    const date = photoDate(p);
    info.textContent = [`${i + 1} / ${list.length}`, p.name ?? "", date].filter(Boolean).join("  ·  ");
    const url = await invoke<string | null>("photo_thumb", { id: p.id, large: true }).catch(() => null);
    if (shown !== p.id) return;
    if (url) img.src = url;
    img.classList.remove("loading");
  };
  const go = (d: number) => {
    i = (i + d + list.length) % list.length;
    show();
  };
  const close = () => {
    back.remove();
    document.removeEventListener("keydown", onKey, true);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") close();
    else if (e.key === "ArrowLeft") go(-1);
    else if (e.key === "ArrowRight") go(1);
    else return;
    e.preventDefault();
    e.stopPropagation();
  };
  back.querySelector(".prev")!.addEventListener("click", () => go(-1));
  back.querySelector(".next")!.addEventListener("click", () => go(1));
  back.querySelector(".vis-viewer-close")!.addEventListener("click", close);
  back.addEventListener("mousedown", (e) => {
    if (e.target === back) close();
  });
  document.addEventListener("keydown", onKey, true);
  $("modal-root").appendChild(back);
  show();
}

// ---------------- メニュー ----------------

/** 右クリックのメニュー。ids はチェックしたもの、チェックしていないものを右クリックしたときはその 1 つ */
function cardMenu(x: number, y: number, ids: string[]) {
  if (!ids.length) return;
  const ro = !writable();
  const items: MenuItem[] = [];
  if (isVideoView()) {
    items.push({ label: tr("再生"), action: () => playVideo(ids[0]) }, { label: tr("情報を編集…"), disabled: ro, action: () => editVideos(ids) });
    const list = (vis.videos ?? []).filter((v) => ids.includes(v.id));
    if (videoArtEditable() && list.length) items.push({ label: tr("アートワーク"), submenu: videoArtMenuItems(list) });
  } else {
    if (ids.length === 1) items.push({ label: tr("大きく表示"), action: () => openViewer(ids[0]) });
    items.push({ label: tr("アルバムに追加"), disabled: ro, submenu: ro ? undefined : albumMenuItems(ids) });
    if (vis.album) items.push({ label: tr("アルバムから外す"), disabled: ro, action: () => void removeFromAlbum(ids) });
  }
  items.push(
    { label: tr("PC に書き出す…"), action: () => void exportSelected(ids) },
    { sep: true },
    { label: tr("iPod から削除…"), danger: true, disabled: ro, action: () => void removeSelected(ids) },
  );
  showMenu(x, y, items);
}

// ---------------- 結び付け ----------------

export function bindVisual() {
  bindResizer($("vis-side-resizer"), "--photo-side-w", "photoSideWidth", ARTIST_MIN, ARTIST_MAX);
  document.addEventListener("paste", (e) => void pasteImages(e));
  // 列の幅: 項目名の境目をドラッグ。ダブルクリックでその列を標準の幅に戻す
  $("vis-head").addEventListener("pointerdown", (e) => {
    const grip = (e.target as HTMLElement).closest<HTMLElement>(".col-grip");
    if (grip && e.button === 0) startVisResize(grip, e);
  });
  $("vis-head").addEventListener("dblclick", (e) => {
    const grip = (e.target as HTMLElement).closest<HTMLElement>(".col-grip");
    if (grip) resetVisWidth(grip);
  });
  // 横のスクロールバーはリストだけに出し、項目名はリストに合わせて動かす
  $("vis-grid").addEventListener("scroll", () => {
    $("vis-head").scrollLeft = $("vis-grid").scrollLeft;
  });
  $("vis-head").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openVisColumnMenu(e.clientX, e.clientY);
  });
  $("vis-head").addEventListener("click", (e) => {
    const colset = (e.target as HTMLElement).closest<HTMLElement>("[data-colset]");
    if (colset) {
      const r = colset.getBoundingClientRect();
      openVisColumnMenu(r.right - 220, r.bottom + 4);
      return;
    }
    if (!(e.target as HTMLElement).closest("[data-selall]")) return;
    const ids = shownIds();
    const all = ids.every((id) => vis.selected.has(id));
    for (const id of ids) {
      if (all) vis.selected.delete(id);
      else vis.selected.add(id);
    }
    renderSelection(all ? undefined : new Set(ids));
  });
  $("vis-layout").addEventListener("change", (e) => {
    setStored(isPhotoView() ? "photoLayout" : "videoLayout", (e.target as HTMLInputElement).value);
    vis.selected.clear();
    renderVisualView();
  });
  const grid = $("vis-grid");
  grid.addEventListener("click", (e) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>(".vis-card[data-id]");
    // 空白のクリックではカーソルだけ外す (チェックは消さない。曲の一覧と同じ)
    if (!el) {
      if (vis.focused && !(e.target as HTMLElement).closest("button")) {
        vis.focused = null;
        renderSelection();
      }
      return;
    }
    select(el.dataset.id!, e, !!(e.target as HTMLElement).closest(".vis-chk"));
  });
  grid.addEventListener("dblclick", (e) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>(".vis-card[data-id]");
    if (!el || (e.target as HTMLElement).closest(".vis-chk")) return;
    if (isPhotoView()) openViewer(el.dataset.id!);
    else playVideo(el.dataset.id!);
  });
  grid.addEventListener("contextmenu", (e) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>(".vis-card[data-id]");
    if (!el) return;
    e.preventDefault();
    // チェックしたものならチェックしたものすべて、それ以外はそれだけ (チェックは変えず、カーソルを置く)
    const id = el.dataset.id!;
    if (vis.selected.has(id)) return cardMenu(e.clientX, e.clientY, selectedIds());
    vis.focused = id;
    setVisAnchor(id);
    renderSelection();
    cardMenu(e.clientX, e.clientY, [id]);
  });
  const side = $("vis-side");
  side.addEventListener("click", (e) => {
    const it = (e.target as HTMLElement).closest<HTMLElement>(".side-item");
    if (!it) return;
    if ((e.target as HTMLElement).closest(".tree-chev:not(.none)")) return toggleTree(it);
    if (it.dataset.vnode !== undefined) return openVideoNode(it.dataset.vnode);
    if (it.dataset.newalbum) return void newAlbum([], null);
    openAlbum(it.dataset.album || null);
  });
  side.addEventListener("contextmenu", (e) => {
    const id = (e.target as HTMLElement).closest<HTMLElement>("[data-album]")?.dataset.album;
    if (!id) return;
    e.preventDefault();
    albumContextMenu(e.clientX, e.clientY, id);
  });
  $("vis-selbar").addEventListener("click", (e) => {
    const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-act]");
    if (!b || b.disabled) return;
    switch (b.dataset.act) {
      case "edit":
        return editVideos(selectedIds());
      case "album": {
        const r = b.getBoundingClientRect();
        return showMenu(r.left, r.top - 8, albumMenuItems(selectedIds()));
      }
      case "unalbum":
        return void removeFromAlbum();
      case "export":
        return void exportSelected();
      case "remove":
        return void removeSelected();
      case "clear":
        vis.selected.clear();
        return renderSelection();
    }
  });
}

/** 貼り付け: 写真の表示で、コピーした画像 (ブラウザの「画像をコピー」など) を写真として送る */
async function pasteImages(e: ClipboardEvent) {
  if (!isPhotoView() || (e.target as HTMLElement).closest("input, textarea, .modal")) return;
  const files = [...(e.clipboardData?.items ?? [])].filter((i) => i.kind === "file" && i.type.startsWith("image/")).map((i) => i.getAsFile()).filter((f) => !!f);
  if (!files.length) return;
  e.preventDefault();
  await sendImages(files);
}

/** 「＋ 写真を追加」の「コピーした画像を貼り付け」: クリップボードを読む (読めなければキーでの貼り付けを案内する) */
export async function pasteFromClipboard() {
  const key = navigator.userAgent.includes("Mac") ? "⌘V" : "Ctrl+V";
  try {
    const items = await navigator.clipboard.read();
    const blobs: Blob[] = [];
    for (const it of items) {
      const type = it.types.find((t) => t.startsWith("image/"));
      if (type) blobs.push(await it.getType(type));
    }
    if (blobs.length) return void (await sendImages(blobs));
    toast(tr("コピーした画像がありません。ブラウザで画像を右クリックして「画像をコピー」を選んでください"));
  } catch {
    toast(tr("ブラウザの画像はコピーして {k} で貼り付けられます", { k: key }));
  }
}

async function sendImages(files: Blob[]) {
  if (!state.snap || !writable()) return void toast(tr("この iPod には書き込めません"), "error");
  const paths: string[] = [];
  for (const f of files) {
    const bytes = new Uint8Array(await f.arrayBuffer());
    let bin = "";
    for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    const ext = f.type.split("/")[1]?.replace("jpeg", "jpg") ?? "png";
    const path = await call<string>("save_pasted_image", { data: btoa(bin), ext });
    if (path) paths.push(path);
  }
  if (paths.length) await startTransfer(paths, null);
}

/** 動画・写真の表示でのキー操作 (bind.ts の全体のキー操作から。入力欄の中では呼ばれない) */
export function visualKey(e: KeyboardEvent) {
  const mod = e.ctrlKey || e.metaKey;
  if (mod && e.key.toLowerCase() === "a") {
    e.preventDefault();
    for (const id of shownIds()) vis.selected.add(id);
    renderSelection();
  } else if (mod && e.key.toLowerCase() === "f") {
    e.preventDefault();
    $("search").focus();
  } else if (e.key === "Escape" && (vis.selected.size || vis.focused)) {
    // チェックがあれば外し、無ければカーソルを外す
    if (vis.selected.size) vis.selected.clear();
    else vis.focused = null;
    renderSelection();
  } else if ((e.key === "Delete" || (e.key === "Backspace" && mod)) && targetIds().length) {
    e.preventDefault();
    removeSelected(targetIds());
  } else if (isEnter(e) && targetIds().length) {
    e.preventDefault();
    const id = vis.focused && targetIds().includes(vis.focused) ? vis.focused : targetIds()[0];
    if (isPhotoView()) openViewer(id);
    else playVideo(id);
  }
}

/** 表示を切り替えたとき (sidebar の selectView から) */
export function onVisualViewSelected() {
  vis.selected.clear();
  vis.anchor = null;
  vis.focused = null;
  $("vis-grid").scrollTop = 0;
  renderHeader();
}

export { PHOTO_VIEW, VIDEO_VIEW };
