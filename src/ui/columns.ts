// 曲の一覧の列 (表示する列・幅・見出し)

import { t as tr } from "../i18n";
import { isPodView } from "../app/podcasts";
import { isVisualView } from "../app/visual";
import { currentPcFolder, currentPlaylist, editablePlaylist, isPcAll, isPcView, writable } from "../app/selectors";
import { $, state } from "../app/state";
import type { SortKey } from "../domain/types";
import { getStored, setStored } from "../infra/prefs";
import { albumArtCache, albumArtSource } from "./album-grid";
import { esc } from "./dom";
import { ICON_COLUMNS } from "./icons";
import { showMenu } from "./menu";
import type { MenuItem } from "./menu";
import { folderName } from "./pc";
import { renderRows } from "./rows";
import { placeSegThumb } from "./segments";
import { updateSelectAllBox } from "./selbar";
import { VIEW_TITLE } from "./sidebar";
import { renderSortButton } from "./sort-menu";

interface Column {
  key: SortKey;
  label: string;
  /** 表示する項目のメニューでの名前 (見出しと違うとき) */
  menuLabel?: string;
  /** grid-template-columns での幅 */
  width: string;
  num?: boolean;
  /** iPod の一覧だけ / PC のフォルダだけに出す列 */
  only?: "ipod" | "pc";
  /** 消せない列 */
  fixed?: boolean;
  title?: string;
}

/** 一覧の列 (この順に並ぶ)。表示するかは「表示する項目」で選ぶ */
export const COLUMNS: Column[] = [
  // 曲順 (トラック番号) ではなく、一覧の中の順番 (プレイリストではプレイリストの順)
  { key: "order", label: "#", menuLabel: "一覧の順番", width: "40px", num: true, title: "一覧の中の順番 (プレイリストではプレイリストの順)" },
  { key: "title", label: "タイトル", width: "minmax(160px, 2.4fr)", fixed: true },
  { key: "artist", label: "アーティスト", width: "minmax(100px, 1.5fr)" },
  { key: "album", label: "アルバム", width: "minmax(100px, 1.6fr)" },
  { key: "albumArtist", label: "アルバムアーティスト", width: "minmax(100px, 1.3fr)" },
  { key: "composer", label: "作曲者", width: "minmax(90px, 1.2fr)" },
  { key: "genre", label: "ジャンル", width: "minmax(70px, 0.9fr)" },
  { key: "year", label: "年", width: "56px", num: true },
  { key: "trackNr", label: "曲順", width: "56px", num: true, title: "トラック番号 (2 枚目以降のディスクは「2-5」)" },
  { key: "filetype", label: "種類", width: "64px" },
  { key: "fileName", label: "ファイル名", width: "minmax(120px, 1.6fr)" },
  { key: "bitrate", label: "ビットレート", width: "96px", num: true, only: "ipod" },
  { key: "size", label: "サイズ", width: "76px", num: true },
  { key: "duration", label: "時間", width: "64px", num: true },
  { key: "dateAdded", label: "追加日", width: "96px", num: true, only: "ipod" },
  { key: "rating", label: "評価", width: "76px", only: "ipod" },
  { key: "mediaKind", label: "分類", menuLabel: "分類 (音楽・ポッドキャスト・オーディオブック)", width: "132px", only: "ipod" },
  { key: "plays", label: "再生", menuLabel: "再生回数", width: "56px", num: true, only: "ipod" },
  {
    key: "onIpod",
    label: "iPod",
    menuLabel: "iPod に転送済み",
    width: "84px",
    num: true,
    only: "pc",
    title: "iPod に同じ曲があるか (クリックで未転送の曲を先に並べる)",
  },
  {
    key: "artwork",
    label: "アートワーク",
    width: "96px",
    num: true,
    title: "アートワークがあるか (クリックでアートワークの無い曲を先に並べる)",
  },
];
const DEFAULT_COLUMNS: SortKey[] = ["order", "title", "artist", "album", "genre", "duration", "plays", "onIpod"];


/** 出す列は iPod の一覧と PC の一覧で別々に覚える (PC の一覧はファイル名も最初から出す) */
function columnsPrefKey(): string {
  return isPcView() ? "columnsPc" : "columns";
}

function storedColumns(key: string): SortKey[] | null {
  try {
    const v = JSON.parse(getStored(key) ?? "null");
    if (Array.isArray(v)) return v as SortKey[];
  } catch {
    /* 壊れていたら標準に戻す */
  }
  return null;
}

function shownColumnKeys(): Set<SortKey> {
  const own = storedColumns(columnsPrefKey());
  if (own) return new Set([...own, "title"]);
  if (!isPcView()) return new Set(DEFAULT_COLUMNS);
  // PC の一覧をまだ変えていなければ、iPod の一覧の列 (前は共通だった) にファイル名を足したもの
  return new Set([...(storedColumns("columns") ?? DEFAULT_COLUMNS), "title", "fileName"]);
}

/** 今の一覧 (iPod / PC) に出す列 */
export function visibleColumns(): Column[] {
  const shown = shownColumnKeys();
  const pc = isPcView();
  return COLUMNS.filter((c) => shown.has(c.key) && (!c.only || (c.only === "pc") === pc));
}

// ---------------- 列の幅 ----------------
// 見出しの境目をドラッグして変えた幅を、列ごとに iPod の一覧と PC の一覧で別々に覚える。
// 決まった幅の列 (番号・年など) は px、割合で広がる列 (タイトルなど) は幅の比として覚え、
// 割合の列はウィンドウの幅に合わせて今までどおり伸び縮みする

/** 列の幅の上限 (px) */
const COL_MAX = 800;

type Widths = Partial<Record<SortKey, number>>;

function widthsPrefKey(): string {
  return isPcView() ? "columnWidthsPc" : "columnWidths";
}

function storedWidths(): Widths {
  try {
    const v = JSON.parse(getStored(widthsPrefKey()) ?? "null");
    if (v && typeof v === "object" && !Array.isArray(v)) return v as Widths;
  } catch {
    /* 壊れていたら標準の幅に戻す */
  }
  return {};
}

/** 割合で広がる列の標準の比 (決まった幅の列は null) */
function flexOf(c: Column): number | null {
  const m = /([\d.]+)fr/.exec(c.width);
  return m ? Number(m[1]) : null;
}

/** 今の見出しの列と、見出しの文字が切れない最小の幅 (renderTableHead で測る。ドラッグ中に使う) */
let headLayout: { cols: Column[]; need: number[] } = { cols: [], need: [] };

/**
 * 列の幅を --cols / --cols-min に入れる。saved は覚えた幅、live はドラッグ中に px で止めている幅。
 * 割合の列のうち覚えていないもの (後から出した列・標準に戻した列) は、覚えた列と同じ物差しにそろえた標準の比にする
 */
function applyColumnWidths(saved: Widths, live: Widths = {}) {
  const { cols, need } = headLayout;
  let savedFlex = 0;
  let savedBase = 0;
  for (const c of cols) {
    const f = flexOf(c);
    if (f !== null && saved[c.key] !== undefined) {
      savedFlex += saved[c.key]!;
      savedBase += f;
    }
  }
  const scale = savedBase ? savedFlex / savedBase : 1;
  const widths = cols.map((c, i) => {
    const min = Math.max(need[i], Number(/(\d+)px/.exec(c.width)?.[1] ?? 0));
    const px = live[c.key] ?? (flexOf(c) === null ? saved[c.key] : undefined);
    if (px !== undefined) {
      const w = Math.min(COL_MAX, Math.max(need[i], Math.round(px)));
      return { width: `${w}px`, min: w };
    }
    const f = flexOf(c);
    if (f === null) return { width: `${min}px`, min };
    // 狭めた列は、標準の最小の幅より細くても縮めすぎない (見出しの文字が切れない幅まで)
    const weight = saved[c.key] ?? f * scale;
    const lo = saved[c.key] !== undefined ? Math.max(need[i], Math.min(min, Math.round(weight))) : min;
    return { width: `minmax(${lo}px, ${weight}fr)`, min: lo };
  });
  // 先頭はチェック、末尾は表示する項目の歯車
  $("table").style.setProperty("--cols", `32px ${widths.map((w) => w.width).join(" ")} 32px`);
  // 列の最小の幅の合計。狭いときは一覧をこの幅にして横にスクロールさせる (行の背景も右端まで届かせる)
  const minW = widths.reduce((n, w) => n + w.min, 64);
  $("table").style.setProperty("--cols-min", `${minW}px`);
}

/** 見出しの列ごとの今の幅 (px) */
function measuredWidths(): number[] {
  return [...$("thead").querySelectorAll<HTMLElement>(".th[data-sort]")].map((th) => th.getBoundingClientRect().width);
}

/**
 * 見出しの境目 (.col-grip) のドラッグで、その左の列の幅を変える。
 * ドラッグの間は左の列を今の幅で止める (止めないと、広げた分を左の割合の列も分け合って縮み、境目がポインタについてこない)。
 * 離したら、割合の列はすべて今の幅を比として覚える (見た目を変えずに、ウィンドウの幅に合わせて伸び縮みさせる)
 */
export function startColumnResize(grip: HTMLElement, e: PointerEvent) {
  const key = grip.closest<HTMLElement>(".th")?.dataset.sort as SortKey | undefined;
  const index = headLayout.cols.findIndex((c) => c.key === key);
  if (!key || index < 0) return;
  e.preventDefault();
  const saved = storedWidths();
  const start = measuredWidths();
  const live: Widths = {};
  for (let i = 0; i <= index; i++) live[headLayout.cols[i].key] = start[i];
  const startX = e.clientX;
  let moved = false;
  grip.setPointerCapture(e.pointerId);
  grip.classList.add("dragging");
  document.body.classList.add("col-resizing");
  const move = (ev: PointerEvent) => {
    moved = true;
    live[key] = Math.min(COL_MAX, Math.max(headLayout.need[index], start[index] + ev.clientX - startX));
    applyColumnWidths(saved, live);
  };
  const up = () => {
    grip.removeEventListener("pointermove", move);
    grip.removeEventListener("pointerup", up);
    grip.removeEventListener("pointercancel", up);
    grip.classList.remove("dragging");
    document.body.classList.remove("col-resizing");
    // 押して離しただけ (ダブルクリックの 1 回目など) では覚えない
    if (!moved) return;
    const now = measuredWidths();
    headLayout.cols.forEach((c, i) => {
      if (flexOf(c) !== null || c.key === key) saved[c.key] = Math.round(now[i]);
    });
    setStored(widthsPrefKey(), JSON.stringify(saved));
    applyColumnWidths(saved);
  };
  grip.addEventListener("pointermove", move);
  grip.addEventListener("pointerup", up);
  grip.addEventListener("pointercancel", up);
}

/** 境目のダブルクリック: その列だけ標準の幅に戻す (割合の列はほかの列と同じ物差しの標準の比) */
export function resetColumnWidth(grip: HTMLElement) {
  const key = grip.closest<HTMLElement>(".th")?.dataset.sort as SortKey | undefined;
  if (!key) return;
  const saved = storedWidths();
  delete saved[key];
  setStored(widthsPrefKey(), Object.keys(saved).length ? JSON.stringify(saved) : null);
  applyColumnWidths(saved);
}

function setColumnShown(key: SortKey, on: boolean) {
  const shown = shownColumnKeys();
  if (on) shown.add(key);
  else shown.delete(key);
  setStored(columnsPrefKey(), JSON.stringify(COLUMNS.map((c) => c.key).filter((k) => shown.has(k))));
  renderTableHead();
  renderRows();
}

/** 表示する項目のメニュー (見出しの右端の歯車 / 見出しの右クリック) */
export function openColumnMenu(x: number, y: number) {
  const shown = shownColumnKeys();
  const pc = isPcView();
  const items: MenuItem[] = [{ header: tr("表示する項目") }];
  for (const c of COLUMNS) {
    if (c.only && (c.only === "pc") !== pc) continue;
    items.push({
      label: tr(c.menuLabel ?? c.label),
      checked: shown.has(c.key),
      disabled: c.fixed,
      keepOpen: true,
      action: () => setColumnShown(c.key, !shownColumnKeys().has(c.key)),
    });
  }
  items.push(
    { sep: true },
    {
      label: tr("標準に戻す"),
      action: () => {
        // PC の一覧の標準は、iPod と同じ標準の列にファイル名を足したもの
        setStored(columnsPrefKey(), isPcView() ? JSON.stringify([...DEFAULT_COLUMNS, "fileName"]) : null);
        renderTableHead();
        renderRows();
      },
    },
    {
      label: tr("列の幅を標準に戻す"),
      disabled: Object.keys(storedWidths()).length === 0,
      action: () => {
        setStored(widthsPrefKey(), null);
        renderTableHead();
      },
    },
  );
  showMenu(x, y, items);
}

export function renderTableHead() {
  if (isPodView() || isVisualView()) return;
  const cols = visibleColumns();
  // 列の幅は、狭いときも見出しの文字 (並べ替えの ▲ を含む) が切れない幅より細くしない。
  // 見出しの文字の幅は言語で変わるので、見出しと同じ形の見えない箱で測る
  const thead = $("thead");
  const probe = document.createElement("div");
  probe.className = "th";
  probe.style.cssText = "position:absolute;visibility:hidden;width:max-content";
  thead.append(probe);
  const need = cols.map((c) => {
    probe.textContent = `${tr(c.label)} ▲`;
    return Math.ceil(probe.getBoundingClientRect().width);
  });
  probe.remove();
  headLayout = { cols, need };
  applyColumnWidths(storedWidths());
  const grip = esc(tr("ドラッグで幅を変更 (ダブルクリックで元に戻す)"));
  thead.innerHTML =
    `<div class="th chk" data-selall="1" title="${esc(tr("表示中の曲をすべて選択 / 解除"))}"><span class="box"></span></div>` +
    cols
      .map((c) => {
        const sorted = state.layout !== "artists" && state.sort.key === c.key;
        const arrow = sorted ? (state.sort.asc ? " ▲" : " ▼") : "";
        const title = c.title ? ` title="${esc(tr(c.title))}"` : "";
        return `<div class="th ${c.num ? "num" : ""} ${sorted ? "sorted" : ""}" data-sort="${c.key}"${title}>${esc(tr(c.label))}${arrow}<span class="col-grip" title="${grip}"></span></div>`;
      })
      .join("") +
    `<div class="th col-set" data-colset="1" title="${esc(tr("表示する項目"))}"><button class="btn icon ghost mini" aria-label="${esc(tr("表示する項目"))}">${ICON_COLUMNS}</button></div>`;
  updateSelectAllBox();
  $("table").classList.toggle("pc-table", isPcView());
  const pl = currentPlaylist();
  const folder = currentPcFolder();
  const albumOpen = state.layout === "albums" && state.openAlbum !== null;
  const openedTitle = albumOpen ? state.rows[0]?.album || tr("アルバムなし") : null;
  $("view-title").textContent =
    openedTitle ?? (isPcAll() ? tr("PC のすべての曲") : folder ? folderName(folder) : pl ? (pl.podcast ? tr("ポッドキャスト") : pl.name) : (VIEW_TITLE[state.view] ? tr(VIEW_TITLE[state.view]) : tr("すべての曲")));
  $("btn-album-back").classList.toggle("hidden", !albumOpen);
  $("btn-edit-rules").classList.toggle("hidden", !(pl?.smart && !albumOpen && writable()));
  $("btn-reorder").classList.toggle("hidden", !(pl && editablePlaylist(pl.id) && !albumOpen && writable() && !state.reorder));
  // スマートプレイリストには曲を直接足せない (足した曲は条件に合うときだけ入る) ので、曲を追加のボタンは出さない
  $("btn-add").classList.toggle("hidden", !!pl?.smart);
  // 動画・写真の表示で「動画を追加」などに変えたものを戻す
  $("btn-add").textContent = tr("＋ 曲を追加");
  // ジャケットを出すときは、ジャケット・タイトル・曲数を上下の中央でそろえる
  document.querySelector(".view-head")!.classList.toggle("album-open", albumOpen);
  // 開いたアルバムの見出しにジャケットを出す (アルバムの一覧で読み込み済みのもの)
  const headArt = $<HTMLImageElement>("album-head-art");
  const headSrc = albumOpen ? albumArtSource({ key: "", album: "", artist: "", tracks: state.rows, rank: 0 }) : null;
  const headUrl = headSrc ? albumArtCache.get(headSrc) : null;
  if (headUrl) headArt.src = headUrl;
  headArt.classList.toggle("hidden", !headUrl);
  document.querySelectorAll<HTMLInputElement>('input[name="layout"]').forEach((r) => (r.checked = r.value === state.layout));
  renderSortButton();
  placeSegThumb($("layout-toggle"));
}
