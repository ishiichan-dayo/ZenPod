// ジャケットのネット検索と、画像ファイルからの設定

import { t as tr } from "../../i18n";
import { invoke, listen, openDialog } from "../../infra/tauri";
import { analyticsNet, netFunnel, netReach, netSearched } from "../../app/analytics";
import { isRockbox, pcPool } from "../../app/selectors";
import { artSize, artSizeHtml, bindArtSize, setShrinkOnSetPref, shrinkOnSetPref } from "../art-size";
import { rescanCurrentPc } from "../pc";
import { state } from "../../app/state";
import { fmtBytes } from "../../domain/format";
import { albumKey } from "../../domain/track-order";
import type { Snapshot, Track } from "../../domain/types";
import { pcArtCache } from "../artwork-cells";
import { esc, isEnter } from "../dom";
import { call, toast, withArtProgress } from "../feedback";
import { confirmBox, modal } from "../modal";
import { findTrack } from "../player";
import { applySnapshot } from "../render";
import { renderRows } from "../rows";

// ---------------- アートワーク検索 ----------------

export interface ArtCandidate {
  source: string;
  title: string;
  artist: string;
  thumbUrl: string;
  fullUrl: string;
}

export interface SearchResult {
  candidates: ArtCandidate[];
  artistQuery: string;
  albumQuery: string;
  artistAliases: string[];
}

interface ImageProbe {
  width: number;
  height: number;
  bytes: number | null;
}

/** 同じアルバム (アルバム名 + アルバムアーティスト/アーティスト) の曲 */
export function sameAlbumIds(t: Track): string[] {
  if (!t.album) return [t.id];
  // PC の曲は同じフォルダの曲に限る (シングルのタグに誤ってアルバム名が入っていることがある。albumKey にフォルダが入る)
  const k = albumKey(t);
  return (t.pcPath ? pcPool() : state.snap!.tracks).filter((x) => albumKey(x) === k).map((x) => x.id);
}

let searchToken = 0;

/**
 * ジャケットを検索する。何をしているか (別名の確認・各サービスの検索・iTunes の回数の上限で待っている秒数) を
 * `onProgress` に文で渡し、サービスの結果が届くたびに、それまでの候補 (よい順) も渡す (届いたものから並べられるように)。
 * 検索ごとに番号を付けて、ほかの検索 (先に探しているアルバム) の経過と見分ける
 */
export async function searchArtworkOnline(artist: string, album: string, onProgress: (msg: string, found?: ArtCandidate[]) => void): Promise<SearchResult | null> {
  const token = ++searchToken;
  let timer = 0;
  const unlisten = await listen<{ token: number; stage: string; done?: number; total?: number; secs?: number; candidates?: ArtCandidate[] }>("artwork-search", (ev) => {
    const p = ev.payload;
    if (p.token !== token) return;
    clearInterval(timer);
    if (p.stage === "aliases") onProgress(tr("アーティストの別名を確認中…"));
    else if (p.stage === "sources") onProgress(tr("iTunes・Deezer・MusicBrainz を検索中… {a} / {n}", { a: p.done ?? 0, n: p.total ?? 0 }), p.candidates);
    else if (p.stage === "waiting") {
      // 待つ秒数を数え下げる (0 にはしない。待ち終わってから検索の結果が届くまでの間も出ているため)
      let left = p.secs ?? 0;
      const paint = () => onProgress(tr("iTunes の検索の回数の上限 (1 分に 20 回) のため待っています… あと {n} 秒", { n: Math.max(1, left) }));
      paint();
      timer = window.setInterval(() => {
        left--;
        paint();
      }, 1000);
    }
  });
  try {
    return await invoke<SearchResult>("search_artwork", { artist, album, token }).catch(() => null);
  } finally {
    clearInterval(timer);
    unlisten();
  }
}

/** 候補の解像度の印 (高・中・低・不明) */
function resBadge(p: ImageProbe | null): string {
  if (!p) return `<span class="res res-unknown">${esc(tr("解像度不明"))}</span>`;
  const q = Math.min(p.width, p.height);
  const cls = q >= 1000 ? "res-high" : q >= 320 ? "res-mid" : "res-low";
  const size = p.bytes ? ` · ${fmtBytes(p.bytes)}` : "";
  const shape = p.width !== p.height ? tr(" · 正方形に切り抜き") : "";
  const low = q < 320 ? tr(" · 低解像度") : "";
  return `<span class="res ${cls}">${p.width}×${p.height}${size}${shape}${low}</span>`;
}

/** 検索中の候補の場所。本物のカードの形で、画像と文字の場所だけを光らせる (光り方は一覧のスケルトンと同じ) */
export function skeletonCards(n = 8): string {
  const at = `--skel-at:-${Math.round(performance.now() % 1400)}ms`;
  const bar = (w: number, cls = "") => `<span class="sk skel${cls}" style="width:${w}%;${at}"></span>`;
  return Array.from(
    { length: n },
    (_, i) => `<div class="as-skel" aria-hidden="true"><span class="as-skel-img skel" style="${at}"></span>${bar(55, " as-skel-res")}${bar(30)}${bar(50 + ((i * 29) % 40))}${bar(30 + ((i * 41) % 30))}</div>`,
  ).join("");
}

/** 候補の上の行: 検索中は回る印と経過、終わったら検索語・別名の説明 */
export function searchStatus(el: HTMLElement, msg: string, busy: boolean) {
  el.classList.toggle("busy", busy);
  el.innerHTML = `${busy ? '<span class="spinner small"></span>' : ""}<span>${esc(msg)}</span>`;
}

/** 確かめた候補の解像度 (画像の URL → 結果)。届いた候補から並べ直すたびに確かめ直さないよう覚えておく */
const probeCache = new Map<string, ImageProbe | null>();

/**
 * 候補を並べる (1 アルバムずつ探すときと、まとめて探すときで共通)。押すと `onPick`。`selectedUrl` の候補は選んだ状態で出す。
 * 前と同じ並びなら描き直さない (届いた候補から並べ直すので、同じ並びで何度も呼ばれる)。`alive` が false になったら
 * (検索し直した・閉じた) 解像度の確認をやめる。解像度は 4 件ずつ並列に確かめ、終わったら resolve する
 */
export function renderCandidates(
  grid: HTMLElement,
  list: ArtCandidate[],
  alive: () => boolean,
  onPick: (c: ArtCandidate, card: HTMLElement) => void,
  selectedUrl?: string,
  searching = false,
): Promise<void> {
  // まだ検索中なら、届いた候補のあとをスケルトンで埋めて 8 枚分の場所を保つ (届くたびにダイアログの高さが変わらないように)
  const fill = () => {
    grid.querySelectorAll(".as-skel").forEach((x) => x.remove());
    if (searching && list.length < 8) grid.insertAdjacentHTML("beforeend", skeletonCards(8 - list.length));
  };
  const key = list.map((c) => c.fullUrl).join("\n");
  if (grid.dataset.key === key && grid.querySelector(".as-card")) {
    grid.querySelectorAll<HTMLElement>(".as-card").forEach((card) => card.classList.toggle("selected", list[Number(card.dataset.i)].fullUrl === selectedUrl));
    fill();
    return Promise.resolve();
  }
  grid.dataset.key = key;
  const badge = (c: ArtCandidate) => (probeCache.has(c.fullUrl) ? resBadge(probeCache.get(c.fullUrl)!) : `<span class="res res-unknown">${esc(tr("解像度を確認中…"))}</span>`);
  grid.innerHTML = list
    .map(
      (c, i) => `<button class="as-card${c.fullUrl === selectedUrl ? " selected" : ""}" data-i="${i}" title="${esc(c.title)} / ${esc(c.artist)}">
        <img src="${esc(c.thumbUrl)}" alt="" loading="lazy">
        <div class="as-res" data-res="${i}">${badge(c)}</div>
        <div class="as-src">${esc(c.source)}</div>
        <div class="as-title">${esc(c.title)}</div>
        <div class="as-artist">${esc(c.artist)}</div>
      </button>`,
    )
    .join("");
  fill();
  // WebView が外部画像を直接読めない場合はバックエンド経由で取得
  grid.querySelectorAll<HTMLImageElement>(".as-card img").forEach((img, i) =>
    img.addEventListener(
      "error",
      async () => {
        const url = await invoke<string | null>("fetch_image", { url: list[i].thumbUrl }).catch(() => null);
        if (url) img.src = url;
      },
      { once: true },
    ),
  );
  grid.querySelectorAll<HTMLElement>(".as-card").forEach((card) =>
    card.addEventListener("click", () => {
      grid.querySelectorAll(".as-card.selected").forEach((x) => x.classList.remove("selected"));
      card.classList.add("selected");
      onPick(list[Number(card.dataset.i)], card);
    }),
  );
  const todo = list.map((c, i) => [c, i] as const).filter(([c]) => !probeCache.has(c.fullUrl));
  let next = 0;
  const worker = async () => {
    while (next < todo.length && alive()) {
      const [c, i] = todo[next++];
      const p = await invoke<ImageProbe | null>("probe_artwork", { url: c.fullUrl }).catch(() => null);
      probeCache.set(c.fullUrl, p);
      // 並べ直していれば、その候補の今の場所に入れる
      const el = grid.dataset.key === key ? grid.querySelector(`[data-res="${i}"]`) : null;
      if (el && alive()) el.innerHTML = resBadge(p);
    }
  };
  return Promise.all([worker(), worker(), worker(), worker()]).then(() => undefined);
}

/** iPod の動画のアートワークを探すとき: 最初に入れておく検索語と、反映したあとの処理 (動画の一覧を読み直す) */
export interface VideoArtTarget {
  artist: string;
  album: string;
  applied: (s: Snapshot) => Promise<void>;
}

/** `done` は反映したあとに呼ぶ (情報の編集の画面の画像を描き直す) */
/** 曲に設定する画像を、埋め込む前に縮める大きさ (PC の曲と Rockbox の曲。iPod OS は曲のファイルに埋め込まない)。縮めなければ null */
export function shrinkOnSet(ids: string[]): number | null {
  const pc = !!findTrack(ids[0])?.pcPath;
  return (pc || isRockbox()) && shrinkOnSetPref(pc) ? artSize() : null;
}

/** 候補の画面の説明の、画像をどこに入れるかの文 (ネットで探す・アルバムごとに探すで同じ) */
export function artDestNote(pc: boolean, video: boolean): string {
  if (pc) return tr("画像は PC の音楽ファイルに埋め込まれます。");
  if (!video && isRockbox()) return tr("画像は iPod の曲のファイルに埋め込み、アルバムのフォルダに cover.jpg も置きます。");
  return tr("iPod には 320×320 に縮小して転送されます。");
}

/** 設定する画像を縮めるかのチェックと大きさの欄 (PC の曲と Rockbox の曲。ネットで探す・アルバムごとに探す・アートワークのタブで同じ) */
export function shrinkOnSetHtml(pc: boolean, video: boolean, id = "as"): string {
  if (video || (!pc && !isRockbox())) return "";
  return `<div class="as-shrink"><label class="check"><input type="checkbox" id="${id}-shrink" ${shrinkOnSetPref(pc) ? "checked" : ""}> ${esc(
    tr("設定する画像を縮小して埋め込む"),
  )}</label>${artSizeHtml(`${id}-size`)}</div>`;
}

/** `shrinkOnSetHtml` の欄を動かす。`onSize` は大きさを選び直したとき。戻り値は今の大きさを返す関数 (欄が無ければ null) */
export function bindShrinkOnSet(root: HTMLElement, pc: boolean, id = "as", onSize?: (px: number) => void): (() => number) | null {
  root.querySelector<HTMLInputElement>(`#${id}-shrink`)?.addEventListener("change", (e) => setShrinkOnSetPref(pc, (e.currentTarget as HTMLInputElement).checked));
  return root.querySelector(`#${id}-size`) ? bindArtSize(root, `${id}-size`, onSize) : null;
}

export async function searchArtwork(ids: string[], video?: VideoArtTarget, done?: () => void) {
  const first = video ? null : findTrack(ids[0]);
  if (!video && !first) return;
  // PC のフォルダの曲なら、元のファイルに埋め込む
  const pc = !!first?.pcPath;
  const albumIds = first ? sameAlbumIds(first) : ids;
  const widerAlbum = albumIds.length > ids.length;
  let selected: ArtCandidate | null = null;
  let generation = 0;
  const funnel = netFunnel(["searched", "results", "applied"]);


  await modal(
    `<h3>${esc(tr("アートワークをネットで探す"))}</h3>
     <div class="as-bar">
       <input type="text" id="as-artist" placeholder="${esc(tr("アーティスト"))}" value="${esc(video ? video.artist : first!.albumArtist || first!.artist)}">
       <input type="text" id="as-album" placeholder="${esc(tr("アルバム"))}" value="${esc(video ? video.album : first!.album)}">
       <button class="btn" id="as-go">${esc(tr("検索"))}</button>
     </div>
     <div id="as-info" class="as-info"></div>
     <div id="as-grid" class="as-grid"></div>
     <div class="hint">${esc(tr("iTunes・Deezer・MusicBrainz から探します（アーティスト名とアルバム名が各サービスに送信されます）。"))} ${esc(artDestNote(pc, !!video))}</div>
     ${shrinkOnSetHtml(pc, !!video)}
     <div class="modal-actions">
       ${
         widerAlbum
           ? `<label class="check as-scope"><input type="checkbox" id="as-all" checked> ${esc(tr("同じアルバムの {n} 曲すべてに適用", { n: albumIds.length }))}</label>`
           : `<span class="muted as-scope">${esc(video ? tr("{n} 本の動画に適用", { n: ids.length }) : tr("{n} 曲に適用", { n: ids.length }))}</span>`
       }
       <span class="spacer"></span>
       <button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button>
       <button class="btn primary" data-x="ok" disabled>${esc(tr("このアートワークにする"))}</button>
     </div>`,
    (root, close) => {
      const grid = root.querySelector<HTMLElement>("#as-grid")!;
      const infoEl = root.querySelector<HTMLElement>("#as-info")!;
      const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      const artistIn = root.querySelector<HTMLInputElement>("#as-artist")!;
      bindShrinkOnSet(root, pc);
      const albumIn = root.querySelector<HTMLInputElement>("#as-album")!;

      const run = async () => {
        const gen = ++generation;
        selected = null;
        okBtn.disabled = true;
        grid.innerHTML = skeletonCards();
        grid.dataset.key = "";
        searchStatus(infoEl, tr("検索中…"), true);
        const started = performance.now();
        const pick = (c: ArtCandidate) => {
          selected = c;
          okBtn.disabled = false;
        };
        /** 届いた候補を並べる (選んでいた候補が外れたら選び直してもらう) */
        const show = (list: ArtCandidate[], searching = false) => {
          if (selected && !list.some((c) => c.fullUrl === selected!.fullUrl)) {
            selected = null;
            okBtn.disabled = true;
          }
          return renderCandidates(grid, list, () => gen === generation, pick, selected?.fullUrl, searching);
        };
        const res = await searchArtworkOnline(artistIn.value, albumIn.value, (m, found) => {
          if (gen !== generation) return;
          if (found?.length) void show(found, true);
          searchStatus(infoEl, m, true);
        });
        if (gen !== generation) return;
        searchStatus(infoEl, "", false);
        netSearched(funnel, started, res ? res.candidates : null);
        const list = res?.candidates ?? [];
        if (res) {
          const parts: string[] = [];
          if (res.artistQuery !== artistIn.value.trim() || res.albumQuery !== albumIn.value.trim())
            parts.push(tr("検索語: {a} / {b}", { a: res.artistQuery, b: res.albumQuery }));
          if (res.artistAliases.length) parts.push(tr("別名でも検索: {a}", { a: res.artistAliases.join(tr("、")) }));
          infoEl.textContent = parts.join("　");
        }
        if (!list.length) {
          grid.innerHTML = `<div class="as-empty">${esc(tr("見つかりませんでした。アーティスト名やアルバム名を変えて検索してください。"))}</div>`;
          return;
        }
        await show(list);
      };

      root.querySelector("#as-go")!.addEventListener("click", run);
      [artistIn, albumIn].forEach((el) => el.addEventListener("keydown", (e) => isEnter(e) && run()));
      root.querySelector('[data-x="cancel"]')!.addEventListener("click", () => {
        generation++;
        close();
      });
      /** 反映した候補の検索先 ("iTunes (JP)" → itunes) */
      const artApplied = () => {
        netReach(funnel, "applied");
        funnel.source = (selected?.source ?? "").toLowerCase().split(/[^a-z]/)[0];
      };
      okBtn.addEventListener("click", async () => {
        if (!selected) return;
        const all = root.querySelector<HTMLInputElement>("#as-all")?.checked ?? false;
        const target = all ? albumIds : ids;
        okBtn.disabled = true;
        // 反映中は候補を選び直せないようにして、ボタンに経過を出す
        root.classList.add("art-applying");
        const show = (m: string) => (okBtn.innerHTML = `<span class="spinner small"></span>${esc(m)}`);
        const url = selected.fullUrl;
        if (pc) {
          const paths = pcPathsOf(target);
          const n = await withArtProgress(tr("画像をダウンロード中…"), () => call<number>("apply_pc_artwork_url", { paths, url, shrink: shrinkOnSet(target) }), show);
          root.classList.remove("art-applying");
          refreshPcArtwork(paths, true);
          done?.();
          if (n !== null) {
            artApplied();
            toast(tr("{n} 曲のアートワークを設定しました", { n: n.toLocaleString() }));
            generation++;
            close();
            return;
          }
          okBtn.disabled = false;
          okBtn.textContent = tr("このアートワークにする");
          return;
        }
        const s = await withArtProgress(tr("画像をダウンロード中…"), () => call<Snapshot>("apply_artwork_url", { ids: target, url, shrink: video ? null : shrinkOnSet(target) }), show);
        root.classList.remove("art-applying");
        if (s) {
          artApplied();
          if (video) await video.applied(s);
          else applySnapshot(s);
          done?.();
          toast(video ? tr("{n} 本の動画のアートワークを設定しました", { n: target.length }) : tr("{n} 曲のアートワークを設定しました", { n: target.length }));
          generation++;
          close();
        } else {
          okBtn.disabled = false;
          okBtn.textContent = tr("このアートワークにする");
        }
      });
      run();
    },
    "wide art-pick",
  );
  analyticsNet("artwork_search", funnel);
}

export async function pickArtwork(ids: string[], done?: () => void) {
  const f = await openDialog({
    multiple: false,
    title: tr("アートワークにする画像"),
    filters: [{ name: tr("画像"), extensions: ["jpg", "jpeg", "png", "webp", "bmp", "gif"] }],
  });
  if (typeof f !== "string") return;
  if (findTrack(ids[0])?.pcPath) {
    const paths = pcPathsOf(ids);
    const n = await withArtProgress(tr("書き込み中…"), () => call<number>("set_pc_artwork", { paths, path: f, shrink: shrinkOnSet(ids) }));
    refreshPcArtwork(paths, true);
    done?.();
    if (n !== null) toast(tr("{n} 曲のアートワークを設定しました", { n: n.toLocaleString() }));
    return;
  }
  const s = await withArtProgress(tr("書き込み中…"), () => call<Snapshot>("set_artwork", { ids, path: f, shrink: shrinkOnSet(ids) }));
  if (s) {
    applySnapshot(s);
    done?.();
    toast(tr("{n} 曲のアートワークを設定しました", { n: ids.length }));
  }
}

/**
 * iPod の曲のファイルに埋め込まれた画像を縮めた JPEG にする (Rockbox の表示が速くなり、容量も減る)。
 * `size` (長い辺の px) を渡したとき (アートワークのタブで選んである) は確認だけ、無ければ確認の画面で大きさも選ぶ
 */
export async function shrinkArtwork(ids: string[], done?: () => void, size?: number) {
  const pc = !!findTrack(ids[0])?.pcPath;
  const title = tr("{n} 曲の埋め込み画像を縮小しますか？", { n: ids.length.toLocaleString() });
  const note = pc
    ? tr("PC の元のファイルの埋め込み画像を、長い辺がこの大きさまでの JPEG にします。元の画像には戻せません。")
    : tr("iPod の中の曲のファイルの埋め込み画像を、長い辺がこの大きさまでの JPEG にします。PC の元のファイルは変わりません。Rockbox ではアートワークの表示が速くなります。");
  let px: number | null = null;
  if (size) {
    const fixed = pc
      ? tr("PC の元のファイルの埋め込み画像を、長い辺 {a}px までの JPEG にします。元の画像には戻せません。", { a: size })
      : tr("iPod の中の曲のファイルの埋め込み画像を、長い辺 {a}px までの JPEG にします。PC の元のファイルは変わりません。Rockbox ではアートワークの表示が速くなります。", { a: size });
    if (await confirmBox(title, fixed, tr("縮小する"))) px = size;
  } else {
    await modal(
      `<h3>${esc(title)}</h3><p>${esc(note)}</p>
       <div class="shrink-size"><label for="sh-size">${esc(tr("縮小する大きさ (長い辺)"))}</label>${artSizeHtml("sh-size")}</div>
       <div class="modal-actions"><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok">${esc(tr("縮小する"))}</button></div>`,
      (root, close) => {
        const value = bindArtSize(root, "sh-size");
        const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
        okBtn.focus();
        okBtn.addEventListener("click", () => {
          px = value();
          close();
        });
        root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
      },
    );
  }
  if (px === null) return;
  if (pc) {
    const paths = pcPathsOf(ids);
    const n = await withArtProgress(tr("縮小中…"), () => call<number>("shrink_pc_artwork", { paths, size: px }));
    // 一部だけ失敗したときもエラーの表示のあとで読み直す (ファイルの大きさが変わる)
    refreshPcArtwork(paths, true);
    rescanCurrentPc();
    done?.();
    if (n !== null) toast(n ? tr("{n} 曲の埋め込み画像を縮小しました", { n: n.toLocaleString() }) : tr("縮小が必要な埋め込み画像はありませんでした"));
    return;
  }
  const r = await withArtProgress(tr("縮小中…"), () => call<{ snapshot: Snapshot; shrunk: number; failed: number }>("shrink_artwork", { ids, size: px }));
  if (!r) return;
  applySnapshot(r.snapshot);
  done?.();
  if (r.failed) toast(tr("{n} 曲の埋め込み画像を縮小しました。{f} 曲は書き換えられませんでした", { n: r.shrunk.toLocaleString(), f: r.failed.toLocaleString() }), "error");
  else if (r.shrunk) toast(tr("{n} 曲の埋め込み画像を縮小しました", { n: r.shrunk.toLocaleString() }));
  else toast(tr("縮小が必要な埋め込み画像はありませんでした"));
}

export async function removeArtwork(ids: string[], done?: () => void) {
  if (findTrack(ids[0])?.pcPath) {
    const paths = pcPathsOf(ids);
    const n = await withArtProgress(tr("削除中…"), () => call<number>("remove_pc_artwork", { paths }));
    refreshPcArtwork(paths, false);
    done?.();
    // 同じフォルダの cover.jpg などは消さないので、それがあれば一覧にはそちらが出る
    if (n !== null) toast(tr("{n} 曲の埋め込み画像を削除しました", { n: n.toLocaleString() }));
    return;
  }
  const s = await withArtProgress(tr("削除中…"), () => call<Snapshot>("remove_artwork", { ids }));
  if (!s) return;
  applySnapshot(s);
  done?.();
}

export function pcPathsOf(ids: string[]): string[] {
  return ids.map((id) => findTrack(id)?.pcPath).filter((p): p is string => !!p);
}

/** PC の曲のジャケットを変えたあと、一覧の小さい画像を読み直し、アートワークの列を直す (一部が失敗した場合も。
 * 失敗した曲の列は、次にフォルダを読み直したときに戻る) */
export function refreshPcArtwork(paths: string[], has: boolean) {
  for (const p of paths) {
    pcArtCache.delete(p);
    const t = findTrack(`pc:${p}`);
    if (t) t.hasPicture = has;
  }
  renderRows();
}
