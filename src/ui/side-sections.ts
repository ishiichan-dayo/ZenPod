// サイドバーの塊 (ライブラリ・プレイリスト・PC・ポッドキャスト) の開け閉めと、見出しのドラッグでの並べ替え

import { getStored, setStored } from "../infra/prefs";

// ---------------- サイドバーの塊 (開け閉め・並べ替え) ----------------

function sideSecs(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(".side-scroll > .side-sec")];
}

export function storedList(key: string): string[] {
  try {
    const v = JSON.parse(getStored(key) ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

/** 覚えている並びと開け閉めを当てる (起動時)。知らない塊 (あとの版で増えたもの) は後ろに足す */
function applySideLayout() {
  const parent = document.querySelector<HTMLElement>(".side-scroll")!;
  const secs = sideSecs();
  const byKey = new Map(secs.map((x) => [x.dataset.sec!, x]));
  const order = storedList("sideOrder").filter((k) => byKey.has(k));
  for (const k of [...order, ...secs.map((x) => x.dataset.sec!).filter((k) => !order.includes(k))]) parent.append(byKey.get(k)!);
  const collapsed = new Set(storedList("sideCollapsed"));
  for (const x of secs) setSideCollapsed(x, collapsed.has(x.dataset.sec!));
}

function setSideCollapsed(sec: HTMLElement, on: boolean) {
  sec.classList.toggle("collapsed", on);
  sec.querySelector(".side-head-toggle")?.setAttribute("aria-expanded", String(!on));
}

function toggleSideSec(sec: HTMLElement) {
  setSideCollapsed(sec, !sec.classList.contains("collapsed"));
  setStored("sideCollapsed", JSON.stringify(sideSecs().filter((x) => x.classList.contains("collapsed")).map((x) => x.dataset.sec!)));
}

const secDrag = {
  pending: null as { el: HTMLElement; y: number } | null,
  active: false,
  /** 入れる場所 (塊の並びの中の位置。末尾は塊の数) */
  index: -1,
  /** 並べ替えを終えた時刻 (直後のクリックで開け閉めしないように) */
  endedAt: 0,
};

function onSecMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;
  const t = e.target as HTMLElement;
  const head = t.closest<HTMLElement>(".side-head");
  // 見出しの「＋」などのボタンは押せるままにする (つかめるのは見出しの名前と余白)
  if (!head || (t.closest(".btn") && !t.closest(".side-head-toggle"))) return;
  secDrag.pending = { el: head.closest<HTMLElement>(".side-sec")!, y: e.clientY };
}

function onSecMouseMove(e: MouseEvent) {
  const p = secDrag.pending;
  if (!p) return;
  if (!secDrag.active) {
    if (Math.abs(e.clientY - p.y) < 6) return;
    secDrag.active = true;
    p.el.classList.add("sec-dragging");
    document.body.classList.add("sec-reordering");
  }
  const secs = sideSecs();
  secs.forEach((x) => x.classList.remove("insert-before", "insert-after"));
  const i = secs.findIndex((x) => {
    const r = x.getBoundingClientRect();
    return e.clientY < r.top + r.height / 2;
  });
  secDrag.index = i < 0 ? secs.length : i;
  if (i >= 0) secs[i].classList.add("insert-before");
  else secs[secs.length - 1]?.classList.add("insert-after");
}

function onSecMouseUp() {
  const p = secDrag.pending;
  const active = secDrag.active;
  secDrag.pending = null;
  secDrag.active = false;
  if (!p || !active) return;
  secDrag.endedAt = Date.now();
  document.body.classList.remove("sec-reordering");
  const secs = sideSecs();
  secs.forEach((x) => x.classList.remove("insert-before", "insert-after", "sec-dragging"));
  const from = secs.indexOf(p.el);
  const to = secDrag.index > from ? secDrag.index - 1 : secDrag.index;
  if (from < 0 || to === from) return;
  secs.splice(from, 1);
  secs.splice(to, 0, p.el);
  const parent = document.querySelector<HTMLElement>(".side-scroll")!;
  for (const x of secs) parent.append(x);
  setStored("sideOrder", JSON.stringify(secs.map((x) => x.dataset.sec!)));
}

export function bindSideSections() {
  applySideLayout();
  const side = document.querySelector<HTMLElement>(".side-scroll")!;
  side.addEventListener("mousedown", onSecMouseDown);
  document.addEventListener("mousemove", onSecMouseMove);
  document.addEventListener("mouseup", onSecMouseUp);
  side.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>(".side-head-toggle");
    if (!btn || Date.now() - secDrag.endedAt < 300) return;
    toggleSideSec(btn.closest<HTMLElement>(".side-sec")!);
  });
}
