// Rockbox の転送先の形の設定と、iPod ごとに覚えた形を使うこと

import { t as tr } from "../i18n";
import { invoke } from "../infra/tauri";
import { isRockbox } from "../app/selectors";
import { state } from "../app/state";
import { LAYOUT_DEFAULT, LAYOUT_PARTS, LAYOUT_PRESETS, layoutFromDisplay, layoutToDisplay } from "../domain/rockbox-layout";
import type { Snapshot } from "../domain/types";
import { getStored, setStored } from "../infra/prefs";
import { esc } from "./dom";
import { call } from "./feedback";
import { confirmBox, modal } from "./modal";
import { layoutExample } from "./onboarding";
import { openSettings, renderSettingsDevice } from "./settings";
import { tokenEditor, tokensHtml } from "./token-editor";

/** iPod ごとに覚えた形 (Rust の形式)。無ければ null */
function storedLayout(root: string): string | null {
  try {
    return (JSON.parse(getStored("rbLayouts") ?? "{}") as Record<string, string>)[root] ?? null;
  } catch {
    return null;
  }
}

function storeLayout(root: string, template: string) {
  let map: Record<string, string> = {};
  try {
    map = JSON.parse(getStored("rbLayouts") ?? "{}");
  } catch {
    /* 壊れていたら作り直す */
  }
  map[root] = template;
  setStored("rbLayouts", JSON.stringify(map));
}

/**
 * Rockbox の iPod を開いたら、覚えている転送先の形を渡す。まだ決めていない iPod なら、今ある曲の並びから推測して、
 * 今までの形 (Music/アーティスト/アルバム) と違えば「この形で転送しますか？」と聞く (勝手には変えない)
 */
/**
 * 聞かずに決めた転送先の形を知らせる。「設定で変える」なら設定の「転送」の形の欄を開く
 */
async function noticeLayout(template: string, matched: boolean) {
  const example = (await call<string>("preview_rockbox_layout", { template })) ?? "";
  let toSettings = false;
  await modal(
    `<h3>${esc(tr("転送先のフォルダの形"))}</h3>
     <p>${esc(
       matched
         ? tr("この iPod の曲の並びに合わせて、新しく転送する曲も「{a}」の形で置きます。", { a: layoutToDisplay(template) })
         : tr("新しく転送する曲は「{a}」の形で置きます。", { a: layoutToDisplay(template) }),
     )}</p>
     ${example ? `<p class="layout-notice-example muted">${esc(tr("例: {a}", { a: example }))}</p>` : ""}
     <div class="modal-actions"><button class="btn" data-x="settings">${esc(tr("設定で変える"))}</button><button class="btn primary" data-x="ok">OK</button></div>`,
    (root, close) => {
      const ok = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      ok.focus();
      ok.addEventListener("click", close);
      root.querySelector('[data-x="settings"]')!.addEventListener("click", () => {
        toSettings = true;
        close();
      });
    },
    "",
    false,
  );
  if (!toSettings) return;
  state.settingsCat = "transfer";
  await openSettings();
  document.getElementById("set-layout")?.scrollIntoView({ block: "start" });
}

/** 推測できない (曲が少ない) iPod に、既定の形で置くと知らせたか (iPod の root の一覧。iPod ごとに 1 回) */
function layoutNoted(root: string): boolean {
  try {
    return (JSON.parse(getStored("rbLayoutNoted") ?? "[]") as string[]).includes(root);
  } catch {
    return false;
  }
}

function setLayoutNoted(root: string) {
  let list: string[] = [];
  try {
    list = JSON.parse(getStored("rbLayoutNoted") ?? "[]");
  } catch {
    /* 壊れていたら作り直す */
  }
  if (!list.includes(root)) setStored("rbLayoutNoted", JSON.stringify([...list, root]));
}

export async function applyRockboxLayout(s: Snapshot) {
  const root = s.device.root;
  const saved = storedLayout(root);
  if (saved) {
    await call("set_rockbox_layout", { template: saved });
    return;
  }
  // はじめの案内で選んだ、新しい iPod の既定の形
  const def = getStored("rbLayoutDefault") ?? LAYOUT_DEFAULT;
  const g = await call<{ template: string; matched: number; total: number } | null>("guess_rockbox_layout");
  if (!g) {
    // 曲がまだ少ない iPod は推測できないので、既定の形で転送する (覚えずに、次に開いたときにまた推測する)。
    // どの形で置くかは、iPod ごとに 1 回だけ知らせる
    await call("set_rockbox_layout", { template: def });
    if (!layoutNoted(root)) {
      setLayoutNoted(root);
      await noticeLayout(def, false);
    }
    return;
  }
  if (g.template === def) {
    // 聞かずに決めたときも、どの形で置くかは知らせる (覚えるので次からは出さない)
    storeLayout(root, g.template);
    await call("set_rockbox_layout", { template: def });
    await noticeLayout(def, true);
    return;
  }
  const example = await call<string>("preview_rockbox_layout", { template: g.template });
  const ok = await confirmBox(
    tr("転送先のフォルダの形を合わせますか？"),
    tr("この iPod の曲は「{a}」の形で並んでいるようです ({n} 曲中 {m} 曲)。新しく転送する曲も同じ形で置きますか？ 例: {b}。あとから設定の「転送」で変えられます。", {
      a: layoutToDisplay(g.template),
      n: g.total.toLocaleString(),
      m: g.matched.toLocaleString(),
      b: example ?? "",
    }),
    tr("この形にする"),
    false,
    false,
  );
  const template = ok ? g.template : def;
  storeLayout(root, template);
  await call("set_rockbox_layout", { template });
  if (state.settingsOpen) renderSettingsDevice();
}

/**
 * 設定の「転送」の、Rockbox の転送先の形の欄。Rockbox の iPod を開いていればその iPod の形、
 * そうでなければ新しい iPod の既定の形 (はじめの案内で選んだもの) を変える
 */
export function renderLayoutCard() {
  const el = document.getElementById("set-layout");
  if (!el) return;
  const s = state.snap;
  const device = s && isRockbox() ? s.device.root : null;
  const key = device ?? "(default)";
  if (el.dataset.key === key && el.contains(document.activeElement)) return;
  el.dataset.key = key;
  const current = (device && storedLayout(device)) ?? getStored("rbLayoutDefault") ?? LAYOUT_DEFAULT;
  el.innerHTML = `
    <div class="set-title">${esc(tr("転送先のフォルダの形 (Rockbox)"))}</div>
    <div class="layout-target muted">${esc(device ? tr("この iPod: {a}", { a: s!.name }) : tr("新しい iPod の既定の形 (Rockbox の iPod を開くと、その iPod の形を変えられます)"))}</div>
    <div class="hint">${esc(tr("Rockbox の iPod に曲を転送するときの置き場所です。「/」で区切った最後がファイル名です。ポッドキャストとオーディオブックは Podcasts / Audiobooks に置きます。変えても、今ある曲は動かしません。"))}</div>
    <div class="rn-parts" id="set-layout-parts"></div>
    <div class="tok-editor" id="set-layout-input" contenteditable="plaintext-only" spellcheck="false" role="textbox" aria-label="${esc(tr("転送先のフォルダの形 (Rockbox)"))}"></div>
    <div class="tok-hint muted">${esc(tr("部品はドラッグで足したり並べ替えたりできます。欄の外へ出すと外れます。"))}</div>
    <div class="layout-preview muted" id="set-layout-preview"></div>
    <div class="set-title layout-presets-title">${esc(tr("よく使う形"))}</div>
    <div class="lp-box"><div class="layout-presets">${LAYOUT_PRESETS.map(
      (p) => `<button type="button" class="lp-item" data-lpreset="${esc(p)}">
        <span class="lp-main"><span class="lp-pattern">${tokensHtml(layoutToDisplay(p), LAYOUT_PARTS.map(([, l]) => tr(l)))}</span>${p === LAYOUT_DEFAULT ? `<span class="onb-badge">${esc(tr("おすすめ"))}</span>` : ""}</span>
        <span class="lp-example">${esc(tr("例: {a}", { a: layoutExample(p) }))}</span>
      </button>`,
    ).join("")}</div><div class="onb-folder-fade" aria-hidden="true"></div></div>`;
  const preview = el.querySelector<HTMLElement>("#set-layout-preview")!;
  let timer = 0;
  // 今の形と同じ (一番上のフォルダ名は問わない) よく使う形に印を付ける
  const markPreset = (template: string) => {
    const tail = (t: string) => t.split("/").slice(1).join("/");
    el.querySelectorAll<HTMLElement>("[data-lpreset]").forEach((b) => b.classList.toggle("on", tail(b.dataset.lpreset!) === tail(template)));
  };
  // よく使う形の一覧は高さを決めてスクロールさせる (はじめの案内の PC の音楽フォルダと同じ)。下にまだあるときは下端をぼかす
  const presetList = el.querySelector<HTMLElement>(".layout-presets")!;
  const fadePresets = () => {
    const more = presetList.scrollHeight - presetList.scrollTop - presetList.clientHeight > 1;
    el.querySelector(".lp-box .onb-folder-fade")?.classList.toggle("on", more);
  };
  presetList.addEventListener("scroll", fadePresets, { passive: true });
  const editor = tokenEditor(
    el.querySelector<HTMLElement>("#set-layout-input")!,
    el.querySelector<HTMLElement>("#set-layout-parts")!,
    LAYOUT_PARTS.map(([, label]) => tr(label)),
    layoutToDisplay(current),
    () => {
      clearTimeout(timer);
      timer = window.setTimeout(() => void apply(), 300);
    },
  );
  const showError = (msg: string) => {
    preview.textContent = msg;
    preview.classList.add("layout-error");
  };
  // save = false: 見本を出すだけ (欄を開いただけで形を覚えると、その iPod を開いたときの推測が出なくなる)
  const apply = async (save = true) => {
    const template = layoutFromDisplay(editor.value());
    let example: string | null;
    if (device) {
      // 間違っていれば理由を出し、覚えている形はそのまま
      example = await invoke<string>("preview_rockbox_layout", { template }).catch((e) => {
        showError(String(e));
        return null;
      });
    } else {
      // iPod が無いときは画面だけで確かめる (Rust の layout::validate と同じ決まりの主なもの)
      const parts = template.split("/").map((x) => x.trim());
      const unknown = [...template.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).find((k) => !LAYOUT_PARTS.some(([key]) => key === k));
      if (parts.length < 2 || parts.some((x) => !x || x === "." || x === "..")) example = (showError(tr("フォルダとファイル名を「/」で区切って指定してください")), null);
      else if (!parts[parts.length - 1].includes("{title}")) example = (showError(tr("ファイル名の部分に [タイトル] を入れてください")), null);
      else if (parts[0].includes("{") || parts[0].startsWith(".") || parts[0].toLowerCase() === "ipod_control")
        example = (showError(tr("一番上のフォルダは、決まった名前 (Music など) にしてください")), null);
      else if (unknown) example = (showError(tr("知らない部品です: {a}", { a: unknown })), null);
      else example = layoutExample(template);
    }
    markPreset(template);
    if (example === null) return;
    preview.classList.remove("layout-error");
    preview.textContent = tr("例: {a}", { a: example });
    if (!save) return;
    if (device) {
      await call("set_rockbox_layout", { template });
      storeLayout(device, template);
    } else {
      setStored("rbLayoutDefault", template);
    }
  };
  el.querySelectorAll<HTMLElement>("[data-lpreset]").forEach((b) =>
    b.addEventListener("click", () => {
      // 一番上のフォルダ名 (MUSIC など) は今のまま
      const top = editor.value().split("/")[0].trim();
      const preset = b.dataset.lpreset!;
      const keepTop = top && !top.includes("[") ? `${top}/${preset.split("/").slice(1).join("/")}` : preset;
      editor.set(layoutToDisplay(keepTop));
      void apply();
    }),
  );
  void apply(false);
  // 設定の別のページを見ている間は高さが 0 なので、見えたとき (大きさが変わったとき) に測る。
  // 初めて見えたときは、今の形の行が見えるところまで送る
  let scrolled = false;
  new ResizeObserver(() => {
    if (!presetList.isConnected || !presetList.clientHeight) return;
    if (!scrolled) {
      scrolled = true;
      const on = presetList.querySelector<HTMLElement>(".lp-item.on");
      if (on && on.offsetTop + on.offsetHeight > presetList.clientHeight) presetList.scrollTop = on.offsetTop - 8;
    }
    fadePresets();
  }).observe(presetList);
}
