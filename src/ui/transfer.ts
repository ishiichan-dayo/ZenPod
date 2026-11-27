// 転送: 重複の扱い・変換先の確認・転送の経過の表示

import { t as tr } from "../i18n";
import { invoke, listen, openDialog } from "../infra/tauri";
import { analyticsTransfer } from "../app/analytics";
import { isPodView, pod } from "../app/podcasts";
import { currentPlaylist, editablePlaylist, isPcView, isRockbox, writable } from "../app/selectors";
import { $, AUDIO_EXTS, state } from "../app/state";
import type { Progress, Snapshot, TransferSummary } from "../domain/types";
import { getStored, pref, setPref, setStored } from "../infra/prefs";
import { hintAutoresume } from "./dialogs/edit-tags";
import { esc } from "./dom";
import { call, justDone, toast } from "./feedback";
import { renderHeader } from "./header";
import { ICON_LIB } from "./icons";
import { modal } from "./modal";
import { renderPodStatuses } from "./podcasts";
import { applySnapshot } from "./render";
import { renderRows } from "./rows";
import { renderSidebar } from "./sidebar";
import { artSize, artSizeHtml, bindArtSize } from "./art-size";
import { albumChain, isPhotoView, isVideoView, vis } from "../app/visual";
import { reloadVisual } from "./visual";

/** ファイルを選ぶときの種類 */
// tr: 使う側で訳す
const PICK_KINDS = {
  audio: { title: "転送する曲", filter: "音声ファイル", exts: AUDIO_EXTS },
  video: { title: "転送する動画", filter: "動画ファイル", exts: ["mp4", "m4v", "mov", "mkv", "avi", "webm", "wmv", "flv", "mpg", "mpeg", "ts", "m2ts", "3gp", "ogv"] },
  photo: { title: "転送する写真", filter: "画像ファイル", exts: ["jpg", "jpeg", "png", "heic", "heif", "bmp", "gif", "webp", "tif", "tiff"] },
};

/** 写真の表示でアルバムを開いていれば、その場所 ("旅行/2024"。落とした写真をそのアルバムに入れる) */
export function dropPhotoAlbum(): string | null {
  if (!isPhotoView() || !vis.album) return null;
  const chain = albumChain(vis.album);
  return chain.length ? chain.map((a) => a.name).join("/") : null;
}

/**
 * ビデオの表示で開いているところに合わせた、落とした動画の種類・番組・アーティスト
 * (ムービー → ムービー、テレビ番組 / 番組 → テレビ番組 (番組名も)、ミュージックビデオ / アーティスト → ミュージックビデオ (アーティストも))。
 * すべてのビデオ・ほかの表示では指定しない (動画のタグのまま)
 */
export function dropVideoPlacement(): { kind?: string; show?: string; artist?: string } {
  if (!isVideoView()) return {};
  const node = vis.videoNode;
  const val = node.slice(2);
  if (node.startsWith("k:")) return { kind: val };
  if (node.startsWith("s:")) return { kind: "tvshow", show: val };
  if (node.startsWith("a:")) return { kind: "musicvideo", artist: val };
  return {};
}

/** iPod に同じ曲があるとき: "skip" (スキップ) / "replace" (上書き) / "keep" (両方残す) */
export type DuplicatePolicy = "skip" | "replace" | "keep";

export function duplicatePolicy(): DuplicatePolicy {
  const v = getStored("duplicates");
  if (v === "skip" || v === "replace" || v === "keep") return v;
  // 以前の「同じ曲はスキップする」のチェックを引き継ぐ
  return getStored("skipDuplicates") === "0" ? "keep" : "skip";
}

/** `policy` を渡すと、その転送だけ設定の代わりに使う (右クリックの「同じ曲は上書き」など)。`convertAllTo` は「変換して転送」で
 * 選んだ形式 ("aac:192" など。ロスレスの曲と、それより高いビットレートの曲をその形式にする)。転送を始めたら true */
/** 転送の完了の知らせを受けた回数 (完了が add_paths の返事より先に届いたかを見分ける) */
let transfersDone = 0;

/** 設定の「転送するとき、いつも変換する」で選んだ形式 ("aac:256" など)。しないなら undefined (配布リポジトリの Issue #16) */
export function alwaysConvertTo(): string | undefined {
  const v = getStored("convertAlways");
  return v && CONVERT_LOSSY.some(([k]) => k === v) ? v : undefined;
}

export async function startTransfer(
  paths: string[],
  playlistId: string | null,
  policy: DuplicatePolicy = duplicatePolicy(),
  // 省略すると設定の「いつも変換する」。null なら変換しない (ポッドキャストの同期)
  convertAllTo: string | null | undefined = alwaysConvertTo(),
): Promise<boolean> {
  if (!state.snap) {
    toast(tr("iPod を接続してください"));
    return false;
  }
  if (!writable()) {
    toast(state.snap.writableError ?? tr("書き込みできません"), "error");
    return false;
  }
  if (state.transferring) {
    toast(tr("転送中です。完了後にもう一度ドロップしてください"), "error");
    return false;
  }
  // 初めての転送: これからの転送で埋め込み画像を縮小するかと、その大きさを選んでもらう (1 回だけ。設定の「転送」で変えられる)
  if (getStored("shrinkArtworkAsked") !== "1" && !(await askShrinkArtwork())) return false;
  // iPod OS は再生できない形式を変換して送る。変換がある転送では、変換先を確かめてもらう (「次回から確認しない」まで)。
  // 「変換して転送」は形式を選んだばかりなので聞かない (設定の「いつも変換する」も形式を選んであるので聞かない)
  if (!isRockbox() && !convertAllTo && pref("confirmConvert")) {
    const n = await call<{ lossless: number; lossy: number }>("count_conversions", { paths });
    if (n && n.lossless + n.lossy > 0 && !(await confirmConvert(n))) return false;
  }
  const doneBefore = transfersDone;
  const total = await call<number>("add_paths", {
    paths,
    playlistId,
    options: {
      skipDuplicates: policy !== "keep",
      replaceDuplicates: policy === "replace",
      autoReading: pref("autoReading"),
      artwork: pref("artwork"),
      shrinkArtwork: pref("shrinkArtwork"),
      artworkSize: artSize(),
      losslessTo: getStored("convertLossless"),
      lossyTo: getStored("convertLossy"),
      convertAllTo: convertAllTo ?? undefined,
      videoSize: getStored("videoSize") ?? "640",
      photoAlbum: dropPhotoAlbum(),
      videoPlacement: dropVideoPlacement(),
    },
  });
  if (total == null) return false;
  // 重複だけのようにすぐ終わる転送は、返事より先に完了の知らせが届く。そのときは「転送中」にしない
  // (しておくと帯が残り、次の転送を始められなくなる)
  if (transfersDone !== doneBefore) return true;
  state.transferring = true;
  state.transferAdded = 0;
  renderSidebar();
  $("transfer").classList.remove("hidden");
  $("transfer-label").textContent = tr("転送中 0 / {n}", { n: total });
  $("transfer-fill").style.width = "0%";
  $("transfer-pct").textContent = "0%";
  setTransferArt(null);
  renderHeader();
  return true;
}

export async function pickAndTransfer(directory: boolean, kind: keyof typeof PICK_KINDS = "audio") {
  const k = PICK_KINDS[kind];
  const sel = await openDialog(
    directory
      ? { directory: true, multiple: true, title: tr("転送するフォルダ") }
      : { multiple: true, title: tr(k.title), filters: [{ name: tr(k.filter), extensions: k.exts }] },
  );
  if (!sel) return;
  const paths = Array.isArray(sel) ? sel : [sel];
  const pl = currentPlaylist();
  await startTransfer(paths, pl && editablePlaylist(pl.id) ? pl.id : null);
}


/** 変換先の選択肢: [設定の値, 表示名, 4 分の曲の容量の目安] */
// tr: 使う側で訳す
export const CONVERT_LOSSLESS: [string, string, string][] = [
  ["alac", "ALAC (Apple Lossless・劣化なし)", "約 25〜35MB"],
  ["aac:320", "AAC 320kbps", "約 9.6MB"],
  ["aac:256", "AAC 256kbps", "約 7.7MB"],
  ["mp3:320", "MP3 320kbps", "約 9.6MB"],
];
// tr: 使う側で訳す
export const CONVERT_LOSSY: [string, string, string][] = [
  ["aac:256", "AAC 256kbps", "約 7.7MB"],
  ["aac:192", "AAC 192kbps", "約 5.8MB"],
  ["aac:128", "AAC 128kbps", "約 3.8MB"],
  ["mp3:320", "MP3 320kbps", "約 9.6MB"],
  ["mp3:192", "MP3 192kbps", "約 5.8MB"],
];

export function convertOptions(list: [string, string, string][], current: string): string {
  return list
    .map(([v, label, size]) => `<option value="${v}" ${v === current ? "selected" : ""}>${esc(tr(label))} — ${esc(tr(size))}</option>`)
    .join("");
}

/** 下のバーの、転送中の曲のジャケット。無ければ音符 */
function setTransferArt(url: string | null) {
  const el = $("transfer-art");
  el.style.backgroundImage = url ? `url("${url}")` : "";
  el.innerHTML = url ? "" : ICON_LIB;
}

/**
 * 変換して送る曲があるときの確認。変換先の形式を選んでもらい、選んだものを設定にも残す (設定の「変換」と同じ項目)。
 * 「次回から確認しない」にしたら、以後は設定の形式でそのまま変換する
 */
/** これからの転送で、曲に埋め込まれた画像を縮小するか (と大きさ) を選んでもらう。Esc などで閉じたら転送をやめる (false) */
function askShrinkArtwork(): Promise<boolean> {
  return new Promise((resolve) => {
    let ok = false;
    modal(
      `<h3>${esc(tr("曲に埋め込まれた画像を縮小しますか？"))}</h3>
       <p>${esc(
         isRockbox()
           ? tr("Rockbox は曲に埋め込まれた画像を先に読み、表示のたびに縮小するので、画像が大きいとアートワークの表示が遅くなります。")
           : tr("iPod OS は曲に埋め込まれた画像を使わないので、縮小すると iPod の容量を節約できます。iPod の画面のアートワークは変わりません。"),
       )} ${esc(tr("縮小するのは iPod に入れるコピーだけで、PC の元のファイルは変わりません。"))}</p>
       <div class="shrink-size"><label for="first-size">${esc(tr("縮小する大きさ (長い辺)"))}</label>${artSizeHtml("first-size")}</div>
       <div class="hint">${esc(tr("これからの転送すべてに使います。あとから設定の「転送」で変えられます。"))}</div>
       <div class="modal-actions"><button class="btn" data-x="no">${esc(tr("縮小しない"))}</button><button class="btn primary" data-x="yes">${esc(tr("縮小する"))}</button></div>`,
      (root, close) => {
        bindArtSize(root, "first-size");
        const choose = (on: boolean) => {
          setPref("shrinkArtwork", on);
          setStored("shrinkArtworkAsked", "1");
          ok = true;
          close();
        };
        const yes = root.querySelector<HTMLButtonElement>('[data-x="yes"]')!;
        yes.focus();
        yes.addEventListener("click", () => choose(true));
        root.querySelector('[data-x="no"]')!.addEventListener("click", () => choose(false));
      },
    ).then(() => resolve(ok));
  });
}

function confirmConvert(n: { lossless: number; lossy: number }): Promise<boolean> {
  return new Promise((resolve) => {
    let ok = false;
    const row = (id: string, label: string, count: number, list: [string, string, string][], current: string) =>
      count
        ? `<label for="${id}">${esc(label)} <span class="muted">${esc(tr("{n} 曲", { n: count.toLocaleString() }))}</span></label>
           <select id="${id}">${convertOptions(list, current)}</select>`
        : "";
    modal(
      `<h3>${esc(tr("変換して転送します"))}</h3>
       <p>${esc(tr("iPod で再生できない形式の曲があるので、変換してから転送します。変換後の形式を選んでください。"))}</p>
       <div class="set-convert cv-ask">
         ${row("cv-lossless", tr("可逆圧縮の曲 (FLAC / APE など)"), n.lossless, CONVERT_LOSSLESS, getStored("convertLossless") ?? "alac")}
         ${row("cv-lossy", tr("非可逆の曲 (Ogg / Opus / WMA など)"), n.lossy, CONVERT_LOSSY, getStored("convertLossy") ?? "aac:256")}
       </div>
       <div class="hint">${esc(tr("MP3 / AAC / ALAC / WAV / AIFF の曲は変換せずにそのまま転送します。容量は 4 分の曲 1 曲あたりの目安です。"))}</div>
       <label class="check rd-skip"><input type="checkbox" id="cv-skip"> ${esc(tr("次回から確認しない (設定の「変換」でいつでも変えられます)"))}</label>
       <div class="modal-actions"><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok">${esc(tr("転送する"))}</button></div>`,
      (root, close) => {
        root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
        root.querySelector('[data-x="ok"]')!.addEventListener("click", () => {
          const lossless = root.querySelector<HTMLSelectElement>("#cv-lossless");
          const lossy = root.querySelector<HTMLSelectElement>("#cv-lossy");
          if (lossless) setStored("convertLossless", lossless.value);
          if (lossy) setStored("convertLossy", lossy.value);
          if (root.querySelector<HTMLInputElement>("#cv-skip")!.checked) setPref("confirmConvert", false);
          ok = true;
          close();
        });
      },
    ).then(() => resolve(ok));
  });
}

// ---------------- 転送イベント ----------------

export async function bindTransferEvents() {
  await listen<Progress>("transfer-progress", (ev) => {
    const p = ev.payload;
    // 動画の変換中は、その動画の進み具合のぶんだけバーを進める
    const done = p.status === "converting" ? p.index + (p.percent ?? 0) : p.status === "working" ? p.index : p.index + 1;
    $("transfer-label").textContent =
      p.status === "converting"
        ? tr("動画を変換中 {p}% ({n} / {a})", { p: Math.floor((p.percent ?? 0) * 100), n: p.index + 1, a: p.total })
        : tr("転送中 {n} / {a}", { n: Math.floor(done), a: p.total });
    $("transfer-fill").style.width = `${(done / p.total) * 100}%`;
    $("transfer-pct").textContent = `${Math.floor((done / p.total) * 100)}%`;
    if (p.status === "working") setTransferArt(p.art ?? null);
    if (p.status === "converting") return;
    // PC の一覧: 終わった曲から「転送済み」にする (重複でスキップした曲も iPod にはある)
    // サイドバーの「ミュージック」の曲数を 1 曲ずつ増やす (上書き・重複は曲数が変わらない)
    if (p.status === "added") {
      state.transferAdded++;
      const master = state.snap?.playlists.find((x) => x.master);
      const count = document.querySelector<HTMLElement>('#lib-list [data-view="library"] .count');
      if (count && master) count.textContent = String(master.trackIds.length + state.transferAdded);
    }
    if (p.status === "working") state.sending.set(p.path, "working");
    else if (p.status === "error") state.sending.delete(p.path);
    else {
      state.sending.set(p.path, "done");
      justDone.set(p.path, Date.now() + 700);
    }
    if (isPcView() && !state.settingsOpen) renderRows();
    if (isPodView()) renderPodStatuses();
  });
  // Rockbox のデータベースを裏で作り直し終えた。失敗したときだけ知らせる
  await listen<{ tracks: number; error: string | null }>("rockbox-db-done", (ev) => {
    const rb = state.snap?.rockbox;
    if (rb) {
      rb.dbError = ev.payload.error;
      if (!ev.payload.error) rb.dbTracks = ev.payload.tracks;
    }
    if (ev.payload.error) toast(tr("Rockbox のデータベースを作れませんでした: {a}", { a: ev.payload.error }), "error", 10000);
  });
  await listen<TransferSummary>("transfer-done", async (ev) => {
    const s = ev.payload;
    transfersDone++;
    state.transferring = false;
    state.transferAdded = 0;
    $("transfer").classList.add("hidden");
    const snap = await invoke<Snapshot | null>("snapshot").catch(() => null);
    // チェックして送った曲のうち、転送できたもの (上書き・重複でスキップも iPod にある) はチェックを外す。
    // 失敗した曲はチェックを残して、送り直しやすくする
    for (const id of state.transferChecked) if (state.sending.get(id.slice(3)) === "done") state.selected.delete(id);
    state.transferChecked.clear();
    // ポッドキャストの回: 送れた回は緑の iPod をポンと出す (印は描いたあとで消す)
    for (const [p, id] of pod.paths) if (state.sending.get(p) === "done") pod.arrived.add(id);
    pod.paths.clear();
    if (pod.arrived.size) setTimeout(() => pod.arrived.clear(), 600);
    // 読み直した iPod の曲で判定できるので、転送中の印は消す
    state.sending.clear();
    applySnapshot(snap);
    analyticsTransfer(s);
    // 動画・写真を送ったら一覧を読み直す
    if (s.videos || s.photos) await reloadVisual(s.videos > 0, s.photos > 0);
    const visual = s.videos + s.photos > 0;
    const parts: string[] = [];
    if (s.added || !visual) parts.push(tr("{n} 曲を転送しました", { n: s.added }));
    if (s.videos) parts.push(tr("{n} 本の動画を転送しました", { n: s.videos }));
    if (s.photos) parts.push(tr("{n} 枚の写真を転送しました", { n: s.photos }));
    if (s.replaced) parts.push(tr("{n} 曲を上書き", { n: s.replaced }));
    if (s.duplicates) parts.push(visual ? tr("{n} 件は重複のためスキップ", { n: s.duplicates }) : tr("{n} 曲は重複のためスキップ", { n: s.duplicates }));
    if (s.cancelled) parts.push(tr("中止しました"));
    if (s.saveError) {
      await modal(
        `<h3>${esc(tr("データベースの保存に失敗しました"))}</h3><p>${esc(s.saveError)}</p><div class="modal-actions"><button class="btn primary" data-x="ok">OK</button></div>`,
        (root, close) => root.querySelector('[data-x="ok"]')!.addEventListener("click", close),
      );
    } else if (s.errors.length) {
      await modal(
        `<h3>${esc(parts.join(" / "))}</h3><p>${esc(tr("{n} 件のエラーがありました。", { n: s.errors.length }))}</p><pre class="errors">${esc(s.errors.join("\n"))}</pre>
         <div class="modal-actions"><button class="btn primary" data-x="ok">OK</button></div>`,
        (root, close) => root.querySelector('[data-x="ok"]')!.addEventListener("click", close),
      );
    } else {
      toast(parts.join(" / "));
    }
    if (snap?.tracks?.some((t) => t.kind && t.kind !== "music")) hintAutoresume();
    // Rockbox: データベースはアプリが裏で作り直す (結果は rockbox-db-done)。作れない版で自動更新もオフなら、本体で更新してもらう
    const rb = state.snap?.rockbox;
    if (isRockbox() && rb && s.added + s.replaced > 0 && !rb.dbSupported && !rb.dbAutoupdate)
      toast(tr("Rockbox の「データベース」→「今すぐ更新」を実行すると、転送した曲がデータベースの一覧に出ます"), "info", 10000);
    const next = state.afterTransfer;
    state.afterTransfer = null;
    if (next) await next();
  });
}
