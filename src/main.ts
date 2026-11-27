// 起動: 設定と言語を読み、画面の部品をつないで、iPod を探し始める。
//
// 画面のコードの層 (依存は上から下への一方向):
// - ui/      描画・ダイアログ・メニュー・操作 (DOM を触る)
// - app/     画面の状態 (state) と、状態から決まる値 (今の表示・一覧の行)
// - infra/   外とのやり取り (Tauri・設定の保存・利用状況)
// - domain/  型と純粋な決まりごと (DOM も Tauri も知らない)

import { detectLang, lang, setLang, translateStatic } from "./i18n";
import { initMediaBase, invoke } from "./infra/tauri";
import { NO_IPOD_AFTER_MS, analyticsLaunch, analyticsNoIpod } from "./app/analytics";
import { PC_ALL } from "./app/selectors";
import { loadAppVersion, state } from "./app/state";
import { POD_REFRESH_AGE } from "./domain/podcast";
import type { Snapshot } from "./domain/types";
import { devFirstRun, getStored, loadPrefs, onPrefSaveError, pref } from "./infra/prefs";
import { applyTheme, applyZoom, onZoomKey } from "./ui/appearance";
import { bind } from "./ui/bind";
import { DEV_ACTIONS, devCanRun } from "./ui/dev";
import { pollDevices, watchUsbIpod } from "./ui/device";
import { toast } from "./ui/feedback";
import { bindFileDrop } from "./ui/file-drop";
import { ONB_VERSION, showOnboarding } from "./ui/onboarding";
import { loadPcFolders } from "./ui/pc";
import { bindPlayer } from "./ui/player";
import { bindPodcasts, refreshPodcasts, startPodcastsLoad } from "./ui/podcasts";
import { applySnapshot } from "./ui/render";
import { ARTIST_MAX, ARTIST_MIN, SIDE_MAX, SIDE_MIN, restoreWidth } from "./ui/resizer";
import { initScrollbars } from "./ui/scrollbar";
import { bindSideSections } from "./ui/side-sections";
import { selectView } from "./ui/sidebar";
import { bindTransferEvents } from "./ui/transfer";
import { idleUpdateButton, initTitlebar } from "./ui/titlebar";
import { checkForUpdate } from "./ui/update";

async function main() {
  // 画面の思わぬ例外もログ (zenpod.log) に残す。通知は出さない
  const uiLog = (msg: string) => void invoke("ui_log", { msg }).catch(() => undefined);
  window.addEventListener("error", (e) => uiLog(`error: ${e.message} (${e.filename?.split("/").pop()}:${e.lineno})`));
  window.addEventListener("unhandledrejection", (e) => uiLog(`unhandled: ${String(e.reason)}`));
  onPrefSaveError((message) => toast(message, "error"));
  // 画面が起動できたことを知らせる (知らせが無いと、しばらくして起動し直すか確認するダイアログが出る)
  invoke("ui_ready").catch(() => undefined);
  // 設定を先に読む (音量・ffmpeg の場所・登録フォルダなどが使う)
  await loadPrefs();
  // 言語は最初に決める (以降に作る画面の文言はすべて t() で訳す)
  setLang(detectLang(getStored("lang")));
  // Rust 側の文言 (エラー・転送の結果など) も同じ言語にする
  await invoke("set_lang", { lang: lang }).catch(() => undefined);
  translateStatic(document.body);
  initTitlebar();
  idleUpdateButton(() => checkForUpdate(true));
  applyTheme();
  applyZoom();
  document.addEventListener("keydown", onZoomKey, true);
  const storedLayout = getStored("layout");
  if (storedLayout === "albums" || storedLayout === "artists") state.layout = storedLayout;
  restoreWidth("--side-w", "sidebarWidth", SIDE_MIN, SIDE_MAX);
  restoreWidth("--artist-w", "artistListWidth", ARTIST_MIN, ARTIST_MAX);
  restoreWidth("--photo-side-w", "photoSideWidth", ARTIST_MIN, ARTIST_MAX);
  state.pc.folders = loadPcFolders();
  await loadAppVersion();
  void initMediaBase(); // 起動は待たせない (再生できるようになる前に返る)
  analyticsLaunch();
  bindPlayer();
  bind();
  initScrollbars();
  bindPodcasts();
  bindSideSections();
  // 購読の一覧は iPod を探す前に読み始める (起動したときにつながっていた iPod でも同期を確かめられるように)
  const podLoad = startPodcastsLoad();
  await bindFileDrop();
  await bindTransferEvents();
  const existing = await invoke<Snapshot | null>("snapshot").catch(() => null);
  applySnapshot(existing);
  // はじめの案内: 初めて起動したときと、案内の版 (ONB_VERSION) が上がった版に更新したとき (iPod を探す前に。
  // 確認のダイアログが案内に重ならないように)。前の版から使っている人にも 1 回出す
  if (getStored("onboarded") !== ONB_VERSION) await showOnboarding();
  await watchUsbIpod();
  await pollDevices();
  // iPod が無ければ PC の音楽フォルダを開いておく (iPod はあとから接続すれば読み込む)
  if (!state.snap && state.view === "library" && state.pc.folders.length) selectView(PC_ALL);
  setInterval(pollDevices, 2500);
  // ポッドキャスト: 購読の一覧を読み、前回から時間がたった番組だけフィードを確かめる
  void podLoad.then(() => refreshPodcasts(undefined, true, POD_REFRESH_AGE));
  setTimeout(analyticsNoIpod, NO_IPOD_AFTER_MS);
  // 開発モードだけ: Ctrl + Shift + キーで確認用の操作 (DEV_ACTIONS。設定の「デバッグ」のボタンからも同じものを呼ぶ)
  if (import.meta.env.DEV) {
    // tr: 訳さない (開発用)
    if (devFirstRun) toast("初回起動として起動しました (開発用)。設定は保存しません。Ctrl + Shift + N で元に戻ります", "info", 8000);
    document.addEventListener("keydown", (e) => {
      if (!(e.ctrlKey && e.shiftKey)) return;
      const a = DEV_ACTIONS.find((x) => x.key === e.key.toLowerCase());
      if (!a || (a.needsIpod && !devCanRun(a))) return;
      e.preventDefault();
      a.run();
    });
  }
  // 開発モード (Vite から読み込み中) では更新確認をしない
  if (pref("autoUpdate") && !import.meta.env.DEV) setTimeout(() => checkForUpdate(false), 4000);
}

main();
