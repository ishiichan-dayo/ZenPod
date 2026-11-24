// 利用状況を自前の Umami に送る仕組み (ID・送信)。何を送るかは app/analytics.ts

import { lang } from "../i18n";
import { devFirstRun, getStored, pref, setStored } from "./prefs";

// ---------------- 利用状況 ----------------
// 利用者の数を知って改善に役立てるため、自前の Umami に 1 日 1 回だけ送る。
// 送るのは起動 (版)・開いた iPod のモード・機種・開く速さ・案内の結果・転送の結果など (どれも範囲や種類だけ)。曲名・ファイル名・iPod の名前・エラーの文章などは送らない。
// 設定の「このアプリについて」で止められる (止めたら ID も消す)。開発版では送らない
// 送り先とサイトの ID はビルドのときの環境変数で渡す (配布するリリースのビルドだけ。.github/workflows/build.yml)。
// 無ければ送らない (フォークや手元でビルドしたアプリが、配布版の数に混ざらないように)

const ANALYTICS_URL: string = import.meta.env.VITE_ANALYTICS_URL ?? "";
const ANALYTICS_WEBSITE: string = import.meta.env.VITE_ANALYTICS_WEBSITE ?? "";

/** 同じ人を 1 人と数えるための ID。端末の情報からは作らず、ランダムに作って保存する */
function analyticsId(): string {
  let id = getStored("analyticsId");
  if (!id) {
    id = crypto.randomUUID();
    setStored("analyticsId", id);
  }
  return id;
}

export function forgetAnalytics() {
  setStored("analyticsId", null);
  setStored("analyticsSent", null);
}

/** `key` をその日まだ送っていなければ送る。送信は待たず、失敗しても何もしない (画面の操作を止めない) */
export function sendAnalytics(key: string, payload: { url: string; title?: string; name?: string; data?: Record<string, string> }) {
  if (import.meta.env.DEV || !ANALYTICS_URL || !ANALYTICS_WEBSITE || devFirstRun || !pref("analytics")) return;
  const today = new Date().toLocaleDateString("sv-SE");
  const sent = (() => {
    try {
      return JSON.parse(getStored("analyticsSent") ?? "{}") as Record<string, string>;
    } catch {
      return {};
    }
  })();
  if (sent[key] === today) return;
  sent[key] = today;
  setStored("analyticsSent", JSON.stringify(sent));
  fetch(ANALYTICS_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      type: "event",
      payload: { website: ANALYTICS_WEBSITE, hostname: "app.zenpod", language: lang, id: analyticsId(), ...payload },
    }),
    keepalive: true,
  }).catch(() => undefined);
}

/** 数をおおまかな範囲にする (正確な数は送らない)。`edges` は区切りの値 (昇順)、`labels` は edges より 1 つ多い */
export function bucket(v: number, edges: number[], labels: string[]): string {
  const i = edges.findIndex((e) => v < e);
  return labels[i < 0 ? labels.length - 1 : i];
}
