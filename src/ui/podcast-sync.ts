// ポッドキャストの iPod との同期と、回の取得・転送

import { t as tr } from "../i18n";
import { listen } from "../infra/tauri";
import { epOnIpod, epPlayed, pod } from "../app/podcasts";
import { writable } from "../app/selectors";
import { state } from "../app/state";
import { POD_CONNECT_REFRESH_AGE, epDate, epVideo } from "../domain/podcast";
import type { PodDownloadResult, PodEpisode, PodFeed } from "../domain/podcast";
import type { Snapshot, Track } from "../domain/types";
import { pref, setPref } from "../infra/prefs";
import { esc } from "./dom";
import { busyToast, call, toast } from "./feedback";
import { modal } from "./modal";
import { loadPodcasts, podFirstLoad, refreshPodcasts, renderPodAll, renderPodStatuses } from "./podcasts";
import { applySnapshot } from "./render";
import { startTransfer } from "./transfer";

/** 回を取得して iPod に送る (iPod が無ければ取得だけ)。転送を始めたら true */
export async function sendEpisodes(f: PodFeed, eps: PodEpisode[]): Promise<boolean> {
  const list = eps.filter((e) => !epVideo(e));
  if (!list.length) {
    if (eps.length) toast(tr("動画の回には対応していません"), "error");
    return false;
  }
  return sendEpisodeItems(list.map((e) => ({ feed: f, ep: e })));
}

async function sendEpisodeItems(items: { feed: PodFeed; ep: PodEpisode }[]): Promise<boolean> {
  const toIpod = !!state.snap;
  if (toIpod && !writable()) {
    toast(state.snap!.writableError ?? tr("書き込みできません"), "error");
    return false;
  }
  if (state.transferring) {
    toast(tr("転送中です。完了までお待ちください"), "error");
    return false;
  }
  for (const it of items) if (!it.ep.file) pod.downloading.set(it.ep.id, -1);
  renderPodStatuses();
  const unlisten = await listen<{ epId: string; received: number; size: number }>("podcast-download", (ev) => {
    const p = ev.payload;
    pod.downloading.set(p.epId, p.size ? Math.min(1, p.received / p.size) : 0);
    renderPodStatuses();
  });
  const r = await call<PodDownloadResult>("podcast_download", { items: items.map((x) => ({ feedId: x.feed.id, epId: x.ep.id })) });
  unlisten();
  pod.downloading.clear();
  await loadPodcasts();
  if (!r) return false;
  if (r.errors.length) {
    await modal(
      `<h3>${esc(tr("{n} 件の回を取得できませんでした", { n: r.errors.length }))}</h3><pre class="errors">${esc(r.errors.join("\n"))}</pre>
       <div class="modal-actions"><button class="btn primary" data-x="ok">OK</button></div>`,
      (root, close) => root.querySelector('[data-x="ok"]')!.addEventListener("click", close),
    );
  }
  if (r.cancelled || !r.done.length) return false;
  for (const d of r.done) pod.paths.set(d.path, d.epId);
  if (!toIpod || !state.snap) {
    toast(tr("{n} 件の回を取得しました。iPod を接続すると送れます", { n: r.done.length }));
    return false;
  }
  // 同じ回がもう iPod にあれば送り直さない
  const started = await startTransfer(r.done.map((d) => d.path), null, "skip", null);
  renderPodStatuses();
  return started;
}

/** 同期: 「iPod に置く回」を決めた番組の、まだ聞いていない新しい回を送り、聞き終わった回を iPod から消す。
 * 実行する前に、送る回と消す回の一覧を見せて選んでもらう */
/** iPod をつないで同期を確かめたもの (iPod の場所)。抜いたら空にする (つなぎ直したらまた確かめる) */
export const podOffered = new Set<string>();

/** iPod をつないだとき (開き終えたとき) に、送る回か消す回があれば同期の確認を出す */
export async function offerPodcastSync() {
  const s = state.snap;
  if (!s || state.partialOpen || !pref("podcastSyncOnConnect")) return;
  const key = s.device.root;
  if (podOffered.has(key)) return;
  podOffered.add(key);
  await podFirstLoad;
  if (state.snap !== s && state.snap?.device.root !== key) return;
  // ほかのダイアログ・設定・転送の邪魔をしない
  if (!pod.feeds.some((f) => f.keep > 0) || state.transferring || state.settingsOpen || state.onboarding || document.querySelector(".modal")) return;
  await syncPodcasts(true);
}

/** `onConnect` は iPod をつないだときの確認 (何も無ければ黙っている。フィードは少し前に確かめた番組なら取り直さない)。
 * `only` を渡すとその番組だけ (番組の画面の「同期」)。渡さなければ登録した番組すべて (サイドバーの同期のボタン) */
export async function syncPodcasts(onConnect = false, only?: PodFeed) {
  if (!state.snap) {
    if (!onConnect) toast(tr("iPod を接続してください"));
    return;
  }
  if (!writable()) {
    if (!onConnect) toast(state.snap.writableError ?? tr("書き込みできません"), "error");
    return;
  }
  const inScope = (f: PodFeed) => f.keep > 0 && (!only || f.id === only.id);
  const managed = pod.feeds.filter(inScope);
  if (!managed.length) {
    if (!onConnect)
      toast(
        only
          ? tr("「iPod に置く回」を選ぶと、同期でこの番組の新しい回を送り、聞き終わった回を iPod から消します")
          : tr("番組の「iPod に置く回」を選ぶと、同期で新しい回を送り、聞き終わった回を iPod から消します"),
        "info",
        7000,
      );
    return;
  }
  pod.syncing = true;
  renderPodAll();
  const busy = busyToast(tr("番組を確かめています…"));
  await refreshPodcasts(managed.map((f) => f.id), true, onConnect ? POD_CONNECT_REFRESH_AGE : undefined);
  busy.done();
  pod.syncing = false;
  // 確かめている間に iPod を外した・ほかの作業が始まった
  if (!state.snap || (onConnect && (state.transferring || document.querySelector(".modal")))) {
    renderPodAll();
    return;
  }
  const send: { feed: PodFeed; ep: PodEpisode }[] = [];
  const remove: { feed: PodFeed; ep: PodEpisode; track: Track }[] = [];
  for (const f of pod.feeds.filter(inScope)) {
    // まだ聞いていない回を、新しい方から (古い方からを選んだ番組は古い方から) 数だけ
    const unplayed = f.episodes.filter((e) => !epPlayed(f, e) && !epVideo(e));
    if (f.keepOldest) unplayed.sort((a, b) => a.published - b.published);
    const fresh = unplayed.slice(0, f.keep);
    for (const e of fresh) if (!epOnIpod(f, e)) send.push({ feed: f, ep: e });
    for (const e of f.episodes) {
      const t = epOnIpod(f, e);
      if (t && epPlayed(f, e)) remove.push({ feed: f, ep: e, track: t });
    }
  }
  renderPodAll();
  if (!send.length && !remove.length) {
    if (!onConnect) toast(tr("同期するものはありません"));
    return;
  }
  const rowHtml = (kind: "s" | "r", i: number, f: PodFeed, e: PodEpisode) => `<div class="imp-row">
      <label class="imp-main">
        <input type="checkbox" data-k="${kind}" data-i="${i}" checked />
        <span class="imp-name">${esc(e.title)}${only ? "" : `<span class="muted pod-sync-show">${esc(f.title)}</span>`}</span>
        <span class="imp-count muted">${esc(epDate(e))}</span>
      </label>
    </div>`;
  const section = (title: string, kind: "s" | "r", rows: string) =>
    `<div class="pod-sync-head">${esc(title)}</div><div class="imp-list pod-sync-list" data-list="${kind}">${rows}</div>`;
  const picked = { s: new Set(send.map((_, i) => i)), r: new Set(remove.map((_, i) => i)) };
  await modal(
    `<h3>${esc(only ? tr("「{a}」を同期", { a: only.title }) : tr("ポッドキャストを同期"))}</h3>
     ${onConnect ? `<p>${esc(tr("登録した番組に、iPod に送る回か消す回があります。"))}</p>` : ""}
     ${send.length ? section(tr("iPod に送る新しい回 ({n})", { n: send.length }), "s", send.map((x, i) => rowHtml("s", i, x.feed, x.ep)).join("")) : ""}
     ${remove.length ? section(tr("iPod から消す聞き終わった回 ({n})", { n: remove.length }), "r", remove.map((x, i) => rowHtml("r", i, x.feed, x.ep)).join("")) : ""}
     ${remove.length ? `<label class="check imp-transfer"><input type="checkbox" id="pod-sync-del" checked /><span>${esc(tr("消した回の取得したファイルも PC から削除する"))}</span></label>` : ""}
     ${onConnect ? `<label class="check pod-sync-skip"><input type="checkbox" id="pod-sync-skip" /><span>${esc(tr("iPod をつないだときに確かめない"))}</span></label>` : ""}
     <div class="modal-actions"><button class="btn" data-x="cancel">${esc(onConnect ? tr("あとで") : tr("キャンセル"))}</button><button class="btn primary" data-x="ok">${esc(tr("同期"))}</button></div>`,
    (m, close) => {
      const okBtn = m.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      const update = () => (okBtn.disabled = !picked.s.size && !picked.r.size);
      m.addEventListener("change", (e) => {
        const cb = e.target as HTMLInputElement;
        const k = cb.dataset.k as "s" | "r" | undefined;
        if (!k) return;
        const i = Number(cb.dataset.i);
        if (cb.checked) picked[k].add(i);
        else picked[k].delete(i);
        update();
      });
      // 「確かめない」は、同期してもしなくても覚える (設定の「転送」で戻せる)
      const skip = m.querySelector<HTMLInputElement>("#pod-sync-skip");
      const close2 = () => {
        if (skip?.checked) setPref("podcastSyncOnConnect", false);
        close();
      };
      m.querySelector('[data-x="cancel"]')!.addEventListener("click", close2);
      okBtn.addEventListener("click", async () => {
        if (skip?.checked) setPref("podcastSyncOnConnect", false);
        const delFiles = m.querySelector<HTMLInputElement>("#pod-sync-del")?.checked ?? false;
        const sendList = send.filter((_, i) => picked.s.has(i));
        const removeList = remove.filter((_, i) => picked.r.has(i));
        close();
        pod.syncing = true;
        renderPodAll();
        try {
          // 空き容量を先に空けるため、聞き終わった回は取得・転送の前に消す
          const removeIds = removeList.map((x) => x.track.id);
          if (removeIds.length) {
            const snap = await call<Snapshot>("remove_tracks", { ids: removeIds });
            if (!snap) return;
            applySnapshot(snap);
            if (delFiles)
              await call("podcast_delete_files", { items: removeList.filter((x) => x.ep.file).map((x) => ({ feedId: x.feed.id, epId: x.ep.id })) });
            if (!sendList.length) toast(tr("聞き終わった回を {n} 件消しました", { n: removeIds.length }));
          }
          if (sendList.length) await sendEpisodeItems(sendList);
        } finally {
          pod.syncing = false;
          await loadPodcasts();
        }
      });
    },
    "imp-modal",
    false,
  );
}
