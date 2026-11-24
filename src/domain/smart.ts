// スマートプレイリストの条件: 型・項目と比較の表・iPod の値 (Mac 時刻・ビット) と画面の値の変換。
// 画面の部品 (HTML) は ui/dialogs/smart-editor.ts

import { t as tr } from "../i18n";
import { fmtDur } from "./format";

// ---------------- スマートプレイリスト ----------------
// 条件の形は crates/ipod-core/src/smartpl.rs。
// 画面では iTunes の条件の画面と同じ項目・比較・値を出し、保存のときに iTunes と同じ形に直す

/** 文字以外の条件の値 (2^53 を超える値があるので文字で持つ) */
interface SplNum {
  from: string;
  fromDate: string;
  fromUnits: string;
  to: string;
  toDate: string;
  toUnits: string;
}
type SplRule = { kind: "group"; any: boolean; items: SplRule[] } | { kind: "cond"; field: number; action: number; value: string | SplNum };
export interface SplPrefs {
  liveUpdate: boolean;
  checkRules: boolean;
  checkLimits: boolean;
  limitType: number;
  limitSort: number;
  reverseSort: boolean;
  limitValue: number;
  matchChecked: boolean;
}
export type SplMedia = "all" | "music" | "movies" | "tv" | "podcasts" | "audiobooks";
export interface SplDef {
  media: SplMedia;
  rules: { any: boolean; items: SplRule[] };
  prefs: SplPrefs;
}
export interface SplPreview {
  count: number;
  lengthMs: number;
  size: number;
  notOnIpod: boolean;
  /** 入る曲の先頭いくつかの id */
  sample: string[];
}

type SplType = "text" | "num" | "bitrate" | "samplerate" | "size" | "time" | "date" | "rating" | "bool" | "boolNot" | "media" | "playlist" | "icloud" | "location" | "love";
export type SplOp = "contains" | "notContains" | "is" | "isNot" | "starts" | "ends" | "gt" | "lt" | "after" | "before" | "range" | "inLast" | "notInLast";

// 項目 (iTunes の条件の画面のプルダウンの順)。boolNot は「である」を「0 ではない」で書く項目
// tr: 使う側で訳す
const SPL_FIELDS: [number, string, SplType][] = [
  [0x23, "BPM", "num"],
  [0x86, "iCloud の状況", "icloud"],
  [0x50, "アーティスト (読み)", "text"],
  [0x04, "アーティスト", "text"],
  [0x25, "アートワーク付き", "boolNot"],
  [0x4f, "アルバム (読み)", "text"],
  [0x51, "アルバムアーティスト (読み)", "text"],
  [0x47, "アルバムアーティスト", "text"],
  [0x03, "アルバム", "text"],
  [0x5a, "アルバムの評価", "rating"],
  [0x9c, "アルバムラブ", "love"],
  [0x37, "カテゴリ", "text"],
  [0x27, "グループ", "text"],
  [0x0e, "コメント", "text"],
  [0x59, "コンテンツレート", "text"],
  [0x1f, "コンピレーション", "bool"],
  [0x0c, "サイズ", "size"],
  [0x06, "サンプルレート", "samplerate"],
  [0x3f, "シーズン", "num"],
  [0x08, "ジャンル", "text"],
  [0x44, "スキップ回数", "num"],
  [0x1d, "チェック済み", "bool"],
  [0x18, "ディスク番号", "num"],
  [0x0b, "トラック番号", "num"],
  [0x05, "ビットレート", "bitrate"],
  [0x28, "@spl:プレイリスト", "playlist"],
  [0x3c, "メディアの種類", "media"],
  [0x9a, "ラブ", "love"],
  [0xa1, "楽章番号", "num"],
  [0xa0, "楽章名", "text"],
  [0x29, "購入した項目", "boolNot"],
  [0x16, "再生回数", "num"],
  [0x45, "最後にスキップした日", "date"],
  [0x17, "最後に再生した日", "date"],
  [0x52, "作曲者 (読み)", "text"],
  [0x12, "作曲者", "text"],
  [0x9f, "作品", "text"],
  [0x0d, "時間", "time"],
  [0x09, "種類", "text"],
  [0x85, "場所", "location"],
  [0x36, "説明", "text"],
  [0x10, "追加日", "date"],
  [0x07, "年", "num"],
  [0x53, "番組 (読み)", "text"],
  [0x3e, "番組", "text"],
  [0x19, "評価", "rating"],
  [0x0a, "変更日", "date"],
  [0x4e, "名前 (読み)", "text"],
  [0x02, "名前", "text"],
];

// tr: 使う側で訳す
export const SPL_OP_LABEL: Record<SplOp, string> = {
  contains: "を含む",
  notContains: "を含まない",
  is: "である",
  isNot: "ではない",
  starts: "で始まる",
  ends: "で終わる",
  gt: "より大きい",
  lt: "より小さい",
  after: "より後",
  before: "より前",
  range: "の範囲内",
  inLast: "以内",
  notInLast: "以内ではない",
};

// 真偽の項目の比較 (英語は iTunes と同じ is true / is false)
// tr: 使う側で訳す
export const SPL_BOOL_OP_LABEL: Partial<Record<SplOp, string>> = { is: "@bool:である", isNot: "@bool:ではない" };

// 値を選ぶ項目の選択肢 (値, 表示)。iTunes の画面と同じ順
// tr: 使う側で訳す
export const SPL_CHOICES: Partial<Record<SplType, [string, string][]>> = {
  media: [["1", "ミュージック"], ["32", "ミュージックビデオ"], ["2", "ムービー"], ["64", "テレビ番組"], ["4", "ポッドキャスト"], ["8", "オーディオブック"], ["1048576", "ボイスメモ"], ["65536", "iTunes Extras"], ["1024", "ホームビデオ"]],
  icloud: [["2", "マッチ"], ["1", "購入した項目"], ["3", "アップロード済み"], ["4", "不適格"], ["5", "削除済み"], ["6", "エラー"], ["7", "重複"], ["8", "Apple Music"], ["9", "利用不可"], ["10", "未アップロード"]],
  location: [["1", "このコンピュータ上"], ["16", "iCloud"]],
  love: [["2", "ラブ済み"], ["3", "好きじゃない"], ["0", "なし"]],
};

/** iTunes がビットの AND (0x400) で書く値 (メディアの種類のポッドキャスト・iTunes Extras、場所の iCloud) */
function splBitAnd(type: SplType, v: string): boolean {
  return (type === "media" && (v === "4" || v === "65536")) || (type === "location" && v === "16");
}

// 上限の単位と選択方法 (smartpl.rs の LIMIT_* / SORT_*)
// tr: 使う側で訳す
export const SPL_LIMIT_TYPES: [number, string][] = [[3, "@limit:曲"], [1, "分"], [4, "@limit:時間"], [2, "MB"], [5, "GB"]];
// tr: 使う側で訳す
export const SPL_SORTS: [number, boolean, string][] = [
  [0x02, false, "ランダム"],
  [0x06, false, "アルバム"],
  [0x07, false, "アーティスト"],
  [0x09, false, "ジャンル"],
  [0x05, false, "名前"],
  [0x1c, false, "評価の高い順"],
  [0x1c, true, "評価の低い順"],
  [0x1a, false, "最近再生した順"],
  [0x1a, true, "再生してから長い順"],
  [0x19, false, "再生回数の多い順"],
  [0x19, true, "再生回数の少ない順"],
  [0x15, false, "最近追加した順"],
  [0x15, true, "追加してから長い順"],
];
// tr: 使う側で訳す
export const SPL_MEDIA_LABEL: Record<SplMedia, string> = {
  music: "ミュージック",
  movies: "ムービー",
  tv: "テレビ番組",
  podcasts: "ポッドキャスト",
  audiobooks: "オーディオブック",
  all: "@media:すべて",
};
// tr: 使う側で訳す
export const SPL_UNITS: [number, string][] = [[86400, "日"], [604800, "週間"], [2628000, "か月"]];

const SPL_NOT = 0x0200_0000;
const SPL_STR = 0x0100_0000;
/** 1904-01-01 から 1970-01-01 までの秒 */
const MAC_EPOCH = 2082844800;
/** 「以内」の from に iTunes が書く決まった値 */
const SPL_IN_LAST = "3291618603768360366";

/** 画面で編集する条件。a / b は画面の単位の値 (MB・m:ss・星の数・日付など)、unit は「以内」の単位の秒 */
export interface SplCond {
  kind: "cond";
  field: number;
  op: SplOp;
  a: string;
  b: string;
  unit: number;
}
/** 画面で編集できない条件 (知らない項目・比較) はそのまま残す */
export type SplNode = SplCond | { kind: "group"; any: boolean; items: SplNode[] } | { kind: "raw"; rule: SplRule };

export function splType(field: number): SplType | null {
  return SPL_FIELDS.find((f) => f[0] === field)?.[2] ?? null;
}

export function splOps(type: SplType): SplOp[] {
  switch (type) {
    case "text":
      return ["contains", "notContains", "is", "isNot", "starts", "ends"];
    case "num":
    case "bitrate":
    case "samplerate":
    case "size":
    case "time":
    case "rating":
      return ["is", "isNot", "gt", "lt", "range"];
    case "date":
      return ["is", "isNot", "after", "before", "inLast", "notInLast", "range"];
    default:
      return ["is", "isNot"];
  }
}

function splNum(from: number | string, to: number | string = from): SplNum {
  return { from: String(from), fromDate: "0", fromUnits: "1", to: String(to), toDate: "0", toUnits: "1" };
}

/** 日付の欄 (YYYY-MM-DD) ↔ その日の 0:00 (PC の現地時刻のまま 1904 年からの秒。iTunes と同じ) */
function splDayStart(d: string): number {
  const [y, m, day] = d.split("-").map(Number);
  const t = Date.UTC(y || 2000, (m || 1) - 1, day || 1) / 1000;
  return t + MAC_EPOCH;
}
function splDayOf(mac: number): string {
  return new Date((mac - MAC_EPOCH) * 1000).toISOString().slice(0, 10);
}
export function splToday(): string {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}
function splParseTime(s: string): number {
  const p = s.split(":").map((x) => Number(x) || 0);
  return (p.length > 1 ? p[0] * 60 + p[1] : p[0]) * 1000;
}

/** iTunes の形の条件 → 画面の条件。分からない形は null */
function splDecode(r: SplRule & { kind: "cond" }): SplCond | null {
  const type = splType(r.field);
  if (!type) return null;
  const not = (r.action & SPL_NOT) !== 0;
  const op = r.action & 0xffffff;
  const c: SplCond = { kind: "cond", field: r.field, op: "is", a: "", b: "", unit: 86400 };
  if (type === "text") {
    if (typeof r.value !== "string") return null;
    const m: Record<number, [SplOp, SplOp | null]> = { 2: ["contains", "notContains"], 1: ["is", "isNot"], 4: ["starts", null], 8: ["ends", null] };
    const o = m[op]?.[not ? 1 : 0];
    if (!o) return null;
    return { ...c, op: o, a: r.value };
  }
  if (typeof r.value === "string") return null;
  const from = Number(r.value.from);
  const to = Number(r.value.to);
  const isNot = (yes: SplOp, no: SplOp) => (not ? no : yes);
  switch (type) {
    case "bool":
    case "boolNot":
      if (op !== 1) return null;
      return { ...c, op: (from !== 0) !== not ? "is" : "isNot" };
    case "media":
    case "icloud":
    case "location":
    case "love":
      if (op !== 1 && op !== 0x400) return null;
      return { ...c, op: isNot("is", "isNot"), a: r.value.from };
    case "playlist":
      if (op !== 1) return null;
      return { ...c, op: isNot("is", "isNot"), a: r.value.from };
    case "date":
      if (op === 0x100 && to - from === 86399) return { ...c, op: isNot("is", "isNot"), a: splDayOf(from), b: splDayOf(from) };
      if (op === 0x100 && !not) return { ...c, op: "range", a: splDayOf(from), b: splDayOf(to) };
      if (op === 0x10 && !not) return { ...c, op: "after", a: splDayOf(from) };
      if (op === 0x40 && !not) return { ...c, op: "before", a: splDayOf(from) };
      if (op === 0x200) return { ...c, op: isNot("inLast", "notInLast"), a: String(-Number(r.value.fromDate)), unit: Number(r.value.fromUnits) };
      return null;
    case "rating":
      if (op === 0x100 && to - from === 9) return { ...c, op: isNot("is", "isNot"), a: String(from / 20), b: String(from / 20) };
      if (op === 0x100 && !not) return { ...c, op: "range", a: String(Math.round(from / 20)), b: String(Math.floor(to / 20)) };
      if (op === 0x10 && !not) return { ...c, op: "gt", a: String(Math.floor(from / 20)) };
      if (op === 0x40 && !not) return { ...c, op: "lt", a: String(Math.round(from / 20)) };
      return null;
    case "time":
      if (op === 0x100 && to - from === 999) return { ...c, op: isNot("is", "isNot"), a: fmtDur(from), b: fmtDur(from) };
      if (op === 0x100 && !not) return { ...c, op: "range", a: fmtDur(from), b: fmtDur(to) };
      if (op === 0x10 && !not) return { ...c, op: "gt", a: fmtDur(from) };
      if (op === 0x40 && !not) return { ...c, op: "lt", a: fmtDur(from) };
      return null;
    default: {
      // 数 (サイズは MB で出す)
      const show = (v: number) => (type === "size" ? String(Math.round((v / 1048576) * 100) / 100) : String(v));
      if (op === 1) return { ...c, op: isNot("is", "isNot"), a: show(from), b: show(from) };
      if (not) return null;
      if (op === 0x10) return { ...c, op: "gt", a: show(from) };
      if (op === 0x40) return { ...c, op: "lt", a: show(from) };
      if (op === 0x100) return { ...c, op: "range", a: show(from), b: show(to) };
      return null;
    }
  }
}

/** 画面の条件 → iTunes の形 */
function splEncode(c: SplCond): SplRule {
  const type = splType(c.field) ?? "text";
  const neg = c.op === "isNot" || c.op === "notContains" || c.op === "notInLast";
  const rule = (action: number, value: string | SplNum): SplRule => ({ kind: "cond", field: c.field, action: neg ? action | SPL_NOT : action, value });
  const n = (s: string) => Number(s) || 0;
  switch (type) {
    case "text": {
      const base = { contains: 2, notContains: 2, is: 1, isNot: 1, starts: 4, ends: 8 }[c.op as "is"] ?? 2;
      return rule(SPL_STR | base, c.a);
    }
    case "bool":
      return rule(1, splNum(1));
    case "boolNot":
      // 「である」は「0 ではない」、「ではない」は「0 である」(iTunes と同じ)
      return { kind: "cond", field: c.field, action: c.op === "is" ? 1 | SPL_NOT : 1, value: splNum(0) };
    case "media":
    case "icloud":
    case "location":
    case "love":
    case "playlist":
      return rule(splBitAnd(type, c.a) ? 0x400 : 1, splNum(c.a || "0"));
    case "date": {
      const s = splDayStart(c.a);
      if (c.op === "inLast" || c.op === "notInLast")
        return rule(0x200, { from: SPL_IN_LAST, fromDate: String(-Math.max(1, n(c.a))), fromUnits: String(c.unit), to: SPL_IN_LAST, toDate: "0", toUnits: "1" });
      if (c.op === "after") return rule(0x10, splNum(s + 86399));
      if (c.op === "before") return rule(0x40, splNum(s));
      if (c.op === "range") return rule(0x100, splNum(s, splDayStart(c.b) + 86399));
      return rule(0x100, splNum(s, s + 86399));
    }
    case "rating": {
      const a = Math.max(0, Math.min(5, n(c.a))) * 20;
      if (c.op === "gt") return rule(0x10, splNum(a + 9));
      if (c.op === "lt") return rule(0x40, splNum(a));
      if (c.op === "range") return rule(0x100, splNum(a, Math.max(0, Math.min(5, n(c.b))) * 20 + 9));
      return rule(0x100, splNum(a, a + 9));
    }
    case "time": {
      const a = splParseTime(c.a);
      if (c.op === "gt") return rule(0x10, splNum(a + 999));
      if (c.op === "lt") return rule(0x40, splNum(a));
      if (c.op === "range") return rule(0x100, splNum(a, splParseTime(c.b) + 999));
      return rule(0x100, splNum(a, a + 999));
    }
    default: {
      const v = (s: string) => (type === "size" ? Math.round(n(s) * 1048576) : Math.round(n(s)));
      if (c.op === "gt") return rule(0x10, splNum(v(c.a)));
      if (c.op === "lt") return rule(0x40, splNum(v(c.a)));
      if (c.op === "range") return rule(0x100, splNum(v(c.a), v(c.b)));
      return rule(1, splNum(v(c.a)));
    }
  }
}

export function splToNodes(items: SplRule[]): SplNode[] {
  return items.map((r): SplNode => (r.kind === "group" ? { kind: "group", any: r.any, items: splToNodes(r.items) } : (splDecode(r) ?? { kind: "raw", rule: r })));
}
export function splToRules(items: SplNode[]): SplRule[] {
  return items.map((n): SplRule => (n.kind === "group" ? { kind: "group", any: n.any, items: splToRules(n.items) } : n.kind === "raw" ? n.rule : splEncode(n)));
}

// 項目のまとまり (プルダウンの見出し)。iPod に入っていない情報は最後にまとめて、それと分かるようにする
// tr: 使う側で訳す
export const SPL_FIELD_GROUPS: [string, number[]][] = [
  ["基本の情報", [0x02, 0x04, 0x03, 0x47, 0x12, 0x08, 0x07, 0x27, 0x0e, 0x1f, 0x3c, 0x09]],
  ["再生と評価", [0x16, 0x17, 0x44, 0x45, 0x19, 0x10, 0x0a, 0x28, 0x1d]],
  ["ファイル", [0x0d, 0x0c, 0x05, 0x06, 0x25]],
  ["@spl:番号", [0x0b, 0x18, 0x23]],
  ["番組 (テレビ・ポッドキャスト)", [0x3e, 0x3f, 0x36, 0x37]],
  ["読み (並べ替え用)", [0x4e, 0x50, 0x4f, 0x51, 0x52, 0x53]],
  ["iTunes だけの情報 (iPod には入っていない)", [0x9a, 0x9c, 0x5a, 0x29, 0x86, 0x85, 0x59, 0x9f, 0xa0, 0xa1]],
];

/** Rockbox の曲で ZenPod が持っていない情報 (iTunes だけの情報のほかに) */
export const SPL_NOT_IN_ROCKBOX = [0x17, 0x44, 0x45, 0x0a, 0x23, 0x0e, 0x27, 0x1f, 0x06, 0x3e, 0x3f, 0x36, 0x37, 0x53, 0x52];

export function splFieldLabel(field: number): string {
  const f = SPL_FIELDS.find((x) => x[0] === field);
  return f ? tr(f[1]) : `0x${field.toString(16)}`;
}
