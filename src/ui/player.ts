// 再生 (ipodaudio:// から少しずつ読む) と下のプレーヤー

import { t as tr } from "../i18n";
import { rowTop } from "../app/rows";
import { PC_ALL } from "../app/selectors";
import { $, ROW_H, state } from "../app/state";
import type { Layout } from "../app/state";
import { fmtDur } from "../domain/format";
import { albumKey, artistKey } from "../domain/track-order";
import type { Track } from "../domain/types";
import { getStored, setStored } from "../infra/prefs";
import { albumArtCache, loadAlbumArt } from "./album-grid";
import { esc, isEnter } from "./dom";
import { reduceMotion, replayClass, toast } from "./feedback";
import { ICON_LIB } from "./icons";
import { renderAll } from "./render";
import { renderRows } from "./rows";
import { renderSelbar } from "./selbar";
import { closeSettings } from "./settings";
import { selectView } from "./sidebar";
import { setAnchor } from "./table-select";

// ---------------- 再生 ----------------

// 三角は重心 (x = 8 + 12/3 = 12) を中央に置く。外枠を中央にすると平らな辺の側に寄って見える
export const ICON_PLAY = '<svg viewBox="0 0 24 24"><path d="M8 5v14l12-7L8 5Z"/></svg>';
export const ICON_PAUSE = '<svg viewBox="0 0 24 24"><path d="M6 5h4v14H6V5Zm8 0h4v14h-4V5Z"/></svg>';
/** チェックマーク。線で描くので、左の短い線から右上へ一筆で描く動きを付けられる */
export const CHECK_MARK = '<svg class="check-mark" viewBox="0 0 12 12" aria-hidden="true"><path d="M2.6 6.3 5 8.6 9.5 3.7"/></svg>';

/** 再生中の行の印: 上下に動く 3 本のバー (一時停止中は止まる) */
/** 再生中の印 (動く 3 本のバー)。aria-label を訳すので関数にする */
export function eqBars(): string {
  return `<span class="eq" aria-label="${esc(tr("再生中"))}"><i></i><i></i><i></i></span>`;
}

/** これより狭いプレーヤーでは音量のスライダーを隠す */
const PLAYER_NARROW = 640;

/** リピート: しない / 全曲 (最後の次は最初へ) / 1 曲 (終わったら同じ曲をもう一度) */
type RepeatMode = "off" | "all" | "one";

/** iPod の曲を PC にコピーせずに再生する (ipodaudio プロトコルで少しずつ読む) */
export const player = {
  audio: new Audio(),
  /** 再生中の曲 ID */
  current: null as string | null,
  /** 前後の曲の送り順 (再生を始めたときの一覧の並び。シャッフル中は混ぜた順) */
  queue: [] as string[],
  /** シャッフルする前の並び (シャッフルをやめたら、この並びの今の曲の位置から続ける) */
  ordered: [] as string[],
  shuffle: false,
  repeat: "off" as RepeatMode,
};

const ICON_VOL_OFF =
  '<svg viewBox="0 0 24 24"><path d="M16.5 12A4.5 4.5 0 0 0 14 7.97v2.21l2.45 2.45c.03-.2.05-.41.05-.63Zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51A8.8 8.8 0 0 0 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71ZM4.27 3 3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06a8.99 8.99 0 0 0 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3ZM12 4 9.91 6.09 12 8.18V4Z"/></svg>';
const ICON_VOL_LOW = '<svg viewBox="0 0 24 24"><path d="M18.5 12A4.5 4.5 0 0 0 16 7.97v8.05c1.48-.73 2.5-2.25 2.5-4.02ZM5 9v6h4l5 5V4L9 9H5Z"/></svg>';
const ICON_VOL_HIGH =
  '<svg viewBox="0 0 24 24"><path d="M3 9v6h4l5 5V4L7 9H3Zm13.5 3A4.5 4.5 0 0 0 14 7.97v8.05c1.48-.73 2.5-2.25 2.5-4.02ZM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77Z"/></svg>';

/** 音量のアイコン (動画の再生画面でも使う) */
export function volumeIcon(volume: number, muted: boolean): string {
  return muted || volume === 0 ? ICON_VOL_OFF : volume < 0.5 ? ICON_VOL_LOW : ICON_VOL_HIGH;
}

/** 音量を変える (動画の再生画面から)。下のプレーヤーのスライダーとアイコンもそろえ、save なら次回のために覚える */
export function setPlayerVolume(volume: number, muted: boolean, save: boolean) {
  const a = player.audio;
  a.volume = volume;
  a.muted = muted;
  const vol = $<HTMLInputElement>("pl-vol");
  vol.value = String(Math.round(volume * 100));
  vol.style.setProperty("--p", `${vol.value}%`);
  renderVolumeIcon();
  if (save) setStored("playerVolume", vol.value);
}

/** 音量のアイコン: ミュート中か 0 なら消音、半分未満なら小、それ以上なら大 */
export function renderVolumeIcon() {
  const a = player.audio;
  const kind = a.muted || a.volume === 0 ? "off" : a.volume < 0.5 ? "low" : "high";
  const btn = $("pl-mute");
  if (btn.dataset.icon !== kind) {
    btn.dataset.icon = kind;
    btn.innerHTML = kind === "off" ? ICON_VOL_OFF : kind === "low" ? ICON_VOL_LOW : ICON_VOL_HIGH;
  }
  btn.setAttribute("aria-pressed", String(a.muted));
  btn.title = a.muted ? tr("ミュートを解除") : tr("ミュート");
}

const ICON_REPEAT = '<svg viewBox="0 0 24 24"><path d="M7 7h10v3l4-4-4-4v3H5v6h2V7Zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2v4Z"/></svg>';
const ICON_REPEAT_ONE =
  '<svg viewBox="0 0 24 24"><path d="M7 7h10v3l4-4-4-4v3H5v6h2V7Zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2v4Zm-4-2V9h-1l-2 1v1h1.5v4H13Z"/></svg>';

/** 今の曲を先頭に、残りを混ぜた並び (Fisher–Yates) */
function shuffled(ids: string[], first: string | null): string[] {
  const rest = ids.filter((id) => id !== first);
  for (let i = rest.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [rest[i], rest[j]] = [rest[j], rest[i]];
  }
  return first && ids.includes(first) ? [first, ...rest] : rest;
}

function setShuffle(on: boolean) {
  player.shuffle = on;
  setStored("playerShuffle", on ? "1" : null);
  if (player.current) player.queue = on ? shuffled(player.ordered, player.current) : player.ordered;
  renderPlayerModes();
}

function cycleRepeat() {
  player.repeat = player.repeat === "off" ? "all" : player.repeat === "all" ? "one" : "off";
  setStored("playerRepeat", player.repeat === "off" ? null : player.repeat);
  renderPlayerModes();
}

export function renderPlayerModes() {
  const sh = $("pl-shuffle");
  sh.classList.toggle("on", player.shuffle);
  sh.setAttribute("aria-pressed", String(player.shuffle));
  sh.title = player.shuffle ? tr("シャッフル: オン") : tr("シャッフル: オフ");
  const rp = $("pl-repeat");
  const icon = player.repeat === "one" ? ICON_REPEAT_ONE : ICON_REPEAT;
  if (rp.dataset.icon !== player.repeat) {
    rp.dataset.icon = player.repeat;
    rp.innerHTML = icon;
  }
  rp.classList.toggle("on", player.repeat !== "off");
  rp.setAttribute("aria-pressed", String(player.repeat !== "off"));
  rp.title =
    player.repeat === "all" ? tr("リピート: 全曲") : player.repeat === "one" ? tr("リピート: 1 曲") : tr("リピート: オフ");
}

/** iPod の曲か PC のフォルダの曲を ID で探す */
export function findTrack(id: string): Track | undefined {
  return state.trackMap.get(id) ?? state.pc.byId.get(id);
}

function audioUrl(id: string): string {
  // Windows の WebView2 では独自プロトコルが http://<名前>.localhost になる
  const base = navigator.userAgent.includes("Windows") ? "http://ipodaudio.localhost/" : "ipodaudio://localhost/";
  return id.startsWith("pc:") ? `${base}pc?p=${encodeURIComponent(id.slice(3))}` : base + id;
}

export function playTrack(id: string, queue?: string[]) {
  const t = findTrack(id);
  if (!t) return;
  if (state.partialOpen && !t.pcPath) {
    toast(tr("読み込みが終わると再生できます"));
    return;
  }
  // 前後の曲は、同じ種類 (iPod / PC) の今の一覧の並び。queue を渡したときは送り順をそのまま使う (前後の曲へ進むとき)
  if (!queue) {
    player.ordered = state.rows.filter((r) => !!r.pcPath === !!t.pcPath).map((r) => r.id);
    player.queue = player.shuffle ? shuffled(player.ordered, id) : player.ordered;
  } else if (queue !== player.queue) {
    // 選んだ曲たちを再生するとき (右クリックの「再生」など)
    player.ordered = queue;
    player.queue = player.shuffle ? shuffled(queue, id) : queue;
  }
  player.current = id;
  player.audio.src = audioUrl(id);
  player.audio.play().catch(() => {
    /* 失敗の理由は error イベントで表示 */
  });
  $("player").classList.remove("hidden");
  renderPlayerTitle(t);
  showPlayerArt(t);
  $("pl-sub").textContent = tr("読み込み中…");
  if ("mediaSession" in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({ title: t.title, artist: t.artist, album: t.album });
  }
  renderPlayer();
  renderRows();
}

/** プレイヤーのジャケット。読み込み済みならすぐ出し、無ければ読み込んでから出す */
function showPlayerArt(t: Track) {
  const el = $("pl-art");
  const src = t.pcPath ? `p:${t.pcPath}` : t.artworkId && state.snap?.artworkSupported ? `i:${t.artworkId}` : null;
  const put = (url: string | null | undefined) => {
    el.innerHTML = url ? `<img src="${url}" alt="">` : ICON_LIB;
    el.classList.toggle("none", !url);
  };
  if (!src) return put(null);
  if (albumArtCache.has(src)) return put(albumArtCache.get(src));
  put(null);
  loadAlbumArt(src).then((url) => {
    // 読み込んでいる間に次の曲へ進んでいたら出さない
    if (player.current === t.id) put(url);
  });
}

/** プレーヤーの曲名。押すとその曲のアルバム (無ければ曲の一覧) でその曲を出す */
function renderPlayerTitle(t: Track) {
  $("pl-title").innerHTML = plLink("track", t.title);
}

/** プレーヤーのアーティスト — アルバム。それぞれ押すとアーティスト・アルバムの一覧へ移る */
function renderPlayerSub(t: Track | undefined) {
  $("pl-sub").innerHTML = t
    ? [t.artist && plLink("artist", t.artist), t.album && plLink("album", t.album)].filter(Boolean).join('<span class="pl-sep"> — </span>')
    : "";
}

/** 一覧へ移るリンク。長い名前を「…」で省略できるよう、ボタンではなくインラインの要素にする */
function plLink(go: string, text: string): string {
  return `<span class="pl-link" role="link" tabindex="0" data-go="${go}" title="${esc(tr("{a} を一覧で表示", { a: text }))}">${esc(text)}</span>`;
}

/**
 * 再生中の曲を一覧で出す。iPod の曲は iPod の一覧、PC の曲は PC のすべての曲へ移り、検索は解除する。
 * go: track = その曲 (アルバムを開いてその行へ) / artist = アーティスト表示 / album = アルバム表示
 */
function revealPlaying(go: "track" | "artist" | "album") {
  const t = findTrack(player.current ?? "");
  if (!t || (!t.pcPath && !state.snap)) return;
  if (state.settingsOpen) closeSettings();
  if (state.search) {
    state.search = "";
    $<HTMLInputElement>("search").value = "";
  }
  const v = t.pcPath ? PC_ALL : "library";
  if (state.view !== v) selectView(v);
  const layout: Layout = go === "artist" ? "artists" : go === "album" || t.album ? "albums" : "tracks";
  if (state.layout !== layout) {
    state.layout = layout;
    setStored("layout", layout);
  }
  state.openAlbum = layout === "albums" ? albumKey(t) : null;
  if (layout === "artists") state.artist = artistKey(t);
  state.selected.clear();
  state.anchor = -1;
  state.focused = null;
  $("album-grid").scrollTop = 0;
  $("tbody").scrollTop = 0;
  renderAll();
  // その曲の行を選んで、見える位置 (まん中あたり) までスクロールする
  const i = state.rows.findIndex((r) => r.id === t.id);
  if (i < 0) return;
  setAnchor(i);
  state.focused = t.id;
  const body = $("tbody");
  body.scrollTop = Math.max(0, rowTop(i) - body.clientHeight / 2 + ROW_H / 2);
  renderRows();
  renderSelbar();
}

export function stopPlayer() {
  if (!player.current) return;
  player.audio.pause();
  player.audio.removeAttribute("src");
  player.audio.load();
  player.current = null;
  $("player").classList.add("hidden");
  renderRows();
}

export function togglePlay() {
  if (!player.current) return;
  if (player.audio.paused) player.audio.play().catch(() => undefined);
  else player.audio.pause();
}

/** 前後の曲へ。ended は曲が終わって自動で進むとき (1 曲リピートは同じ曲をもう一度) */
function stepTrack(delta: number, ended = false) {
  if (ended && player.repeat === "one") {
    player.audio.currentTime = 0;
    player.audio.play().catch(() => undefined);
    return;
  }
  const q = player.queue;
  const i = q.indexOf(player.current ?? "");
  let next = q[i + delta];
  // 全曲リピート: 最後の次は最初へ、最初の前は最後へ
  if (!next && player.repeat !== "off" && q.length) next = q[(i + delta + q.length) % q.length];
  if (next) playTrack(next, player.queue);
  else if (delta > 0) stopPlayer();
  else player.audio.currentTime = 0;
}

function renderPlayer() {
  const a = player.audio;
  // timeupdate で 1 秒に数回呼ばれる。毎回アイコンを作り直すと、押している途中で押した要素が
  // 入れ替わってクリックにならないことがあるので、再生 / 一時停止が変わったときだけ替える
  const toggle = $("pl-toggle");
  const icon = a.paused ? "play" : "pause";
  if (toggle.dataset.icon !== icon) {
    if (toggle.dataset.icon && !reduceMotion) requestAnimationFrame(() => replayClass(toggle, "swap"));
    toggle.dataset.icon = icon;
    // 一覧の再生中の印のアニメーションも止める / 動かす
    document.body.classList.toggle("player-paused", a.paused);
    toggle.innerHTML = a.paused ? ICON_PLAY : ICON_PAUSE;
  }
  const dur = Number.isFinite(a.duration) ? a.duration : (findTrack(player.current ?? "")?.durationMs ?? 0) / 1000;
  $("pl-cur").textContent = fmtDur(a.currentTime * 1000);
  $("pl-dur").textContent = fmtDur(dur * 1000);
  const seek = $<HTMLInputElement>("pl-seek");
  if (!seek.matches(":active")) seek.value = String(dur ? Math.round((a.currentTime / dur) * 1000) : 0);
  seek.style.setProperty("--p", `${Number(seek.value) / 10}%`);
}

export function bindPlayer() {
  const a = player.audio;
  // 音量は最後に設定した値を次回も使う
  const vol = $<HTMLInputElement>("pl-vol");
  let saved = 80;
  const stored = getStored("playerVolume");
  if (stored !== null && Number(stored) >= 0 && Number(stored) <= 100) saved = Number(stored);
  vol.value = String(saved);
  vol.style.setProperty("--p", `${saved}%`);
  a.volume = saved / 100;
  a.addEventListener("timeupdate", renderPlayer);
  a.addEventListener("play", renderPlayer);
  a.addEventListener("pause", renderPlayer);
  a.addEventListener("playing", () => {
    const t = findTrack(player.current ?? "");
    renderPlayerSub(t);
  });
  a.addEventListener("ended", () => stepTrack(1, true));
  a.addEventListener("error", async () => {
    if (!player.current || !a.getAttribute("src")) return;
    // 失敗の理由 (ffmpeg が無い・曲が見つからない等) はプロトコルの応答本文に入っている
    const msg = await fetch(audioUrl(player.current), { headers: { Range: "bytes=0-0" } })
      .then((r) => (r.ok ? "" : r.text()))
      .catch(() => "");
    toast(msg || tr("この曲は再生できませんでした"), "error", 6000);
    $("pl-sub").textContent = tr("再生できませんでした");
    renderPlayer();
  });
  $("pl-toggle").addEventListener("click", togglePlay);
  $("pl-prev").addEventListener("click", () => (a.currentTime > 3 ? (a.currentTime = 0) : stepTrack(-1)));
  $("pl-next").addEventListener("click", () => stepTrack(1));
  $("pl-close").addEventListener("click", stopPlayer);
  const info = $("player").querySelector<HTMLElement>(".player-info")!;
  const onLink = (e: Event) => {
    const go = (e.target as HTMLElement).closest<HTMLElement>(".pl-link")?.dataset.go;
    if (go === "track" || go === "artist" || go === "album") revealPlaying(go);
  };
  info.addEventListener("click", onLink);
  info.addEventListener("keydown", (e) => isEnter(e) && onLink(e));
  // とても狭いときは音量のスライダーを隠す (プレーヤーの幅で判断。macOS 10.15 の WebKit は container queries を使えない)
  const bar = $("player");
  new ResizeObserver(() => {
    if (bar.offsetWidth) bar.classList.toggle("narrow", bar.offsetWidth < PLAYER_NARROW);
  }).observe(bar);
  // シャッフル・リピートは最後に選んだものを次回も使う
  player.shuffle = getStored("playerShuffle") === "1";
  const rep = getStored("playerRepeat");
  player.repeat = rep === "all" || rep === "one" ? rep : "off";
  renderPlayerModes();
  $("pl-shuffle").addEventListener("click", () => setShuffle(!player.shuffle));
  $("pl-repeat").addEventListener("click", cycleRepeat);
  $<HTMLInputElement>("pl-seek").addEventListener("input", (e) => {
    const dur = a.duration;
    const el = e.target as HTMLInputElement;
    el.style.setProperty("--p", `${Number(el.value) / 10}%`);
    if (Number.isFinite(dur)) a.currentTime = (Number(el.value) / 1000) * dur;
  });
  vol.addEventListener("input", () => {
    a.volume = Number(vol.value) / 100;
    // スライダーを動かしたらミュートは解除する
    a.muted = false;
    vol.style.setProperty("--p", `${vol.value}%`);
    renderVolumeIcon();
  });
  // 保存は動かし終えたときだけ (動かしている間に何十回も設定ファイルを書かない)
  vol.addEventListener("change", () => setStored("playerVolume", vol.value));
  $("pl-mute").addEventListener("click", () => {
    a.muted = !a.muted;
    renderVolumeIcon();
  });
  renderVolumeIcon();
  if ("mediaSession" in navigator) {
    navigator.mediaSession.setActionHandler("play", togglePlay);
    navigator.mediaSession.setActionHandler("pause", togglePlay);
    navigator.mediaSession.setActionHandler("previoustrack", () => stepTrack(-1));
    navigator.mediaSession.setActionHandler("nexttrack", () => stepTrack(1));
  }
}
