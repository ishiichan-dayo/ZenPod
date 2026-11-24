// 訳の漏れを調べる。
//   node scripts/i18n-check.mjs          辞書 (src/locales/en.json) に無い文言と、tr() で包んでいない日本語を一覧にする
//   node scripts/i18n-check.mjs --keys   辞書に無い文言だけを JSON で出す (辞書に足すとき用)
// 辞書に無い文言があると終了コード 1
import fs from "node:fs";

const JA = /[぀-ヿ一-鿿]/;
const html = fs.readFileSync("index.html", "utf8");
const dict = JSON.parse(fs.readFileSync("src/locales/en.json", "utf8"));

// 画面のコード: src の下の .ts を全部 (i18n.ts は訳の仕組みなので除く)
const tsFiles = fs
  .readdirSync("src", { recursive: true })
  .map((f) => f.replaceAll("\\", "/"))
  .filter((f) => f.endsWith(".ts") && !f.endsWith(".d.ts") && f !== "i18n.ts")
  .sort()
  .map((f) => `src/${f}`);

// 表で持っていて後から tr() に渡す文言 (一覧の列の label / menuLabel / title、メニューの header など)
const PROP = /\b(?:label|menuLabel|title|header|hint):\s*"((?:[^"\\]|\\.)*)"/g;

const keys = new Set();
// tr() の外にある日本語の文字列リテラル (訳し忘れの候補)
const loose = [];

for (const file of tsFiles) {
  const src = fs.readFileSync(file, "utf8");
  // 目印のコメント:
  //   // tr: 使う側で訳す   次の const の表の文言は、使う側で tr() に渡している (辞書に載せる)
  //   // tr: 訳さない       次の行の日本語は訳さない (Rust の文言との比較など)
  const rawLines = src.split("\n");
  const ignoreLines = new Set();
  rawLines.forEach((l, i) => {
    if (l.includes("// tr: 訳さない")) ignoreLines.add(i + 1);
    if (!l.includes("// tr: 使う側で訳す")) return;
    for (let j = i + 1; j < rawLines.length; j++) {
      for (const m of rawLines[j].matchAll(/"((?:[^"\\]|\\.)*)"/g)) if (JA.test(m[1])) keys.add(JSON.parse(`"${m[1]}"`));
      ignoreLines.add(j);
      if (/^[}\]];?$|\];$|};$/.test(rawLines[j]) || (j > i + 1 && /;\s*$/.test(rawLines[j]) && !/^\s/.test(rawLines[j]))) break;
    }
  });

  // コメントを除いたコード (文字列の中の // は残す簡易版: 行頭のコメントと /* */ だけ除く)
  const code = src
    .replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, ""))
    .split("\n")
    .map((l) => (l.trim().startsWith("//") ? "" : l.replace(/\s\/\/ .*$/, "")))
    .join("\n");

  for (const m of code.matchAll(/\btr\(\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(JSON.parse(`"${m[1]}"`));
  for (const m of code.matchAll(PROP)) {
    const s = JSON.parse(`"${m[1]}"`);
    if (JA.test(s)) keys.add(s);
  }

  const stripped = code.replace(/\btr\(\s*"(?:[^"\\]|\\.)*"/g, "tr(").replace(PROP, "");
  stripped.split("\n").forEach((l, i) => {
    if (ignoreLines.has(i)) return;
    // 訳の関数と表の外に日本語の文字があれば報告する (数行にまたがる HTML の中も拾う)
    if (JA.test(l)) loose.push(`${file}:${i + 1}: ${l.trim().slice(0, 100)}`);
  });
}

// index.html: 文字と title / placeholder / aria-label (コメントは除く)
// data-tr の要素の中の文字は日本語の表示用 (キーは data-tr) なので、文字の集計から外す
const body = html.replace(/<!--[\s\S]*?-->/g, "").replace(/(<[^>]*\sdata-tr="[^"]+"[^>]*>)[^<]*</g, "$1<");
for (const m of body.matchAll(/>([^<>]+)</g)) {
  const s = m[1].trim();
  if (s && JA.test(s)) keys.add(s);
}
for (const m of body.matchAll(/\s(?:title|placeholder|aria-label)="([^"]+)"/g)) if (JA.test(m[1])) keys.add(m[1]);
// data-tr="@場面:文言" (場面で訳し分けるキー)
for (const m of body.matchAll(/\sdata-tr="([^"]+)"/g)) keys.add(m[1]);

// ---- Rust (ipod-core と zenpod-app と src-tauri) ----
// t!("…") のキー。目印は画面側と同じ ("// tr: 使う側で訳す" は次の行の日本語を辞書に載せる、"// tr: 訳さない" は無視)
const rsUnder = (dir) =>
  fs.readdirSync(dir, { recursive: true }).filter((f) => f.endsWith(".rs")).map((f) => `${dir}/${f.replaceAll("\\", "/")}`).sort();
const rustFiles = [
  ...rsUnder("src-tauri/src"),
  ...rsUnder("crates/zenpod-app/src"),
  ...rsUnder("crates/ipod-core/src").filter((f) => !f.endsWith("/yomi.rs")),
];
const rustLoose = [];
for (const f of rustFiles) {
  let text = fs.readFileSync(f, "utf8");
  const cut = text.indexOf("#[cfg(test)]");
  if (cut >= 0) text = text.slice(0, cut);
  // コメントの行は除く (説明の例を拾わないように。行数は保つ)。目印は元の行 (raw) から探す
  const raw = text.split("\n");
  text = raw.map((l) => (l.trim().startsWith("//") ? "" : l)).join("\n");
  for (const m of text.matchAll(/\bt!\(\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(JSON.parse(`"${m[1]}"`));
  const lines = text.split("\n");
  lines.forEach((l, i) => {
    const prev = raw[i - 1] ?? "";
    if (l.trim().startsWith("//")) return;
    if (prev.includes("// tr: 使う側で訳す")) {
      for (const m of l.matchAll(/"((?:[^"\\]|\\.)*)"/g)) if (JA.test(m[1])) keys.add(JSON.parse(`"${m[1]}"`));
      return;
    }
    if (prev.includes("// tr: 訳さない")) return;
    // t!( の次の行に文言を書いた形
    if (/\bt!\(\s*$/.test(prev)) return;
    const rest = l.replace(/\bt!\(\s*"(?:[^"\\]|\\.)*"/g, "t!(").replace(/\s\/\/ .*$/, "");
    if (JA.test(rest)) rustLoose.push(`${f}:${i + 1}: ${l.trim().slice(0, 100)}`);
  });
}

const missing = [...keys].filter((k) => !(k in dict));
const unused = Object.keys(dict).filter((k) => !keys.has(k));

if (process.argv.includes("--keys")) {
  console.log(JSON.stringify(Object.fromEntries(missing.map((k) => [k, ""])), null, 2));
} else {
  console.log(`文言 ${keys.size} 件 / 辞書 ${Object.keys(dict).length} 件`);
  if (missing.length) console.log(`\n辞書に無い文言 (${missing.length}):\n` + missing.map((k) => `  ${k}`).join("\n"));
  if (unused.length) console.log(`\n使われていない訳 (${unused.length}):\n` + unused.map((k) => `  ${k}`).join("\n"));
  if (rustLoose.length) console.log(`\nRust で t!() で包んでいない日本語 (${rustLoose.length}):\n` + rustLoose.map((k) => `  ${k}`).join("\n"));
  if (loose.length) console.log(`\ntr() で包んでいない日本語 (${loose.length}):\n` + loose.slice(0, process.env.ALL ? 9999 : 40).map((k) => `  ${k}`).join("\n") + (loose.length > (process.env.ALL ? 9999 : 40) ? `\n  … ほか ${loose.length - 40} 件` : ""));
}
process.exit(missing.length ? 1 : 0);
