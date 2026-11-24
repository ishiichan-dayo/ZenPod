// 並べ替えのメニューと、曲 / アルバム / アーティストの表示の切り替え

import { t as tr } from "../i18n";
import { computeRows } from "../app/rows";
import { albumSort, artistSort, currentPlaylist, isPcView, isRockbox } from "../app/selectors";
import type { AlbumSort, ArtistSort } from "../app/selectors";
import { $, state } from "../app/state";
import type { Layout } from "../app/state";
import type { SortKey } from "../domain/types";
import { setStored } from "../infra/prefs";
import { COLUMNS } from "./columns";
import { withTransition } from "./feedback";
import { showMenu } from "./menu";
import type { MenuItem } from "./menu";
import { renderAll } from "./render";

// tr: 使う側で訳す
const SORT_LABEL = { artist: "アーティスト", album: "アルバム名", year: "年", added: "追加日", name: "名前", count: "曲の数", order: "デフォルト" };

/** 曲の並べ替えの名前。デフォルト (保存してあるプレイリストの順) のほかは列の名前 */
export function trackSortLabel(k: SortKey): string {
  if (k === "order") return SORT_LABEL.order;
  const c = COLUMNS.find((x) => x.key === k);
  return c?.menuLabel ?? c?.label ?? "";
}

/** 一覧の上の「並べ替え」: 今の見方 (曲 / アルバム / アーティスト) に合わせて選べるものを変える */
export function openSortMenu(anchor: HTMLElement) {
  const r = anchor.getBoundingClientRect();
  const pc = isPcView();
  // 追加日と再生回数は iPod OS の曲だけにある (PC の曲と Rockbox の曲には無い)
  const noDates = pc || isRockbox();
  const items: MenuItem[] = [];
  const dirItems = (desc: boolean, set: (d: boolean) => void): MenuItem[] => [
    { sep: true },
    { label: tr("昇順"), checked: !desc, action: () => set(false) },
    { label: tr("降順"), checked: desc, action: () => set(true) },
  ];
  if (state.layout === "albums") {
    const cur = albumSort();
    for (const k of ["artist", "album", "year", "added"] as AlbumSort[]) {
      const label = SORT_LABEL[k];
      if (k === "added" && noDates) continue;
      // 年と追加日は新しい順から
      items.push({ label: tr(label), checked: cur.key === k, action: () => applyAlbumSort(k, k === "year" || k === "added") });
    }
    items.push(...dirItems(cur.desc, (d) => applyAlbumSort(cur.key, d)));
  } else if (state.layout === "artists") {
    const cur = artistSort();
    for (const k of ["name", "count"] as ArtistSort[])
      items.push({ label: tr(SORT_LABEL[k]), checked: cur.key === k, action: () => applyArtistSort(k, k === "count") });
    items.push(...dirItems(cur.desc, (d) => applyArtistSort(cur.key, d)));
  } else {
    // 曲の一覧は見出しの並べ替えと同じ (表示だけ。プレイリストの順を変えるのは「並び替え」のパネル)
    const keys: SortKey[] = [
      "title",
      "artist",
      "album",
      "trackNr",
      "year",
      "duration",
      ...(noDates ? [] : (["dateAdded", "plays"] as SortKey[])),
      ...(pc ? [] : (["rating"] as SortKey[])),
      // 無い曲を先に並べる (昇順)
      "artwork",
    ];
    if (currentPlaylist()) keys.unshift("order");
    for (const k of keys) {
      const label = trackSortLabel(k);
      const newestFirst = k === "dateAdded" || k === "plays" || k === "rating" || k === "year";
      items.push({ label: tr(label), checked: state.sort.key === k, action: () => applyTrackSort({ key: k, asc: !newestFirst }) });
    }
    items.push(...dirItems(!state.sort.asc, (d) => applyTrackSort({ key: state.sort.key, asc: !d })));
  }
  showMenu(r.right - 264, r.bottom + 4, items);
}

function applyAlbumSort(key: AlbumSort, desc: boolean) {
  setStored("albumSort", key);
  setStored("albumSortDesc", desc ? "1" : "0");
  $("album-grid").scrollTop = 0;
  renderAll();
}

function applyArtistSort(key: ArtistSort, desc: boolean) {
  setStored("artistSort", key);
  setStored("artistSortDesc", desc ? "1" : "0");
  computeRows();
  renderAll();
}

function applyTrackSort(sort: { key: SortKey; asc: boolean }) {
  state.sort = sort;
  $("tbody").scrollTop = 0;
  computeRows();
  renderAll();
}

/** 「並べ替え」のボタンの文字: 今の並べ方の名前 */
export function renderSortButton() {
  const pc = isPcView();
  let label: string;
  let desc: boolean;
  if (state.layout === "albums") {
    const a = albumSort();
    const key = (pc || isRockbox()) && a.key === "added" ? "artist" : a.key;
    label = SORT_LABEL[key];
    desc = a.desc;
  } else if (state.layout === "artists") {
    const a = artistSort();
    label = SORT_LABEL[a.key];
    desc = a.desc;
  } else {
    label = trackSortLabel(state.sort.key);
    desc = !state.sort.asc;
  }
  // デフォルト (保存してあるプレイリストの順) には向きを付けない
  $("sort-label").textContent = state.layout === "tracks" && state.sort.key === "order" && state.sort.asc ? tr(label) : `${tr(label)} ${desc ? "↓" : "↑"}`;
  $("btn-sort").title = tr("並べ替え: {a}", { a: tr(label) });
  // アルバムを開いているときは、アルバムの中の曲順なので出さない
  $("btn-sort").classList.toggle("hidden", state.layout === "albums" && state.openAlbum !== null);
}

export function setLayout(layout: Layout): Promise<void> {
  if (state.layout === layout) return Promise.resolve();
  return withTransition(() => {
    state.layout = layout;
    state.openAlbum = null;
    state.gridFocus = -1;
    setStored("layout", layout);
    $("album-grid").scrollTop = 0;
    renderAll();
  });
}
