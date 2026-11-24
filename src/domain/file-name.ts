// タグのパーツを組み合わせたファイル名 (複数の曲のファイル名の変更)

import { t as tr } from "../i18n";
import type { Track } from "./types";

// ---- 複数の曲のファイル名を、タグのパーツを組み合わせて付ける ----

/** [パーツ名] → 曲のタグの値 */
// tr: 使う側で訳す
export const NAME_PARTS: [string, (t: Track, pad: boolean) => string][] = [
  ["曲順", (t, pad) => (t.trackNr ? (pad ? String(t.trackNr).padStart(2, "0") : String(t.trackNr)) : "")],
  ["ディスク", (t) => (t.discNr ? String(t.discNr) : "")],
  ["タイトル", (t) => t.title],
  ["アーティスト", (t) => t.artist],
  ["アルバム", (t) => t.album],
  ["アルバムアーティスト", (t) => t.albumArtist],
  ["年", (t) => (t.year ? String(t.year) : "")],
];
// tr: 使う側で訳す
export const NAME_PRESETS = ["[曲順] - [タイトル]", "[曲順]. [タイトル]", "[アーティスト] - [タイトル]", "[ディスク]-[曲順] [タイトル]"];

/** ファイル名に使えない文字は見た目の近い全角文字にする (? → ？ など)。末尾の . と空白は取る */
function safeFileName(name: string): string {
  const full: Record<string, string> = { "\\": "＼", "/": "／", ":": "：", "*": "＊", "?": "？", '"': "＂", "<": "＜", ">": "＞", "|": "｜" };
  let s = [...name]
    .map((c) => full[c] ?? c)
    .filter((c) => c.charCodeAt(0) >= 0x20)
    .join("")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[. ]+$/, "");
  if (/^(con|prn|aux|nul|com\d|lpt\d)$/i.test(s)) s += "_";
  return s;
}

/** パターンから名前を作る。使ったパーツが空なら missing に名前を入れる */
/** 書式の部品の名前を、画面の言語の名前にする ([曲順] → [Track] など)。知らない部品はそのまま */
export function localizePattern(pattern: string): string {
  return pattern.replace(/\[([^\]]+)\]/g, (whole, key: string) => {
    const part = NAME_PARTS.find(([k]) => k === key || tr(k) === key);
    return part ? `[${tr(part[0])}]` : whole;
  });
}

export function nameFromPattern(pattern: string, t: Track, pad: boolean): { name: string; missing: string[] } {
  const missing: string[] = [];
  const name = pattern.replace(/\[([^\]]+)\]/g, (whole, key: string) => {
    // 部品の名前は日本語でも、画面の言語の名前 ([Title] など) でもよい
    const part = NAME_PARTS.find(([k]) => k === key || tr(k) === key);
    if (!part) return whole;
    const v = part[1](t, pad);
    if (!v) missing.push(part[0]);
    return v;
  });
  return { name: safeFileName(name), missing };
}

export function splitFileName(path: string): { dir: string; stem: string; ext: string } {
  const i = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  const file = path.slice(i + 1);
  const dot = file.lastIndexOf(".");
  return { dir: path.slice(0, i + 1), stem: dot > 0 ? file.slice(0, dot) : file, ext: dot > 0 ? file.slice(dot) : "" };
}
