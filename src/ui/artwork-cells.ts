// 一覧の小さなジャケット (見えている行の分だけ読む)

import { invoke } from "../infra/tauri";
import { state } from "../app/state";
import { skelStyle } from "./album-grid";
import { esc } from "./dom";

// ---------------- アートワーク (一覧用の小さい画像) ----------------

export const artCache = new Map<number, string | null>();

/** 見えている行の PC の曲のジャケットを 4 件ずつ並行して読む */
function loadVisiblePcArtwork() {
  const want: string[] = [];
  document.querySelectorAll<HTMLElement>("#rows [data-pcart]").forEach((el) => {
    const p = el.dataset.pcart!;
    if (!pcArtCache.has(p) && !pcArtPending.has(p) && !want.includes(p)) want.push(p);
  });
  const worker = async () => {
    for (let p = want.shift(); p !== undefined; p = want.shift()) {
      pcArtPending.add(p);
      const url = await invoke<string | null>("pc_artwork", { path: p }).catch(() => null);
      pcArtPending.delete(p);
      pcArtCache.set(p, url);
      document.querySelectorAll<HTMLElement>("#rows [data-pcart]").forEach((el) => {
        if (el.dataset.pcart !== p) return;
        if (url) el.outerHTML = `<img class="art fade" src="${url}" alt="">`;
        else {
          el.classList.remove("skel");
          el.classList.add("none");
        }
      });
    }
  };
  for (let i = 0; i < Math.min(4, want.length); i++) worker();
}
const artPending = new Set<number>();

export function artHtml(id: number): string {
  // 初めて開く途中は、ジャケットの割り当てがまだなので読み込み中として出す
  if (state.partialOpen) return `<span class="art skel"${skelStyle()}></span>`;
  if (!id || !state.snap?.artworkSupported) return `<span class="art none"></span>`;
  const url = artCache.get(id);
  if (url) return `<img class="art" src="${url}" alt="">`;
  // 読んで無かった (null) ときは無いときと同じ灰色の四角
  return artCache.has(id) ? `<span class="art none"></span>` : `<span class="art skel" data-art="${id}"${skelStyle()}></span>`;
}

/** PC の曲のジャケット (場所 → data URL、null = なし)。画面を開いている間だけ覚える */
export const pcArtCache = new Map<string, string | null>();
const pcArtPending = new Set<string>();

export function pcArtHtml(path: string): string {
  const url = pcArtCache.get(path);
  if (url) return `<img class="art" src="${url}" alt="">`;
  if (url === null) return `<span class="art none"></span>`;
  return `<span class="art skel" data-pcart="${esc(path)}"${skelStyle()}></span>`;
}

export function loadVisibleArtwork() {
  loadVisiblePcArtwork();
  const ids = new Set<number>();
  document.querySelectorAll<HTMLElement>("#rows [data-art]").forEach((el) => ids.add(Number(el.dataset.art)));
  for (const id of ids) {
    if (artCache.has(id) || artPending.has(id)) continue;
    artPending.add(id);
    invoke<string | null>("get_artwork", { imageId: id, large: false })
      .catch(() => null)
      .then((url) => {
        artPending.delete(id);
        artCache.set(id, url);
        document.querySelectorAll<HTMLElement>(`#rows [data-art="${id}"]`).forEach((el) => {
          if (url) el.outerHTML = `<img class="art fade" src="${url}" alt="">`;
          else {
            el.classList.remove("skel");
            el.classList.add("none");
          }
        });
      });
  }
}
