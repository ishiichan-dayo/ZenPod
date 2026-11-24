// 上の帯: iPod の名前・容量・モードの切り替え中の表示

import { t as tr } from "../i18n";
import { isRockbox, writable } from "../app/selectors";
import { $, state } from "../app/state";
import { fmtBytes } from "../domain/format";
import type { Snapshot } from "../domain/types";
import { checkingDisk, ejectedIpod, waitedTooLong, waitingForDrive, waitingWithSpinner } from "./device";
import { esc } from "./dom";
import { placeSegThumb } from "./segments";
import { openSettings } from "./settings";

/**
 * 容量のバー: 使用中の部分を音楽・写真・アートワーク・その他に色分けする。
 * マウスを乗せると、色・名前・容量・割合の表を出す (色だけに頼らない)
 */
function renderCapacity(s: Snapshot) {
  const d = s.device;
  const total = d.totalBytes;
  const used = Math.max(0, total - d.freeBytes);
  const u = s.usage;
  const known = u.music + u.photos + (u.videos ?? 0) + u.artwork;
  // 曲のサイズは DB の値なので、ファイルシステムの使用量をわずかに超えることがある
  const parts = [
    { k: "music", label: tr("音楽"), bytes: Math.min(u.music, used) },
    { k: "videos", label: tr("@side:ビデオ"), bytes: u.videos ?? 0 },
    { k: "photos", label: tr("写真"), bytes: u.photos },
    { k: "artwork", label: tr("アートワーク"), bytes: u.artwork },
    { k: "other", label: tr("その他"), bytes: Math.max(0, used - known) },
  ];
  const pct = (b: number) => (total ? (b / total) * 100 : 0);
  $("cap-used").style.width = `${pct(used).toFixed(1)}%`;
  // 小さすぎて見えない区切りは出さない (隙間だけが並ぶのを避ける。表には出す)
  $("cap-used").innerHTML = parts
    .filter((p) => pct(p.bytes) >= 0.3)
    .map((p) => `<i class="cap-seg" data-k="${p.k}" style="flex-grow:${p.bytes}"></i>`)
    .join("");
  $("cap-text").textContent = total ? tr("{free} 空き / {total}", { free: fmtBytes(d.freeBytes), total: fmtBytes(total) }) : "";
  const row = (k: string, label: string, bytes: number) =>
    `<div class="cap-row"><span class="cap-sw" data-k="${k}"></span><span>${esc(label)}</span><span class="cap-size">${fmtBytes(bytes)}</span><span class="cap-pct">${pct(bytes).toFixed(pct(bytes) < 10 ? 1 : 0)}%</span></div>`;
  $("cap-pop").innerHTML = total
    ? parts
        .filter((p) => p.bytes > 0 || p.k === "music")
        .map((p) => row(p.k, p.label, p.bytes))
        .join("") + row("free", tr("空き"), d.freeBytes)
    : "";
}

/**
 * モードの切り替え (と開き直し) で読み込んでいる間、今の一覧の上に「〜で読み込んでいます」を重ねる。
 * 初めて Rockbox モードで開くときは全曲のタグを読むので、読んだ数も出す
 */
export function renderSwitchOverlay() {
  // 初めて開く途中 (読めた曲から一覧に出している) は重ねない。進み具合は左上に出す
  const on = state.opening && !!state.snap && !state.partialOpen;
  $("switch-overlay").classList.toggle("hidden", !on);
  if (!on) return;
  const name = state.openingMode === "rockbox" ? "Rockbox" : "iPod OS";
  $("switch-title").textContent = tr("{a} で読み込んでいます…", { a: name });
  const p = state.openProgress;
  $("switch-sub").textContent = p
    ? tr("曲の情報を読んでいます {n} / {a} 曲", { n: p[0].toLocaleString(), a: p[1].toLocaleString() })
    : state.openingMode === "rockbox"
      ? tr("初めて開くときは、曲の数が多いと時間がかかります")
      : "";
}

export function renderHeader() {
  // 開発版の「ドライブが出ない状態を真似る」の間は、開いている iPod が無いものとして描く
  const s = state.devNoDrive ? null : state.snap;
  const top = document.querySelector<HTMLElement>(".topbar")!;
  top.classList.toggle("no-device", !s);
  // 読み込み中と、USB で認識してからドライブとして使えるようになるまで (準備中) は、絵を動かして回る印を出す
  const waiting = !s && !state.opening && waitingForDrive();
  // Mac がディスクを検査している間も (終われば出る)。取り出し済みは出さない
  const pending = waiting && waitingWithSpinner();
  top.classList.toggle("loading", !s && (state.opening || pending));
  $("btn-open-root").classList.toggle("hidden", !!s || state.opening);
  // モードの切り替えで読み込み直している間も出す
  $("device-spin").classList.toggle("hidden", !(state.opening || pending));
  // 絵の iPod の画面を、Rockbox のときは暗い画面 (メニューの線付き) にする
  top.classList.toggle("rockbox", isRockbox());
  // 名前は iTunesDB にあるので変えられるのは iPod OS だけ。変えられることに気づけるよう、変えられるときは鉛筆のボタンを出す
  const renamable = !!s && writable() && !isRockbox();
  $("device-name").title = renamable ? tr("ダブルクリックで名前を変更") : "";
  $("btn-rename").classList.toggle("hidden", !renamable);
  const seg = $("mode-seg");
  // Rockbox が入っていない iPod では切り替えを出さない (Rockbox モードで開いているときは戻れるように出す)
  seg.classList.toggle("hidden", !s || !(s.hasRockbox || s.mode === "rockbox"));
  // 読み込んでいる間は切り替えられない
  seg.querySelectorAll<HTMLInputElement>('input[name="lib-mode"]').forEach((r) => (r.disabled = state.opening));
  seg.classList.toggle("busy", state.opening);
  renderSwitchOverlay();
  if (s) {
    // 切り替えて読み込んでいる間は、切り替え先を選んだ表示にする
    const shown = state.opening && state.openingMode ? state.openingMode : s.mode;
    seg.querySelectorAll<HTMLInputElement>('input[name="lib-mode"]').forEach((r) => (r.checked = r.value === shown));
    placeSegThumb(seg);
  }
  if (!s) {
    $("device-name").textContent = state.opening ? tr("iPod を読み込んでいます…") : waiting ? tr("iPod を認識しました") : tr("iPod 未接続");
    $("device-sub").textContent = state.opening
      ? tr("曲の数が多いと少し時間がかかります")
      : waiting
        ? ejectedIpod()
          ? tr("iPod は取り出されています")
          : checkingDisk()
          ? tr("Mac がディスクを検査しています")
          : waitedTooLong()
          ? tr("ドライブとして表示されません")
          : tr("ドライブとして使えるようになるまで待っています…")
        : tr("接続すると自動で読み込みます");
    $("warn").classList.add("hidden");
    $<HTMLButtonElement>("btn-add").disabled = true;
    return;
  }
  const d = s.device;
  $("device-name").textContent = s.name;
  const sub = [d.modelName, tr("{n} 曲", { n: s.tracks.length })];
  if (isRockbox()) {
    // 版は rockbox-info.txt から
    const v = s.rockbox?.version;
    sub.push(v ? `Rockbox ${v}` : "Rockbox");
  }
  // 開く途中は、曲数の代わりに進み具合 (前回の一覧を出している間は、iPod の中をたどっている)
  const p = state.openProgress;
  $("device-sub").textContent = !state.partialOpen
    ? sub.join(" · ")
    : p
      ? tr("曲の情報を読んでいます {n} / {a} 曲", { n: p[0].toLocaleString(), a: p[1].toLocaleString() })
      : state.partialCached
        ? tr("iPod の中の曲を確かめています…")
        : tr("曲の情報を読んでいます…");
  renderCapacity(s);

  const warn = $("warn");
  if (s.writableError) {
    warn.classList.remove("hidden");
    const needGuid = d.checksum === "hash58" && !d.firewireId;
    warn.innerHTML = `<span>⚠ ${esc(tr("読み取り専用: {reason}", { reason: s.writableError ?? "" }))}</span>${needGuid ? `<button class="btn mini" id="btn-warn-guid">${esc(tr("FireWire GUID を入力"))}</button>` : ""}`;
    document.getElementById("btn-warn-guid")?.addEventListener("click", () => {
      state.settingsCat = "ipod";
      openSettings();
    });
  } else {
    warn.classList.add("hidden");
  }
  $<HTMLButtonElement>("btn-add").disabled = !writable() || state.transferring;
}
