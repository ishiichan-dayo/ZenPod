// 切り替えのボタン (曲 / アルバム / アーティストなど) の、選んだところに動く背景

import { reduceMotion } from "./feedback";

/**
 * 切り替え (.seg): 選んだ項目の後ろの背景を 1 つの要素にして、選び直すと滑って動かす。
 * 項目ごとに幅が違うので位置と幅は測る。表示されていない (幅 0) ときは出さず、見えたら置き直す
 */
// 選んだ項目を太字にすると切り替え全体の幅も少し変わり、そのたびにここが呼ばれる。
// 動かしている途中で飛ばさないよう、すでに見えている背景は動かしたまま置き直す
const segResize = new ResizeObserver((entries) =>
  entries.forEach((e) => {
    const seg = e.target as HTMLElement;
    const visible = seg.querySelector<HTMLElement>(":scope > .seg-thumb")?.style.opacity === "1";
    placeSegThumb(seg, visible);
  }),
);

// 描いている途中で測るとそのたびにレイアウトが走るので、次のフレームの頭にまとめて測って置く (測る → 置く の順)
const segQueue = new Map<HTMLElement, boolean>();
export function placeSegThumb(seg: HTMLElement, animate = true) {
  if (!segQueue.size) requestAnimationFrame(flushSegThumbs);
  segQueue.set(seg, (segQueue.get(seg) ?? true) && animate);
}

function flushSegThumbs() {
  const jobs = [...segQueue].filter(([seg]) => seg.isConnected);
  segQueue.clear();
  // 初めての切り替えには背景の要素を作る (最初は動かさない)
  const prepared = jobs.map(([seg, animate]) => {
    let thumb = seg.querySelector<HTMLElement>(":scope > .seg-thumb");
    if (!thumb) {
      thumb = document.createElement("span");
      thumb.className = "seg-thumb";
      seg.prepend(thumb);
      segResize.observe(seg);
      animate = false;
    }
    return { seg, thumb, animate };
  });
  const measured = prepared.map((p) => {
    const item = p.seg.querySelector<HTMLElement>("input:checked")?.closest<HTMLElement>(".seg-item");
    return { ...p, w: item?.offsetWidth ?? 0, x: item?.offsetLeft ?? 0 };
  });
  let jump = false;
  for (const { thumb, animate, w, x } of measured) {
    if (!w) {
      thumb.style.opacity = "0";
      continue;
    }
    if (!animate || reduceMotion) {
      thumb.classList.add("no-anim");
      jump = true;
    }
    thumb.style.opacity = "1";
    thumb.style.width = `${w}px`;
    thumb.style.transform = `translateX(${x - 2}px)`;
  }
  if (!jump) return;
  // 動かさずに置いたものは、置いた位置を確定させてから動かせるように戻す (レイアウトは 1 回だけ)
  void document.body.offsetWidth;
  for (const { thumb } of measured) thumb.classList.remove("no-anim");
}
