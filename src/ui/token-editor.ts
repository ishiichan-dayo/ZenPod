// 部品の欄 (転送先のフォルダの形・ファイル名の変更で、[アーティスト] などを組み合わせる)

import { esc } from "./dom";

// ---------------- 部品の欄 (転送先のフォルダの形・ファイル名の変更) ----------------

/** "[部品]" を含む文字列を、部品の見た目 (押せない印) と区切りの文字で表示する。`labels` に無い "[...]" はそのまま */
export function tokensHtml(v: string, labels: string[]): string {
  let out = "";
  let last = 0;
  for (const m of v.matchAll(/\[([^[\]]+)\]/g)) {
    if (!labels.includes(m[1])) continue;
    out += esc(v.slice(last, m.index)) + `<span class="tok tok-static">${esc(m[1])}</span>`;
    last = m.index! + m[0].length;
  }
  return out + esc(v.slice(last));
}

interface TokenEditor {
  value(): string;
  set(v: string): void;
  /** 欄にカーソルを置く (最後に) */
  focus(): void;
}

/**
 * [アルバム] などの部品を、欄の中の部品として出す入力欄。部品はつかんで並べ替え、欄の外へ出すと外れる。
 * 上の一覧 (`palette`) の部品は欄へドラッグで足す (押すとカーソルの位置に足す)。区切りの文字 (/ や - など) は欄の中で打つ。
 * 値は "[部品の名前]" を含む文字列 (前の入力欄と同じ形)。`labels` は画面の言語の部品の名前。
 * ドラッグは pointer イベントで作る (Tauri はファイルのドロップを受けるので、HTML5 のドラッグ＆ドロップは使わない)
 */
export function tokenEditor(host: HTMLElement, palette: HTMLElement, labels: string[], initial: string, onChange: (v: string) => void): TokenEditor {
  const known = new Set(labels);
  const chipHtml = (label: string) => `<span class="tok" contenteditable="false" data-tok="${esc(label)}">${esc(label)}</span>`;
  const chipNode = (label: string) => {
    const t = document.createElement("template");
    t.innerHTML = chipHtml(label);
    return t.content.firstElementChild as HTMLElement;
  };
  const toHtml = (v: string) => {
    let out = "";
    let last = 0;
    for (const m of v.matchAll(/\[([^[\]]+)\]/g)) {
      if (!known.has(m[1])) continue;
      out += esc(v.slice(last, m.index)) + chipHtml(m[1]);
      last = m.index! + m[0].length;
    }
    return out + esc(v.slice(last));
  };
  // 欄の中身を文字列にする (ドラッグ中の印と、ブラウザが入れる <br> は数えない)
  const serialize = (nodes: NodeListOf<ChildNode> | ChildNode[]): string =>
    [...nodes]
      .map((n) =>
        n.nodeType === Node.TEXT_NODE
          ? (n.textContent ?? "")
          : n instanceof HTMLElement && n.dataset.tok
            ? `[${n.dataset.tok}]`
            : n instanceof HTMLElement && !n.classList.contains("tok-caret")
              ? serialize(n.childNodes)
              : "",
      )
      .join("");
  const value = () => serialize(host.childNodes).replace(/[\r\n]+/g, "");
  const emit = () => onChange(value());
  const set = (v: string) => {
    host.innerHTML = toHtml(v);
  };

  // カーソルの位置を、文字列の中の位置で覚える・戻す (打った "[アルバム]" を部品に直すときに使う)
  const caretOffset = (): number | null => {
    const sel = getSelection();
    if (!sel?.rangeCount || !host.contains(sel.getRangeAt(0).startContainer)) return null;
    const r = sel.getRangeAt(0);
    const pre = document.createRange();
    pre.setStart(host, 0);
    pre.setEnd(r.startContainer, r.startOffset);
    return serialize([...pre.cloneContents().childNodes]).length;
  };
  const setCaret = (offset: number) => {
    const r = document.createRange();
    let acc = 0;
    for (const n of [...host.childNodes]) {
      const len = serialize([n]).length;
      if (offset <= acc + len) {
        if (n.nodeType === Node.TEXT_NODE) r.setStart(n, offset - acc);
        else r.setStartAfter(n);
        r.collapse(true);
        getSelection()?.removeAllRanges();
        getSelection()?.addRange(r);
        return;
      }
      acc += len;
    }
    r.selectNodeContents(host);
    r.collapse(false);
    getSelection()?.removeAllRanges();
    getSelection()?.addRange(r);
  };
  // 最後のカーソルの位置 (一覧の部品を押したときにそこへ足す)
  let lastRange: Range | null = null;
  const remember = () => {
    const sel = getSelection();
    if (sel?.rangeCount && host.contains(sel.getRangeAt(0).startContainer)) lastRange = sel.getRangeAt(0).cloneRange();
  };
  host.addEventListener("keyup", remember);
  host.addEventListener("mouseup", remember);

  // 1 行だけ。改行は入れず、貼り付けは文字だけ (改行は空白にする)
  host.addEventListener("beforeinput", (e) => {
    if (e.inputType === "insertParagraph" || e.inputType === "insertLineBreak") e.preventDefault();
  });
  host.addEventListener("paste", (e) => {
    e.preventDefault();
    const text = (e.clipboardData?.getData("text/plain") ?? "").replace(/[\r\n]+/g, " ");
    document.execCommand("insertText", false, text);
  });
  host.addEventListener("input", () => {
    // 打った・貼った "[アルバム]" は部品にする (日本語の変換中は直さない)
    const plain = [...host.childNodes].some((n) => n.nodeType === Node.TEXT_NODE && [...(n.textContent ?? "").matchAll(/\[([^[\]]+)\]/g)].some((m) => known.has(m[1])));
    if (plain) {
      const off = caretOffset();
      set(value());
      if (off !== null) setCaret(off);
    }
    remember();
    emit();
  });

  // ドロップする位置。部品の上なら、左半分は前・右半分は後ろ
  const rangeAt = (x: number, y: number): Range => {
    const box = host.getBoundingClientRect();
    const cx = Math.min(Math.max(x, box.left + 2), box.right - 2);
    const cy = Math.min(Math.max(y, box.top + 2), box.bottom - 2);
    const r = document.caretRangeFromPoint(cx, cy);
    if (!r || !host.contains(r.startContainer)) {
      const end = document.createRange();
      end.selectNodeContents(host);
      end.collapse(false);
      return end;
    }
    const chip = (r.startContainer instanceof Element ? r.startContainer : r.startContainer.parentElement)?.closest<HTMLElement>(".tok, .tok-caret");
    if (chip && host.contains(chip)) {
      const cr = chip.getBoundingClientRect();
      if (cx > cr.left + cr.width / 2) r.setStartAfter(chip);
      else r.setStartBefore(chip);
      r.collapse(true);
    }
    return r;
  };
  // 押した部品をつかむ。`from` は欄の中の部品 (動かす)、null なら一覧から足す。4px 動かすまではドラッグにしない (押しただけと分ける)
  let suppressClick = false;
  const drag = (e: PointerEvent, label: string, from: HTMLElement | null) => {
    if (e.button !== 0) return;
    e.preventDefault();
    // 欄の中の部品は動かす間は隠すので、ポインターは欄で捕まえる
    const src = from ? host : (e.currentTarget as HTMLElement);
    src.setPointerCapture(e.pointerId);
    const [sx, sy] = [e.clientX, e.clientY];
    let ghost: HTMLElement | null = null;
    const marker = document.createElement("span");
    marker.className = "tok-caret";
    const move = (ev: PointerEvent) => {
      if (!ghost) {
        if (Math.hypot(ev.clientX - sx, ev.clientY - sy) < 4) return;
        ghost = chipNode(label);
        ghost.classList.add("tok-ghost");
        document.body.appendChild(ghost);
        from?.classList.add("tok-moving");
      }
      ghost.style.left = `${ev.clientX}px`;
      ghost.style.top = `${ev.clientY}px`;
      const box = host.getBoundingClientRect();
      const inside = ev.clientX >= box.left - 8 && ev.clientX <= box.right + 8 && ev.clientY >= box.top - 16 && ev.clientY <= box.bottom + 16;
      // 欄の中の部品を外へ出したら、離すと外れる印
      ghost.classList.toggle("tok-remove", !inside && !!from);
      if (!inside) marker.remove();
      else rangeAt(ev.clientX, ev.clientY).insertNode(marker);
    };
    const end = (drop: boolean) => {
      src.removeEventListener("pointermove", move);
      src.removeEventListener("pointerup", up);
      src.removeEventListener("pointercancel", cancel);
      if (!ghost) return;
      suppressClick = true;
      setTimeout(() => (suppressClick = false));
      ghost.remove();
      from?.classList.remove("tok-moving");
      if (drop && marker.isConnected) marker.replaceWith(from ?? chipNode(label));
      else if (drop && from) from.remove();
      marker.remove();
      host.normalize();
      if (drop) emit();
    };
    const up = () => end(true);
    const cancel = () => end(false);
    src.addEventListener("pointermove", move);
    src.addEventListener("pointerup", up);
    src.addEventListener("pointercancel", cancel);
  };
  host.addEventListener("pointerdown", (e) => {
    const chip = (e.target as HTMLElement).closest<HTMLElement>(".tok");
    if (chip && host.contains(chip)) drag(e, chip.dataset.tok!, chip);
  });
  palette.innerHTML = labels.map((l) => `<button type="button" class="tok" data-tok="${esc(l)}">${esc(l)}</button>`).join("");
  palette.querySelectorAll<HTMLElement>(".tok").forEach((b) => {
    b.addEventListener("pointerdown", (e) => drag(e, b.dataset.tok!, null));
    // 押しただけ (と、キーボードで押したとき) は、カーソルの位置 (無ければ最後) に足す
    b.addEventListener("click", () => {
      if (suppressClick) return;
      const sel = getSelection();
      const r = sel?.rangeCount && host.contains(sel.getRangeAt(0).startContainer) ? sel.getRangeAt(0) : lastRange && host.contains(lastRange.startContainer) ? lastRange : null;
      const at = r ?? (() => {
        const x = document.createRange();
        x.selectNodeContents(host);
        x.collapse(false);
        return x;
      })();
      const node = chipNode(b.dataset.tok!);
      at.deleteContents();
      at.insertNode(node);
      const after = document.createRange();
      after.setStartAfter(node);
      after.collapse(true);
      host.focus();
      getSelection()?.removeAllRanges();
      getSelection()?.addRange(after);
      lastRange = after.cloneRange();
      host.normalize();
      emit();
    });
  });
  set(initial);
  const focus = () => {
    host.focus();
    setCaret(Number.MAX_SAFE_INTEGER);
    remember();
  };
  return { value, set, focus };
}
