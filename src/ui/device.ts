// iPod を探す・開く・モード (iPod OS / Rockbox) を切り替える・名前を変える・つなぐ前の案内

import { t as tr } from "../i18n";
import { invoke, listen } from "../infra/tauri";
import { analyticsOpen, analyticsSeen, analyticsUsbNoDrive } from "../app/analytics";
import { isPcView, isRockbox, writable } from "../app/selectors";
import { $, state } from "../app/state";
import { rankCache } from "../domain/track-order";
import type { DeviceEntry, DeviceInfo, LibMode, Snapshot, Track } from "../domain/types";
import { getStored, setStored } from "../infra/prefs";
import { artCache } from "./artwork-cells";
import { esc } from "./dom";
import { call, toast } from "./feedback";
import { renderHeader, renderSwitchOverlay } from "./header";
import { promptText } from "./modal";
import { offerRockboxDb } from "./onboarding";
import { offerPodcastSync } from "./podcast-sync";
import { applySnapshot, renderAll } from "./render";
import { applyRockboxLayout } from "./rockbox-layout";

/** iPod ごとに選んだモード (場所 → モード)。選んでいなければ、Rockbox が入っていれば Rockbox で開く */
function storedMode(root: string): LibMode | null {
  try {
    const v = (JSON.parse(getStored("libModes") ?? "{}") as Record<string, string>)[root];
    return v === "ipod" || v === "rockbox" ? v : null;
  } catch {
    return null;
  }
}

function setStoredMode(root: string, mode: LibMode) {
  let map: Record<string, string> = {};
  try {
    map = JSON.parse(getStored("libModes") ?? "{}");
  } catch {
    /* 壊れていたら作り直す */
  }
  map[root] = mode;
  setStored("libModes", JSON.stringify(map));
}

/** モードを切り替えて iPod を開き直す */
export async function switchMode(mode: LibMode) {
  const s = state.snap;
  if (!s || s.mode === mode) return;
  if (state.transferring) {
    toast(tr("転送中は切り替えられません"), "error");
    renderHeader();
    return;
  }
  setStoredMode(s.device.root, mode);
  await openDevice(s.device.root, false, mode);
}


export async function pollDevices() {
  // はじめの案内の後ろで読み込みが始まらないように、閉じるまで待つ (もう開いている iPod はそのまま)
  if (state.polling || (state.onboarding && !state.snap)) return;
  state.polling = true;
  try {
    await pollDevicesInner();
  } finally {
    state.polling = false;
  }
}

async function pollDevicesInner() {
  if (state.opening || state.transferring || state.ejecting) return;
  if (state.snap) {
    // 開いている iPod があるなら、外したあとは「探しています」ではなく「接続してください」でよい
    state.connectChecked = true;
    const present = await invoke<boolean>("device_present").catch(() => false);
    if (!present) {
      await invoke("close_device");
      applySnapshot(null);
      toast(tr("iPod が取り外されました"));
    }
    return;
  }
  const list = await invoke<DeviceEntry[]>("scan_devices").catch(() => [] as DeviceEntry[]);
  if (state.ejectedRoot && !list.some((d) => d.root === state.ejectedRoot)) state.ejectedRoot = null;
  if (state.failedRoot && !list.some((d) => d.root === state.failedRoot)) state.failedRoot = null;
  state.devices = list.filter((d) => d.root !== state.ejectedRoot);
  if (list.length) analyticsSeen.drive = true;
  if (!state.connectChecked) {
    state.connectChecked = true;
    renderAll();
  } else if (!state.snap) {
    renderConnectView();
  }
  if (state.devices.length === 1 && state.devices[0].root !== state.failedRoot) await openDevice(state.devices[0].root, true);
}

/** USB でつながってからドライブとして見えるまでに、これより長くかかったら案内を出す */
export const USB_WAIT_HINT_MS = 20_000;

/** USB でつながったが、まだドライブとして見えていない (Windows がディスクを準備している間など) */
/** USB で認識してから USB_WAIT_HINT_MS たってもドライブが出ない (直し方の案内を出す段階。回る印は消す) */
export function waitedTooLong(): boolean {
  return waitingForDrive() && Date.now() - state.usbSince >= USB_WAIT_HINT_MS;
}

export function waitingForDrive(): boolean {
  if (state.devNoDrive) return true;
  return state.usbSince > 0 && !state.usbEjected && !state.snap && !state.devices.length && state.ejectedRoot === null;
}

/** ドライブが出ない理由を調べ始めるまでの時間 (つないだ直後は、ふつうに準備している間なので) */
const DRIVE_CHECK_AFTER_MS = 3000;
let driveCheck = 0;
/** 最後に調べた理由 (案内に出すのは、取り出し済みが 2 回続いてから) */
let lastReason: string | null = null;

/**
 * USB でつながってからドライブが出るまでの間、理由 (取り出し済み・Mac のディスクの検査中・読む許可が無い) を 2 秒ごとに調べて
 * 案内を変える。「取り出し済み」は、つないだ直後の準備中と取り違えないよう、2 回続けて同じだったときだけ出す
 */
function watchDriveWait() {
  clearTimeout(driveCheck);
  lastReason = null;
  state.driveWaitReason = null;
  const tick = async () => {
    if (!waitingForDrive()) return;
    if (Date.now() - state.usbSince >= DRIVE_CHECK_AFTER_MS) {
      const reason = await invoke<string | null>("drive_wait_reason").catch(() => null);
      if (!waitingForDrive()) return;
      const shown = reason === "ejected" && lastReason !== "ejected" ? state.driveWaitReason : reason;
      lastReason = reason;
      if (shown !== state.driveWaitReason) {
        state.driveWaitReason = shown;
        if (!state.snap && !state.opening) {
          renderConnectView();
          renderHeader();
        }
      }
    }
    driveCheck = window.setTimeout(tick, 2000);
  };
  driveCheck = window.setTimeout(tick, 1000);
}

/** 取り出したあと、ケーブルを挿したままの iPod (挿し直すまでドライブは出ないので、待たずに案内する) */
export function ejectedIpod(): boolean {
  return waitingForDrive() && state.driveWaitReason === "ejected";
}

/** Mac がディスクを検査している (終わればドライブが出るので、待っている表示のままにする) */
export function checkingDisk(): boolean {
  return waitedTooLong() && state.driveWaitReason === "fsck";
}

/** 回る印を出して待つ段階か (ドライブが出るのを待っている・Mac がディスクを検査している) */
export function waitingWithSpinner(): boolean {
  return waitingForDrive() && !ejectedIpod() && (!waitedTooLong() || checkingDisk());
}

/** 未接続・読み込み中の案内 (iPod の一覧の代わりに表示) */
export function renderConnectView() {
  const loading = state.opening;
  const waiting = !loading && waitingForDrive();
  $("connect-title").textContent = loading
    ? tr("iPod を読み込んでいます…")
    : waiting
      ? ejectedIpod()
        ? tr("iPod は取り出されています")
        : checkingDisk()
        ? tr("Mac がディスクを検査しています")
        : waitedTooLong()
          ? tr("ドライブとして表示されません")
          : tr("iPod を認識しました")
      : state.connectChecked
        ? tr("iPod を接続してください")
        : tr("iPod を探しています…");
  $("connect-text").classList.toggle("hidden", loading || waiting);
  $("connect-actions").classList.toggle("hidden", loading);
  const wait = $("connect-wait");
  wait.classList.toggle("hidden", !waiting);
  // しばらく待ってもドライブが出ないときは、直し方の案内を出して、回る印は消す (待てば出る段階ではないので)。
  // Mac がディスクを検査しているときは、終われば出るので回る印のまま。取り出し済みは待っても出ないので、すぐ案内する
  const long = !waitingWithSpinner();
  if (waiting) {
    // Windows では、Mac 形式 (HFS+) の iPod はドライブとして表示されない (よくある原因なので、直し方まで出す)
    const win = navigator.userAgent.includes("Windows");
    let html: string;
    if (ejectedIpod()) {
      html = esc(tr("取り出したあと、ケーブルを挿したままになっています。使うときは、ケーブルを抜いて挿し直してください。"));
    } else if (checkingDisk()) {
      html = esc(tr("取り出さずに外したあとなどに、Mac が iPod のディスクを検査します。容量が大きいと数分かかります。終わるとドライブとして表示されるので、ケーブルを抜かずにお待ちください。"));
    } else if (!long) {
      html = esc(tr("ドライブとして使えるようになるまで待っています…"));
    } else if (state.driveWaitReason === "denied") {
      html = esc(tr("ZenPod に iPod の中を読む許可がありません。システム設定の「プライバシーとセキュリティ」→「ファイルとフォルダ」で、ZenPod の「リムーバブルボリューム」をオンにしてください。"));
    } else {
      html = `${esc(tr("ドライブとして表示されるまで、しばらくかかることがあります。iPod の画面に「接続解除してもかまいません」と出ているときは、取り出し済みです。ケーブルを抜いて挿し直してください。挿し直しても表示されない場合は、iPod を再起動してください。"))}${
        win
          ? `<span class="connect-hfs">${esc(tr("それでも表示されないときは、iPod が Mac 形式 (HFS+) かもしれません。Mac 形式の iPod は Windows では読めません。iPod OS で使うなら、Windows の iTunes か Apple Devices で「復元」すると Windows 形式になります (iPod の中身は消えます)。Rockbox で使うなら、Rockbox Utility の案内に従ってください。"))}</span>`
          : ""
      }`;
    }
    if (wait.innerHTML !== html) wait.innerHTML = html;
  }
  const card = document.querySelector<HTMLElement>("#connect-view .empty-card")!;
  let spin = card.querySelector(".spinner");
  const spinning = loading || (waiting && waitingWithSpinner());
  if (spinning && !spin) {
    spin = document.createElement("div");
    spin.className = "spinner";
    card.appendChild(spin);
  } else if (!spinning) {
    spin?.remove();
  }
  // 開発版の「ドライブが出ない状態を真似る」の間は、つないでいる本物の iPod のボタンも出さない (本当にドライブが出ないときと同じに)
  const devices = state.devNoDrive ? [] : state.devices;
  const html = devices.map((d) => `<button class="btn" data-root="${esc(d.root)}">${esc(tr("{a} を開く", { a: d.label }))}</button>`).join("");
  // ボタンを押している最中に描き直さない
  if ($("device-list").innerHTML !== html) $("device-list").innerHTML = html;
}

/**
 * 初めて開く途中で読めた曲を、仮の一覧に足す (Rockbox を初めて開くときは、読み終わるまで何十秒もかかることがあるため)。
 * モードの切り替え (前の一覧が出ている) では使わない
 */
function addPartialTracks(p: { device: DeviceInfo; name: string; tracks: Track[]; cached: boolean }) {
  if (!state.opening || (state.snap && !state.partialOpen)) return;
  if (p.cached) state.partialCached = true;
  if (!state.partialOpen) {
    state.partialOpen = true;
    state.snap = {
      device: p.device,
      name: p.name,
      tracks: [],
      playlists: [],
      writableError: null,
      usage: { music: 0, photos: 0, artwork: 0 },
      mode: "rockbox",
      rockbox: null,
      hasRockbox: true,
      artworkSupported: false,
    };
    state.trackMap = new Map();
  }
  for (const t of p.tracks) {
    state.snap!.tracks.push(t);
    state.trackMap.set(t.id, t);
    state.snap!.usage.music += t.size;
  }
  rankCache.clear();
  // 続けて届いても、描き直すのは 1 フレームに 1 回
  if (!partialFrame)
    partialFrame = requestAnimationFrame(() => {
      partialFrame = 0;
      if (state.partialOpen) renderAll();
    });
}
let partialFrame = 0;

/** mode を省くと、前に選んだモード (無ければ Rockbox が入っているかで決める) で開く */
export async function openDevice(root: string, auto = false, mode?: LibMode) {
  state.opening = true;
  state.openingMode = mode ?? storedMode(root);
  state.openProgress = null;
  renderAll();
  try {
    const started = performance.now();
    const s = await call<Snapshot>("open_device", { root, mode: mode ?? storedMode(root) });
    if (s) analyticsOpen(s, performance.now() - started);
    if (s) analyticsSeen.opened = true;
    else analyticsSeen.failed = true;
    if (!s && auto) state.failedRoot = root;
    // 読めた曲から一覧に出していたときは、選んだ曲や検索をそのまま残す
    const shown = state.partialOpen;
    state.partialOpen = false;
    state.partialCached = false;
    if (!s && shown) applySnapshot(null);
    if (s) {
      state.failedRoot = null;
      artCache.clear();
      // PC のフォルダを見ている (編集中の) ときは画面を切り替えない
      if (!isPcView() && !shown) {
        state.view = "library";
        state.selected.clear();
        state.search = "";
        $<HTMLInputElement>("search").value = "";
      }
      state.opening = false;
      // 容量のバーを 0 から伸ばす
      const cap = $("cap-used");
      cap.style.transition = "none";
      cap.style.width = "0%";
      void cap.offsetWidth;
      cap.style.transition = "";
      applySnapshot(s);
      // モードを切り替えたときも分かるよう、どちらのモードで開いたかを入れる
      toast(tr("{a} を {b} で読み込みました", { a: s.name, b: s.mode === "rockbox" ? "Rockbox" : "iPod OS" }), "info", 3500, "opened");
      // Rockbox の案内 (転送先の形・データベース) が先。そのあとでポッドキャストの同期を確かめる
      void (async () => {
        if (isRockbox()) {
          await applyRockboxLayout(s);
          await offerRockboxDb(s);
        }
        await offerPodcastSync();
      })();
    }
  } finally {
    state.openingMode = null;
    state.openProgress = null;
    state.partialCached = false;
    if (state.opening) {
      state.opening = false;
      renderAll();
    } else renderSwitchOverlay();
  }
}

/**
 * iPod が USB でつながったらすぐに知らせ、ドライブとして見えるまでは早めに (0.5 秒ごとに) 探す。
 * つながってからドライブになるまで数秒〜十数秒かかり、その間なにも出ないと認識されていないように見えるため
 */
export async function watchUsbIpod() {
  let fast = 0;
  const onChange = (connected: boolean) => {
    const was = state.usbSince > 0;
    state.usbSince = connected ? state.usbSince || Date.now() : 0;
    if (connected) analyticsSeen.usb = true;
    if (!connected) state.usbEjected = false;
    if (connected && !was) watchDriveWait();
    if (connected && !was && !state.snap) {
      // PC のフォルダを見ているときは案内の画面が見えないので、通知でも知らせる
      if (isPcView()) toast(tr("iPod を認識しました。読み込めるようになるまでお待ちください…"));
      clearInterval(fast);
      let reported = false;
      fast = window.setInterval(() => {
        // USB では認識したのに、しばらくたってもドライブが出ない (取り出し済み・Windows の Mac 形式の iPod など)。
        // 利用状況に 1 日 1 回。理由は watchDriveWait が最後に調べたもの
        if (!reported && waitingForDrive() && Date.now() - state.usbSince >= USB_WAIT_HINT_MS) {
          reported = true;
          analyticsUsbNoDrive(lastReason);
        }
        if (!waitingForDrive() || Date.now() - state.usbSince > 60_000) {
          clearInterval(fast);
          // 待つのをやめたら、案内も消す (つながっていれば、以降は定期の確認で見つける)
          if (!state.snap) {
            renderConnectView();
            renderHeader();
          }
          return;
        }
        pollDevices();
        // 経過時間で文言が変わるので描き直す
        if (!state.snap && !state.opening) {
          renderConnectView();
          renderHeader();
        }
      }, 500);
    }
    if (!state.snap) {
      renderConnectView();
      renderHeader();
    }
  };
  await listen<boolean>("usb-ipod", (ev) => onChange(ev.payload));
  // 開いている間のタグの読み取りの進み具合
  await listen<[number, number]>("open-progress", (ev) => {
    state.openProgress = ev.payload;
    renderSwitchOverlay();
    if (state.partialOpen) renderHeader();
  });
  await listen<{ device: DeviceInfo; name: string; tracks: Track[]; cached: boolean }>("open-partial", (ev) => addPartialTracks(ev.payload));
  // Rockbox を初めて開いたときにデータベースから埋めた曲を、裏でファイルから読み直した (並べ替え用のタグ・埋め込みのジャケット)。
  // 転送中は、終わったときに一覧を読み直すので待たない
  await listen<number>("rockbox-reread-done", async () => {
    if (!state.snap || state.opening || state.transferring) return;
    const snap = await invoke<Snapshot | null>("snapshot").catch(() => null);
    if (snap && !state.opening && !state.transferring) applySnapshot(snap);
  });
  onChange(await invoke<boolean>("usb_ipod_connected").catch(() => false));
}

export async function renameIpod() {
  if (!state.snap || !writable() || isRockbox()) return;
  const old = state.snap.name;
  await promptText(tr("iPod の名前"), old, tr("保存"), async (name) => {
    if (name === old) return true;
    const s = await call<Snapshot>("rename_ipod", { name });
    if (s) applySnapshot(s);
    return !!s;
  });
}
