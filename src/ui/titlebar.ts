// 自前のタイトルバー (Windows・Mac)。Windows は OS の枠を消して (tauri.windows.conf.json の decorations: false)
// 最小化・最大化・閉じるもここで描く。Mac は OS の信号機のボタンを残して画面を上端まで広げる (tauri.macos.conf.json の
// titleBarStyle: Overlay)。Linux はウィンドウマネージャで見た目も動きも違うので OS のタイトルバーのまま (ここは何もしない)。
// ウィンドウを動かすのは data-tauri-drag-region の要素 (ダブルクリックで最大化も Tauri がする)。子の要素にも付ける

import { t as tr } from "../i18n";
import { getCurrentWindow } from "../infra/tauri";
import { $ } from "../app/state";

// Segoe Fluent Icons (Windows 11) / Segoe MDL2 Assets (Windows 10) の字。OS の枠のボタンと同じ形・同じ太さになる
const GLYPH_MAX = "\uE922";
const GLYPH_RESTORE = "\uE923";

export function initTitlebar() {
  const ua = navigator.userAgent;
  const os = ua.includes("Windows") ? "win" : ua.includes("Mac") ? "mac" : null;
  if (!os) return;
  document.body.classList.add("has-titlebar", `tb-${os}`);
  $("titlebar").classList.remove("hidden");
  const win = getCurrentWindow();
  // 開発版は「ZenPod Dev」になる
  void win
    .title()
    .then((t) => ($("tb-title").textContent = t))
    .catch(() => ($("tb-title").textContent = "ZenPod"));

  if (os === "win") {
    $("tb-caption").classList.remove("hidden");
    $("tb-min").addEventListener("click", () => void win.minimize());
    $("tb-max").addEventListener("click", () => void win.toggleMaximize());
    $("tb-close").addEventListener("click", () => void win.close());
    // ほかのウィンドウを使っている間は OS の枠と同じく文字を薄くする
    window.addEventListener("blur", () => document.body.classList.add("win-blur"));
    window.addEventListener("focus", () => document.body.classList.remove("win-blur"));
  }

  // 最大化の印の切り替えと、全画面 (動画・Mac の緑のボタン) の間はタイトルバーを隠す。
  // 端をドラッグして大きさを変えている間は何度も来るので、止まってから 1 回だけ調べる
  let timer = 0;
  const sync = async () => {
    try {
      const [max, full] = await Promise.all([win.isMaximized(), win.isFullscreen()]);
      document.body.classList.toggle("win-full", full);
      const btn = $("tb-max");
      btn.textContent = max ? GLYPH_RESTORE : GLYPH_MAX;
      const label = max ? tr("元に戻す") : tr("最大化");
      btn.title = label;
      btn.setAttribute("aria-label", label);
    } catch {
      /* 調べられなくても、印がずれるだけ */
    }
  };
  void win.onResized(() => {
    clearTimeout(timer);
    timer = window.setTimeout(() => void sync(), 120);
  });
  void sync();
}

/** タイトルバーの「アップデート」のボタン。いつも出しておき、新しい版が無い間は薄い文字で、押すと確認する
 * (結果はトーストで出る)。Linux はタイトルバーが無いので出さない */
export function idleUpdateButton(check: () => Promise<void>) {
  const btn = $<HTMLButtonElement>("tb-update");
  btn.title = tr("アップデートを確認");
  let checking = false;
  btn.onclick = async () => {
    if (checking) return;
    checking = true;
    try {
      await check();
    } finally {
      checking = false;
    }
  };
}

/** 新しい版があるとき: ボタンを色付きにして、押すと案内を開くようにする */
export function showUpdateButton(version: string, open: () => void) {
  const btn = $<HTMLButtonElement>("tb-update");
  btn.title = tr("バージョン {a} に更新できます", { a: version });
  btn.onclick = open;
  btn.classList.add("active");
}
