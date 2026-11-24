// DOM の小さな道具 (要素の取得・HTML のエスケープ・Enter の判定)


/**
 * 入力欄で「決定」の Enter が押されたか。日本語入力の変換を確定する Enter は除く。
 * Mac の WebKit は確定の Enter を isComposing = false で送ってくるので、keyCode 229 (IME が処理中) も見る
 */
export function isEnter(e: KeyboardEvent): boolean {
  return e.key === "Enter" && !e.isComposing && e.keyCode !== 229;
}

export function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}
