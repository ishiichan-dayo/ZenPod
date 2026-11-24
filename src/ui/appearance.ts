// 表示の大きさと外観 (ライト / ダーク)

import { t as tr } from "../i18n";
import { getCurrentWebview, getCurrentWindow } from "../infra/tauri";
import { $ } from "../app/state";
import { getStored, setStored } from "../infra/prefs";
import { dismissToast } from "./feedback";

/** 画面の拡大率 (ブラウザと同じ刻み)。Ctrl/⌘ + プラス・マイナスで変え、0 で 100% に戻す */
const ZOOM_STEPS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];

function zoomPref(): number {
  const v = Number(getStored("zoom"));
  return ZOOM_STEPS.includes(v) ? v : 1;
}

export function applyZoom() {
  getCurrentWebview().setZoom(zoomPref()).catch(() => undefined);
}

/** dir: 1 = 拡大, -1 = 縮小, 0 = 100% に戻す。設定の画面から変えたときは、数字がその場に出るので通知を出さない */
export function changeZoom(dir: 1 | -1 | 0, notify = true) {
  const i = ZOOM_STEPS.indexOf(zoomPref());
  const next = dir === 0 ? 1 : ZOOM_STEPS[Math.min(ZOOM_STEPS.length - 1, Math.max(0, i + dir))];
  setStored("zoom", next === 1 ? null : String(next));
  applyZoom();
  renderZoomSetting();
  if (notify) showZoomToast(tr("表示の大きさ: {a}%", { a: Math.round(next * 100) }));
}

/** 設定の「一般」の表示の大きさの欄 (開いていれば)。キーで変えたときも書き直す */
export function renderZoomSetting() {
  const v = zoomPref();
  const value = document.getElementById("set-zoom-value");
  if (!value) return;
  value.textContent = `${Math.round(v * 100)}%`;
  document.querySelector<HTMLButtonElement>("#set-zoom-out")!.disabled = v === ZOOM_STEPS[0];
  document.querySelector<HTMLButtonElement>("#set-zoom-in")!.disabled = v === ZOOM_STEPS[ZOOM_STEPS.length - 1];
  document.querySelector<HTMLButtonElement>("#set-zoom-reset")!.disabled = v === 1;
}

/** 拡大率の通知。出ている間に続けて変えたら、新しく出さずに数値だけ書き換えて表示時間を延ばす */
let zoomToast: { el: HTMLElement; timer: number } | null = null;
function showZoomToast(msg: string) {
  if (zoomToast && zoomToast.el.isConnected && !zoomToast.el.classList.contains("leaving")) {
    clearTimeout(zoomToast.timer);
    zoomToast.el.textContent = msg;
  } else {
    const el = document.createElement("div");
    el.className = "toast";
    el.textContent = msg;
    $("toast-root").appendChild(el);
    zoomToast = { el, timer: 0 };
  }
  const el = zoomToast.el;
  zoomToast.timer = window.setTimeout(() => dismissToast(el), 2000);
}

/** 拡大・縮小のキー。設定画面やモーダルの中でも効かせるので、ほかのショートカットより先に見る */
export function onZoomKey(e: KeyboardEvent) {
  if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
  // US 配列は = (Shift で +)、JIS 配列は ; (Shift で +)。テンキーの + / - も同じ key になる
  const dir = ["+", "=", ";"].includes(e.key) ? 1 : ["-", "_"].includes(e.key) ? -1 : e.key === "0" ? 0 : null;
  if (dir === null) return;
  e.preventDefault();
  e.stopPropagation();
  changeZoom(dir);
}

/** 外観: "system" (OS に合わせる) / "light" / "dark" */
export type Theme = "system" | "light" | "dark";

export function themePref(): Theme {
  const v = getStored("theme");
  return v === "light" || v === "dark" ? v : "system";
}

/** 画面の色 (CSS の data-theme) と、ウィンドウのタイトルバーの色をそろえる */
export function applyTheme() {
  const t = themePref();
  if (t === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = t;
  getCurrentWindow()
    .setTheme(t === "system" ? null : t)
    .catch(() => null);
}
