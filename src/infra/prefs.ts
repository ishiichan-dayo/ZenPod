// 画面の設定。Rust 側の prefs.json に置く (localStorage は WebView2 が落ちると直前の変更が消えるため)

import { t as tr } from "../i18n";
import { invoke } from "./tauri";
import { forgetAnalytics } from "./analytics";

// ---------------- 設定の保存 ----------------
// Rust 側の prefs.json に置く (localStorage は WebView2 が落ちると直前の変更が消えることがあった)。
// 起動時に全部読み込み、読み取りは手元の値、変更はその場でファイルに書く

export let prefStore: Record<string, string> = {};

export function getStored(key: string): string | null {
  return prefStore[key] ?? null;
}

/** 開発版の「初回起動として起動し直す」(Ctrl + Shift + N) の間は、設定を空から始めてファイルには書かない */
export const devFirstRun = import.meta.env.DEV && sessionStorage.getItem("devFirstRun") === "1";
if (devFirstRun) sessionStorage.removeItem("devFirstRun");

/** 設定を保存できなかったときの知らせ先 (起動時に main がトーストにつなぐ) */
let reportPrefError: (message: string) => void = () => undefined;

export function onPrefSaveError(report: (message: string) => void) {
  reportPrefError = report;
}

export function setStored(key: string, value: string | null) {
  if (value === null) delete prefStore[key];
  else prefStore[key] = value;
  if (devFirstRun) return;
  invoke("set_pref", { key, value }).catch((e) => reportPrefError(tr("設定を保存できませんでした: {a}", { a: e })));
}

/** Rust が専用のコマンドで書くキー (set_pref では書けない)。画面の手元の値だけ合わせる */
export function setStoredLocal(key: string, value: string | null) {
  if (value === null) delete prefStore[key];
  else prefStore[key] = value;
}

// ffmpegPath と pcFolders は set_pref で書けないので引き継がない (設定し直してもらう)
const PREF_KEYS = ["theme", "skipDuplicates", "autoReading", "artwork", "confirmReadings", "autoUpdate", "playerVolume"];

export async function loadPrefs() {
  if (devFirstRun) {
    prefStore = {};
    return;
  }
  prefStore = (await invoke<Record<string, string>>("load_prefs").catch(() => null)) ?? {};
  // 初回だけ、以前の localStorage の値を引き継ぐ
  if (Object.keys(prefStore).length === 0) {
    try {
      for (const k of PREF_KEYS) {
        const v = localStorage.getItem(k);
        if (v !== null) setStored(k, v);
      }
    } catch {
      /* localStorage が読めなければ引き継がない */
    }
  }
}

export function pref(key: "skipDuplicates" | "autoReading" | "artwork" | "shrinkArtwork" | "confirmReadings" | "confirmConvert" | "autoUpdate" | "analytics" | "podcastSyncOnConnect"): boolean {
  return getStored(key) !== "0";
}

export function setPref(key: string, on: boolean) {
  setStored(key, on ? "1" : "0");
  if (key === "analytics" && !on) forgetAnalytics();
}
