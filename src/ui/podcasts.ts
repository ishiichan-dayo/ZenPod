// ポッドキャスト: 購読の一覧の読み込み・番組の画面・回の画像・選択・メニュー・登録

import { lang, t as tr } from "../i18n";
import { invoke } from "../infra/tauri";
import { currentFeed, epOnIpod, epPlayed, isPodView, newEpisodeCount, pod } from "../app/podcasts";
import { writable } from "../app/selectors";
import { $, state } from "../app/state";
import { POD_DEFAULT_KEEP, POD_KEEP_CHOICES, POD_META_VERSION, epArtKey, epDate, epVideo, fmtEpDur } from "../domain/podcast";
import type { PodEpisode, PodFeed } from "../domain/podcast";
import { setStored } from "../infra/prefs";
import { dataUrlToObjectUrl, skelStyle } from "./album-grid";
import { esc, isEnter } from "./dom";
import { call, callOk, failToast, reduceMotion, replayClass, toast } from "./feedback";
import { ICON_POD } from "./icons";
import { showMenu } from "./menu";
import type { MenuItem } from "./menu";
import { busySave, confirmBox, modal } from "./modal";
import { CHECK_MARK } from "./player";
import { sendEpisodes, syncPodcasts } from "./podcast-sync";
import { removeSelected } from "./selection-actions";
import { ICON_IPOD } from "./settings";
import { storedList } from "./side-sections";
import { renderSidebar, selectView } from "./sidebar";

/** 再生済みの回の印 (Lucide の circle-check アイコン、ISC ライセンス) */
const ICON_PLAYED = '<svg class="icon-line" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/></svg>';

/** 起動して最初に購読の一覧を読み終えるまで (iPod をつないだときの確認はこれを待つ) */
export let podFirstLoad: Promise<void> | null = null;

/** 購読の一覧を読み始める (起動時に 1 回) */
export function startPodcastsLoad(): Promise<void> {
  podFirstLoad = loadPodcasts();
  return podFirstLoad;
}

export async function loadPodcasts() {
  const list = await call<PodFeed[]>("podcast_list");
  if (!list) return;
  pod.feeds = list;
  for (const f of list) if (!pod.covers.has(f.id)) loadPodCover(f.id);
  markPlayedFromIpod();
  if (isPodView() && !currentFeed()) selectView("library");
  else renderPodAll();
}

/** 番組の画像を読み込み中 (読み終わるまでスケルトンを出す) */
const podCoverLoading = new Set<string>();

async function loadPodCover(id: string) {
  if (pod.covers.has(id) || podCoverLoading.has(id)) return;
  podCoverLoading.add(id);
  const c = await invoke<{ small: string; large: string } | null>("podcast_cover", { id }).catch(() => null);
  podCoverLoading.delete(id);
  pod.covers.set(id, c ? { small: dataUrlToObjectUrl(c.small), large: dataUrlToObjectUrl(c.large) } : null);
  renderPodAll();
}

/** サイドバーと、番組を表示中ならその一覧を描き直す */
export function renderPodAll() {
  renderSidebar();
  if (isPodView() && !state.settingsOpen) renderPodView();
}

/** iPod で再生した回に「聞いた」の印を付けて覚える (iPod から消したあとも、同期で送り直さないように) */
export function markPlayedFromIpod() {
  if (!state.snap || state.partialOpen) return;
  const items: { feedId: string; epId: string; plays: number }[] = [];
  for (const f of pod.feeds)
    for (const e of f.episodes) {
      if (e.played) continue;
      const plays = epOnIpod(f, e)?.playCount ?? 0;
      if (plays > (e.playsSeen ?? 0)) {
        e.played = true;
        e.playsSeen = plays;
        items.push({ feedId: f.id, epId: e.id, plays });
      }
    }
  if (items.length) invoke("podcast_mark", { items, played: true }).catch(() => undefined);
}

export function renderPodSidebar(): string {
  const items = pod.feeds
    .map((f) => {
      const n = newEpisodeCount(f);
      const icon = ICON_POD;
      const spin = pod.refreshing.has(f.id) ? `<span class="spinner small"></span>` : "";
      return `<li class="side-item ${state.view === `pod:${f.id}` ? "active" : ""}" data-view="pod:${f.id}" data-pod="${f.id}" title="${esc(f.title)}">${icon}<span class="label">${esc(f.title || f.url)}</span>${spin}<span class="count">${n || ""}</span></li>`;
    })
    .join("");
  return items || `<li class="side-item new-pl" data-addpod="1">${ICON_POD}<span class="label">${tr("番組を登録…")}</span></li>`;
}

function subscribePodcast() {
  modal(
    `<h3>${esc(tr("番組を登録"))}</h3>
     <p>${esc(tr("番組のフィード (RSS) の URL を入力してください。番組のサイトや、ポッドキャストのアプリの「RSS をコピー」などで分かります。"))}</p>
     <input type="text" id="pod-url" placeholder="https://example.com/feed.xml" autocomplete="off" spellcheck="false" />
     <div class="modal-actions"><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok">${esc(tr("登録"))}</button></div>`,
    (root, close) => {
      const input = root.querySelector<HTMLInputElement>("#pod-url")!;
      const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      input.focus();
      let saving = false;
      const ok = async () => {
        const url = input.value.trim();
        if (saving || !url) return;
        saving = true;
        const restore = busySave(okBtn);
        const id = await call<string>("podcast_subscribe", { url });
        saving = false;
        if (!id) {
          restore();
          input.focus();
          return;
        }
        await restore.settle();
        close();
        pod.covers.delete(id);
        await loadPodcasts();
        selectView(`pod:${id}`);
      };
      input.addEventListener("keydown", (e) => isEnter(e) && ok());
      okBtn.addEventListener("click", ok);
      root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
    },
  );
}

async function unsubscribePodcast(f: PodFeed) {
  const ok = await confirmBox(
    tr("「{a}」の登録を解除しますか？", { a: f.title }),
    tr("取得したファイルも削除します。iPod にある回はそのまま残ります。"),
    tr("登録を解除"),
    true,
  );
  if (!ok) return;
  if (!(await callOk("podcast_unsubscribe", { id: f.id }))) return;
  pod.covers.delete(f.id);
  if (state.view === `pod:${f.id}`) selectView("library");
  await loadPodcasts();
}

/** フィードの確かめ直しは 1 つずつ順に (起動時とつないだときが重なっても、同じ番組を 2 回取りに行かない) */
let podRefreshQueue: Promise<void> = Promise.resolve();

/** フィードを確かめ直す (ids が無ければ全部)。quiet なら新しい回の数を知らせない (起動時)。
 * minAge (秒) を渡すと、前回からそれだけたった番組だけにする */
export function refreshPodcasts(ids?: string[], quiet = false, minAge?: number): Promise<void> {
  const run = podRefreshQueue.then(() => doRefreshPodcasts(ids, quiet, minAge));
  podRefreshQueue = run.catch(() => undefined);
  return run;
}

async function doRefreshPodcasts(ids?: string[], quiet = false, minAge?: number) {
  const nowSec = Date.now() / 1000;
  const targets = (ids ?? pod.feeds.map((f) => f.id)).filter((id) => {
    const f = pod.feeds.find((x) => x.id === id);
    // 保存の形が古い番組 (回の画像・説明の全文がまだ無い) は、時間に関係なく取り直す
    return !!f && (minAge === undefined || nowSec - f.checked >= minAge || (f.metaVersion ?? 0) < POD_META_VERSION);
  });
  if (!targets.length) return;
  targets.forEach((id) => pod.refreshing.add(id));
  renderPodAll();
  const r = await invoke<{ id: string; title: string; added: number; error: string | null }[]>("podcast_refresh", { ids: targets, minAge: minAge ?? null }).catch(
    (e) => {
      if (!quiet) failToast(e, 6000);
      return null;
    },
  );
  targets.forEach((id) => pod.refreshing.delete(id));
  await loadPodcasts();
  if (!r || quiet) return;
  const errors = r.filter((x) => x.error);
  const added = r.reduce((n, x) => n + x.added, 0);
  if (errors.length) toast(errors.map((x) => `${x.title}: ${x.error}`).join("\n"), "error", 8000);
  else toast(added ? tr("新しい回が {n} 件あります", { n: added }) : tr("新しい回はありません"));
}

async function markEpisodes(f: PodFeed, eps: PodEpisode[], played: boolean) {
  if (!eps.length) return;
  // 今の iPod の再生回数も覚える (未再生に戻した回が、前の再生ですぐ再生済みに戻らないように)
  const items = eps.map((e) => ({ feedId: f.id, epId: e.id, plays: epOnIpod(f, e)?.playCount ?? e.playsSeen ?? 0 }));
  if (!(await callOk("podcast_mark", { items, played }))) return;
  eps.forEach((e, i) => {
    e.played = played;
    e.playsSeen = items[i].plays;
  });
  renderPodAll();
}

async function deleteEpisodeFiles(f: PodFeed, eps: PodEpisode[]) {
  const list = eps.filter((e) => e.file);
  if (!list.length) return;
  if (!(await callOk("podcast_delete_files", { items: list.map((e) => ({ feedId: f.id, epId: e.id })) }))) return;
  await loadPodcasts();
}

async function removeEpisodesFromIpod(f: PodFeed, eps: PodEpisode[]) {
  const ids = eps.map((e) => epOnIpod(f, e)?.id).filter((id): id is string => !!id);
  if (!ids.length || !writable()) return;
  await removeSelected(ids);
}

/** 回を送るボタン (Lucide の download アイコン、ISC ライセンス) */
const ICON_EP_SEND =
  '<svg class="icon-line" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 15V3"/><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/></svg>';
/** 動画の回の印 (Material の videocam) */
const ICON_VIDEO = '<svg viewBox="0 0 24 24"><path d="M17 10.5V7a1 1 0 0 0-1-1H4a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-3.5l4 4v-11l-4 4Z"/></svg>';

/** 回の今の状態 (一覧の右端のアイコン 1 つ): iPod にある印・送るボタン・取得の進み具合・転送中・動画 */
function epStatusHtml(f: PodFeed, e: PodEpisode): string {
  // 送るボタンを押したら iPod のアイコンにして、周りの輪で進み具合を出す:
  // 取得を待つ間は薄い輪だけ → 取得中は輪が伸びる → 転送を待つ間・転送中は輪の一部が回る → 終わったら緑の iPod
  // (iPod が無いときの「取得」は、iPod ではなくダウンロードのアイコン)
  const inner = state.snap ? ICON_IPOD : ICON_EP_SEND;
  const busy = (label: string, ring: string) =>
    `<span class="pod-state busy" title="${esc(label)}" aria-label="${esc(label)}">${ring}<span class="pod-state-icon">${inner}</span></span>`;
  const c = 2 * Math.PI * 14;
  const dl = pod.downloading.get(e.id);
  if (dl !== undefined) {
    if (dl < 0) return busy(tr("取得待ち"), `<svg class="pod-ring" viewBox="0 0 32 32"><circle cx="16" cy="16" r="14" class="track"/></svg>`);
    return busy(
      tr("取得中 {n}%", { n: Math.floor(dl * 100) }),
      `<svg class="pod-ring" viewBox="0 0 32 32"><circle cx="16" cy="16" r="14" class="track"/><circle cx="16" cy="16" r="14" class="fill" stroke-dasharray="${c.toFixed(2)}" stroke-dashoffset="${(c * (1 - dl)).toFixed(2)}"/></svg>`,
    );
  }
  const path = [...pod.paths].find(([, id]) => id === e.id)?.[0];
  const sending = path ? state.sending.get(path) : undefined;
  // 転送の 1 回の中の進み具合は分からないので、回る輪にする (転送を待っている間も)
  if (path && state.transferring && sending !== "done")
    return busy(
      sending === "working" ? tr("転送中") : tr("転送待ち"),
      `<svg class="pod-ring spin" viewBox="0 0 32 32"><circle cx="16" cy="16" r="14" class="track"/><circle cx="16" cy="16" r="14" class="fill" stroke-dasharray="${(c * 0.28).toFixed(2)} ${c.toFixed(2)}"/></svg>`,
    );
  if (epOnIpod(f, e) || sending === "done") {
    // 送り終えた直後だけ、ポンと出す
    const pop = pod.arrived.has(e.id) ? " pop" : "";
    return `<span class="pod-state ok${pop}" title="${esc(tr("iPod にあります"))}" aria-label="${esc(tr("iPod にあります"))}">${ICON_IPOD}</span>`;
  }
  if (epVideo(e)) return `<span class="pod-state muted" title="${esc(tr("動画の回には対応していません"))}" aria-label="${esc(tr("動画"))}">${ICON_VIDEO}</span>`;
  // 送るボタン (iPod が無ければ取得だけ)。取得済みかは説明に出す
  const label = state.snap ? tr("iPod に送る") : tr("取得");
  const title = e.file ? `${label} (${tr("取得済み")})` : label;
  return `<button class="btn icon ghost pod-state-btn" data-send="1" title="${esc(title)}" aria-label="${esc(title)}">${ICON_EP_SEND}</button>`;
}

/** 古い順で見る番組 (番組ごとに覚える。続きものの番組だけ古い順で聞く人がいるため)。ほかは新しい順 */
function podOldestFirst(f: PodFeed): boolean {
  return storedList("podOldestFirst").includes(f.id);
}

function setPodOldestFirst(f: PodFeed, on: boolean) {
  const ids = storedList("podOldestFirst").filter((id) => id !== f.id && pod.feeds.some((x) => x.id === id));
  if (on) ids.push(f.id);
  setStored("podOldestFirst", JSON.stringify(ids));
  renderPodView();
  $("pod-eps").scrollTop = 0;
}

function openPodSortMenu(anchor: HTMLElement, f: PodFeed) {
  const r = anchor.getBoundingClientRect();
  const oldest = podOldestFirst(f);
  showMenu(r.left, r.bottom + 4, [
    { label: tr("新しい順"), checked: !oldest, action: () => setPodOldestFirst(f, false) },
    { label: tr("古い順"), checked: oldest, action: () => setPodOldestFirst(f, true) },
  ]);
}

/** 一覧に出す回 (検索で絞り、番組の並べ方で並べる。同期で選ぶ回は並べ方に関係なく新しい回から) */
function podEpisodesShown(f: PodFeed): PodEpisode[] {
  const q = state.search.trim().toLowerCase();
  const terms = q ? q.split(/\s+/) : [];
  const list = terms.length
    ? f.episodes.filter((e) => {
        const hay = `${e.title} ${e.description}`.toLowerCase();
        return terms.every((w) => hay.includes(w));
      })
    : f.episodes;
  return podOldestFirst(f) ? [...list].sort((a, b) => a.published - b.published) : list;
}

export function renderPodView() {
  const f = currentFeed();
  const view = $("pod-view");
  view.classList.toggle("hidden", !f);
  for (const id of ["list-area", "album-grid", "connect-view", "view-switch", "selbar"]) $(id).classList.add("hidden");
  if (!f) return;
  // 番組のタイトルは見出しの欄に出すので、上の段は「ポッドキャスト」
  $("view-title").textContent = tr("@side:ポッドキャスト");
  $("view-count").textContent = tr("{n} 回", { n: f.episodes.length });
  $("view-stats").textContent = "";
  for (const id of ["btn-add", "btn-edit-rules", "btn-reorder", "btn-album-back", "album-head-art"]) $(id).classList.add("hidden");
  document.querySelector(".view-head")!.classList.remove("album-open");
  const cover = pod.covers.get(f.id);
  // 「iPod に [新しい / 古い / 送らない] 方から [3 回] を置く」と、文の中に 2 つのセレクトを置く (送らないときは数を出さない)
  const dir = f.keep ? (f.keepOldest ? "old" : "new") : "off";
  const opt = (v: string, cur: string, label: string) => `<option value="${v}" ${v === cur ? "selected" : ""}>${esc(label)}</option>`;
  const dirSel = `<select id="pod-keep-dir" aria-label="${esc(tr("iPod に置く回"))}">${opt("new", dir, tr("@keep:新しい"))}${opt("old", dir, tr("@keep:古い"))}${opt("off", dir, tr("@keep:送らない"))}</select>`;
  const nSel = `<select id="pod-keep-n" aria-label="${esc(tr("iPod に置く回の数"))}">${POD_KEEP_CHOICES.map((n) => opt(String(n), String(f.keep), tr("{n} 回", { n }))).join("")}</select>`;
  const keepHtml = f.keep
    ? esc(tr("iPod に {dir} 方から {n} を置く")).replace("{dir}", dirSel).replace("{n}", nSel)
    : esc(tr("iPod に {dir}")).replace("{dir}", dirSel);
  const refreshing = pod.refreshing.has(f.id);
  // 画像 | 1 段目: 番組のタイトルとボタン (同期 (iPod があるときだけ)・更新・メニュー、高さの真ん中をそろえる) / 2 段目: 紹介文 / 3 段目: iPod に置く回。
  // 作者は番組の情報の画面に出す
  $("pod-head").innerHTML = `
    ${
      cover && !state.devSkeleton
        ? `<img class="pod-cover" src="${cover.large}" alt="" />`
        : podCoverLoading.has(f.id) || state.devSkeleton
          ? `<div class="pod-cover skel"${skelStyle()}></div>`
          : `<div class="pod-cover pod-cover-none">${ICON_POD}</div>`
    }
    <div class="pod-meta">
      <div class="pod-meta-top">
        <div class="pod-title" title="${esc(f.title || f.url)}">${esc(f.title || f.url)}</div>
        <span class="pod-btns">
          ${state.snap ? `<button class="btn" id="pod-sync" title="${esc(tr("この番組だけを同期"))}" ${pod.syncing || state.transferring ? "disabled" : ""}>${esc(tr("同期"))}</button>` : ""}
          <button class="btn" id="pod-refresh" ${refreshing ? "disabled" : ""}>${refreshing ? `<span class="spinner small"></span>` : ""}${esc(tr("更新"))}</button>
          <button class="btn icon ghost" id="pod-more" title="${esc(tr("メニュー"))}" aria-label="${esc(tr("メニュー"))}"><svg viewBox="0 0 24 24"><path d="M6 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4Z"/></svg></button>
        </span>
      </div>
      ${f.description ? `<div class="pod-desc muted" title="${esc(f.description)}">${esc(f.description)}</div>` : ""}
      <div class="pod-actions">
        <div class="pod-keep" title="${esc(tr("同期のとき、まだ聞いていない回を新しい方 (または古い方) からこの数だけ iPod に置き、聞き終わった回を消します"))}">${keepHtml}</div>
      </div>
    </div>`;
  // 並べ替えは一覧の上の段 (曲の一覧の並べ替えと同じ置き方・見た目)
  $("pod-listbar").innerHTML = `<button class="pod-chk pod-selall" id="pod-selall" title="${esc(tr("表示中の回をすべて選択 / 解除"))}" aria-label="${esc(tr("表示中の回をすべて選択 / 解除"))}"><span class="box"></span></button><button class="btn sort-btn" id="pod-sort" aria-haspopup="menu" title="${esc(tr("並べ替え"))}"><svg class="icon-line" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 6h18"/><path d="M3 12h12"/><path d="M3 18h6"/></svg><span>${esc(podOldestFirst(f) ? tr("古い順") : tr("新しい順"))}</span></button>`;
  for (const sel of $("pod-head").querySelectorAll<HTMLSelectElement>(".pod-keep select")) fitSelectWidth(sel);
  const eps = podEpisodesShown(f);
  for (const id of [...pod.selected]) if (!f.episodes.some((e) => e.id === id)) pod.selected.delete(id);
  $("pod-eps").innerHTML = eps.length
    ? eps
        .map((e) => {
          const played = epPlayed(f, e);
          const sub = [epDate(e), e.durationMs ? fmtEpDur(e.durationMs) : ""].filter(Boolean).join(" · ");
          return `<div class="pod-ep ${played ? "played" : ""} ${pod.selected.has(e.id) ? "sel" : ""}" data-ep="${e.id}">
            <button class="pod-chk" aria-pressed="${pod.selected.has(e.id)}" aria-label="${esc(tr("選択"))}"><span class="box">${CHECK_MARK}</span></button>
            <button class="pod-played" data-played="${played ? 1 : 0}" title="${esc(played ? tr("再生済み (クリックで未再生にする)") : tr("未再生 (クリックで再生済みにする)"))}" aria-label="${esc(played ? tr("未再生にする") : tr("再生済みにする"))}">${played ? ICON_PLAYED : `<span class="pod-dot"></span>`}</button>
            ${epArtHtml(f, e)}
            <div class="pod-ep-main">
              <div class="pod-ep-title">${esc(e.title)}</div>
              <div class="pod-ep-sub muted">${esc(sub)}${e.description ? `<span class="pod-ep-desc"> — ${esc(e.description)}</span>` : ""}</div>
            </div>
            <span class="pod-ep-status">${epStatusHtml(f, e)}</span>
          </div>`;
        })
        .join("")
    : `<div class="pod-empty muted">${esc(state.search ? tr("一致する回がありません") : tr("回がありません"))}</div>`;
  observeEpArt();
  renderPodSelbar();
}

/** セレクトの幅を、選んでいる項目の文字に合わせる (ふつうはいちばん長い選択肢の幅になり、文の中で間延びするため) */
function fitSelectWidth(sel: HTMLSelectElement) {
  const cs = getComputedStyle(sel);
  const probe = document.createElement("span");
  probe.style.cssText = `position:absolute;visibility:hidden;white-space:pre;font:${cs.font};letter-spacing:${cs.letterSpacing}`;
  probe.textContent = sel.selectedOptions[0]?.textContent ?? "";
  document.body.append(probe);
  const text = probe.getBoundingClientRect().width;
  probe.remove();
  const pad = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight) + parseFloat(cs.borderLeftWidth) + parseFloat(cs.borderRightWidth);
  sel.style.width = `${Math.ceil(text + pad)}px`;
}

// ---- 回の画像 (一覧の行のサムネイル) ----

/** 回の画像 (番組 ID + URL → object URL。無かった画像は null) */
const epArtCache = new Map<string, string | null>();
/** 読み込みを待っている画像 (見えている行のものだけを、1 枚ずつ順に取る。配信元へ一度に問い合わせないため) */
const epArtQueue: string[] = [];
let epArtBusy = false;
let epArtObserver: IntersectionObserver | null = null;

function epArtHtml(f: PodFeed, e: PodEpisode): string {
  const key = epArtKey(f, e);
  const url = state.devSkeleton ? undefined : epArtCache.get(key);
  if (url) return `<span class="pod-ep-art"><img src="${url}" alt="" /></span>`;
  if (url === null) {
    // 回の画像が無かったときは番組の画像、それも無ければ音符
    const show = epArtCache.get(`${f.id}\u0001`);
    return show ? `<span class="pod-ep-art"><img src="${show}" alt="" /></span>` : `<span class="pod-ep-art none">${ICON_POD}</span>`;
  }
  // 回の一覧は全部の回を置くので、画面の外の回まで光らせると何百ものアニメーションが動き続ける
  // (WebView2 では番組の画面を開いたままにすると CPU を 1 コア使い切った)。読み込みを頼むまでは止めておく (.wait)
  return `<span class="pod-ep-art skel wait" data-art="${esc(key)}"${skelStyle()}></span>`;
}

/** 見えてきた行の画像を読み込む */
function observeEpArt() {
  epArtObserver?.disconnect();
  epArtObserver = new IntersectionObserver(
    (entries) => {
      for (const en of entries) {
        if (!en.isIntersecting) continue;
        const key = (en.target as HTMLElement).dataset.art!;
        epArtObserver?.unobserve(en.target);
        // 見えてきたので光らせ始める (読み込みを頼んだ印)
        en.target.classList.remove("wait");
        if (!epArtCache.has(key) && !epArtQueue.includes(key)) epArtQueue.push(key);
      }
      void pumpEpArt();
    },
    { root: $("pod-eps"), rootMargin: "200px 0px" },
  );
  for (const el of $("pod-eps").querySelectorAll<HTMLElement>(".pod-ep-art[data-art]")) epArtObserver.observe(el);
}

async function pumpEpArt() {
  if (epArtBusy || state.devSkeleton) return;
  epArtBusy = true;
  try {
    while (epArtQueue.length) {
      const key = epArtQueue.shift()!;
      if (epArtCache.has(key)) continue;
      const [feedId, url] = key.split("\u0001");
      const data = await invoke<string | null>("podcast_image", { feedId, url }).catch(() => null);
      epArtCache.set(key, data ? dataUrlToObjectUrl(data) : null);
      // 同じ画像の行をまとめて差し替える (番組の画像が来たら、画像の無い回もそれにする)
      const f = pod.feeds.find((x) => x.id === feedId);
      if (!f || state.settingsOpen) continue;
      for (const row of $("pod-eps").querySelectorAll<HTMLElement>(".pod-ep")) {
        const e = f.episodes.find((x) => x.id === row.dataset.ep);
        const art = row.querySelector<HTMLElement>(".pod-ep-art");
        if (!e || !art) continue;
        const k = epArtKey(f, e);
        if (k === key || (url === "" && epArtCache.get(k) === null)) art.outerHTML = epArtHtml(f, e);
      }
    }
  } finally {
    epArtBusy = false;
  }
}

/** 取得中・転送中の印だけを直す (一覧は作り直さない) */
export function renderPodStatuses() {
  const f = currentFeed();
  if (!f || state.settingsOpen) return;
  for (const row of $("pod-eps").querySelectorAll<HTMLElement>(".pod-ep")) {
    const e = f.episodes.find((x) => x.id === row.dataset.ep);
    if (!e) continue;
    const html = epStatusHtml(f, e);
    const el = row.querySelector<HTMLElement>(".pod-ep-status")!;
    if (el.innerHTML !== html) el.innerHTML = html;
  }
}

/** 全選択のチェック: 表示中の回が全部選ばれていればチェック、一部なら横線 */
function updatePodSelAll() {
  const f = currentFeed();
  const btn = document.getElementById("pod-selall");
  if (!f || !btn) return;
  const shown = podEpisodesShown(f);
  const n = shown.filter((e) => pod.selected.has(e.id)).length;
  btn.classList.toggle("all", n > 0 && n === shown.length);
  btn.classList.toggle("some", n > 0 && n < shown.length);
  btn.setAttribute("aria-pressed", n === 0 ? "false" : n === shown.length ? "true" : "mixed");
}

function toggleAllPodSelection(f: PodFeed) {
  const shown = podEpisodesShown(f);
  const all = shown.length > 0 && shown.every((e) => pod.selected.has(e.id));
  for (const e of shown) {
    if (all) pod.selected.delete(e.id);
    else pod.selected.add(e.id);
  }
  for (const row of $("pod-eps").querySelectorAll<HTMLElement>(".pod-ep")) {
    const on = pod.selected.has(row.dataset.ep!);
    row.classList.toggle("sel", on);
    row.querySelector(".pod-chk")?.setAttribute("aria-pressed", String(on));
  }
  renderPodSelbar();
}

function renderPodSelbar() {
  updatePodSelAll();
  const f = currentFeed();
  const n = pod.selected.size;
  const bar = $("pod-selbar");
  bar.classList.toggle("hidden", !f || !n);
  if (!f || !n) return;
  bar.innerHTML = `<span class="sel-count">${esc(tr("{n} 回を選択中", { n }))}</span><div class="spacer"></div>
    <button class="btn" data-act="send">${esc(state.snap ? tr("iPod に送る") : tr("取得"))}</button>
    <button class="btn" data-act="played">${esc(tr("再生済みにする"))}</button>
    <button class="btn" data-act="more"><span>${esc(tr("メニュー"))}</span><svg class="icon-line sel-more-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 15 6-6 6 6"/></svg></button>
    <button class="btn ghost" data-act="clear">${esc(tr("選択を解除"))}</button>`;
}

/** 回のチェックを切り替える。Shift なら、前に押した回からここまでを同じ状態にする */
let podAnchor: string | null = null;
function togglePodSelection(f: PodFeed, id: string, range: boolean) {
  const shown = podEpisodesShown(f).map((e) => e.id);
  const on = !pod.selected.has(id);
  const a = range && podAnchor ? shown.indexOf(podAnchor) : -1;
  const b = shown.indexOf(id);
  const ids = a >= 0 && b >= 0 ? shown.slice(Math.min(a, b), Math.max(a, b) + 1) : [id];
  ids.forEach((x, i) => {
    if (on) pod.selected.add(x);
    else pod.selected.delete(x);
    const row = $("pod-eps").querySelector<HTMLElement>(`.pod-ep[data-ep="${x}"]`);
    if (!row) return;
    row.classList.toggle("sel", on);
    row.querySelector(".pod-chk")?.setAttribute("aria-pressed", String(on));
    if (on && !reduceMotion) {
      row.style.setProperty("--d", `${Math.min(i, 12) * 16}ms`);
      replayClass(row, "chk-anim");
    }
  });
  podAnchor = id;
  renderPodSelbar();
}

function selectedEpisodes(f: PodFeed): PodEpisode[] {
  return f.episodes.filter((e) => pod.selected.has(e.id));
}

function episodeMenuItems(f: PodFeed, eps: PodEpisode[]): MenuItem[] {
  const onIpod = eps.filter((e) => epOnIpod(f, e));
  const files = eps.filter((e) => e.file);
  return [
    { label: state.snap ? tr("iPod に送る") : tr("取得"), disabled: !eps.some((e) => !epOnIpod(f, e) && !epVideo(e)), action: () => void sendEpisodes(f, eps.filter((e) => !epOnIpod(f, e))) },
    { sep: true },
    { label: tr("再生済みにする"), disabled: eps.every((e) => epPlayed(f, e)), action: () => void markEpisodes(f, eps, true) },
    { label: tr("未再生にする"), disabled: eps.every((e) => !epPlayed(f, e)), action: () => void markEpisodes(f, eps, false) },
    { sep: true },
    { label: tr("取得したファイルを削除"), disabled: !files.length, action: () => void deleteEpisodeFiles(f, files) },
    // iPod が無いときは出さない (iPod が要る操作)
    ...(state.snap ? [{ label: tr("iPod から削除…"), danger: true, disabled: !onIpod.length || !writable(), action: () => void removeEpisodesFromIpod(f, onIpod) }] : []),
  ];
}

/** 番組の詳細 (説明の全文・作者・サイト・フィード・回の数など)。曲の「詳細情報」と同じ形 */
async function showPodDetails(f: PodFeed) {
  const date = (sec: number) => (sec ? new Date(sec * 1000).toLocaleString(lang === "ja" ? "ja-JP" : undefined) : "—");
  const rows = (items: [string, string][]) =>
    items
      .filter(([, v]) => v)
      .map(([k, v]) => `<span>${esc(k)}</span><span>${esc(v)}</span>`)
      .join("");
  const unplayed = f.episodes.filter((e) => !epPlayed(f, e)).length;
  const onIpod = f.episodes.filter((e) => epOnIpod(f, e)).length;
  const files = f.episodes.filter((e) => e.file).length;
  const latest = f.episodes.reduce((a, e) => Math.max(a, e.published), 0);
  const cover = pod.covers.get(f.id);
  const keep = f.keep ? (f.keepOldest ? tr("古い方から {n} 回", { n: f.keep }) : tr("新しい {n} 回", { n: f.keep })) : tr("@keep:送らない");
  await modal(
    `<div class="pod-info-head">
       ${cover ? `<img class="pod-info-cover" src="${cover.large}" alt="" />` : `<div class="pod-info-cover pod-cover-none">${ICON_POD}</div>`}
       <div class="pod-info-name"><h3>${esc(f.title || f.url)}</h3>${f.author ? `<div class="muted">${esc(f.author)}</div>` : ""}</div>
     </div>
     <div class="dt-body">
       ${f.description ? `<div class="pod-info-desc">${esc(f.description)}</div>` : ""}
       <div class="dt-sec"><div class="dt-title">${esc(tr("回"))}</div><div class="set-info dt-grid">${rows([
         [tr("回の数"), tr("{n} 回", { n: f.episodes.length })],
         [tr("未再生"), tr("{n} 回", { n: unplayed })],
         [tr("iPod にある回"), state.snap ? tr("{n} 回", { n: onIpod }) : ""],
         [tr("取得したファイル"), tr("{n} 回", { n: files })],
         [tr("最新の回"), latest ? date(latest) : ""],
         [tr("iPod に置く回"), keep],
       ])}</div></div>
       <div class="dt-sec"><div class="dt-title">${esc(tr("番組"))}</div><div class="set-info dt-grid">${rows([
         [tr("フィード"), f.url],
         [tr("サイト"), f.link],
         [tr("登録した日"), date(f.added)],
         [tr("最後に確かめた日時"), date(f.checked)],
       ])}</div></div>
     </div>
     <div class="modal-actions">
       ${/^https?:\/\//.test(f.link) ? `<button class="btn" data-x="site">${esc(tr("サイトを開く"))}</button>` : ""}
       <button class="btn" data-x="copy">${esc(tr("フィードの URL をコピー"))}</button>
       <button class="btn primary" data-x="close">${esc(tr("閉じる"))}</button>
     </div>`,
    (root, close) => {
      const closeBtn = root.querySelector<HTMLButtonElement>('[data-x="close"]')!;
      closeBtn.addEventListener("click", close);
      closeBtn.focus();
      root.querySelector('[data-x="site"]')?.addEventListener("click", () => void call("podcast_open_link", { id: f.id }));
      root.querySelector('[data-x="copy"]')!.addEventListener("click", () =>
        navigator.clipboard.writeText(f.url).then(
          () => toast(tr("コピーしました")),
          () => toast(tr("コピーできませんでした"), "error"),
        ),
      );
    },
    "wide pod-info-modal",
  );
}

function feedMenuItems(f: PodFeed): MenuItem[] {
  const files = f.episodes.filter((e) => e.file);
  return [
    { header: f.title },
    { label: tr("番組の情報…"), action: () => void showPodDetails(f) },
    { sep: true },
    { label: tr("更新"), disabled: pod.refreshing.has(f.id), action: () => void refreshPodcasts([f.id]) },
    { label: tr("フィードの URL をコピー"), action: () => void navigator.clipboard.writeText(f.url).then(() => toast(tr("コピーしました"))) },
    { label: tr("取得したファイルをすべて削除"), disabled: !files.length, action: () => void deleteEpisodeFiles(f, files) },
    { sep: true },
    { label: tr("登録を解除…"), danger: true, action: () => void unsubscribePodcast(f) },
  ];
}

export function bindPodcasts() {
  $("btn-add-pod").addEventListener("click", subscribePodcast);
  $("btn-sync-pod").addEventListener("click", () => void syncPodcasts());
  $("pod-side-list").addEventListener("click", (e) => {
    const item = (e.target as HTMLElement).closest<HTMLElement>(".side-item");
    if (item?.dataset.addpod) subscribePodcast();
    else if (item?.dataset.view) selectView(item.dataset.view);
  });
  $("pod-side-list").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const id = (e.target as HTMLElement).closest<HTMLElement>(".side-item")?.dataset.pod;
    const f = pod.feeds.find((x) => x.id === id);
    if (f) showMenu(e.clientX, e.clientY, feedMenuItems(f));
  });
  const view = $("pod-view");
  view.addEventListener("change", (e) => {
    const f = currentFeed();
    const el = e.target as HTMLElement;
    if (!f) return;
    if (el.id === "pod-keep-dir" || el.id === "pod-keep-n") {
      const v = (el as HTMLSelectElement).value;
      if (el.id === "pod-keep-n") f.keep = Number(v);
      else if (v === "off") f.keep = 0;
      else {
        f.keepOldest = v === "old";
        if (!f.keep) f.keep = POD_DEFAULT_KEEP;
      }
      invoke("podcast_set_keep", { id: f.id, keep: f.keep, oldest: f.keepOldest }).catch((err) => toast(String(err), "error"));
      // 送らない ⇄ 送るで、数のセレクトを出し入れする
      if (el.id === "pod-keep-dir") renderPodView();
      else fitSelectWidth(el as HTMLSelectElement);
      return;
    }
  });
  view.addEventListener("click", (e) => {
    const f = currentFeed();
    const el = e.target as HTMLElement;
    if (!f) return;
    const btn = el.closest<HTMLElement>("button");
    if (btn?.id === "pod-sync") return void syncPodcasts(false, f);
    if (btn?.id === "pod-refresh") return void refreshPodcasts([f.id]);
    if (btn?.id === "pod-sort") return openPodSortMenu(btn, f);
    if (btn?.id === "pod-selall") return toggleAllPodSelection(f);
    if (el.closest(".pod-cover, .pod-title")) return void showPodDetails(f);
    if (btn?.id === "pod-more") {
      const r = btn.getBoundingClientRect();
      return showMenu(r.left, r.bottom + 4, feedMenuItems(f));
    }
    const act = btn?.dataset.act;
    if (act) {
      const eps = selectedEpisodes(f);
      if (act === "send") void sendEpisodes(f, eps.filter((x) => !epOnIpod(f, x)));
      else if (act === "played") void markEpisodes(f, eps, true);
      else if (act === "clear") {
        pod.selected.clear();
        renderPodView();
      } else if (act === "more") {
        const r = btn!.getBoundingClientRect();
        showMenu(r.left, r.top - 4, episodeMenuItems(f, eps));
      }
      return;
    }
    const row = el.closest<HTMLElement>(".pod-ep");
    const ep = row && f.episodes.find((x) => x.id === row.dataset.ep);
    if (!row || !ep) return;
    if (btn?.dataset.send) return void sendEpisodes(f, [ep]);
    if (btn?.classList.contains("pod-played")) return void markEpisodes(f, [ep], btn.dataset.played !== "1");
    if (btn?.classList.contains("pod-chk")) togglePodSelection(f, ep.id, e.shiftKey);
  });
  view.addEventListener("contextmenu", (e) => {
    const f = currentFeed();
    const row = (e.target as HTMLElement).closest<HTMLElement>(".pod-ep");
    if (!f || !row) return;
    e.preventDefault();
    const id = row.dataset.ep!;
    // チェックした回の上なら、チェックした回すべて。そうでなければその回だけ
    const eps = pod.selected.has(id) ? selectedEpisodes(f) : f.episodes.filter((x) => x.id === id);
    showMenu(e.clientX, e.clientY, episodeMenuItems(f, eps));
  });
}
