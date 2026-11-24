// 埋め込み画像を縮める大きさ (長い辺の px) の選択欄。設定・アートワークのタブ・ネットで探す・まとめての縮小で使う。
// どこで選んでも同じ値 (設定の artworkSize) で、次に開いたときは前回選んだ大きさを出す

import { t as tr } from "../i18n";
import { getStored, setStored } from "../infra/prefs";
import { esc } from "./dom";

const PRESETS = [320, 500, 600, 800];
const DEFAULT = 500;
/** 入力できる範囲 (Rust の `cover::embed_size` と同じ) */
const MIN = 64;
const MAX = 3000;

/**
 * 画像を設定するときに縮めてから埋め込むか。iPod の曲は設定 → 転送 の「埋め込み画像を縮小する」と同じ値 (最初はオン)。
 * PC の曲は元のファイルの画像が戻せなくなるので別の値で、最初はオフ
 */
export function shrinkOnSetPref(pc: boolean): boolean {
  return pc ? getStored("shrinkPcArtwork") === "1" : getStored("shrinkArtwork") !== "0";
}

export function setShrinkOnSetPref(pc: boolean, on: boolean) {
  setStored(pc ? "shrinkPcArtwork" : "shrinkArtwork", on ? "1" : "0");
}

/** 前回選んだ大きさ (無ければ 500) */
export function artSize(): number {
  const v = Number(getStored("artworkSize"));
  return Number.isInteger(v) && v >= MIN && v <= MAX ? v : DEFAULT;
}

/** 選択欄の HTML。`id` は画面の中で重ならない名前 */
export function artSizeHtml(id: string): string {
  const cur = artSize();
  const custom = !PRESETS.includes(cur);
  const opts = PRESETS.map((p) => `<option value="${p}" ${p === cur ? "selected" : ""}>${p}px</option>`).join("");
  return `<span class="art-size">
      <select id="${id}" aria-label="${esc(tr("縮小する大きさ (長い辺)"))}">${opts}<option value="custom" ${custom ? "selected" : ""}>${esc(tr("サイズを入力"))}</option></select>
      <input type="number" id="${id}-px" min="${MIN}" max="${MAX}" step="1" value="${cur}" aria-label="${esc(tr("長い辺の大きさ (px)"))}" class="${custom ? "" : "hidden"}">
      <span class="art-size-unit ${custom ? "" : "hidden"}">px</span>
    </span>`;
}

/** 選択欄を動かす。選び直すたびに覚え、`onChange` を呼ぶ。戻り値は今の大きさを返す関数 (入力が範囲の外なら前回の値) */
export function bindArtSize(root: HTMLElement, id: string, onChange?: (px: number) => void): () => number {
  const sel = root.querySelector<HTMLSelectElement>(`#${id}`)!;
  const input = root.querySelector<HTMLInputElement>(`#${id}-px`)!;
  const unit = input.nextElementSibling as HTMLElement;
  const value = (): number => {
    if (sel.value !== "custom") return Number(sel.value);
    const v = Number(input.value);
    return Number.isInteger(v) && v >= MIN && v <= MAX ? v : artSize();
  };
  const changed = () => {
    const v = value();
    if (v !== artSize()) setStored("artworkSize", String(v));
    onChange?.(v);
  };
  sel.addEventListener("change", () => {
    const custom = sel.value === "custom";
    input.classList.toggle("hidden", !custom);
    unit.classList.toggle("hidden", !custom);
    if (custom) {
      input.focus();
      input.select();
    }
    changed();
  });
  input.addEventListener("change", changed);
  return value;
}
