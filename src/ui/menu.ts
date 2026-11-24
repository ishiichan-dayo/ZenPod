// 右クリックなどのメニュー (サブメニュー付き)

import { $ } from "../app/state";
import { esc } from "./dom";
import { ICON_CHECK } from "./icons";

export interface MenuItem {
  label?: string;
  action?: () => void;
  danger?: boolean;
  disabled?: boolean;
  sep?: boolean;
  header?: string;
  /** マウスを乗せると横に開くメニュー */
  submenu?: MenuItem[];
  /** 右端に薄く出すショートカットキー */
  shortcut?: string;
  /** チェックの付く項目 (true / false)。undefined なら普通の項目 */
  checked?: boolean;
  /** 押してもメニューを閉じない (チェックをいくつも切り替える項目用) */
  keepOpen?: boolean;
}

export function showMenu(x: number, y: number, items: MenuItem[]) {
  closeMenu();
  const m = buildMenu(items, 0);
  $("menu-root").appendChild(m);
  const r = m.getBoundingClientRect();
  m.style.left = `${Math.min(x, innerWidth - r.width - 8)}px`;
  m.style.top = `${Math.min(y, innerHeight - r.height - 8)}px`;
  setTimeout(() => document.addEventListener("mousedown", onMenuOutside, true));
}

function buildMenu(items: MenuItem[], depth: number): HTMLElement {
  const m = document.createElement("div");
  m.className = "menu";
  m.dataset.depth = String(depth);
  for (const it of items) {
    const el = document.createElement("div");
    if (it.sep) el.className = "menu-sep";
    else if (it.header) {
      el.className = "menu-label";
      el.textContent = it.header;
      // 幅は決まっているので、長い曲名などは省略する。全体はマウスを乗せると出す
      el.title = it.header;
    } else {
      el.className = `menu-item${it.danger ? " danger" : ""}${it.disabled ? " disabled" : ""}`;
      const check = it.checked === undefined ? "" : `<span class="menu-check">${it.checked ? ICON_CHECK : ""}</span>`;
      if (it.label) el.title = it.label;
      el.innerHTML = `${check}<span class="menu-text">${esc(it.label ?? "")}</span>${
        it.shortcut ? `<span class="menu-key">${esc(it.shortcut)}</span>` : ""
      }${it.submenu ? '<span class="menu-arrow">›</span>' : ""}`;
      const open = () => {
        if (el.classList.contains("open")) return;
        closeSubmenus(depth);
        m.querySelectorAll(".menu-item.open").forEach((x) => x.classList.remove("open"));
        if (it.submenu?.length) {
          el.classList.add("open");
          openSubmenu(el, it.submenu, depth + 1);
        }
      };
      el.addEventListener("mouseenter", open);
      el.addEventListener("click", () => {
        if (it.submenu) return open();
        if (it.disabled) return;
        if (it.keepOpen) {
          it.action?.();
          if (it.checked !== undefined) {
            it.checked = !it.checked;
            el.querySelector(".menu-check")!.innerHTML = it.checked ? ICON_CHECK : "";
          }
          return;
        }
        closeMenu();
        it.action?.();
      });
    }
    m.appendChild(el);
  }
  return m;
}

/** 親の項目の右 (入らなければ左) に開く */
function openSubmenu(anchor: HTMLElement, items: MenuItem[], depth: number) {
  const sub = buildMenu(items, depth);
  $("menu-root").appendChild(sub);
  const a = anchor.getBoundingClientRect();
  const r = sub.getBoundingClientRect();
  const left = a.right + r.width + 8 > innerWidth ? a.left - r.width : a.right;
  sub.style.left = `${Math.max(8, left)}px`;
  sub.style.top = `${Math.max(8, Math.min(a.top - 5, innerHeight - r.height - 8))}px`;
}

function closeSubmenus(depth: number) {
  document.querySelectorAll<HTMLElement>("#menu-root .menu").forEach((m) => {
    if (Number(m.dataset.depth) > depth) m.remove();
  });
}

function onMenuOutside(e: MouseEvent) {
  // メニューのスクロールバーのつまみはメニューの外の要素なので、そこを押しても閉じない
  if (!(e.target as HTMLElement).closest(".menu, .oscroll-track")) closeMenu();
}

export function closeMenu() {
  $("menu-root").innerHTML = "";
  document.removeEventListener("mousedown", onMenuOutside, true);
}
