// 動き (マイクロインタラクション) と、トースト・作業中の表示

import { t as tr } from "../i18n";
import { invoke, listen } from "../infra/tauri";
import { $ } from "../app/state";

// ---------------- 動き (マイクロインタラクション) ----------------

/** OS の「アニメーションを減らす」 */
export const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

/**
 * 「今変わった」行にだけ動きを付けるための印 (キー → 動きを付ける期限)。
 * 一覧はスクロールのたびに行を作り直すので、期限を過ぎたら付けない
 */
export const justDone = new Map<string, number>();
export const justChecked = new Map<string, { until: number; delay: number }>();

export function isFresh(until: number | undefined): boolean {
  return until !== undefined && Date.now() < until;
}

/** 要素のアニメーションを最初から付け直す */
export function replayClass(el: Element, cls: string) {
  el.classList.remove(cls);
  void (el as HTMLElement).offsetWidth;
  el.classList.add(cls);
}

/** View Transitions API があれば、画面の切り替えを滑らかにする (無ければそのまま切り替える) */
export function withTransition(update: () => void): Promise<void> {
  const d = document as Document & { startViewTransition?: (cb: () => void) => { finished: Promise<void> } };
  if (reduceMotion || !d.startViewTransition) {
    update();
    return Promise.resolve();
  }
  return d.startViewTransition(update).finished.catch(() => undefined);
}

export function dismissToast(el: HTMLElement) {
  el.classList.add("leaving");
  setTimeout(() => el.remove(), reduceMotion ? 0 : 180);
}

/** key 付きの通知で、いま出ているもの (key → 要素と消すタイマー) */
const keyedToasts = new Map<string, { el: HTMLElement; timer: number }>();

/**
 * 通知を出す。key を渡すと、同じ key の通知が出ている間は新しく足さず、
 * その中身を差し替えて表示時間を延ばす (続けて同じ種類の通知が積み重ならないように)
 */
export function toast(msg: string, kind: "info" | "error" = "info", ms = 3500, key?: string) {
  const cur = key ? keyedToasts.get(key) : undefined;
  if (cur) {
    clearTimeout(cur.timer);
    cur.el.className = `toast ${kind === "error" ? "error" : ""}`;
    cur.el.textContent = msg;
    // 差し替えたことが分かるよう、一瞬弾ませる (同じ文言のときも)
    if (!reduceMotion) replayClass(cur.el, "refresh");
    cur.timer = window.setTimeout(() => dropKeyed(key!), ms);
    return;
  }
  const el = document.createElement("div");
  el.className = `toast ${kind === "error" ? "error" : ""}`;
  el.textContent = msg;
  $("toast-root").appendChild(el);
  if (key) keyedToasts.set(key, { el, timer: window.setTimeout(() => dropKeyed(key), ms) });
  else setTimeout(() => dismissToast(el), ms);
}

/** 失敗の通知 (コマンドの失敗・保存できなかったときなど)。ログ (zenpod.log) にも残す。
 * 入力の誤りの案内など、ログに残すものでない通知は `toast(…, "error")` のまま */
export function failToast(e: unknown, ms = 6000, where?: string) {
  const msg = String(e);
  void invoke("ui_log", { msg: where ? `${where}: ${msg}` : msg }).catch(() => {});
  toast(msg, "error", ms);
}

function dropKeyed(key: string) {
  const cur = keyedToasts.get(key);
  if (!cur) return;
  keyedToasts.delete(key);
  dismissToast(cur.el);
}

/** 終わるまで出しておく通知 (回る印付き)。set で文言を変え、done で消す */
export function busyToast(msg: string): { set: (m: string) => void; done: () => void } {
  const el = document.createElement("div");
  el.className = "toast busy";
  el.innerHTML = `<span class="spinner small"></span><span class="busy-text"></span>`;
  const text = el.querySelector<HTMLElement>(".busy-text")!;
  text.textContent = msg;
  $("toast-root").appendChild(el);
  return { set: (m) => (text.textContent = m), done: () => dismissToast(el) };
}

/**
 * アートワークの反映 (ダウンロード・曲への書き込み) を、経過を見せながら行う。
 * `show` に文言を渡す (省略時は通知で出す)。バックエンドの artwork-progress を受けて文言を変える
 */
export async function withArtProgress<T>(first: string, run: () => Promise<T>, show?: (m: string) => void): Promise<T> {
  const toastEl = show ? null : busyToast(first);
  const set = show ?? ((m: string) => toastEl!.set(m));
  set(first);
  const unlisten = await listen<{ phase: string; done: number; total: number }>("artwork-progress", (ev) => {
    const { phase, done, total } = ev.payload;
    set(phase === "download" ? tr("画像をダウンロード中…") : total > 1 ? tr("書き込み中… {n} / {a} 曲", { n: done.toLocaleString(), a: total.toLocaleString() }) : tr("書き込み中…"));
  });
  try {
    return await run();
  } finally {
    unlisten();
    toastEl?.done();
  }
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    failToast(e, 6000, cmd);
    return null;
  }
}

/** 値を返さないコマンドを呼ぶ。成功したら true (失敗は通知で出す)。`call` は成功しても null を返すので、成否はこちらで見る */
export async function callOk(cmd: string, args?: Record<string, unknown>): Promise<boolean> {
  try {
    await invoke(cmd, args);
    return true;
  } catch (e) {
    failToast(e, 6000, cmd);
    return false;
  }
}
