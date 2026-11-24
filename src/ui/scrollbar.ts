// 自前のスクロールバー (OS のバーは隠して、細いつまみを重ねて描く)


/**
 * スクロールバー: OS のバーは全部隠し (style.css)、スクロールする要素の直後に置いた溝に細いつまみを重ねて描く。
 * OS のバーと違って場所を取らないので、出たり消えたりしても中身の幅が変わらない。
 * スクロールする要素は CSS の overflow: auto / scroll の規則から拾うので、要素ごとに呼ぶ必要はない
 */
const oscrollTracks = new WeakMap<HTMLElement, HTMLElement>();
/** 付けたスクロールする要素 (親の要素のアニメーションの間はつまみを隠すのに使う) */
const oscrollLive = new Set<HTMLElement>();
/** つまみの溝の幅 (この中に 6px 幅で描く) */
const OSCROLL_W = 8;

/** CSS の規則のうち、overflow で自分がスクロールするもののセレクタをまとめる */
function scrollerSelector(): string {
  const sels: string[] = [];
  const walk = (rules: CSSRuleList) => {
    for (const r of rules) {
      if (r instanceof CSSStyleRule) {
        const st = r.style;
        if (/auto|scroll/.test(`${st.overflow} ${st.overflowX} ${st.overflowY}`)) sels.push(r.selectorText);
      } else if ("cssRules" in r) walk((r as CSSGroupingRule).cssRules);
    }
  };
  for (const sheet of document.styleSheets) {
    try {
      walk(sheet.cssRules);
    } catch {
      // 読めないスタイルシート (別のオリジン) は飛ばす
    }
  }
  return sels.join(", ");
}

export function initScrollbars() {
  const sel = scrollerSelector();
  if (!sel) return;
  const scan = (root: Element) => {
    if (root.matches(sel)) attachScrollbar(root as HTMLElement);
    for (const el of root.querySelectorAll<HTMLElement>(sel)) attachScrollbar(el);
  };
  scan(document.body);
  // あとから出る要素 (ダイアログ・メニュー・設定の中身) にも付ける。外された要素の溝は片付ける
  new MutationObserver((records) => {
    for (const rec of records) {
      for (const n of rec.removedNodes) {
        if (n instanceof HTMLElement && oscrollTracks.has(n) && !n.isConnected) {
          oscrollTracks.get(n)!.remove();
          oscrollLive.delete(n);
        }
      }
      for (const n of rec.addedNodes) if (n instanceof HTMLElement && !n.classList.contains("oscroll-track")) scan(n);
    }
  }).observe(document.body, { childList: true, subtree: true });
  // 親が transform のアニメーションで動いている間は位置が合わないので隠し、終わったら置き直す
  const anim = (on: boolean) => (e: AnimationEvent) => {
    const t = e.target as Element;
    for (const s of oscrollLive) {
      if (!s.isConnected) {
        oscrollLive.delete(s);
        continue;
      }
      if (!t.contains(s)) continue;
      const track = oscrollTracks.get(s)!;
      track.classList.toggle("oscroll-anim", on);
      if (!on) track.dispatchEvent(new Event("oscroll-place"));
    }
  };
  document.addEventListener("animationstart", anim(true), true);
  document.addEventListener("animationend", anim(false), true);
  document.addEventListener("animationcancel", anim(false), true);
  // 窓の大きさが変わると、大きさは同じまま位置だけ動く要素 (中央に置いたダイアログなど) がある。
  // その要素の大きさの見張りでは気づけないので、すべて置き直す (次のフレームでまとめて測る)
  window.addEventListener("resize", () => {
    for (const s of oscrollLive) {
      if (!s.isConnected) {
        oscrollLive.delete(s);
        continue;
      }
      oscrollTracks.get(s)!.dispatchEvent(new Event("oscroll-place"));
    }
  });
}

function attachScrollbar(scroller: HTMLElement) {
  const known = oscrollTracks.get(scroller);
  if (known) {
    // 別の場所へ移されたときは溝もついていく
    if (scroller.nextElementSibling !== known) scroller.after(known);
    oscrollLive.add(scroller);
    return;
  }
  const cs = getComputedStyle(scroller);
  // 規則が上書きされてスクロールしない要素 (overflow: hidden のサイドバーなど) は対象外
  const scrollY = /auto|scroll/.test(cs.overflowY);
  const scrollX = /auto|scroll/.test(cs.overflowX);
  if (!scrollY && !scrollX) return;
  // 中にスクロールする要素があるとき、その溝がこの要素と一緒にスクロールするよう、位置の基準にする
  const fixed = cs.position === "fixed";
  if (cs.position === "static") scroller.style.position = "relative";
  // 角丸の箱 (ダイアログ・メニュー) は、つまみの端が角からはみ出さないよう内側に寄せる
  const inset = Math.max(2, Math.ceil((parseFloat(cs.borderTopRightRadius) || 0) / 2));
  const z = parseInt(cs.zIndex, 10);

  const track = document.createElement("div");
  track.className = "oscroll-track";
  track.style.position = fixed ? "fixed" : "absolute";
  track.style.zIndex = String(Number.isNaN(z) ? 2 : Math.max(2, z));
  const bar = (axis: "y" | "x") => {
    const el = document.createElement("div");
    el.className = `oscroll-bar oscroll-${axis} hidden`;
    const thumb = document.createElement("div");
    thumb.className = "oscroll-thumb";
    el.append(thumb);
    track.append(el);
    return { el, thumb };
  };
  const y = scrollY ? bar("y") : null;
  const x = scrollX ? bar("x") : null;
  oscrollTracks.set(scroller, track);
  oscrollLive.add(scroller);
  scroller.after(track);

  // 大きさはまとめて測り、スクロールしたときは覚えた大きさでつまみだけ動かす (毎回レイアウトを測らない)
  let m = { ch: 0, sh: 0, cw: 0, sw: 0, hy: 0, hx: 0 };
  const move = () => {
    const top = scroller.scrollTop;
    const left = scroller.scrollLeft;
    if (y && m.sh > m.ch) {
      const room = y.el.clientHeight - m.hy;
      y.thumb.style.transform = `translateY(${Math.round((room * top) / (m.sh - m.ch))}px)`;
    }
    if (x && m.sw > m.cw) {
      const room = x.el.clientWidth - m.hx;
      x.thumb.style.transform = `translateX(${Math.round((room * left) / (m.sw - m.cw))}px)`;
    }
  };
  let queued = false;
  const place = () => {
    queued = false;
    const { clientHeight: ch, scrollHeight: sh, clientWidth: cw, scrollWidth: sw } = scroller;
    const showY = !!y && sh > ch + 1 && ch > 0;
    const showX = !!x && sw > cw + 1 && cw > 0;
    y?.el.classList.toggle("hidden", !showY);
    x?.el.classList.toggle("hidden", !showX);
    if (!showY && !showX) return;
    const left = scroller.offsetLeft + scroller.clientLeft;
    const top = scroller.offsetTop + scroller.clientTop;
    // 縦と横の両方が出るときは、角で重ならないよう互いに短くする
    const hLen = ch - inset * 2 - (showX ? OSCROLL_W : 0);
    const wLen = cw - inset * 2 - (showY ? OSCROLL_W : 0);
    m = { ch, sh, cw, sw, hy: 0, hx: 0 };
    if (y && showY) {
      m.hy = Math.max(32, Math.round((hLen * ch) / sh));
      Object.assign(y.el.style, { left: `${left + cw - OSCROLL_W}px`, top: `${top + inset}px`, height: `${hLen}px` });
      y.thumb.style.height = `${m.hy}px`;
    }
    if (x && showX) {
      m.hx = Math.max(32, Math.round((wLen * cw) / sw));
      Object.assign(x.el.style, { left: `${left + inset}px`, top: `${top + ch - OSCROLL_W}px`, width: `${wLen}px` });
      x.thumb.style.width = `${m.hx}px`;
    }
    move();
  };
  const queue = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(place);
  };
  scroller.addEventListener("scroll", move, { passive: true });
  track.addEventListener("oscroll-place", queue);
  // 中身の大きさが変わったとき: 直下の子の大きさを見る (子が入れ替わったら見直す)
  const ro = new ResizeObserver(queue);
  const watch = () => {
    ro.disconnect();
    ro.observe(scroller);
    for (const c of scroller.children) ro.observe(c);
    queue();
  };
  new MutationObserver(watch).observe(scroller, { childList: true });
  watch();

  // つまみのドラッグ。動かした量を中身の量に引き伸ばす
  const drag = (b: { el: HTMLElement; thumb: HTMLElement }, vertical: boolean) => {
    b.thumb.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      const p0 = vertical ? e.clientY : e.clientX;
      const s0 = vertical ? scroller.scrollTop : scroller.scrollLeft;
      const ratio = vertical
        ? (m.sh - m.ch) / Math.max(1, b.el.clientHeight - m.hy)
        : (m.sw - m.cw) / Math.max(1, b.el.clientWidth - m.hx);
      b.thumb.setPointerCapture(e.pointerId);
      track.classList.add("dragging");
      const onMove = (ev: PointerEvent) => {
        const v = s0 + ((vertical ? ev.clientY : ev.clientX) - p0) * ratio;
        if (vertical) scroller.scrollTop = v;
        else scroller.scrollLeft = v;
      };
      const up = () => {
        b.thumb.removeEventListener("pointermove", onMove);
        b.thumb.removeEventListener("pointerup", up);
        b.thumb.removeEventListener("pointercancel", up);
        track.classList.remove("dragging");
      };
      b.thumb.addEventListener("pointermove", onMove);
      b.thumb.addEventListener("pointerup", up);
      b.thumb.addEventListener("pointercancel", up);
    });
  };
  if (y) drag(y, true);
  if (x) drag(x, false);
  // つまみはスクロールする要素の外にあるので、つまみの上でのホイールは渡す
  track.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const k = e.deltaMode === 1 ? 16 : 1;
      scroller.scrollBy({ left: e.deltaX * k, top: e.deltaY * k });
    },
    { passive: false },
  );
}
