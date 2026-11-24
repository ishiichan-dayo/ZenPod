// 数値の表示 (長さ・合計時間・容量・日付)

import { lang, t as tr } from "../i18n";

export function fmtDur(ms: number): string {
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function fmtTotal(ms: number): string {
  const m = Math.round(ms / 60000);
  if (m < 60) return tr("{m}分", { m });
  const h = Math.floor(m / 60);
  return h < 24 ? tr("{h}時間{m}分", { h, m: m % 60 }) : tr("{d}日{h}時間", { d: Math.floor(h / 24), h: h % 24 });
}

export function fmtBytes(b: number): string {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (b >= 1024 && i < u.length - 1) {
    b /= 1024;
    i++;
  }
  return `${b.toFixed(i >= 2 ? 1 : 0)} ${u[i]}`;
}

// 日付の書式は作るたびに WebKit (Mac) で 1 回 0.03ms ほどかかり、一覧の行ごとに作ると積み重なるので使い回す
const dayFormats = new Map<string, Intl.DateTimeFormat>();
let photoFormat: Intl.DateTimeFormat | null = null;

/** 秒 (UNIX 時間) を画面の言語の日付にする。0 は空 */
export function fmtDay(sec: number): string {
  if (!sec) return "";
  let f = dayFormats.get(lang);
  if (!f) {
    f = new Intl.DateTimeFormat(lang === "ja" ? "ja-JP" : undefined);
    dayFormats.set(lang, f);
  }
  return f.format(new Date(sec * 1000));
}

/** 写真の撮影日時 (秒。現地時刻をそのまま UTC として持っている)。0 は空 */
export function fmtPhotoDate(sec: number): string {
  if (!sec) return "";
  photoFormat ??= new Intl.DateTimeFormat(undefined, { timeZone: "UTC", dateStyle: "medium", timeStyle: "short" });
  return photoFormat.format(new Date(sec * 1000));
}
