/**
 * 画面の文言の翻訳。日本語の文言そのものをキーにして、辞書 (locales/*.json) から引く。
 * 辞書に無い文言は日本語のまま出す (訳の漏れがあっても画面は壊れない)。
 *
 * - 値の埋め込み: t("{n} 曲を選択中", { n: 3 })
 * - 英語の単数 / 複数: 訳を "{n} song|{n} songs" のように | で分けると、n が 1 なら前、それ以外は後
 * - index.html の固定の文言は translateStatic で、文字と title / placeholder / aria-label を置き換える
 */
// 辞書は文字列のまま入れて JSON.parse する。JSON として import すると、見出しの日本語 (「ローマ字・英語」など) が
// JS の名前として書き出される。「・」は Unicode 15.1 から名前に使える字になったが、古い ICU の WebKit
// (Ubuntu 22.04 の 2.50 など) は構文エラーにして、画面の JS が 1 行も動かない
import enText from "./locales/en.json?raw";

const en: Record<string, string> = JSON.parse(enText);

export type Lang = "ja" | "en";

const DICTS: Record<Exclude<Lang, "ja">, Record<string, string>> = { en };

let dict: Record<string, string> | null = null;
export let lang: Lang = "ja";

export function setLang(l: Lang) {
  lang = l;
  dict = l === "ja" ? null : DICTS[l];
  document.documentElement.lang = l;
}

/** 設定で選んだ言語。選んでいなければ OS (WebView) の言語。日本語以外は英語 */
export function detectLang(pref: string | null): Lang {
  if (pref === "ja" || pref === "en") return pref;
  return navigator.language.toLowerCase().startsWith("ja") ? "ja" : "en";
}

/** 同じ日本語でも場面で訳し分けたいときは、キーの頭に "@場面:" を付ける (日本語ではこの印を外して出す) */
function plain(key: string): string {
  return key.startsWith("@") ? key.slice(key.indexOf(":") + 1) : key;
}

export function t(key: string, vars?: Record<string, string | number>): string {
  let s = dict?.[key] ?? plain(key);
  if (vars) {
    // n は数か、桁区切り付きの文字列 ("1,234")
    const n = typeof vars.n === "number" ? vars.n : typeof vars.n === "string" ? Number(vars.n.replace(/[,\s]/g, "")) : NaN;
    if (s.includes("|") && !Number.isNaN(n)) {
      const [one, many] = s.split("|");
      s = n === 1 ? one : many;
    }
    s = s.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));
  }
  return s;
}

const ATTRS = ["title", "placeholder", "aria-label"];

/** 訳す前の日本語 (言語を切り替えたときに、ここから訳し直す) */
const origText = new WeakMap<Node, string>();
const origAttr = new WeakMap<Element, Record<string, string>>();

/** 固定の HTML の文言を訳す (起動時と、言語を切り替えたとき)。前後の空白は残す。
 *  最初に見たときの日本語を覚えておき、そこから訳すので、何度でも切り替えられる。
 *  data-tr="@場面:文言" の要素は、その印付きのキーで訳す (日本語では印を外した文言) */
export function translateStatic(root: Element) {
  root.querySelectorAll<HTMLElement>("[data-tr]").forEach((el) => (el.textContent = t(el.dataset.tr!)));
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n.parentElement?.closest("[data-tr]")) continue;
    let orig = origText.get(n);
    if (orig === undefined) {
      orig = n.nodeValue ?? "";
      origText.set(n, orig);
    }
    const key = orig.trim();
    const next = key && dict?.[key] ? orig.replace(key, dict[key]) : orig;
    if (n.nodeValue !== next) n.nodeValue = next;
  }
  root.querySelectorAll("*").forEach((el) => {
    let saved = origAttr.get(el);
    for (const a of ATTRS) {
      const cur = el.getAttribute(a);
      if (cur === null) continue;
      if (!saved) {
        saved = {};
        origAttr.set(el, saved);
      }
      if (!(a in saved)) saved[a] = cur;
      const v = saved[a];
      const next = dict?.[v] ?? v;
      if (cur !== next) el.setAttribute(a, next);
    }
  });
}
