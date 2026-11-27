// 動画をアプリの中で再生する (ビデオの一覧のダブルクリック・Enter・右クリックの「再生」)。
// WebView がそのまま再生できる動画 (H.264 と AAC の MP4) は ipodaudio:// でファイルのまま少しずつ読む。
// それ以外 (Rockbox の MPEG-1 / 2 など) と、ファイルのままでは再生できなかったとき (WebKitGTK に H.264 が無いなど) は、
// ffmpeg が変換しながら流すものを Media Source Extensions で少しずつ足す。位置を飛ばしたら、その位置から変換し直す

import { t as tr } from "../i18n";
import { $ } from "../app/state";
import type { VideoView } from "../domain/visual";
import { fmtDur } from "../domain/format";
import { getCurrentWindow, invoke, mediaBase } from "../infra/tauri";
import { esc } from "./dom";
import { ICON_CHEVRON_LEFT, ICON_CHEVRON_RIGHT, ICON_CLOSE } from "./icons";
import { ICON_PAUSE, ICON_PLAY, player, setPlayerVolume, volumeIcon } from "./player";

interface VideoPlan {
  native: boolean;
  format?: string;
  mime?: string;
  durationMs: number;
}

/** 先まで読んでおく長さ (秒)。これより先は、再生が進むまで読まない */
const AHEAD_SEC = 30;

function videoUrl(id: string): string {
  return `${mediaBase()}video/${id}`;
}

/**
 * 変換して流すときに、MediaSource で再生できる形式 (使いたい順)。H.264 の MP4 を先に使い、WebM (VP8) は
 * H.264 を再生できないとき (GStreamer に H.264 が無い WebKitGTK など) だけ。新しい ffmpeg (Windows に入れる n9.0) の
 * WebM は、塊の中で音声と映像の時刻が前後することがあり、Chromium の MediaSource が受け付けない
 */
function streamFormats(): string[] {
  if (typeof MediaSource === "undefined") return [];
  const ok = (t: string) => MediaSource.isTypeSupported(t);
  const webm = ok('video/webm; codecs="vp8, vorbis"') || ok('video/webm; codecs="vp8, opus"');
  const mp4 = ok('video/mp4; codecs="avc1.42E01F, mp4a.40.2"');
  return [mp4 && "mp4", webm && "webm"].filter((x): x is string => !!x);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** ffmpeg が変換しながら流す動画を、MediaSource に少しずつ足す */
class Stream {
  /** 流し直すたびに増やす (古い読み込みを止める) */
  private gen = 0;
  private ms = new MediaSource();
  private sb: SourceBuffer | null = null;

  constructor(
    private video: HTMLVideoElement,
    private id: string,
    private plan: VideoPlan,
    private fail: (message: string) => void,
  ) {}

  async open(at: number) {
    const url = URL.createObjectURL(this.ms);
    this.video.src = url;
    await new Promise((r) => this.ms.addEventListener("sourceopen", r, { once: true }));
    URL.revokeObjectURL(url);
    this.sb = this.ms.addSourceBuffer(this.plan.mime!);
    if (this.plan.durationMs) this.ms.duration = this.plan.durationMs / 1000;
    this.video.addEventListener("seeking", this.onSeeking);
    if (at) this.video.currentTime = at;
    await this.startAt(at);
  }

  close() {
    this.gen++;
    this.video.removeEventListener("seeking", this.onSeeking);
    invoke("video_stream_stop").catch(() => undefined);
  }

  /** 読んでいない位置へ飛んだら、そこから変換し直す */
  private onSeeking = () => {
    const t = this.video.currentTime;
    if (!this.has(t)) void this.startAt(t);
  };

  /** その位置を読んであるか */
  private has(t: number): boolean {
    const b = this.sb?.buffered;
    if (!b) return false;
    for (let i = 0; i < b.length; i++) if (b.start(i) <= t + 0.25 && t < b.end(i) - 0.25) return true;
    return false;
  }

  /** 今の位置から先に読んである長さ (秒) */
  private ahead(): number {
    const b = this.sb?.buffered;
    const t = this.video.currentTime;
    if (!b) return 0;
    for (let i = 0; i < b.length; i++) if (b.start(i) <= t + 0.25 && t < b.end(i)) return b.end(i) - t;
    return 0;
  }

  private idle(): Promise<void> {
    const sb = this.sb!;
    return sb.updating ? new Promise((r) => sb.addEventListener("updateend", () => r(), { once: true })) : Promise.resolve();
  }

  private async startAt(at: number) {
    const gen = ++this.gen;
    let stream: number;
    try {
      stream = await invoke<number>("video_stream_start", { id: this.id, format: this.plan.format, startMs: Math.round(at * 1000) });
    } catch (e) {
      if (gen === this.gen) this.fail(String(e));
      return;
    }
    if (gen !== this.gen) return;
    const sb = this.sb!;
    await this.idle();
    if (gen !== this.gen) return;
    // 前に読んだ分を捨てて、変換した動画の時刻 (0 から) を開始位置にずらす。
    // 最後まで読んだ後 (ended) は remove で読み込める状態に戻し、読みかけの塊は abort で捨てる (残っていると開始位置を変えられない)
    if (this.ms.readyState === "ended") {
      sb.remove(0, Infinity);
      await this.idle();
    }
    sb.abort();
    if (sb.buffered.length) {
      sb.remove(0, Infinity);
      await this.idle();
    }
    if (gen !== this.gen) return;
    sb.timestampOffset = at;
    // 読み込みは裏で続ける (待たずに戻り、呼んだ側が再生を始められるように)
    void this.pump(gen, stream);
  }

  private async pump(gen: number, stream: number) {
    while (gen === this.gen) {
      if (this.ahead() > AHEAD_SEC) {
        await sleep(500);
        continue;
      }
      let bytes: Uint8Array;
      try {
        bytes = new Uint8Array(await invoke<ArrayBuffer>("video_stream_read", { stream }));
      } catch (e) {
        if (gen === this.gen) this.fail(String(e));
        return;
      }
      if (gen !== this.gen) return;
      // 先頭の 1 バイトは終わりの印、その後ろが動画。足せなければ (形式が合わない・壊れている) 止める
      try {
        if (bytes.length > 1) await this.append(bytes.subarray(1), gen);
      } catch {
        if (gen === this.gen) this.fail(tr("この動画は再生できません"));
        this.gen++;
        invoke("video_stream_stop").catch(() => undefined);
        return;
      }
      if (bytes[0] === 1) {
        await this.idle();
        if (gen === this.gen && this.ms.readyState === "open") this.ms.endOfStream();
        return;
      }
    }
  }

  private async append(data: Uint8Array, gen: number) {
    const sb = this.sb!;
    for (;;) {
      await this.idle();
      if (gen !== this.gen) return;
      try {
        sb.appendBuffer(data as Uint8Array<ArrayBuffer>);
        await this.idle();
        return;
      } catch (e) {
        // 溜めすぎ: 見終わった分を捨ててから足し直す
        if ((e as DOMException).name !== "QuotaExceededError") throw e;
        const behind = this.video.currentTime - 10;
        if (behind > 0) {
          sb.remove(0, behind);
          await this.idle();
        }
        await sleep(500);
      }
    }
  }
}

const ICON_FULLSCREEN = '<svg viewBox="0 0 24 24"><path d="M7 14H5v5h5v-2H7v-3Zm-2-4h2V7h3V5H5v5Zm12 7h-3v2h5v-5h-2v3ZM14 5v2h3v3h2V5h-5Z"/></svg>';
const ICON_FULLSCREEN_EXIT = '<svg viewBox="0 0 24 24"><path d="M5 16h3v3h2v-5H5v2Zm3-8H5v2h5V5H8v3Zm6 11h2v-3h3v-2h-5v5Zm2-11V5h-2v5h5V8h-3Z"/></svg>';


/** 操作を出したままにする長さ (再生中にマウスを止めてから消すまで) */
const CONTROLS_IDLE_MS = 2500;

/**
 * 動画を再生する画面。暗くした背景の真ん中に、ほどよい大きさの動画 (縦横比は動画に合わせる)。
 * 操作はすべて動画の上に重ねる: 上に題名、下に再生位置と再生・一時停止・時刻・音量・全画面。止めているときは真ん中に再生のボタン。
 * list の start 番目から。‹ › で前後の動画へ。閉じたら onClose
 */
export function openVideoPlayer(list: VideoView[], start: number, titleOf: (v: VideoView) => string, onClose?: () => void) {
  let i = Math.max(0, Math.min(list.length - 1, start));
  const back = document.createElement("div");
  back.className = "vis-viewer video-player";
  back.innerHTML = `<button class="vis-viewer-nav btn icon ghost prev" aria-label="${esc(tr("前の動画"))}">${ICON_CHEVRON_LEFT}</button>
    <div class="vp-panel">
      <div class="vp-frame loading paused">
        <video playsinline></video>
        <span class="spinner"></span>
        <button class="vp-big" aria-label="${esc(tr("再生"))}" tabindex="-1">${ICON_PLAY}</button>
        <div class="video-player-error hidden"></div>
        <div class="vp-top"><span class="vp-title"></span><span class="vp-count"></span></div>
        <div class="vp-controls">
          <input class="player-seek vp-seek" type="range" min="0" max="1000" value="0" aria-label="${esc(tr("再生位置"))}" />
          <div class="vp-row">
            <button class="vp-btn vp-toggle" aria-label="${esc(tr("再生 / 一時停止"))}" title="${esc(tr("再生 / 一時停止 (スペース)"))}"></button>
            <span class="vp-time"><span class="vp-cur">0:00</span><span class="vp-sep">/</span><span class="vp-dur">0:00</span></span>
            <span class="vp-space"></span>
            <div class="vp-volume">
              <button class="vp-btn vp-mute" aria-label="${esc(tr("ミュート"))}"></button>
              <input class="player-vol vp-vol" type="range" min="0" max="100" aria-label="${esc(tr("音量"))}" />
            </div>
            <button class="vp-btn vp-fs" aria-label="${esc(tr("全画面"))}" title="${esc(tr("全画面"))} (F)"></button>
          </div>
        </div>
      </div>
    </div>
    <button class="vis-viewer-nav btn icon ghost next" aria-label="${esc(tr("次の動画"))}">${ICON_CHEVRON_RIGHT}</button>
    <button class="vis-viewer-close btn icon ghost" aria-label="${esc(tr("閉じる"))}" title="${esc(tr("閉じる"))} (Esc)">${ICON_CLOSE}</button>`;
  const q = <T extends HTMLElement>(sel: string) => back.querySelector<T>(sel)!;
  const frame = q(".vp-frame");
  const video = q<HTMLVideoElement>("video");
  const title = q(".vp-title");
  const count = q(".vp-count");
  const errorBox = q(".video-player-error");
  const toggle = q<HTMLButtonElement>(".vp-toggle");
  const seek = q<HTMLInputElement>(".vp-seek");
  const cur = q(".vp-cur");
  const dur = q(".vp-dur");
  const muteBtn = q<HTMLButtonElement>(".vp-mute");
  const vol = q<HTMLInputElement>(".vp-vol");
  const fsBtn = q<HTMLButtonElement>(".vp-fs");
  // 1 本だけなら前後のボタンは見せない (場所は空けたまま、動画を真ん中に置く)
  q(".prev").classList.toggle("off", list.length < 2);
  q(".next").classList.toggle("off", list.length < 2);
  // 曲を再生していたら止める。音量は下のプレーヤーと同じ
  if (!player.audio.paused) player.audio.pause();
  video.volume = player.audio.volume;
  video.muted = player.audio.muted;

  let stream: Stream | null = null;
  /** 開いている動画 (前後へ移ったら古い読み込みの結果を捨てる) */
  let token = 0;

  const showError = (message: string) => {
    frame.classList.remove("loading");
    errorBox.textContent = message;
    errorBox.classList.remove("hidden");
  };
  const reset = () => {
    stream?.close();
    stream = null;
    video.removeAttribute("src");
    video.load();
    errorBox.classList.add("hidden");
    frame.classList.add("loading");
    renderTime();
  };

  // ---- 再生・位置・音量の表示 ----
  let seeking = false;
  const fmt = (sec: number) => (Number.isFinite(sec) ? fmtDur(Math.max(0, sec) * 1000) : "0:00");
  function renderTime() {
    const d = video.duration;
    cur.textContent = fmt(video.currentTime);
    dur.textContent = fmt(d);
    if (!seeking) {
      const p = Number.isFinite(d) && d > 0 ? (video.currentTime / d) * 100 : 0;
      seek.value = String(Math.round(p * 10));
      seek.style.setProperty("--p", `${p}%`);
    }
  }
  const renderToggle = () => {
    toggle.innerHTML = video.paused ? ICON_PLAY : ICON_PAUSE;
    frame.classList.toggle("paused", video.paused);
  };
  const renderVolume = () => {
    muteBtn.innerHTML = volumeIcon(video.volume, video.muted);
    muteBtn.title = video.muted ? tr("ミュートを解除") : tr("ミュート");
    const p = video.muted ? 0 : Math.round(video.volume * 100);
    vol.value = String(p);
    vol.style.setProperty("--p", `${p}%`);
  };
  const togglePlay = () => {
    if (video.paused) video.play().catch(() => undefined);
    else video.pause();
  };

  // ---- 全画面 (ウィンドウを全画面にして、動画の枠を画面いっぱいに広げる) ----
  let full = false;
  /** 開く前からウィンドウが全画面だったか (それなら抜けるときにウィンドウはそのまま) */
  let wasFull = false;
  const renderFs = () => {
    fsBtn.innerHTML = full ? ICON_FULLSCREEN_EXIT : ICON_FULLSCREEN;
    const label = full ? tr("全画面を終了") : tr("全画面");
    fsBtn.setAttribute("aria-label", label);
    fsBtn.title = `${label} (F)`;
  };
  const setFull = async (on: boolean) => {
    if (on === full) return;
    full = on;
    frame.classList.toggle("fs", on);
    renderFs();
    const win = getCurrentWindow();
    try {
      if (on) {
        wasFull = await win.isFullscreen();
        if (!wasFull) await win.setFullscreen(true);
      } else if (!wasFull) {
        await win.setFullscreen(false);
      }
    } catch {
      /* ウィンドウを全画面にできなくても、動画はウィンドウいっぱいに出す */
    }
  };
  // OS の操作 (Mac の緑のボタンなど) でウィンドウの全画面が終わったら、こちらも戻す
  const onResize = () => {
    if (!full || wasFull) return;
    void getCurrentWindow()
      .isFullscreen()
      .then((f) => {
        if (!f && full) {
          full = false;
          frame.classList.remove("fs");
          renderFs();
        }
      })
      .catch(() => undefined);
  };

  // ---- 操作の出し入れ: 再生中にマウスを止めたら消す (止めている間・マウスを動かしたら出す) ----
  let idleTimer = 0;
  const wake = () => {
    frame.classList.add("awake");
    clearTimeout(idleTimer);
    idleTimer = window.setTimeout(() => {
      if (!video.paused) frame.classList.remove("awake");
    }, CONTROLS_IDLE_MS);
  };

  /** 変換して流す (at の位置から)。known は決めてある流し方 (無ければ聞く) */
  const playStream = async (v: VideoView, my: number, at: number, known?: VideoPlan) => {
    const formats = streamFormats();
    if (!formats.length) {
      showError(tr("この動画は再生できません (この環境では、変換しながらの再生に対応していません)"));
      return;
    }
    let plan: VideoPlan;
    try {
      plan = known ?? (await invoke<VideoPlan>("video_play_plan", { id: v.id, formats, forceStream: true }));
    } catch (e) {
      if (my === token) showError(String(e));
      return;
    }
    if (my !== token) return;
    // ファイルから長さを読めなければ、一覧の長さ
    if (!plan.durationMs) plan.durationMs = v.durationMs;
    if (!plan.mime || !MediaSource.isTypeSupported(plan.mime)) {
      showError(tr("この動画は再生できません"));
      return;
    }
    stream = new Stream(video, v.id, plan, (m) => my === token && showError(m));
    await stream.open(at);
    if (my === token) video.play().catch(() => undefined);
  };

  const show = async () => {
    const v = list[i];
    if (!v) return;
    const my = ++token;
    reset();
    title.textContent = titleOf(v);
    count.textContent = list.length > 1 ? `${i + 1} / ${list.length}` : "";
    let plan: VideoPlan;
    try {
      plan = await invoke<VideoPlan>("video_play_plan", { id: v.id, formats: streamFormats(), forceStream: false });
    } catch (e) {
      if (my === token) showError(String(e));
      return;
    }
    if (my !== token) return;
    if (!plan.native) return playStream(v, my, 0, plan);
    // ファイルのまま。再生できなければ (WebKitGTK に H.264 が無いなど) 変換して流す
    video.src = videoUrl(v.id);
    video.addEventListener(
      "error",
      () => {
        if (my !== token || stream) return;
        const at = video.currentTime;
        void playStream(v, my, at);
      },
      { once: true },
    );
    video.play().catch(() => undefined);
  };

  // 変換して流している動画が読めなかったとき (ファイルのままのときは、上で変換して流す方に切り替える)
  video.addEventListener("error", () => {
    if (stream) showError(tr("この動画は再生できません"));
  });
  video.addEventListener("waiting", () => frame.classList.add("loading"));
  video.addEventListener("playing", () => frame.classList.remove("loading"));
  video.addEventListener("canplay", () => frame.classList.remove("loading"));
  // 枠の縦横比は動画に合わせる (読めるまでは 16:9)
  video.addEventListener("loadedmetadata", () => {
    if (video.videoWidth && video.videoHeight) back.style.setProperty("--ar", String(video.videoWidth / video.videoHeight));
    renderTime();
  });
  video.addEventListener("timeupdate", renderTime);
  video.addEventListener("durationchange", renderTime);
  video.addEventListener("play", () => {
    renderToggle();
    wake();
  });
  video.addEventListener("pause", () => {
    renderToggle();
    wake();
  });
  video.addEventListener("volumechange", renderVolume);
  video.addEventListener("click", togglePlay);
  video.addEventListener("dblclick", () => void setFull(!full));
  frame.addEventListener("mousemove", wake);
  // 枠の外へ出たら、再生中は操作をすぐ隠す
  frame.addEventListener("mouseleave", () => {
    clearTimeout(idleTimer);
    if (!video.paused) frame.classList.remove("awake");
  });
  toggle.addEventListener("click", togglePlay);
  q(".vp-big").addEventListener("click", togglePlay);
  // 位置: 動かしている間は時刻だけ変え、離したときに飛ぶ (変換して流すときに何度も変換し直さない)
  seek.addEventListener("input", () => {
    seeking = true;
    const p = Number(seek.value) / 10;
    seek.style.setProperty("--p", `${p}%`);
    if (Number.isFinite(video.duration)) cur.textContent = fmt((p / 100) * video.duration);
  });
  seek.addEventListener("change", () => {
    seeking = false;
    if (Number.isFinite(video.duration)) video.currentTime = (Number(seek.value) / 1000) * video.duration;
  });
  muteBtn.addEventListener("click", () => {
    video.muted = !video.muted;
    setPlayerVolume(video.volume, video.muted, false);
  });
  vol.addEventListener("input", () => {
    video.volume = Number(vol.value) / 100;
    video.muted = false;
    setPlayerVolume(video.volume, false, false);
  });
  vol.addEventListener("change", () => setPlayerVolume(video.volume, false, true));
  fsBtn.addEventListener("click", () => void setFull(!full));

  const go = (d: number) => {
    i = (i + d + list.length) % list.length;
    void show();
  };
  const close = () => {
    token++;
    reset();
    void setFull(false);
    clearTimeout(idleTimer);
    back.remove();
    document.removeEventListener("keydown", onKey, true);
    window.removeEventListener("resize", onResize);
    onClose?.();
  };
  const onKey = (e: KeyboardEvent) => {
    const onRange = (e.target as HTMLElement).matches?.('input[type="range"]');
    if (e.key === "Escape") {
      // 全画面のときの Esc は全画面を抜けるだけ
      if (full) void setFull(false);
      else close();
    } else if (e.key === " ") togglePlay();
    else if (e.key === "ArrowLeft" && !onRange) video.currentTime = Math.max(0, video.currentTime - 10);
    else if (e.key === "ArrowRight" && !onRange) video.currentTime = Math.min(video.duration || Infinity, video.currentTime + 10);
    else if (e.key === "f" || e.key === "F") void setFull(!full);
    else if (e.key === "m" || e.key === "M") muteBtn.click();
    else return;
    e.preventDefault();
    e.stopPropagation();
    wake();
  };
  q(".prev").addEventListener("click", () => go(-1));
  q(".next").addEventListener("click", () => go(1));
  q(".vis-viewer-close").addEventListener("click", close);
  back.addEventListener("mousedown", (e) => {
    if (e.target === back) close();
  });
  document.addEventListener("keydown", onKey, true);
  window.addEventListener("resize", onResize);
  renderToggle();
  renderVolume();
  renderFs();
  $("modal-root").appendChild(back);
  void show();
}
