// iPod の状態 (snapshot) を受け取って画面全体を描き直す

import { detectLang, lang, setLang, t as tr, translateStatic } from "../i18n";
import { invoke } from "../infra/tauri";
import { analyticsIpod } from "../app/analytics";
import { isPodView } from "../app/podcasts";
import { isVisualView } from "../app/visual";
import { computeRows } from "../app/rows";
import { isLibraryView, isPcView } from "../app/selectors";
import { $, state } from "../app/state";
import { rankCache, resetSearchText } from "../domain/track-order";
import type { Snapshot } from "../domain/types";
import { setStored } from "../infra/prefs";
import { renderTableHead } from "./columns";
import { renderHeader } from "./header";
import { player, renderPlayerModes, renderVolumeIcon, stopPlayer } from "./player";
import { renderReorderPanel } from "./playlist-reorder";
import { keepPlaylistsSorted } from "./playlist-sort";
import { podOffered } from "./podcast-sync";
import { markPlayedFromIpod, renderPodView } from "./podcasts";
import { animateRowMoves, renderRows } from "./rows";
import { renderSelbar } from "./selbar";
import { showSettingsPane } from "./settings";
import { renderSidebar } from "./sidebar";
import { renderStatus, summaryCache } from "./status";
import { renderVisualView, syncVisual } from "./visual";

/** 今の表示がまだ出せるか (iPod の曲・PC・ポッドキャスト・動画と写真 (iPod があるとき)・プレイリスト) */
function viewValid(s: Snapshot | null): boolean {
  // iPod が無いときのライブラリは「すべて」だけ (ミュージックなどはサイドバーに出さない)
  return state.view === "library" || (isLibraryView() && !!s) || isPcView() || isPodView() || (isVisualView() && !!s) || !!s?.playlists.some((p) => p.id === state.view);
}

export function applySnapshot(s: Snapshot | null) {
  // プレイリストだけを変えた操作の後は、曲の一覧と容量の内訳が来ない (今のものを使い回す。曲の情報は同じなので
  // 並べ替えや検索のキャッシュも捨てない)
  const partial = !!s && !s.tracks && !!state.snap;
  const prevPlaylists = state.snap?.playlists ?? null;
  if (s && partial) {
    s.tracks = state.snap!.tracks;
    s.usage = s.usage ?? state.snap!.usage;
  }
  state.snap = s;
  if (!s) podOffered.clear();
  // 並び替え中のプレイリストが無くなった (iPod を外した・消した) ら、下書きを捨てる
  if (state.reorder && !s?.playlists.some((p) => p.id === state.reorder!.id)) {
    state.reorder = null;
    document.body.classList.remove("reordering");
  }
  if (s && !partial) analyticsIpod(s);
  if (partial) {
    if (!viewValid(s)) state.view = "library";
    renderAll();
    keepPlaylistsSorted(prevPlaylists, sortedApply);
    return;
  }
  // iPod の曲を再生中に、その曲が消えた (iPod を外した・削除した) ら止める
  if (player.current && !player.current.startsWith("pc:") && !s?.tracks.some((t) => t.id === player.current)) stopPlayer();
  rankCache.clear();
  // 同じ曲 ID のままファイルが置き換わることがある (同じ曲の再転送)
  summaryCache.clear();
  resetSearchText();
  state.trackMap = new Map((s?.tracks ?? []).map((t) => [t.id, t]));
  if (!viewValid(s)) state.view = "library";
  // 動画と写真: iPod が替わったら捨て、まだなら裏で読む
  syncVisual();
  // iPod で再生したポッドキャストの回に「聞いた」の印を付ける
  markPlayedFromIpod();
  for (const id of [...state.selected]) if (!id.startsWith("pc:") && !state.trackMap.has(id)) state.selected.delete(id);
  renderAll();
  // 並べ方を覚えたプレイリストは、足した曲・変えた曲の情報に合わせて並べ直す (Issue #22)
  keepPlaylistsSorted(prevPlaylists, sortedApply);
}

/** 並べ方に合わせて並べ直したスナップショットを入れる (開いているプレイリストなら、行が新しい位置へ動く) */
function sortedApply(s: Snapshot) {
  animateRowMoves(() => applySnapshot(s));
}

export function renderAll() {
  // iPod が無くても一覧を出す (PC の音楽フォルダは iPod 無しで編集できる)
  // 設定は一覧の上に重ねるモーダル (一覧は後ろに残す)。設定を開いたまま iPod を抜き差ししたら、iPod の欄だけ描き直す
  showSettingsPane();
  renderHeader();
  renderSidebar();
  computeRows();
  // ポッドキャストの番組は、曲の一覧の代わりに回の一覧を出す
  $("pod-view").classList.toggle("hidden", !isPodView());
  if (isPodView()) {
    $("vis-view").classList.add("hidden");
    renderPodView();
    return;
  }
  // 動画と写真は、曲の一覧の代わりにカードの一覧を出す
  renderVisualView();
  if (isVisualView()) return;
  renderTableHead();
  renderRows();
  renderStatus();
  renderSelbar();
  renderReorderPanel();
}

export function renderSearchPlaceholder() {
  $<HTMLInputElement>("search").placeholder = navigator.userAgent.includes("Mac") ? tr("検索  ⌘F") : tr("検索  Ctrl+F");
}

/**
 * 画面を読み直さずに言語を切り替える。固定の文言は元の日本語から訳し直し、プログラムで付けた文言 (検索欄の
 * ショートカットの表示・プレーヤーのボタンの説明など) と一覧などは作り直す
 */
export async function switchLangLive(value: string | null) {
  setStored("lang", value);
  setLang(detectLang(value));
  await invoke("set_lang", { lang }).catch(() => undefined);
  translateStatic(document.body);
  renderSearchPlaceholder();
  renderVolumeIcon();
  renderPlayerModes();
  renderAll();
}
