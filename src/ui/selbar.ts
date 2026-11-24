// 曲を選んだときに出る帯 (書き出し・削除など)

import { t as tr } from "../i18n";
import { currentPlaylist, isPcView, writable } from "../app/selectors";
import { $, state } from "../app/state";
import { fmtBytes, fmtTotal } from "../domain/format";
import { findTrack } from "./player";
import { renderFormat } from "./status";

/** 見出しのチェックボックス: 全選択 / 一部選択 / 未選択 */
export function updateSelectAllBox() {
  const box = document.querySelector<HTMLElement>('#thead [data-selall]');
  if (!box) return;
  const n = state.rows.reduce((a, t) => a + (state.selected.has(t.id) ? 1 : 0), 0);
  box.classList.toggle("all", n > 0 && n === state.rows.length);
  box.classList.toggle("some", n > 0 && n < state.rows.length);
}

/**
 * 選択バーが幅に収まらないときは、段階的に畳む: 時間と容量を隠す →「PC に書き出し」→「iPod から削除」を隠す
 * (どちらも「メニュー」にある)。「メニュー」と選択の解除 (✕) はいつも出す
 */
const SELBAR_FITS = 3;
export function fitSelbar() {
  const bar = $("selbar");
  if (bar.classList.contains("hidden")) return;
  for (let i = 1; i <= SELBAR_FITS; i++) bar.classList.remove(`fit-${i}`);
  for (let i = 1; i <= SELBAR_FITS && bar.scrollWidth > bar.clientWidth; i++) bar.classList.add(`fit-${i}`);
}

export function renderSelbar() {
  updateSelectAllBox();
  renderFormat();
  const n = state.selected.size;
  $("selbar").classList.toggle("hidden", n === 0);
  if (!n) return;
  let ms = 0;
  let bytes = 0;
  for (const id of state.selected) {
    const t = findTrack(id);
    if (t) {
      ms += t.durationMs;
      bytes += t.size;
    }
  }
  $("sel-n").textContent = tr("{n} 曲を選択中", { n });
  $("sel-detail").textContent = ` · ${fmtTotal(ms)} · ${fmtBytes(bytes)}`;
  // PC のフォルダ表示では、書き出しと削除は出さない (転送などは「メニュー」から)
  const pc = isPcView();
  for (const id of ["btn-sel-export", "btn-sel-remove"]) $(id).classList.toggle("hidden", pc);
  const pl = currentPlaylist();
  const removeBtn = $<HTMLButtonElement>("btn-sel-remove");
  if (pl && !pl.master) {
    removeBtn.textContent = tr("プレイリストから外す");
    removeBtn.disabled = !writable() || pl.smart || pl.podcast;
  } else {
    removeBtn.textContent = tr("iPod から削除");
    removeBtn.disabled = !writable();
  }
  fitSelbar();
}
