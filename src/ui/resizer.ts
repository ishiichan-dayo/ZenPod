// サイドバーとアーティストの一覧の幅を変える境目

import { getStored, setStored } from "../infra/prefs";

/** サイドバー / アーティスト一覧の幅 (px) の範囲 */
export const SIDE_MIN = 160;
export const SIDE_MAX = 420;
export const ARTIST_MIN = 144;
export const ARTIST_MAX = 400;

/**
 * 左の一覧と右の境目 (resizer) をドラッグして、直前の要素の幅を変える。
 * 幅は CSS 変数 cssVar に入れ、離したときに prefKey へ保存する。ダブルクリックで元の幅
 */
export function bindResizer(resizer: HTMLElement, cssVar: string, prefKey: string, min: number, max: number) {
  resizer.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    // 境目の左隣の一覧 (スクロールバーの溝が間に入っていれば、その前)
    let side = resizer.previousElementSibling as HTMLElement;
    if (side.classList.contains("oscroll-track")) side = side.previousElementSibling as HTMLElement;
    const left = side.getBoundingClientRect().left;
    let w = side.getBoundingClientRect().width;
    resizer.setPointerCapture(e.pointerId);
    resizer.classList.add("dragging");
    document.body.classList.add("col-resizing");
    const move = (ev: PointerEvent) => {
      w = Math.round(Math.min(max, Math.max(min, ev.clientX - left)));
      document.documentElement.style.setProperty(cssVar, `${w}px`);
    };
    const up = () => {
      resizer.removeEventListener("pointermove", move);
      resizer.removeEventListener("pointerup", up);
      resizer.removeEventListener("pointercancel", up);
      resizer.classList.remove("dragging");
      document.body.classList.remove("col-resizing");
      setStored(prefKey, String(w));
    };
    resizer.addEventListener("pointermove", move);
    resizer.addEventListener("pointerup", up);
    resizer.addEventListener("pointercancel", up);
  });
  resizer.addEventListener("dblclick", () => {
    document.documentElement.style.removeProperty(cssVar);
    setStored(prefKey, null);
  });
}

/** 保存した幅を戻す (起動時) */
export function restoreWidth(cssVar: string, prefKey: string, min: number, max: number) {
  const w = Number(getStored(prefKey));
  if (w >= min && w <= max) document.documentElement.style.setProperty(cssVar, `${w}px`);
}
