// 選んだ曲への操作 (削除・プレイリストに追加・評価・書き出し) と右クリックのメニュー

import { t as tr } from "../i18n";
import { currentPlaylist, editablePlaylist, isPcView, isRockbox, writable } from "../app/selectors";
import { state } from "../app/state";
import { albumKey } from "../domain/track-order";
import type { Snapshot, Track } from "../domain/types";
import { removeFromIpod } from "./bind";
import { pickArtwork, removeArtwork, searchArtwork, shrinkArtwork } from "./dialogs/artwork-search";
import { searchArtworkBulk, spansAlbums } from "./dialogs/artwork-bulk";
import { showDetails } from "./dialogs/details";
import { editTags } from "./dialogs/edit-tags";
import { renamePcFiles } from "./dialogs/rename-files";
import { call, reduceMotion, replayClass, toast } from "./feedback";
import { showMenu } from "./menu";
import type { MenuItem } from "./menu";
import { confirmBox, promptText } from "./modal";
import { pcMenuItems, renamePcFile, revealMenuItem } from "./pc";
import { playTrack } from "./player";
import { applySnapshot } from "./render";
import { selectView } from "./sidebar";

/** 操作の対象: チェックした曲 (表示順)。チェックが無ければカーソルの 1 曲 */
export function selectedIds(): string[] {
  // アーティスト表示の一覧 (state.rows) は開いているアーティストの曲だけなので、左の一覧でチェックした
  // ほかのアーティストの曲も入れる (配布リポジトリの Issue #25)
  const pool = state.layout === "artists" ? state.artists.flatMap((a) => a.tracks) : state.rows;
  const checked = pool.filter((t) => state.selected.has(t.id)).map((t) => t.id);
  if (checked.length) return checked;
  return state.focused && state.rows.some((t) => t.id === state.focused) ? [state.focused] : [];
}

export async function removeSelected(ids = selectedIds()) {
  if (!ids.length || !writable()) return;
  const pl = currentPlaylist();
  if (pl && !pl.master) {
    if (pl.smart || pl.podcast) return;
    const s = await call<Snapshot>("remove_from_playlist", { id: pl.id, trackIds: ids });
    if (s) {
      await animateOut(ids);
      state.selected.clear();
      applySnapshot(s);
    }
    return;
  }
  const ok = await confirmBox(
    tr("{n} 曲を iPod から削除しますか？", { n: ids.length }),
    tr("iPod 上の音楽ファイルも削除されます。この操作は取り消せません。"),
    tr("削除"),
    true,
  );
  if (!ok) return;
  const s = await call<Snapshot>("remove_tracks", { ids });
  if (s) {
    await animateOut(ids);
    state.selected.clear();
    applySnapshot(s);
    toast(tr("{n} 曲を削除しました", { n: ids.length }));
  }
}

/** 消した曲の行を、一覧から消す前に少し動かして消す (どの曲が消えたか分かるように) */
export async function animateOut(ids: string[]) {
  if (reduceMotion) return;
  const set = new Set(ids);
  let any = false;
  document.querySelectorAll<HTMLElement>("#rows .row").forEach((r) => {
    const t = state.rows[Number(r.dataset.i)];
    if (t && set.has(t.id)) {
      r.classList.add("removing");
      any = true;
    }
  });
  if (any) await new Promise((r) => setTimeout(r, 200));
}

export async function newPlaylist(trackIds: string[] = []) {
  if (!writable()) return;
  const name = await promptText(tr("新規プレイリスト"), tr("新規プレイリスト"), tr("作成"));
  if (!name) return;
  const s = await call<Snapshot>("create_playlist", { name, trackIds });
  if (s) {
    applySnapshot(s);
    const created = s.playlists.filter((p) => p.name === name).pop();
    if (created && !trackIds.length) selectView(created.id);
  }
}

export async function addToPlaylist(plId: string, ids: string[]) {
  const pl = editablePlaylist(plId);
  if (!pl || !ids.length) return;
  const s = await call<Snapshot>("add_to_playlist", { id: plId, trackIds: ids });
  if (s) {
    const after = s.playlists.find((p) => p.id === plId);
    const added = (after?.trackIds.length ?? 0) - pl.trackIds.length;
    applySnapshot(s);
    if (added > 0) bumpPlaylist(plId, added);
    toast(added > 0 ? tr("「{a}」に {n} 曲追加しました", { a: pl.name, n: added }) : tr("すでにプレイリストに入っています"));
  }
}

/** サイドバーのプレイリストを弾ませて、増えた曲数を「＋n」で浮かべる */
function bumpPlaylist(id: string, added: number) {
  const el = document.querySelector<HTMLElement>(`#pl-list [data-pl="${CSS.escape(id)}"]`);
  if (!el || reduceMotion) return;
  replayClass(el, "bump");
  const f = document.createElement("span");
  f.className = "plus-float";
  f.textContent = `＋${added.toLocaleString()}`;
  el.appendChild(f);
  setTimeout(() => f.remove(), 900);
}

/** 選択中の曲に対するメニュー (右クリック / 選択バーの「その他」共通)
 *  - 1 曲: 曲名を見出しにして、読みの編集など 1 曲向けの操作
 *  - 複数 (同じアルバム): アルバム単位の操作
 *  - 複数 (別々のアルバム): 全曲に同じジャケットが付く操作は出さない */
function selectionMenuItems(ids: string[]): MenuItem[] {
  const pl = currentPlaylist();
  const tracks = ids.map((id) => state.trackMap.get(id)).filter((t): t is Track => !!t);
  const single = tracks.length === 1;
  const albums = new Set(tracks.map(albumKey));
  const oneAlbum = albums.size === 1 && !!tracks[0]?.album;
  const hasArt = tracks.some((t) => t.artworkId);
  const w = writable();
  const items: MenuItem[] = [];

  // 何曲が対象かは見出しに出すので、各項目には曲数を付けない
  if (single) {
    const t = tracks[0];
    items.push({ header: t.artist ? `${t.title} — ${t.artist}` : t.title });
  } else {
    items.push({
      header: oneAlbum
        ? tr("{n} 曲を選択中 ·「{a}」", { n: tracks.length, a: tracks[0].album })
        : tr("{n} 曲を選択中 · {a} アルバム", { n: tracks.length.toLocaleString(), a: albums.size }),
    });
  }

  items.push({ label: tr("再生"), shortcut: "Enter", action: () => playTrack(ids[0], single ? undefined : ids) });
  if (w) {
    items.push({ label: tr("情報を編集…"), shortcut: navigator.userAgent.includes("Mac") ? "⌘I" : "Ctrl+I", action: () => editTags(ids) });
    // Rockbox モードの曲はファイルそのものなので、PC の曲と同じくファイル名を変えられる
    if (isRockbox())
      items.push({ label: tr("ファイル名を変更…"), shortcut: "F2", action: () => (single ? renamePcFile(ids[0]) : renamePcFiles(ids)) });

    if (state.snap!.artworkSupported) {
      const art: MenuItem[] = [];
      if (single || oneAlbum) art.push({ label: tr("ネットで探す…"), action: () => searchArtwork(ids) });
      else if (spansAlbums(ids)) art.push({ label: tr("アルバムごとにネットで探す…"), action: () => void searchArtworkBulk(ids) });
      art.push({
        label: single || oneAlbum ? tr("画像ファイルから設定…") : tr("全曲に同じ画像を設定…"),
        action: () => pickArtwork(ids),
      });
      art.push({ label: tr("埋め込み画像を縮小…"), action: () => void shrinkArtwork(ids) });
      if (hasArt) art.push({ sep: true }, { label: tr("削除"), danger: true, action: () => removeArtwork(ids) });
      items.push({ label: tr("アートワーク"), submenu: art });
    }


    if (canRate()) {
      const cur = tracks.every((t) => t.rating === tracks[0].rating) ? Math.round(tracks[0].rating / 20) : -1;
      items.push({
        label: tr("評価"),
        submenu: [5, 4, 3, 2, 1, 0].map((n): MenuItem => ({
          label: n ? "★".repeat(n) : tr("なし"),
          checked: cur === n,
          action: () => setRating(ids, n * 20),
        })),
      });
    }
    items.push({ label: tr("プレイリストに追加"), submenu: playlistMenuItems(ids) });
  }

  items.push({ sep: true });
  if (single) items.push({ label: tr("詳細情報…"), action: () => showDetails(ids[0]) });
  // iPod OS の曲のファイル名はでたらめな名前なので、Rockbox の曲だけ
  if (single && isRockbox()) items.push(revealMenuItem(ids[0]));
  items.push({ label: tr("PC に書き出し…"), action: () => exportSelected(ids) });
  if (pl && !pl.master && !pl.smart && !pl.podcast && w) items.push({ label: tr("プレイリストから外す"), action: () => removeSelected(ids) });
  if (w) items.push({ sep: true }, { label: tr("iPod から削除…"), danger: true, action: () => removeFromIpod(ids) });
  return items;
}

/** 評価を付けられるか (iPod OS の曲か、アプリでデータベースを作れる Rockbox の曲) */
export function canRate(): boolean {
  return writable() && (!isRockbox() || !!state.snap?.rockbox?.dbSupported);
}

async function setRating(ids: string[], rating: number) {
  const r = await call<{ count: number; snapshot: Snapshot }>("edit_tags", { ids, edit: { rating } });
  if (r) applySnapshot(r.snapshot);
}

/** 選択が無いときは何もしない。above = true ならメニューを基準点の上に出す */
export function openSelectionMenu(x: number, y: number, above = false, ids = selectedIds()) {
  if (!ids.length) return;
  showMenu(x, y, isPcView() ? pcMenuItems(ids) : selectionMenuItems(ids));
  if (above) {
    const m = document.querySelector<HTMLElement>("#menu-root .menu");
    if (m) m.style.top = `${Math.max(8, y - m.getBoundingClientRect().height)}px`;
  }
}

function playlistMenuItems(ids: string[]): MenuItem[] {
  const items: MenuItem[] = [{ label: tr("新規プレイリストに追加…"), action: () => newPlaylist(ids) }];
  const pls = state.snap!.playlists.filter((p) => !p.master && !p.smart && !p.podcast);
  if (pls.length) items.push({ sep: true });
  for (const p of pls) items.push({ label: p.name, action: () => addToPlaylist(p.id, ids) });
  return items;
}

/** 書き出し先のフォルダのダイアログは Rust が開く (やめたら null が返る) */
export async function exportSelected(ids = selectedIds()) {
  if (!ids.length) return;
  const n = await call<number | null>("export_tracks", { ids });
  if (n != null) toast(tr("{n} 曲を書き出しました", { n }));
}
