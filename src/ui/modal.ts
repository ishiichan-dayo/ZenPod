// ダイアログ (確認・名前の入力・保存中の表示)

import { t as tr } from "../i18n";
import { $ } from "../app/state";
import { esc, isEnter } from "./dom";
import { reduceMotion } from "./feedback";

export function modal(
  html: string,
  onMount?: (root: HTMLElement, close: () => void) => void,
  cls = "",
  /** false なら外側 (オーバーレイ) を押しても閉じない (ボタンか Esc で閉じる) */
  backdropClose = true,
  /** false なら Esc でも閉じない (ボタンでだけ閉じる) */
  escClose = true,
): Promise<void> {
  return new Promise((resolve) => {
    const back = document.createElement("div");
    back.className = "modal-back";
    back.innerHTML = `<div class="modal ${cls}">${html}</div>`;
    const close = () => {
      back.remove();
      document.removeEventListener("keydown", onKey, true);
      resolve();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        if (escClose) close();
      }
    };
    back.addEventListener("mousedown", (e) => {
      if (backdropClose && e.target === back) close();
    });
    document.addEventListener("keydown", onKey, true);
    $("modal-root").appendChild(back);
    onMount?.(back.querySelector(".modal")!, close);
  });
}

/** 保存がこれより早く終われば「保存中…」を出さない (一瞬だけ変わってちらつかないように) */
const SAVE_SHOW_AFTER = 300;
/** 「保存中…」を出したら、変わり終わる (回る印が出る) までは閉じない */
const SAVE_MIN_SHOWN = 700;

/** busySave の戻り値。呼ぶと元に戻す (失敗してダイアログを閉じないとき)。成功したら settle を待ってから閉じる */
export type SaveBusy = (() => void) & { settle: () => Promise<void> };

/**
 * 保存のボタンを押せなくし、保存が SAVE_SHOW_AFTER より長くかかったら「保存中…」(回る印付き) にする。
 * 保存のボタンはどれもこれで同じ見た目にする。
 * 変わり方は段階的に: 1. 幅が広がる (少し弾む) → 2. 文字が「保存中…」に替わる → 3. 回る印が出てくる (CSS の .btn.saving)
 */
export function busySave(btn: HTMLButtonElement): SaveBusy {
  const label = btn.innerHTML;
  const text = esc(tr("保存中…"));
  const html = (t: string) => `<span class="save-spin" aria-hidden="true"><span class="spinner small"></span></span><span class="save-text">${t}</span>`;
  // 押せなくするが、薄くはしない (すぐ終わる保存でボタンがちらつかないように)
  btn.disabled = true;
  btn.classList.add("saving");
  const timers: number[] = [];
  let shownAt = 0;
  const show = () => {
    if (!btn.isConnected) return;
    shownAt = performance.now();
    const w0 = btn.getBoundingClientRect().width;
    // 広がった後の幅を、印と「保存中…」を並べた形で測る (狭くはしない)
    btn.classList.add("spin");
    btn.innerHTML = html(text);
    const w1 = Math.max(w0, btn.getBoundingClientRect().width);
    if (reduceMotion) {
      btn.style.width = `${w1}px`;
      return;
    }
    btn.classList.remove("spin");
    btn.innerHTML = html(label);
    btn.style.width = `${w0}px`;
    void btn.offsetWidth;
    btn.style.width = `${w1}px`;
    const textEl = () => btn.querySelector<HTMLElement>(".save-text");
    timers.push(
      window.setTimeout(() => textEl()?.classList.add("swap"), 240),
      window.setTimeout(() => {
        const el = textEl();
        if (!el) return;
        el.innerHTML = text;
        el.classList.remove("swap");
      }, 340),
      window.setTimeout(() => btn.classList.add("spin"), 460),
    );
  };
  timers.push(window.setTimeout(show, SAVE_SHOW_AFTER));
  const restore = (() => {
    timers.forEach(clearTimeout);
    if (!btn.isConnected) return;
    btn.classList.remove("saving", "spin");
    btn.style.width = "";
    btn.disabled = false;
    btn.innerHTML = label;
  }) as SaveBusy;
  restore.settle = () => {
    if (!shownAt) {
      // まだ出していない: 出さずに閉じる
      timers.forEach(clearTimeout);
      return Promise.resolve();
    }
    const wait = SAVE_MIN_SHOWN - (performance.now() - shownAt);
    return wait > 0 ? new Promise((r) => setTimeout(r, wait)) : Promise.resolve();
  };
  return restore;
}

/**
 * 1 行の入力。`save` を渡すと、OK で保存が終わるまでダイアログを閉じずにボタンを「保存中…」にする
 * (`save` が false を返したら閉じない)
 */
export function promptText(title: string, value = "", okLabel = "OK", save?: (v: string) => Promise<boolean>): Promise<string | null> {
  return new Promise((resolve) => {
    let result: string | null = null;
    modal(
      `<h3>${esc(title)}</h3><input type="text" value="${esc(value)}" />
       <div class="modal-actions"><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok">${esc(okLabel)}</button></div>`,
      (root, close) => {
        const input = root.querySelector("input")!;
        const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
        input.focus();
        input.select();
        let saving = false;
        const ok = async () => {
          if (saving) return;
          result = input.value.trim() || null;
          if (save && result !== null) {
            saving = true;
            const restore = busySave(okBtn);
            const done = await save(result);
            saving = false;
            if (!done) {
              restore();
              return;
            }
            await restore.settle();
          }
          close();
        };
        input.addEventListener("keydown", (e) => isEnter(e) && ok());
        root.querySelector('[data-x="ok"]')!.addEventListener("click", ok);
        root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
      },
    ).then(() => resolve(result));
  });
}

export function confirmBox(title: string, message: string, okLabel: string, danger = false, backdropClose = true): Promise<boolean> {
  return new Promise((resolve) => {
    let result = false;
    modal(
      `<h3>${esc(title)}</h3><p>${esc(message)}</p>
       <div class="modal-actions"><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn ${danger ? "danger solid" : "primary"}" data-x="ok">${esc(okLabel)}</button></div>`,
      (root, close) => {
        const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
        okBtn.focus();
        okBtn.addEventListener("click", () => {
          result = true;
          close();
        });
        root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
      },
      "",
      backdropClose,
    ).then(() => resolve(result));
  });
}
