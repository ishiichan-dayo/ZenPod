// PC の音楽フォルダ (登録・読み込み・iPod への転送・右クリック)

import { t as tr } from "../i18n";
import { listen } from "../infra/tauri";
import { currentPcFolder, currentPcFolders, isPcAll, onIpodIndex, writable } from "../app/selectors";
import { state } from "../app/state";
import { rankCache } from "../domain/track-order";
import { pcTrackToTrack } from "../domain/types";
import type { PcTrack, Snapshot, Track } from "../domain/types";
import { getStored, setStored, setStoredLocal } from "../infra/prefs";
import { pcArtCache } from "./artwork-cells";
import { pcPathsOf, pickArtwork, removeArtwork, searchArtwork, shrinkArtwork } from "./dialogs/artwork-search";
import { searchArtworkBulk, spansAlbums } from "./dialogs/artwork-bulk";
import { showDetails } from "./dialogs/details";
import { editTags } from "./dialogs/edit-tags";
import { renamePcFiles } from "./dialogs/rename-files";
import { esc } from "./dom";
import { call, callOk, toast } from "./feedback";
import type { MenuItem } from "./menu";
import { modal, promptText } from "./modal";
import { findTrack, playTrack, player, stopPlayer } from "./player";
import { applySnapshot, renderAll } from "./render";
import { renderRows } from "./rows";
import { renderSidebar, selectView } from "./sidebar";
import { CONVERT_LOSSY, convertOptions, startTransfer } from "./transfer";
import type { DuplicatePolicy } from "./transfer";

export function loadPcFolders(): string[] {
  try {
    const v = JSON.parse(getStored("pcFolders") ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}
/** 登録フォルダは Rust が prefs.json に書く (画面からは set_pref で書けない)。手元の値だけ合わせる */
function rememberPcFolders() {
  setStoredLocal("pcFolders", JSON.stringify(state.pc.folders));
}
/** 登録フォルダを並べ替える (サイドバーのドラッグ)。Rust が受け付けなければ元に戻す */
export async function reorderPcFolders(folders: string[]) {
  const before = state.pc.folders;
  state.pc.folders = folders;
  if (await callOk("reorder_pc_folders", { folders })) rememberPcFolders();
  else state.pc.folders = before;
}
/** フォルダを選んで登録する (ダイアログは Rust が開く)。選んだフォルダを返す (やめたら null) */
export async function pickPcFolder(): Promise<string | null> {
  const dir = await call<string | null>("add_pc_folder");
  if (!dir) return null;
  if (!state.pc.folders.includes(dir)) {
    state.pc.folders.push(dir);
    rememberPcFolders();
  }
  return dir;
}
/** 表示中のフォルダを読み直す (編集・名前の変更のあと) */
export function rescanCurrentPc() {
  for (const f of currentPcFolders()) scanPcFolder(f);
}
export function folderName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
}

/** 読み取りは 1 フォルダずつ順番に行う (「すべて」で複数のフォルダを続けて読む) */
let pcScanQueue: Promise<void> = Promise.resolve();
const pcScanPending = new Set<string>();

export function scanPcFolder(folder: string): Promise<void> {
  if (pcScanPending.has(folder)) return pcScanQueue;
  pcScanPending.add(folder);
  pcScanQueue = pcScanQueue.then(() => doScanPcFolder(folder)).finally(() => pcScanPending.delete(folder));
  return pcScanQueue;
}

async function doScanPcFolder(folder: string) {
  state.pc.scanning = folder;
  state.pc.progress = "";
  renderAll();
  const unlisten = await listen<{ done: number; total: number }>("pc-scan-progress", (ev) => {
    state.pc.progress = `${ev.payload.done.toLocaleString()} / ${ev.payload.total.toLocaleString()}`;
    if (currentPcFolders().includes(folder)) renderRows();
  });
  const list = await call<PcTrack[]>("scan_music_folder", { path: folder });
  unlisten();
  state.pc.scanning = null;
  if (list) {
    const tracks = list.map(pcTrackToTrack);
    // ほかのソフトでジャケットを変えた曲 (画像の有無か大きさが変わった) は、覚えている小さい画像を捨てて読み直す
    for (const t of tracks) {
      const old = state.pc.byId.get(t.id);
      if (old && t.pcPath && (old.hasPicture !== t.hasPicture || old.size !== t.size)) pcArtCache.delete(t.pcPath);
    }
    state.pc.tracks.set(folder, tracks);
    for (const t of tracks) state.pc.byId.set(t.id, t);
    rankCache.clear();
  }
  renderAll();
}

export async function addPcFolder() {
  const dir = await pickPcFolder();
  if (!dir) return;
  selectView(`pc:${dir}`);
  renderSidebar();
}

export async function removePcFolder(folder: string) {
  if (!(await callOk("remove_pc_folder", { path: folder }))) return;
  state.pc.folders = state.pc.folders.filter((f) => f !== folder);
  state.pc.tracks.delete(folder);
  rememberPcFolders();
  if (currentPcFolder() === folder || (isPcAll() && !state.pc.folders.length)) selectView("library");
  else if (isPcAll()) renderAll();
  else renderSidebar();
}

/** PC の曲のファイル名を変える (拡張子はそのまま) */
/** 名前を変えられる曲のファイルの場所 (PC の曲、または Rockbox モードの曲の iPod の中の場所) */
export function renamablePath(t: Track | undefined): string | undefined {
  return t?.pcPath ?? t?.rbPath;
}

export async function renamePcFile(id: string) {
  const t = findTrack(id);
  const path = renamablePath(t);
  if (!t || !path) return;
  const file = path.split(/[\\/]/).pop() ?? "";
  const dot = file.lastIndexOf(".");
  const stem = dot > 0 ? file.slice(0, dot) : file;
  const ext = dot > 0 ? file.slice(dot) : "";
  await promptText(tr("ファイル名を変更 (拡張子 {a} はそのまま)", { a: ext }), stem, tr("保存"), async (name) => {
    if (name === stem) return true;
    // 再生中のファイルは開いたままだと名前を変えられないことがあるので止める
    if (player.current === id) stopPlayer();
    if (!t.pcPath) {
      const snap = await call<Snapshot>("rename_rockbox_files", { renames: [{ id, name }] });
      if (!snap) return false;
      applySnapshot(snap);
      toast(tr("ファイル名を変更しました"));
      return true;
    }
    const r = await call<string>("rename_pc_file", { path, name });
    if (!r) return false;
    toast(tr("ファイル名を変更しました"));
    rescanCurrentPc();
    return true;
  });
}

/** 選んだ PC の曲を iPod に転送する (今のモードで。プレイリスト指定可) */
/** `convertAllTo` は「変換して転送」で選んだ形式 (`startTransfer`) */
export async function sendPcTracks(ids: string[], playlistId: string | null = null, convertAllTo?: string) {
  const paths = ids.filter((id) => id.startsWith("pc:")).map((id) => id.slice(3));
  if (!paths.length) return;
  // 転送済みの曲が含まれていれば、上書きするか聞く
  const onIpod = onIpodIndex();
  const dup = ids.filter((id) => {
    const t = findTrack(id);
    return !!t && onIpod(t);
  }).length;
  let policy: DuplicatePolicy | undefined;
  if (dup) {
    const choice = await askOverwrite(paths.length, dup);
    if (!choice) return;
    policy = choice;
  }
  // チェックした曲を送ったときは、終わったらそのチェックを外す (右クリックで 1 曲だけ送ったときなどは何もしない)
  state.transferChecked = new Set(ids.filter((id) => state.selected.has(id)));
  startTransfer(paths, playlistId, policy, convertAllTo);
}

/**
 * 変換して転送 (配布リポジトリの Issue #16): 形式を選んで、選んだ曲をその形式にして送る (容量を減らしたい iPod 向け)。
 * ロスレスの曲 (ALAC などふだんはそのまま送るものも) と、選んだビットレートより高い曲を変換する
 */
function convertAndSend(ids: string[]) {
  let to: string | null = null;
  modal(
    `<h3>${esc(tr("変換して転送"))}</h3>
     <p>${esc(tr("選んだ {n} 曲を、次の形式にして転送します。PC の元のファイルは変わりません。", { n: ids.length.toLocaleString() }))}</p>
     <div class="set-convert cv-ask">
       <label for="ca-to">${esc(tr("変換先の形式"))}</label>
       <select id="ca-to">${convertOptions(CONVERT_LOSSY, getStored("convertAllTo") ?? "aac:256")}</select>
     </div>
     <div class="hint">${esc(tr("ALAC・FLAC・WAV などロスレスの曲はすべて変換します。MP3・AAC などの曲は、選んだビットレートより高いものだけ変換し、それ以下はそのまま送ります (上げても音は良くならず、大きくなるだけのため)。容量は 4 分の曲 1 曲あたりの目安です。"))}</div>
     <div class="modal-actions"><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok">${esc(tr("変換して転送"))}</button></div>`,
    (root, close) => {
      root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
      root.querySelector('[data-x="ok"]')!.addEventListener("click", () => {
        to = root.querySelector<HTMLSelectElement>("#ca-to")!.value;
        setStored("convertAllTo", to);
        close();
      });
    },
  ).then(() => {
    if (to) void sendPcTracks(ids, null, to);
  });
}

/** 転送済みの曲を上書きするか聞く。null = キャンセル */
function askOverwrite(total: number, dup: number): Promise<DuplicatePolicy | null> {
  return new Promise((resolve) => {
    let result: DuplicatePolicy | null = null;
    const all = dup === total;
    modal(
      `<h3>${esc(tr("転送済みの曲があります"))}</h3>
       <p>${esc(all ? tr("選んだ {n} 曲はすべて、iPod に同じ曲があります。上書きしますか？", { n: total.toLocaleString() }) : tr("選んだ {n} 曲のうち {a} 曲は、iPod に同じ曲があります。上書きしますか？", { n: total.toLocaleString(), a: dup.toLocaleString() }))}</p>
       <p class="muted">${esc(tr("上書きしても、再生回数・評価・プレイリストはそのまま残ります。"))}</p>
       <div class="modal-actions">
         <button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button>
         <span class="spacer"></span>
         <button class="btn" data-x="skip">${all ? tr("上書きしない") : tr("上書きせずに残りを転送")}</button>
         <button class="btn primary" data-x="replace">${esc(tr("上書きする"))}</button>
       </div>`,
      (root, close) => {
        const pick = (v: DuplicatePolicy | null) => () => {
          result = v;
          close();
        };
        root.querySelector('[data-x="cancel"]')!.addEventListener("click", pick(null));
        // 全部が転送済みで上書きしないなら、転送するものが無いのでキャンセルと同じ
        root.querySelector('[data-x="skip"]')!.addEventListener("click", pick(all ? null : "skip"));
        root.querySelector<HTMLButtonElement>('[data-x="replace"]')!.addEventListener("click", pick("replace"));
        root.querySelector<HTMLButtonElement>('[data-x="replace"]')!.focus();
      },
    ).then(() => resolve(result));
  });
}

export function pcMenuItems(ids: string[]): MenuItem[] {
  const pls = state.snap?.playlists.filter((p) => !p.master && !p.smart && !p.podcast) ?? [];
  const w = writable();
  const single = ids.length === 1;
  const albums = new Set(ids.map((id) => findTrack(id)?.album ?? ""));
  const oneAlbum = albums.size === 1 && !albums.has("");
  return [
    { header: single ? (state.rows.find((t) => t.id === ids[0])?.title ?? "") : tr("{n} 曲を選択中", { n: ids.length.toLocaleString() }) },
    { label: tr("再生"), shortcut: "Enter", action: () => playTrack(ids[0], single ? undefined : ids) },
    { label: tr("情報を編集…"), shortcut: navigator.userAgent.includes("Mac") ? "⌘I" : "Ctrl+I", action: () => editTags(ids) },
    { label: tr("ファイル名を変更…"), shortcut: "F2", action: () => (single ? renamePcFile(ids[0]) : renamePcFiles(ids)) },
    { label: tr("アートワーク"), submenu: pcArtMenuItems(ids, single || oneAlbum) },
    { sep: true },
    { label: tr("転送"), disabled: !w || state.transferring, action: () => sendPcTracks(ids) },
    { label: tr("変換して転送…"), disabled: !w || state.transferring, action: () => convertAndSend(ids) },
    {
      label: tr("転送してプレイリストに追加"),
      disabled: !w || state.transferring || !pls.length,
      submenu: pls.map((p) => ({ label: p.name, action: () => sendPcTracks(ids, p.id) })),
    },
    { sep: true },
    ...(single ? [{ label: tr("詳細情報…"), action: () => showDetails(ids[0]) }, revealMenuItem(ids[0])] : []),
    { label: tr("フォルダを読み直す"), action: rescanCurrentPc },
  ];
}

/** 曲のファイルを、入っているフォルダを開いて見せるメニューの項目 (PC の曲と Rockbox の曲) */
export function revealMenuItem(id: string): MenuItem {
  const ua = navigator.userAgent;
  const label = ua.includes("Windows") ? tr("エクスプローラーで表示") : ua.includes("Mac") ? tr("Finder で表示") : tr("フォルダで表示");
  return { label, action: () => void callOk("reveal_track", { id }) };
}

/** PC の曲のアートワークのメニュー (元のファイルに埋め込む) */
function pcArtMenuItems(ids: string[], oneAlbum: boolean): MenuItem[] {
  const art: MenuItem[] = [];
  if (oneAlbum) art.push({ label: tr("ネットで探す…"), action: () => searchArtwork(ids) });
  else if (spansAlbums(ids)) art.push({ label: tr("アルバムごとにネットで探す…"), action: () => void searchArtworkBulk(ids) });
  art.push({ label: oneAlbum ? tr("画像ファイルから設定…") : tr("全曲に同じ画像を設定…"), action: () => pickArtwork(ids) });
  // 画像の有無をまだ読んでいない曲もあるので、画像が無いと分かっている曲だけのときに隠す
  if (pcPathsOf(ids).some((p) => pcArtCache.get(p) !== null)) {
    art.push({ label: tr("埋め込み画像を縮小…"), action: () => void shrinkArtwork(ids) });
    art.push({ sep: true }, { label: tr("埋め込み画像を削除"), danger: true, action: () => removeArtwork(ids) });
  }
  return art;
}
