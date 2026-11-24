// 曲の一覧 (仮想スクロール。見えている行だけ作る)

import { t as tr } from "../i18n";
import { isPodView } from "../app/podcasts";
import { isVisualView } from "../app/visual";
import { albumGroups, rowAtY, rowTop } from "../app/rows";
import { currentPcFolders, currentPlaylist, isPcView, onIpodIndex, viewKind } from "../app/selectors";
import { $, ROW_H, state } from "../app/state";
import { fmtBytes, fmtDay, fmtDur } from "../domain/format";
import { fileKind, fileNameOf, hasArtwork } from "../domain/track-order";
import { KIND_LABEL } from "../domain/types";
import type { SortKey, Track } from "../domain/types";
import { renderAlbumGrid, renderAlbumGridSkeleton, skelStyle } from "./album-grid";
import { artistBlockHtml, loadBlockArt, renderArtistList } from "./artist-list";
import { artHtml, loadVisibleArtwork, pcArtHtml } from "./artwork-cells";
import { visibleColumns } from "./columns";
import { renderConnectView } from "./device";
import { esc } from "./dom";
import { isFresh, justChecked, justDone, reduceMotion } from "./feedback";
import { ICON_CHECK, ICON_WARN } from "./icons";
import { CHECK_MARK, ICON_PAUSE, ICON_PLAY, eqBars, player } from "./player";
import { updateSelectAllBox } from "./selbar";
import { renderStatus } from "./status";

/** 番号・タイトル・iPod 以外の列の文字 */
function cellText(key: SortKey, t: Track): string {
  switch (key) {
    case "artist":
      return t.artist;
    case "album":
      return t.album;
    case "albumArtist":
      return t.albumArtist;
    case "composer":
      return t.composer;
    case "genre":
      return t.genre;
    case "year":
      return t.year ? String(t.year) : "";
    // 2 枚目以降のディスクは「2-5」
    case "trackNr":
      return t.trackNr ? (t.discNr > 1 ? `${t.discNr}-${t.trackNr}` : String(t.trackNr)) : "";
    case "filetype":
      return fileKind(t);
    case "fileName":
      return fileNameOf(t);
    case "bitrate":
      return t.bitrate ? `${t.bitrate} kbps` : "";
    case "size":
      return t.size ? fmtBytes(t.size) : "";
    case "duration":
      return fmtDur(t.durationMs);
    case "dateAdded":
      return fmtDay(t.dateAdded);
    case "rating":
      return t.rating ? "★".repeat(Math.round(t.rating / 20)) : "";
    case "mediaKind":
      return t.kind && t.kind !== "music" ? tr(KIND_LABEL[t.kind]) : "";
    case "plays":
      return t.playCount ? String(t.playCount) : "";
    case "artwork":
      return hasArtwork(t) ? "" : tr("なし");
    default:
      return "";
  }
}

/** 今描いている行の範囲。スクロールだけのときは、同じ一覧・同じ列なら、見えなくなった行を外して新しく見える行だけを足す
 * (毎回 50 行ほどを作り直すと、遅い PC ではホイール 1 回ごとに画面が止まっていた) */
let renderedRows: { rows: Track[]; cols: string; first: number; last: number } | null = null;

/** scrollOnly: スクロールと大きさの変更。見える行だけ描き直し、曲数・合計・全選択の印 (全曲を回す) は直さない */
export function renderRows(scrollOnly = false) {
  if (isPodView() || isVisualView()) return;
  const body = $("tbody");
  const rowsEl = $("rows");
  const rows = state.rows;
  // 読み込み中でまだ曲が無い (PC のフォルダを読んでいる・iPod を開いている途中) ときは、一覧の形のスケルトンを出す
  const scanningNow = isPcView() && !!state.pc.scanning && currentPcFolders().includes(state.pc.scanning);
  const ipodLoading = !isPcView() && !state.snap && state.opening && !state.devNoDrive;
  const skeleton = rows.length === 0 && (scanningNow || ipodLoading || state.devSkeleton) && !state.search;
  // iPod の一覧を開いていて iPod が無いときは、一覧の代わりに接続の案内
  const connect = !isPcView() && (!state.snap || state.devNoDrive) && !ipodLoading && !state.devSkeleton;
  // アルバムの一覧 (曲が無いときは空の案内を出すため一覧のまま。読み込み中はカードの形のスケルトン)
  const albumsShown = !connect && state.layout === "albums" && state.openAlbum === null;
  const grid = albumsShown && (rows.length > 0 || skeleton);
  // 見える範囲を測る前に一覧を出しておく。隠れているあいだは scrollTop が 0 になり、
  // 出し直したときに前のスクロール位置へ戻るので、先頭の行だけ描いて画面が空になる
  $("list-area").classList.toggle("hidden", connect || grid);
  $("view-switch").classList.toggle("hidden", connect);
  // 接続の案内も先に出し入れする (残っていると一覧の高さが足りず、スケルトンの行が途中までしか出ない)
  $("connect-view").classList.toggle("hidden", !connect);
  // 読み込み中のアーティスト表示は、左の一覧を出さずに行のスケルトンだけ
  const artistsView = !connect && !skeleton && state.layout === "artists";
  $("artist-list").classList.toggle("hidden", !artistsView);
  $("artist-resizer").classList.toggle("hidden", !artistsView);
  if (artistsView) renderArtistList();
  const lay = state.rowLayout;
  rowsEl.style.height = `${lay ? lay.height : rows.length * ROW_H}px`;
  const top = body.scrollTop;
  const h = body.clientHeight || 600;
  const first = Math.max(0, rowAtY(top) - 10);
  const last = Math.min(rows.length, rowAtY(top + h) + 11);
  const inPlaylistOrder = !!currentPlaylist() && state.sort.key === "order";
  // PC のフォルダ表示: iPod に同じ曲がある行に印を付ける
  const onIpod = isPcView() ? onIpodIndex() : null;
  const cols = visibleColumns();
  const rowHtml = (i: number) => {
    const t = rows[i];
    const sel = (state.selected.has(t.id) ? " selected" : "") + (state.focused === t.id ? " focused" : "");
    const idx = inPlaylistOrder ? i + 1 : t.trackNr || "";
    const sending = onIpod ? state.sending.get(t.pcPath!) : undefined;
    const have = sending === "done" || (onIpod?.(t) ?? false);
    const playing = player.current === t.id ? " playing" : "";
    const jc = justChecked.get(t.id);
    const chkAnim = jc && isFresh(jc.until) && state.selected.has(t.id) ? " chk-anim" : "";
    const pop = onIpod && isFresh(justDone.get(t.pcPath!)) ? " pop" : "";
    const cells = cols
      .map((c) => {
        switch (c.key) {
          case "order":
            return `<div class="cell idx num"><span class="idx-n">${idx}</span></div>`;
          case "title":
            // ジャケットにマウスを乗せると、暗くして再生ボタンを重ねる。
            // 再生中の曲は動くバーを重ねておき、乗せると一時停止 / 再開のボタン (どちらを出すかは CSS)
            return `<div class="cell title" title="${esc(t.pcPath ?? t.title)}"><span class="art-wrap">${
              onIpod ? pcArtHtml(t.pcPath!) : artHtml(t.artworkId)
            }${
              player.current === t.id
                ? `<span class="row-eq">${eqBars()}</span><button class="row-play" data-toggle="1" aria-label="${esc(tr("一時停止 / 再開"))}"><span class="i-pause">${ICON_PAUSE}</span><span class="i-play">${ICON_PLAY}</span></button>`
                : `<button class="row-play" title="${esc(tr("再生"))}" aria-label="${esc(tr("再生"))}">${ICON_PLAY}</button>`
            }</span><span>${esc(t.title)}</span>${
              t.readError
                ? `<span class="read-error" title="${esc(`${tr("タグを読めませんでした (押すと詳細情報)")}\n${t.readError}`)}">${ICON_WARN}</span>`
                : ""
            }</div>`;
          case "onIpod":
            // PC の曲: iPod に同じ曲があれば「転送済み」
            return `<div class="cell on-ipod">${
              have
                ? `<span class="ipod-badge${pop}" title="${esc(tr("iPod に同じ曲があります"))}">${ICON_CHECK}${esc(tr("転送済み"))}</span>`
                : sending === "working"
                  ? `<span class="ipod-badge working"><span class="spinner small"></span>${esc(tr("転送中"))}</span>`
                  : ""
            }</div>`;
          default:
            return `<div class="cell${c.num ? " num" : ""}">${esc(cellText(c.key, t))}</div>`;
        }
      })
      .join("");
    // 縞は行の番号で決める (要素の順は、スクロールで行を足していくと番号の順にならない)
    return `<div class="row${i % 2 ? " alt" : ""}${sel}${playing}${chkAnim}" data-i="${i}" style="top:${rowTop(i)}px${chkAnim ? `;--d:${jc!.delay}ms` : ""}">
      <div class="cell chk"><span class="box">${CHECK_MARK}</span></div>${cells}
    </div>`;
  };
  const colKey = cols.map((c) => c.key).join(",");
  const prev = renderedRows;
  // 読み込み中の仮の行を出しているあいだは作り直さない (光る動きが途中から始め直しにならないように)
  const keepSkeleton = skeleton && rowsEl.firstElementChild?.classList.contains("skel") && rowsEl.dataset.skelCols === colKey;
  if (scrollOnly && !lay && prev && prev.rows === rows && prev.cols === colKey && first < prev.last && last > prev.first) {
    // スクロールだけ: 見えなくなった行を外し、新しく見える行だけを足す (行は top で置くので、要素の順は問わない)
    for (const el of [...rowsEl.children] as HTMLElement[]) {
      const i = Number(el.dataset.i);
      if (i < first || i >= last) el.remove();
    }
    let add = "";
    for (let i = first; i < Math.min(prev.first, last); i++) add += rowHtml(i);
    for (let i = Math.max(prev.last, first); i < last; i++) add += rowHtml(i);
    if (add) rowsEl.insertAdjacentHTML("beforeend", add);
    renderedRows = { rows, cols: colKey, first, last };
  } else {
    let html = "";
    for (let i = first; i < last; i++) html += rowHtml(i);
    // アーティスト表示: 見えている範囲の見出し (アーティスト名・アルバム)
    if (lay) html += lay.blocks.filter((b) => b.top + b.h >= top - 200 && b.top <= top + h + 200).map(artistBlockHtml).join("");
    if (!keepSkeleton) rowsEl.innerHTML = html;
    renderedRows = lay || keepSkeleton ? null : { rows, cols: colKey, first, last };
  }
  loadVisibleArtwork();
  if (lay) loadBlockArt();

  $("album-grid").classList.toggle("hidden", !grid);
  if (grid) {
    if (skeleton) renderAlbumGridSkeleton();
    else renderAlbumGrid();
  }
  $("connect-view").classList.toggle("hidden", !connect);
  if (connect) renderConnectView();
  const empty = $("empty-view");
  // 読み込み中は、空の案内の代わりに仮の行 (スケルトン) を出す
  if (skeleton && !keepSkeleton && !grid) {
    // 本物の行と同じ形 (チェック・表示中の列・ジャケットの枠) にして、文字の所だけを仮の棒にする
    const n = Math.max(6, Math.ceil(h / ROW_H));
    rowsEl.style.height = `${n * ROW_H}px`;
    rowsEl.dataset.skelCols = colKey;
    const bar = (w: number) => `<span class="sk skel" style="width:${w}%"></span>`;
    rowsEl.innerHTML = Array.from({ length: n }, (_, i) => {
      const cells = cols
        .map((c, j) => {
          const w = 35 + ((i * 37 + j * 53) % 45);
          switch (c.key) {
            case "title":
              return `<div class="cell title"><span class="art-wrap"><span class="art skel"${skelStyle()}></span></span>${bar(w + 10)}</div>`;
            // 転送済みの印は読み終えてから出す (読み込み中は空)
            case "onIpod":
              return `<div class="cell on-ipod"></div>`;
            default:
              return `<div class="cell${c.num ? " num" : ""}">${bar(c.num ? 40 + ((i * 29) % 30) : w)}</div>`;
          }
        })
        .join("");
      // 光の帯は、ジャケットの読み込み中と同じもの。どの棒もそろって流れるよう、始める位置を行に入れて引き継ぐ
      return `<div class="row skel${i % 2 ? " alt" : ""}" style="top:${i * ROW_H}px;--skel-at:-${Math.round(performance.now() % 1400)}ms"><div class="cell chk"><span class="box"></span></div>${cells}</div>`;
    }).join("");
  }
  empty.classList.toggle("hidden", rows.length > 0 || skeleton || grid);
  if (rows.length === 0) {
    const scanning = scanningNow;
    // 見出しの 1 行と、小さい補足 (動画・写真の空の表示と同じ形)
    const [text, sub] = state.search
      ? [tr("一致する曲がありません"), ""]
      : isPcView()
        ? scanning
          ? [tr("フォルダを読み込み中… {progress}", { progress: state.pc.progress }), ""]
          : [tr("このフォルダには音楽ファイルがありません"), ""]
        : currentPlaylist()
        ? [tr("ここに曲やフォルダをドラッグ＆ドロップ"), tr("ライブラリの曲をこのプレイリストへドラッグしても入れられます")]
        : viewKind() === "audiobook"
        ? [tr("オーディオブックはありません"), tr("曲の情報を編集の「分類」でオーディオブックにできます")]
        : viewKind() === "podcast"
        ? [tr("ポッドキャストはありません"), tr("曲の情報を編集の「分類」でポッドキャストにできます")]
        : [tr("ここに曲やフォルダをドラッグ＆ドロップ"), ""];
    $("empty-view-text").textContent = text;
    $("empty-view-sub").textContent = sub;
    $("empty-view-sub").classList.toggle("hidden", !sub);
  }
  if (scrollOnly) return;
  $("view-count").textContent = connect
    ? ""
    : skeleton
      ? tr("読み込み中… {progress}", { progress: scanningNow ? state.pc.progress : "" })
      : grid
      ? `${tr("{n} アルバム", { n: albumGroups().length })} · ${tr("{n} 曲", { n: rows.length })}`
      : artistsView
        ? `${tr("{n} アーティスト", { n: state.artists.length })} · ${tr("{n} 曲", { n: state.artists.reduce((a, x) => a + x.tracks.length, 0) })}`
        : tr("{n} 曲", { n: rows.length });
  state.countShown = !connect && !skeleton && rows.length > 0;
  renderStatus();
  updateSelectAllBox();
}

/**
 * 行の並びが変わる操作 (`update`) の前後で、見えている行を元の位置から新しい位置へ動かす (FLIP)。
 * プレイリストを並べ替えたとき、どう並び替わったかが目で追えるように (配布リポジトリの Issue #22)。
 * 前は見えていなかった行は、その場で薄く出す
 */
export function animateRowMoves(update: () => void) {
  if (reduceMotion) return update();
  const body = $("tbody");
  const before = new Map<string, number>();
  body.querySelectorAll<HTMLElement>(".row[data-i]").forEach((el) => {
    const t = state.rows[Number(el.dataset.i)];
    if (t) before.set(t.id, el.offsetTop);
  });
  update();
  // 遠くから来る行は画面の高さまでに縮める (一覧の端から端へ飛ぶと目で追えない)
  const far = body.clientHeight;
  body.querySelectorAll<HTMLElement>(".row[data-i]").forEach((el) => {
    const t = state.rows[Number(el.dataset.i)];
    if (!t) return;
    const from = before.get(t.id);
    if (from === undefined) {
      el.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 240, easing: "ease-out" });
      return;
    }
    const dy = Math.max(-far, Math.min(far, from - el.offsetTop));
    if (dy) el.animate([{ transform: `translateY(${dy}px)` }, { transform: "none" }], { duration: 360, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" });
  });
}
