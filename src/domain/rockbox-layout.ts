// Rockbox で曲を置く場所の形 ({artist}/{album}/{track} {title} など) と、画面での表し方

import { t as tr } from "../i18n";

// ---------------- Rockbox の転送先の形 ----------------

/** 形の部品 (Rust の layout.rs と同じ名前) と、画面での呼び名 */
// tr: 使う側で訳す
export const LAYOUT_PARTS: [string, string][] = [
  ["albumartist", "アルバムアーティスト"],
  ["artist", "アーティスト"],
  ["album", "アルバム"],
  ["year", "年"],
  ["disctrack", "ディスク-曲順"],
  ["track", "曲順"],
  ["disc", "ディスク"],
  ["title", "タイトル"],
  ["genre", "ジャンル"],
];
/** よく使う形 (layout.rs の PRESETS と同じ) */
export const LAYOUT_PRESETS = [
  "Music/{albumartist}/{album}/{disctrack} {title}",
  "Music/{albumartist}/{album}/{disctrack} - {title}",
  "Music/{albumartist}/{album}/{title}",
  "Music/{albumartist} - {album}/{disctrack} {title}",
  "Music/{album}/{disctrack} {title}",
  "Music/{album} [{year}]/{title} - {artist}",
  "Music/{album} [{year}]/{disctrack} {title}",
  "Music/{albumartist}/{album} ({year})/{disctrack} {title}",
  "Music/{albumartist}/{year} - {album}/{disctrack} {title}",
];
export const LAYOUT_DEFAULT = LAYOUT_PRESETS[0];

/** "{album}" → "[アルバム]" (画面の言語で) */
export function layoutToDisplay(t: string): string {
  return t.replace(/\{(\w+)\}/g, (whole, k: string) => {
    const p = LAYOUT_PARTS.find(([key]) => key === k);
    return p ? `[${tr(p[1])}]` : whole;
  });
}

/** "[アルバム]" (日本語でも画面の言語でも) → "{album}"。知らない "[...]" はそのまま (年を囲む括弧など) */
export function layoutFromDisplay(s: string): string {
  return s.trim().replace(/\[([^\[\]]+)\]/g, (whole, name: string) => {
    const p = LAYOUT_PARTS.find(([, label]) => label === name || tr(label) === name);
    return p ? `{${p[0]}}` : whole;
  });
}
