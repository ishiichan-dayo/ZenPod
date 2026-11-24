// 画面の部品にイベントをつなぐ

import { t as tr } from "../i18n";
import { invoke } from "../infra/tauri";
import { isPodView } from "../app/podcasts";
import { isPhotoView, isVisualView } from "../app/visual";
import { bindVisual, openVideoNode, pasteFromClipboard, renderVisualView, toggleVideoSide, visualKey } from "./visual";
import { albumGroups, computeRows } from "../app/rows";
import { currentPcFolders, currentPlaylist, isPcView, isRockbox, writable } from "../app/selectors";
import { $, state } from "../app/state";
import type { SettingsCat } from "../app/state";
import type { LibMode, Snapshot, SortKey } from "../domain/types";
import { albumTrackIds, openAlbum, renderAlbumGrid, toggleAlbumSelection } from "./album-grid";
import { artistChecked, blockTrackIds, selectArtist, toggleArtistSelection } from "./artist-list";
import { openColumnMenu, renderTableHead, resetColumnWidth, startColumnResize } from "./columns";
import { openDevice, renameIpod, switchMode } from "./device";
import { showDetails } from "./dialogs/details";
import { editTags } from "./dialogs/edit-tags";
import { exportM3u8, exportSmartXml } from "./dialogs/import-playlists";
import { renamePcFiles } from "./dialogs/rename-files";
import { openNewPlaylistMenu, openSmartEditor } from "./dialogs/smart-editor";
import { drag, onArtistMouseDown, onCardMouseDown, onMouseMove, onMouseUp, onRowMouseDown } from "./drag";
import { call, failToast, toast } from "./feedback";
import { backToAlbums, focusMain, moveCursor, navKey, setKeyboardMode, sidebarKey } from "./keyboard";
import { closeMenu, showMenu } from "./menu";
import { confirmBox } from "./modal";
import { addPcFolder, removePcFolder, renamePcFile, scanPcFolder } from "./pc";
import { playTrack, player, stopPlayer, togglePlay } from "./player";
import { deletePlaylist, onSideMouseDown, onSideMouseMove, onSideMouseUp, renamePlaylist, sideDrag, sortPlaylistsByName } from "./playlists";
import { renderPodView } from "./podcasts";
import { applySnapshot, renderSearchPlaceholder } from "./render";
import { ARTIST_MAX, ARTIST_MIN, SIDE_MAX, SIDE_MIN, bindResizer } from "./resizer";
import { renderRows } from "./rows";
import { placeSegThumb } from "./segments";
import { fitSelbar, renderSelbar } from "./selbar";
import { animateOut, exportSelected, newPlaylist, openSelectionMenu, removeSelected, selectedIds } from "./selection-actions";
import { closeSettings, openSettings, renderSettingsNav, tidyMenuItems } from "./settings";
import { selectView } from "./sidebar";
import { bindReorderPanel, cancelReorder, chooseReorderKey } from "./playlist-reorder";
import { openSortMenu, renderSortButton, setLayout } from "./sort-menu";
import { renderFormat, renderStatus } from "./status";
import { rowIndexAt, selectAllVisible, setAnchor } from "./table-select";
import { pickAndTransfer } from "./transfer";

export function bind() {
  // 切り替え (.seg) の背景: 選び直したら動かす。後から作られた切り替え (設定画面) にも付ける
  document.addEventListener("change", (e) => {
    const seg = (e.target as HTMLElement).closest?.<HTMLElement>(".seg");
    if (seg) placeSegThumb(seg);
  });
  new MutationObserver(() => {
    document.querySelectorAll<HTMLElement>(".seg").forEach((seg) => {
      if (!seg.querySelector(":scope > .seg-thumb")) placeSegThumb(seg, false);
    });
  }).observe(document.body, { childList: true, subtree: true });
  // 横のスクロールバーは一覧だけに出し、見出しは一覧に合わせて動かす
  let lastTop = 0;
  $("tbody").addEventListener("scroll", () => {
    const body = $("tbody");
    $("thead").scrollLeft = body.scrollLeft;
    if (body.scrollTop === lastTop) return;
    lastTop = body.scrollTop;
    renderRows(true);
  });
  window.addEventListener("resize", () => state.snap && renderRows(true));
  $("rows").addEventListener("mousedown", onRowMouseDown);
  $("album-grid").addEventListener("mousedown", onCardMouseDown);
  $("artist-list").addEventListener("mousedown", onArtistMouseDown);
  document.addEventListener("mousemove", onMouseMove);
  document.addEventListener("mouseup", onMouseUp);
  // サイドバーのプレイリスト・PC のフォルダの並べ替え
  $("pl-list").addEventListener("mousedown", onSideMouseDown);
  $("pc-list").addEventListener("mousedown", onSideMouseDown);
  document.addEventListener("mousemove", onSideMouseMove);
  document.addEventListener("mouseup", onSideMouseUp);

  $("tbody").addEventListener("mousedown", (e) => {
    // 空白のクリックではカーソルだけ外す (チェックは消さない)
    if (rowIndexAt(e.target) < 0 && e.button === 0 && state.focused) {
      state.focused = null;
      renderRows();
      renderFormat();
    }
  });


  // 曲の行: チェックした行ならチェックした曲すべて、それ以外はその行だけ (チェックは変えない)
  $("rows").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const i = rowIndexAt(e.target);
    const t = state.rows[i];
    if (!t) return;
    if (state.selected.has(t.id)) {
      openSelectionMenu(e.clientX, e.clientY);
      return;
    }
    state.focused = t.id;
    setAnchor(i);
    renderRows();
    renderFormat();
    openSelectionMenu(e.clientX, e.clientY, false, [t.id]);
  });
  // 一覧の空白・見出し・選択バー: 今の選択に対するメニュー
  for (const id of ["tbody", "thead", "selbar"]) {
    $(id).addEventListener("contextmenu", (e) => {
      e.preventDefault();
      if ((e.target as HTMLElement).closest("#rows .row")) return;
      openSelectionMenu(e.clientX, e.clientY);
    });
  }
  new ResizeObserver(() => fitSelbar()).observe($("selbar"));
  $("btn-sel-more").addEventListener("click", (e) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openSelectionMenu(r.left, r.top - 8, true);
  });
  $("btn-sel-clear").addEventListener("click", () => {
    state.selected.clear();
    renderRows();
    renderSelbar();
  });
  $("btn-sel-export").addEventListener("click", () => exportSelected());
  $("btn-sel-remove").addEventListener("click", () => removeSelected());
  // WebView 標準のメニュー (戻る・再読み込み等) は出さない。入力欄だけは残す
  document.addEventListener("contextmenu", (e) => {
    if (!(e.target as HTMLElement).closest("input, textarea")) e.preventDefault();
  });

  // 列の幅: 見出しの境目をドラッグ。ダブルクリックでその列を標準の幅に戻す
  $("thead").addEventListener("pointerdown", (e) => {
    const grip = (e.target as HTMLElement).closest<HTMLElement>(".col-grip");
    if (grip && e.button === 0) startColumnResize(grip, e);
  });
  $("thead").addEventListener("dblclick", (e) => {
    const grip = (e.target as HTMLElement).closest<HTMLElement>(".col-grip");
    if (grip) resetColumnWidth(grip);
  });
  $("thead").addEventListener("click", (e) => {
    // 境目のドラッグ・ダブルクリックでは並べ替えない
    if ((e.target as HTMLElement).closest(".col-grip")) return;
    const th = (e.target as HTMLElement).closest<HTMLElement>(".th");
    if (!th) return;
    if (th.dataset.colset) {
      const r = th.getBoundingClientRect();
      openColumnMenu(r.right - 220, r.bottom + 4);
      return;
    }
    if (th.dataset.selall) {
      const allSelected = state.rows.length > 0 && state.rows.every((t) => state.selected.has(t.id));
      selectAllVisible(!allSelected);
      return;
    }
    const key = th.dataset.sort as SortKey;
    // アーティスト表示はアルバムごとの並びに固定
    if (state.layout === "artists") return;
    if (key === "order" && !currentPlaylist()) return;
    // プレイリストの並び替えのパネルを開いているときは、見出しで下書きを並べ替える (Issue #22)
    if (state.reorder) {
      if (key !== "order") chooseReorderKey(key);
      return;
    }
    state.sort = state.sort.key === key ? { key, asc: !state.sort.asc } : { key, asc: true };
    computeRows();
    renderTableHead();
    renderRows();
    renderSortButton();
  });

  // サイドバー / アーティスト一覧の幅: 境目をドラッグ。ダブルクリックで元の幅に戻す
  bindResizer($("side-resizer"), "--side-w", "sidebarWidth", SIDE_MIN, SIDE_MAX);
  bindResizer($("artist-resizer"), "--artist-w", "artistListWidth", ARTIST_MIN, ARTIST_MAX);

  // 見出しの右クリック: 表示する項目
  $("thead").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openColumnMenu(e.clientX, e.clientY);
  });

  const sidebarClick = (e: MouseEvent) => {
    const item = (e.target as HTMLElement).closest<HTMLElement>(".side-item");
    // 並べ替えのドラッグを離したときのクリックでは、表示を切り替えない
    if (!item || Date.now() - sideDrag.endedAt < 300) return;
    if (item.dataset.newpl) newPlaylist();
    else if ((e.target as HTMLElement).closest(".tree-chev")) toggleVideoSide(item);
    else if (item.dataset.vnode !== undefined) openVideoNode(item.dataset.vnode);
    else if (item.dataset.view) selectView(item.dataset.view);
  };
  $("lib-list").addEventListener("click", sidebarClick);
  // キーボード: サイドバーの中は ↑ ↓ で表示を切り替える。マウスを押したらキーボードの枠を消す
  document.querySelector<HTMLElement>(".main-side")!.addEventListener("keydown", sidebarKey);
  document.addEventListener("mousedown", () => setKeyboardMode(false), true);
  $("album-grid").addEventListener("mousedown", () => {
    if (state.gridFocus < 0) return;
    state.gridFocus = -1;
    $("album-grid").querySelectorAll(".kb-focus").forEach((c) => c.classList.remove("kb-focus"));
  });
  $("pl-list").addEventListener("click", sidebarClick);
  $("pc-list").addEventListener("click", (e) => {
    const item = (e.target as HTMLElement).closest<HTMLElement>(".side-item");
    if (Date.now() - sideDrag.endedAt < 300) return;
    if (item?.dataset.addpc) addPcFolder();
    else if (item?.dataset.view) selectView(item.dataset.view);
  });
  $("pc-list").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const f = (e.target as HTMLElement).closest<HTMLElement>(".side-item")?.dataset.pc;
    if (!f) return;
    showMenu(e.clientX, e.clientY, [
      { header: f },
      {
        label: tr("読み直す"),
        action: () => {
          state.pc.tracks.delete(f);
          if (currentPcFolders().includes(f)) scanPcFolder(f);
          else selectView(`pc:${f}`);
        },
      },
      { sep: true },
      { label: tr("一覧から外す"), action: () => removePcFolder(f) },
    ]);
  });
  $("btn-add-pc").addEventListener("click", addPcFolder);
  $("pl-list").addEventListener("dblclick", (e) => {
    const id = (e.target as HTMLElement).closest<HTMLElement>(".side-item")?.dataset.pl;
    // ポッドキャストのプレイリストは分類から自動で作るので、名前の変更も削除もしない
    const pl = id ? state.snap?.playlists.find((p) => p.id === id) : null;
    if (!id || !pl || !writable() || pl.podcast) return;
    // スマートプレイリストは条件の画面 (名前もそこで変えられる)
    if (pl.smart) openSmartEditor(id);
    else renamePlaylist(id);
  });
  $("pl-list").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const id = (e.target as HTMLElement).closest<HTMLElement>(".side-item")?.dataset.pl;
    if (!id || !writable()) return;
    const pl = state.snap!.playlists.find((p) => p.id === id)!;
    if (pl.podcast) return;
    showMenu(e.clientX, e.clientY, [
      { header: pl.name },
      ...(pl.smart
        ? [
            { label: tr("ルールを編集…"), action: () => openSmartEditor(id) },
            { label: tr("iTunes 用に書き出す…"), action: () => exportSmartXml([id]) },
          ]
        : []),
      { label: tr(".m3u8 で書き出す…"), action: () => exportM3u8([id]) },
      { label: tr("名前を変更…"), action: () => renamePlaylist(id) },
      { label: tr("削除…"), danger: true, action: () => deletePlaylist(id) },
      { sep: true },
      { label: tr("プレイリストを名前順に並べる"), action: () => void sortPlaylistsByName() },
    ]);
  });

  $("btn-new-pl").addEventListener("click", (e) => openNewPlaylistMenu(e.currentTarget as HTMLElement));
  $("btn-edit-rules").addEventListener("click", () => {
    const pl = currentPlaylist();
    if (pl?.smart) openSmartEditor(pl.id);
  });
  $("btn-sort").addEventListener("click", (e) => openSortMenu(e.currentTarget as HTMLElement));
  $("device-name").addEventListener("dblclick", () => state.snap && renameIpod());
  // 設定のモーダルの外側 (暗い所) を押したら閉じる。中で押して外で離したとき (文字の選択など) は閉じない
  let setDownOutside = false;
  $("settings").addEventListener("mousedown", (e) => (setDownOutside = e.target === e.currentTarget));
  $("settings").addEventListener("click", (e) => {
    if (setDownOutside && e.target === e.currentTarget) closeSettings();
  });
  $("btn-rename").addEventListener("click", renameIpod);
  document.querySelectorAll<HTMLInputElement>('input[name="lib-mode"]').forEach((r) =>
    r.addEventListener("change", () => switchMode(r.value as LibMode)),
  );
  $("btn-settings").addEventListener("click", openSettings);
  $("set-back").addEventListener("click", closeSettings);
  const pickSettingsCat = (e: Event) => {
    const cat = (e.target as HTMLElement).closest<HTMLElement>("[data-cat]")?.dataset.cat as SettingsCat | undefined;
    if (!cat || cat === state.settingsCat) return;
    state.settingsCat = cat;
    renderSettingsNav();
    $("settings-body").scrollTop = 0;
  };
  $("set-nav").addEventListener("click", pickSettingsCat);
  $("set-nav-foot").addEventListener("click", pickSettingsCat);
  $("lib-list").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    if (!state.snap || !(e.target as HTMLElement).closest("[data-lib]")) return;
    showMenu(e.clientX, e.clientY, tidyMenuItems());
  });
  $("btn-add").addEventListener("click", (e) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const kind = isVisualView() ? (isPhotoView() ? "photo" : "video") : "audio";
    showMenu(r.left, r.bottom + 4, [
      { label: tr("ファイルを選択…"), action: () => pickAndTransfer(false, kind) },
      { label: tr("フォルダを選択…"), action: () => pickAndTransfer(true, kind) },
      ...(kind === "photo"
        ? [{ sep: true }, { label: tr("コピーした画像を貼り付け"), shortcut: navigator.userAgent.includes("Mac") ? "⌘V" : "Ctrl+V", action: () => void pasteFromClipboard() }]
        : []),
    ]);
  });
  bindVisual();
  bindReorderPanel();
  renderSearchPlaceholder();
  $("status-format").addEventListener("click", () => {
    const id = $("status-format").dataset.id;
    if (id) showDetails(id);
  });
  $("btn-cancel").addEventListener("click", () => invoke("cancel_transfer"));

  $("btn-eject").addEventListener("click", async () => {
    // 再生中のファイルを開いたままだと取り出せないので止める
    stopPlayer();
    const root = state.snap?.device.root ?? null;
    const btn = $<HTMLButtonElement>("btn-eject");
    const label = btn.innerHTML;
    btn.disabled = true;
    btn.textContent = tr("取り出し中…");
    state.ejecting = true;
    // 取り出しの失敗は理由と「まだ抜かないで」を長めに表示する
    // 戻り値は OS に取り出してもらったか (false はフォルダで開いた iPod を閉じただけ)
    const r = await invoke<boolean>("eject_device").then(
      (ejected) => ejected,
      (e) => {
        failToast(e, 10000);
        return null;
      },
    );
    state.ejecting = false;
    btn.disabled = false;
    btn.innerHTML = label;
    if (r !== null) {
      state.ejectedRoot = root;
      if (r) state.usbEjected = true;
      applySnapshot(null);
      toast(r ? tr("iPod を取り出しました。ケーブルを抜いても安全です") : tr("保存して閉じました"));
    } else {
      // 取り出し失敗でも DB は保存済み。状態を再取得
      applySnapshot(await invoke<Snapshot | null>("snapshot").catch(() => null));
    }
  });

  // フォルダのダイアログは Rust が開く (選んだフォルダだけを open_device で開ける)
  const pickRoot = async () => {
    const dir = await call<string | null>("pick_device_folder");
    if (dir) openDevice(dir);
  };
  $("btn-pick-root").addEventListener("click", pickRoot);
  $("btn-open-root").addEventListener("click", pickRoot);
  $("device-list").addEventListener("click", (e) => {
    const root = (e.target as HTMLElement).closest<HTMLElement>("[data-root]")?.dataset.root;
    if (root) openDevice(root);
  });

  $("layout-toggle").addEventListener("change", (e) => {
    const v = (e.target as HTMLInputElement).value;
    if (v !== "tracks" && v !== "albums" && v !== "artists") return;
    const mouse = layoutByMouse;
    layoutByMouse = false;
    setLayout(v).then(() => mouse && focusMain());
  });
  // マウスで見方を切り替えたら、矢印キーがすぐその一覧で効くように一覧へフォーカスを移す
  // (キーボードで切り替えたとき (detail = 0) は切り替えのボタンに残す)。click は change より先に来る
  let layoutByMouse = false;
  $("layout-toggle").addEventListener("click", (e) => {
    if ((e.target as HTMLElement).closest("input")) layoutByMouse = e.detail > 0;
  });
  $("btn-album-back").addEventListener("click", () => openAlbum(null));
  const grid = $("album-grid");
  // アルバムの一覧は見えている分だけ描くので、スクロールしたら描く範囲を直す
  grid.addEventListener("scroll", () => renderAlbumGrid(true), { passive: true });
  const cardKey = (e: Event) => {
    const i = (e.target as HTMLElement).closest<HTMLElement>(".album-card")?.dataset.album;
    return i === undefined ? undefined : albumGroups()[Number(i)]?.key;
  };
  grid.addEventListener("click", (e) => {
    const key = cardKey(e);
    if (key === undefined) return;
    // カードをドラッグして離したときのクリックでは開かない
    if (Date.now() - drag.endedAt < 300) return;
    if ((e.target as HTMLElement).closest(".album-chk")) {
      toggleAlbumSelection(Number((e.target as HTMLElement).closest<HTMLElement>(".album-card")!.dataset.album));
      return;
    }
    const playBtn = (e.target as HTMLElement).closest<HTMLElement>(".album-play");
    if (playBtn?.dataset.toggle) {
      togglePlay();
      return;
    }
    if (playBtn) {
      const ids = albumTrackIds(key);
      if (ids.length) playTrack(ids[0], ids);
      return;
    }
    openAlbum(key);
  });
  // アーティスト表示: 左の一覧で選ぶ。名前の左のチェック (または Ctrl / ⌘ + クリック) でそのアーティストの全曲をチェック、
  // Shift + クリックで前にチェックしたアーティストからここまで (配布リポジトリの Issue #25)。
  // 右クリックはそのアーティストの全曲へのメニュー。チェックしたアーティストの上なら、チェックした曲全部
  const artistIndex = (e: Event) => {
    const i = (e.target as HTMLElement).closest<HTMLElement>("[data-artist]")?.dataset.artist;
    return i === undefined ? -1 : Number(i);
  };
  $("artist-list").addEventListener("click", (e) => {
    const i = artistIndex(e);
    const a = state.artists[i];
    // ドラッグして落とした直後のクリックでは開かない
    if (!a || Date.now() - drag.endedAt < 300) return;
    if ((e.target as HTMLElement).closest(".artist-chk") || e.ctrlKey || e.metaKey || e.shiftKey) {
      toggleArtistSelection(i, e.shiftKey);
      return;
    }
    selectArtist(a.key);
  });
  $("artist-list").addEventListener("contextmenu", (e) => {
    const i = artistIndex(e);
    const a = state.artists[i];
    if (!a) return;
    e.preventDefault();
    if (artistChecked(i)) {
      openSelectionMenu(e.clientX, e.clientY, false, selectedIds());
      return;
    }
    selectArtist(a.key);
    openSelectionMenu(e.clientX, e.clientY, false, state.rows.map((t) => t.id));
  });
  // アルバムの見出しの再生ボタン / 見出しの右クリック (そのアルバムの曲へのメニュー)
  $("rows").addEventListener("click", (e) => {
    const head = (e.target as HTMLElement).closest<HTMLElement>(".ab-album");
    if (!head || !(e.target as HTMLElement).closest(".ab-play")) return;
    const ids = blockTrackIds(head.dataset.first!);
    if (ids.length) playTrack(ids[0], ids);
  });
  $("rows").addEventListener(
    "contextmenu",
    (e) => {
      const head = (e.target as HTMLElement).closest<HTMLElement>(".ab-album");
      if (!head) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      openSelectionMenu(e.clientX, e.clientY, false, blockTrackIds(head.dataset.first!));
    },
    true,
  );

  grid.addEventListener("contextmenu", (e) => {
    const key = cardKey(e);
    if (key === undefined) return;
    e.preventDefault();
    // 曲を選んだときと同じメニューを出す (チェックは変えない)。チェックしたアルバムの上なら、チェックした曲 (ほかのアルバムも) 全部、
    // そうでなければ押したアルバムの全曲が対象 (複数のアルバムをチェックして「アルバムごとにネットで探す」などを使えるように)
    const album = albumTrackIds(key);
    const checked = album.some((id) => state.selected.has(id)) ? selectedIds() : [];
    openSelectionMenu(e.clientX, e.clientY, false, checked.length ? checked : album);
  });

  let searchTimer = 0;
  $("search").addEventListener("input", (e) => {
    clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => {
      state.search = (e.target as HTMLInputElement).value;
      if (isPodView()) return renderPodView();
      if (isVisualView()) return renderVisualView();
      computeRows();
      renderRows();
      renderStatus();
    }, 120);
  });

  document.addEventListener("keydown", (e) => {
    // メニューが開いていれば、Esc はメニューを閉じるだけ (一覧のチェックを外したりしない)
    if (e.key === "Escape" && document.querySelector(".menu")) {
      closeMenu();
      return;
    }
    // 設定ページでは Esc で戻る以外のショートカットを効かせない
    if (state.settingsOpen) {
      if (e.key === "Escape" && !(e.target as HTMLElement).closest(".modal") && !document.querySelector(".modal")) closeSettings();
      return;
    }
    if ((!state.snap && !isPcView()) || (e.target as HTMLElement).closest("input, .modal")) return;
    if (isVisualView()) return visualKey(e);
    // プレイリストの並び替え中の Esc はキャンセル (変えていれば確かめる)
    if (e.key === "Escape" && state.reorder) {
      void cancelReorder();
      return;
    }
    if (e.key === "Escape" && state.layout === "albums" && state.openAlbum !== null && !state.selected.size) {
      backToAlbums();
      return;
    }
    if (e.key.startsWith("Arrow") || ["Home", "End", "PageUp", "PageDown", "Enter", "Backspace"].includes(e.key)) setKeyboardMode(true);
    if (navKey(e)) return;
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault();
      state.rows.forEach((t) => state.selected.add(t.id));
      renderRows();
      renderSelbar();
    } else if (e.key === " " && player.current) {
      e.preventDefault();
      togglePlay();
    } else if (e.key === "F2" && (isPcView() || (isRockbox() && writable()))) {
      // Windows のエクスプローラーと同じく F2 でファイル名を変更
      const ids = selectedIds();
      if (ids.length) {
        e.preventDefault();
        if (ids.length === 1) renamePcFile(ids[0]);
        else renamePcFiles(ids);
      }
    } else if (e.key === "Enter" && !mod) {
      const ids = selectedIds();
      if (ids.length) {
        e.preventDefault();
        playTrack(ids[0], ids.length > 1 ? ids : undefined);
      }
    } else if (mod && e.key.toLowerCase() === "i") {
      e.preventDefault();
      const ids = selectedIds();
      if (ids.length) editTags(ids);
    } else if (mod && e.key.toLowerCase() === "f") {
      e.preventDefault();
      $("search").focus();
    } else if ((e.key === "Delete" || (e.key === "Backspace" && mod)) && !isPcView()) {
      e.preventDefault();
      removeSelected();
    } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && !mod) {
      e.preventDefault();
      moveCursor(e.key === "ArrowDown" ? 1 : -1, e.shiftKey);
    } else if (e.key === "Escape") {
      state.selected.clear();
      renderRows();
      renderSelbar();
    }
  });
}

export async function removeFromIpod(ids: string[]) {
  const ok = await confirmBox(
    tr("{n} 曲を iPod から削除しますか？", { n: ids.length }),
    tr("iPod 上の音楽ファイルも削除され、すべてのプレイリストから外れます。この操作は取り消せません。"),
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
