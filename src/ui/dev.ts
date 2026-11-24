// 開発版だけの確認用の操作 (Ctrl + Shift + キー、設定の「デバッグ」)

import { invoke } from "../infra/tauri";
import type { Update } from "../infra/tauri";
import { isPcView } from "../app/selectors";
import { $, appVersion, state } from "../app/state";
import { devFirstRun } from "../infra/prefs";
import { USB_WAIT_HINT_MS, renderConnectView } from "./device";
import { esc } from "./dom";
import { toast } from "./feedback";
import { renderHeader, renderSwitchOverlay } from "./header";
import { showOnboarding } from "./onboarding";
import { renderAll } from "./render";
import { offerUpdate } from "./update";

// ---------------- 開発版だけの確認用の操作 ----------------

/** 開発版だけの確認用の操作。Ctrl + Shift + `key` と、設定の「デバッグ」のボタンから呼ぶ (製品版には出さない) */
interface DevAction {
  key: string;
  name: string;
  about: string;
  /** iPod を開いているときだけ使える */
  needsIpod?: boolean;
  run: () => void;
}

export const DEV_ACTIONS: DevAction[] = [
  // tr: 訳さない (開発用)
  { key: "l", name: "モードの切り替え中の表示を再生", about: "一覧の上の「〜で読み込んでいます」と読んだ数を 8 秒かけて再生します (本当には読み込みません)。iPod を開いているときだけ", needsIpod: true, run: devPlaySwitch },
  // tr: 訳さない (開発用)
  { key: "d", name: "ドライブが出ない状態を真似る", about: "USB で認識したのにドライブが出ない状態にします。3 秒で Windows の Mac 形式の案内まで出ます。もう一度で戻ります", run: devToggleNoDrive },
  // tr: 訳さない (開発用)
  { key: "o", name: "はじめの案内を出す", about: "最初のページから出します。閉じたあとの「見た」の印はふだんどおり付きます", run: () => void (document.querySelector(".onb-modal") || showOnboarding()) },
  // tr: 訳さない (開発用)
  { key: "n", name: "初回起動として起動し直す", about: "設定を空から始め、この間に変えた設定はファイルに書きません。もう一度で (または読み直すと) ふだんの設定に戻ります", run: devRestartFirstRun },
  // tr: 訳さない (開発用)
  { key: "s", name: "読み込み中の表示を出す", about: "今の一覧 (曲・アルバム・アーティスト) と番組の画像を、読み込み中のスケルトンにしたままにします (本当には読み込みません)。もう一度で戻ります", run: devToggleSkeleton },
  // tr: 訳さない (開発用)
  { key: "u", name: "更新の案内を出す", about: "v0.7.1 のリリースノートで、更新の案内とタイトルバーのボタンの見た目を確かめます (更新はしません)", run: devShowUpdate },
];

export function devCanRun(a: DevAction): boolean {
  return !a.needsIpod || (!!state.snap && !state.opening);
}

/** Ctrl + Shift + L: モードを切り替えて読み込んでいる間の表示を、ゆっくり再生する (本当には読み込まない) */
function devPlaySwitch() {
  if (!state.snap || state.opening) return;
  const total = 20000;
  state.opening = true;
  state.openingMode = state.snap.mode === "rockbox" ? "ipod" : "rockbox";
  state.openProgress = null;
  renderAll();
  const started = Date.now();
  // 最初の 1.5 秒は数を出さない (本物も、読み直す曲が分かるまでは数が無い)
  const timer = window.setInterval(() => {
    const t = Date.now() - started;
    if (t > 8000) {
      clearInterval(timer);
      state.opening = false;
      state.openingMode = null;
      state.openProgress = null;
      renderAll();
      return;
    }
    if (t > 1500) state.openProgress = [Math.min(total, Math.round(((t - 1500) / 6000) * total)), total];
    renderSwitchOverlay();
  }, 100);
}

/** Ctrl + Shift + S: 読み込み中の表示 (スケルトン) を出したままにする / 戻す */
function devToggleSkeleton() {
  state.devSkeleton = !state.devSkeleton;
  // スケルトンを作り直す (列やアルバムの大きさが前のままにならないように)
  delete $("rows").dataset.skelCols;
  delete $("album-grid").dataset.skel;
  renderAll();
  // tr: 訳さない (開発用)
  if (state.devSkeleton) toast("読み込み中の表示を出しています (開発用)。Ctrl + Shift + S で戻ります", "info", 5000);
}

let devNoDriveTimer = 0;
let devUsbBefore = 0;
/** Ctrl + Shift + D: USB で認識したのにドライブが出ない状態を真似る (Mac 形式の案内の確認用)。開発版は利用状況を送らない */
function devToggleNoDrive() {
  state.devNoDrive = !state.devNoDrive;
  clearInterval(devNoDriveTimer);
  if (state.devNoDrive) {
    devUsbBefore = state.usbSince;
    state.usbSince = Date.now() - USB_WAIT_HINT_MS + 3000;
    if (isPcView()) state.view = "library";
    devNoDriveTimer = window.setInterval(() => {
      renderConnectView();
      renderHeader();
    }, 500);
    // tr: 訳さない (開発用)
    toast("ドライブが出ない状態を真似ています (開発用)。Ctrl + Shift + D で戻ります", "info", 5000);
  } else {
    state.usbSince = devUsbBefore;
  }
  renderAll();
}

/** Ctrl + Shift + N: 初回起動として起動し直す (もう一度でふだんの設定に戻る)。開いている iPod は閉じてから読み直す */
async function devRestartFirstRun() {
  await invoke("close_device").catch(() => null);
  if (!devFirstRun) sessionStorage.setItem("devFirstRun", "1");
  location.reload();
}

/** Ctrl + Shift + U: 更新の案内の見た目を確かめる (v0.7.1 のノートを使う。更新はしない) */
function devShowUpdate() {
  offerUpdate({
    version: "0.7.1",
    currentVersion: appVersion,
    // tr: 訳さない (確認用のリリースノート)
    body: "iPod Sync 0.7.1\n\n新しい機能\n- 曲情報をネットで取得で、曲ごとに対応する曲目を選べるように。情報の無い曲も、探したアルバムの曲目から選んで反映できます\n- 情報の無い曲は、ファイル名の番号と曲の長さから対応する曲目を自動で推測します\n\n修正\n- 情報の無い曲を複数選んだとき、「曲情報をネットで取得」が出なかったのを直した\n\nNew in 0.7.1\n- When getting song info online, you can now choose which album track each song is. Songs with no info can be filled in from the album you find\n- For songs with no info, the matching track is guessed from the file name's number and the song length\n\nFixes\n- \"Get Song Info Online\" now appears when several songs with no info are selected",
    downloadAndInstall: async () => {
      // tr: 訳さない (開発版だけ)
      throw new Error("開発版の確認用の表示です");
    },
  } as unknown as Update);
}

/** 設定の「デバッグ」のページ (開発版だけ)。各操作のボタンとショートカット */
export function devSettingsHtml(): string {
  const mac = navigator.userAgent.includes("Mac");
  const rows = DEV_ACTIONS.map(
    (a) =>
      `<div class="set-tidy-row"><div><div>${esc(a.name)} <kbd class="dev-kbd">${mac ? "⌃⇧" : "Ctrl + Shift + "}${a.key.toUpperCase()}</kbd></div><div class="check-hint">${esc(a.about)}</div></div><button class="btn" data-dev="${a.key}" ${devCanRun(a) ? "" : "disabled"}>${esc("実行")}</button></div>`,
  ).join("");
  // tr: 訳さない (開発用)
  const head = `<h2 class="set-head">デバッグ (開発版)</h2>`;
  // tr: 訳さない (開発用)
  const note = `<div class="hint">開発版だけに出るページです。押すと設定を閉じてから実行します。</div>`;
  return `<section class="set-section" data-cat="debug">${head}<div class="set-card">${note}${rows}</div></section>`;
}
