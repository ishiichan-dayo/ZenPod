// アルバム表示 (カードを作り直さずに印だけ直す: gridGroups)

import { lang, t as tr } from "../i18n";
import { invoke } from "../infra/tauri";
import { albumGroups } from "../app/rows";
import type { AlbumGroup } from "../app/rows";
import { $, state } from "../app/state";
import { esc } from "./dom";
import { replayClass, withTransition } from "./feedback";
import { ICON_LIB } from "./icons";
import { CHECK_MARK, ICON_PAUSE, ICON_PLAY, eqBars, player } from "./player";
import { renderAll } from "./render";
import { renderSelbar } from "./selbar";

/** アルバムのジャケット (大きめ)。"i:<画像 ID>" / "p:<場所>" → data URL (null = なし) */
export const albumArtCache = new Map<string, string | null>();
let albumArtObserver: IntersectionObserver | null = null;

export function albumArtSource(g: AlbumGroup): string | null {
  const pc = g.tracks.find((t) => t.pcPath);
  if (pc) return `p:${pc.pcPath}`;
  const withArt = g.tracks.find((t) => t.artworkId);
  return withArt && state.snap?.artworkSupported ? `i:${withArt.artworkId}` : null;
}

/** スケルトンの光の動き (1.4 秒) を、作った時刻に合わせて途中から始める。読み込み中にカードを作り直しても動きが飛ばない */
export function skelStyle(): string {
  return ` style="--skel-at:-${Math.round(performance.now() % 1400)}ms"`;
}

/** ジャケットをまだ読んでいない (初めて開く途中は、ジャケットの割り当てがまだなので全部) */
export function artLoading(src: string | null): boolean {
  return state.partialOpen || (!!src && !albumArtCache.has(src));
}

export async function loadAlbumArt(src: string): Promise<string | null> {
  if (albumArtCache.has(src)) return albumArtCache.get(src)!;
  const url = src.startsWith("p:")
    ? await invoke<string | null>("pc_album_art", { path: src.slice(2) }).catch(() => null)
    : await invoke<string | null>("get_artwork", { imageId: Number(src.slice(2)), large: true }).catch(() => null);
  // data URL (1 枚 30KB ほど) のまま HTML に入れると、アルバムが多いと描き直すたびに何十 MB も作ることになる。
  // Blob の URL (数十バイト) に替えて使い回す
  const blob = url ? dataUrlToObjectUrl(url) : null;
  albumArtCache.set(src, blob);
  return blob;
}

export function dataUrlToObjectUrl(url: string): string {
  const [head, b64] = url.split(",", 2);
  const mime = head.match(/^data:([^;]+)/)?.[1] ?? "image/jpeg";
  const bin = atob(b64 ?? "");
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return URL.createObjectURL(new Blob([bytes], { type: mime }));
}

/** 最後に作ったアルバムの一覧 (同じなら、カードは作り直さずに選択と再生中の印だけ直す) */
let gridGroups: AlbumGroup[] | null = null;
/** 描いたカードの中身の控え (アルバム・曲数・ジャケット)。一覧を計算し直しても同じなら、カードは作り直さない */
let gridSig = "";
let gridLang = "";

/** カードの選択の印: 全曲を選んでいれば " selected"、一部なら " some" */
function albumSelClass(g: AlbumGroup): string {
  let n = 0;
  for (const t of g.tracks) if (state.selected.has(t.id)) n++;
  return n === 0 ? "" : n === g.tracks.length ? " selected" : " some";
}

/** ジャケットの左上のチェック (アルバムの全曲を選ぶ / 外す) */
function albumCheck(): string {
  return `<button class="album-chk" title="${esc(tr("このアルバムの曲を選択 / 解除"))}" aria-label="${esc(tr("このアルバムの曲を選択 / 解除"))}"><span class="box">${CHECK_MARK}</span></button>`;
}

/** アルバムの全曲を選ぶ (全部選んでいれば外す) */
export function toggleAlbumSelection(i: number) {
  const g = albumGroups()[i];
  if (!g) return;
  const all = g.tracks.every((t) => state.selected.has(t.id));
  for (const t of g.tracks) {
    if (all) state.selected.delete(t.id);
    else state.selected.add(t.id);
  }
  renderAlbumGrid();
  const card = document.querySelector<HTMLElement>(`.album-card[data-album="${i}"]`);
  if (card && !all) replayClass(card, "chk-anim");
  renderSelbar();
}

/** 再生ボタン (再生中のアルバムは、動くバーと一時停止 / 再開) */
function albumPlayButton(playing: boolean): string {
  // マウスを乗せると一時停止 / 再開 (どちらを出すかは CSS)
  return playing
    ? `<button class="album-play playing" data-toggle="1" title="${esc(tr("一時停止 / 再開"))}" aria-label="${esc(tr("一時停止 / 再開"))}">${eqBars()}<span class="i-pause">${ICON_PAUSE}</span><span class="i-play">${ICON_PLAY}</span></button>`
    : `<button class="album-play" title="${esc(tr("このアルバムを再生"))}" aria-label="${esc(tr("このアルバムを再生"))}">${ICON_PLAY}</button>`;
}

/** アルバムの一覧は、見えている行と上下 GRID_BUF 行のカードだけを置く (1000 枚を一度に作ると、遅い PC で 0.5 秒ほど止まっていた)。
 * 列の数は前の CSS (auto-fill, minmax(150px, 1fr)) と同じ計算。カードは位置を計算して置き (.album-canvas の中に絶対位置で)、
 * スクロールでは見えなくなったカードを外して新しく見えるカードだけを足す (曲の一覧の renderRows と同じ)。1 行の高さは描いたカードで測る */
const GRID_MIN = 150;
export const GRID_GAP = 16;
export const GRID_PAD = 16;
const GRID_BUF = 4;
/** カードのうち、ジャケット (列の幅と同じ正方形) を除いた高さ。描いたカードで測り直す */
export let gridText = 48;
/** 置いているカードの範囲 (番号) と、そのときの列の数・列の幅 */
export let gridRange = { first: -1, last: -1, cols: 0, colW: 0 };

/** 読み込み中のアルバムの一覧: 本物と同じ大きさのカードで、ジャケット・アルバム名・アーティストの所をスケルトンにする */
export function renderAlbumGridSkeleton() {
  const el = $("album-grid");
  const w = el.clientWidth - GRID_PAD * 2;
  if (w <= 0) return;
  const cols = Math.max(1, Math.floor((w + GRID_GAP) / (GRID_MIN + GRID_GAP)));
  const colW = (w - GRID_GAP * (cols - 1)) / cols;
  const pitch = colW + gridText + GRID_GAP;
  const n = cols * Math.max(1, Math.ceil(el.clientHeight / pitch));
  // 本物を描くときは作り直す
  gridGroups = null;
  gridSig = "";
  const sig = `skel:${cols}:${Math.round(colW)}`;
  if (el.dataset.skel === sig) return;
  el.dataset.skel = sig;
  const at = skelStyle();
  const cards = Array.from({ length: n }, (_, i) => {
    const w1 = 45 + ((i * 37) % 40);
    const w2 = 30 + ((i * 53) % 35);
    return `<div class="album-card skel-card" style="left:${(i % cols) * (colW + GRID_GAP)}px;top:${Math.floor(i / cols) * pitch}px;width:${colW + 12}px">
        <div class="album-art"><span class="album-noart skel"${at}></span></div>
        <div class="album-name"><span class="sk skel" style="width:${w1}%"></span></div>
        <div class="album-sub"><span class="sk skel" style="width:${w2}%"></span></div>
      </div>`;
  }).join("");
  el.innerHTML = `<div class="album-canvas" style="height:${Math.ceil(n / cols) * pitch - GRID_GAP}px"${at}>${cards}</div>`;
}

export function renderAlbumGrid(scrollOnly = false, retry = 0) {
  delete $("album-grid").dataset.skel;
  const groups = albumGroups();
  const el = $("album-grid");
  // 曲を選んでいる間は、どのカードにもチェックを出す (続けて選びやすいように)
  el.classList.toggle("selecting", state.selected.size > 0);
  const w = el.clientWidth - GRID_PAD * 2;
  const cols = Math.max(1, Math.floor((w + GRID_GAP) / (GRID_MIN + GRID_GAP)));
  const colW = (w - GRID_GAP * (cols - 1)) / cols;
  const pitch = colW + gridText + GRID_GAP;
  const rowsN = Math.ceil(groups.length / cols);
  const top = Math.max(0, el.scrollTop - GRID_PAD);
  const first = Math.max(0, Math.floor(top / pitch) - GRID_BUF) * cols;
  const last = Math.min(groups.length, (Math.ceil((top + el.clientHeight) / pitch) + GRID_BUF) * cols);
  // 一覧が同じなら (選択・再生の切り替え・曲の一覧から戻ったときなど)、置いてあるカードはそのまま使う。
  // 曲の一覧から戻ると一覧は計算し直されるが、中身が同じなら作り直さない
  // 初めて開く途中かどうかも入れる (途中はジャケットが全部スケルトン。読み終わったら、ジャケットの無いアルバムも描き直す)
  const sig =
    groups === gridGroups
      ? gridSig
      : (state.partialOpen ? "partial\u0002" : "") + groups.map((g) => `${g.key}\u0001${g.tracks.length}\u0001${albumArtSource(g) ?? ""}`).join("\u0002");
  const same = sig === gridSig && gridLang === lang && gridRange.cols === cols && Math.abs(gridRange.colW - colW) < 0.01 && !!el.firstElementChild;
  gridGroups = groups;
  const card = (i: number) => {
    const g = groups[i];
    const src = albumArtSource(g);
    const url = src ? albumArtCache.get(src) : null;
    // 読み込み中 (初めて開く途中はジャケットがまだ無い) はスケルトン。読んで無かったときだけ音符
    const art = url
      ? `<img src="${url}" alt="">`
      : artLoading(src)
        ? `<span class="album-noart skel"${skelStyle()}></span>`
        : `<span class="album-noart">${ICON_LIB}</span>`;
    const sel = albumSelClass(g) + (i === state.gridFocus ? " kb-focus" : "");
    // 再生中の曲があるアルバムは、再生ボタンを常に出して動くバーにする
    const playBtn = albumPlayButton(!!player.current && g.tracks.some((t) => t.id === player.current));
    // カードは上下左右に 6px はみ出す (padding と負の margin) ので、幅はその分を足す。
    // キーには区切りの制御文字が入っていて HTML の属性に入れると化けるので、番号で指す
    return `<div class="album-card${sel}" data-album="${i}" title="${esc(g.album || tr("アルバムなし"))}" style="left:${(i % cols) * (colW + GRID_GAP)}px;top:${Math.floor(i / cols) * pitch}px;width:${colW + 12}px">
        <div class="album-art"${src && !albumArtCache.has(src) ? ` data-src="${esc(src)}"` : ""}>${art}
          ${albumCheck()}
          ${playBtn}
        </div>
        <div class="album-name">${esc(g.album || tr("アルバムなし"))}</div>
        <div class="album-sub">${[esc(g.artist), g.rank === 1 ? "EP" : g.rank === 2 ? tr("シングル") : "", esc(tr("{n} 曲", { n: g.tracks.length }))]
          .filter(Boolean)
          .join(" · ")}</div>
      </div>`;
  };
  let canvas = el.firstElementChild as HTMLElement | null;
  const prev = gridRange;
  if (same && canvas && first < prev.last && last > prev.first) {
    if (first === prev.first && last === prev.last) {
      if (scrollOnly) return;
      // 選択・再生中の印だけ直す
      canvas.querySelectorAll<HTMLElement>(".album-card").forEach((c) => {
        const g = groups[Number(c.dataset.album)];
        const cls = albumSelClass(g);
        c.classList.toggle("selected", cls === " selected");
        c.classList.toggle("some", cls === " some");
        const playing = !!player.current && g.tracks.some((t) => t.id === player.current);
        const btn = c.querySelector(".album-play");
        if (btn && btn.classList.contains("playing") !== playing) btn.outerHTML = albumPlayButton(playing);
      });
      return;
    }
    // 見えなくなったカードを外し、新しく見えるカードだけを足す
    for (const c of [...canvas.children] as HTMLElement[]) {
      const i = Number(c.dataset.album);
      if (i < first || i >= last) c.remove();
    }
    let add = "";
    for (let i = first; i < Math.min(prev.first, last); i++) add += card(i);
    for (let i = Math.max(prev.last, first); i < last; i++) add += card(i);
    if (add) canvas.insertAdjacentHTML("beforeend", add);
  } else {
    let html = "";
    for (let i = first; i < last; i++) html += card(i);
    el.innerHTML = `<div class="album-canvas" style="height:${Math.max(0, rowsN * pitch - GRID_GAP)}px">${html}</div>`;
    canvas = el.firstElementChild as HTMLElement;
    // カードの高さを測り直す (文字の大きさ・拡大率で変わる)。違っていたら置き直す
    const c = canvas.querySelector<HTMLElement>(".album-card");
    if (c) {
      const text = c.getBoundingClientRect().height - 12 - colW;
      if (Math.abs(text - gridText) > 0.01 && retry < 2) {
        gridText = text;
        gridRange = { first: -1, last: -1, cols: 0, colW: 0 };
        return renderAlbumGrid(scrollOnly, retry + 1);
      }
    }
  }
  gridSig = sig;
  gridLang = lang;
  gridRange = { first, last, cols, colW };
  // 置いたカードのジャケットだけを読む
  albumArtObserver?.disconnect();
  albumArtObserver = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        const box = e.target as HTMLElement;
        albumArtObserver?.unobserve(box);
        const src = box.dataset.src!;
        loadAlbumArt(src).then((url) => {
          const ph = box.querySelector(".album-noart");
          if (url) ph?.replaceWith(Object.assign(document.createElement("img"), { src: url, alt: "", className: "fade" }));
          else if (ph) {
            ph.classList.remove("skel");
            ph.innerHTML = ICON_LIB;
          }
        });
      }
    },
    { root: el, rootMargin: "300px" },
  );
  canvas.querySelectorAll<HTMLElement>(".album-art[data-src]").forEach((box) => albumArtObserver!.observe(box));
}

export function albumTrackIds(key: string): string[] {
  return albumGroups().find((g) => g.key === key)?.tracks.map((t) => t.id) ?? [];
}

/**
 * アルバムを開く / 一覧に戻る。タイルのジャケットと、開いたアルバムの見出しのジャケットに
 * 同じ view-transition-name を付けて、ジャケットがその位置へ動くように見せる
 */
export function openAlbum(key: string | null): Promise<void> {
  const prev = state.openAlbum;
  const cardArt = (k: string) => {
    const i = albumGroups().findIndex((g) => g.key === k);
    return document.querySelector<HTMLElement>(`.album-card[data-album="${i}"] .album-art`);
  };
  if (key !== null) cardArt(key)?.style.setProperty("view-transition-name", "album-art");
  return withTransition(() => {
    state.openAlbum = key;
    state.selected.clear();
    state.anchor = -1;
    state.focused = null;
    $("tbody").scrollTop = 0;
    renderAll();
    if (key === null && prev !== null) cardArt(prev)?.style.setProperty("view-transition-name", "album-art");
  }).then(() => document.querySelectorAll<HTMLElement>(".album-art").forEach((el) => el.style.removeProperty("view-transition-name")));
}
