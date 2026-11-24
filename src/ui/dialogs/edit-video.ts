// 動画の情報の編集とアートワーク。iPod OS は iTunes の「情報を見る」と同じ形 (詳細・アートワーク・説明・オプション・
// 読みがな・ファイル) で iTunesDB に書く。Rockbox は動画のタグを読まないので、タイトル (ファイル名) と番組名 (フォルダ) だけ

import { lang, t as tr } from "../../i18n";
import { isRockbox, writable } from "../../app/selectors";
import { state } from "../../app/state";
import { setVideos, vis } from "../../app/visual";
import { invoke, openDialog } from "../../infra/tauri";
import { fmtBytes } from "../../domain/format";
import type { Snapshot } from "../../domain/types";
import { VIDEO_KINDS } from "../../domain/visual";
import type { VideoEdit, VideoFileInfo, VideoKind, VideoView } from "../../domain/visual";
import { esc, isEnter } from "../dom";
import { call, failToast, toast, withArtProgress } from "../feedback";
import { ICON_VIDEO } from "../icons";
import type { MenuItem } from "../menu";
import { busySave, modal } from "../modal";
import type { SaveBusy } from "../modal";
import { applySnapshot } from "../render";
import { placeSegThumb } from "../segments";
import { canRate } from "../selection-actions";
import { searchArtwork } from "./artwork-search";
import { artPageHtml, bindRatingStars, bindReadingTools, ratingStarsHtml, readingHeadHtml } from "./edit-tags";
import { bindPlayOptions, fmtTime, playOptionsHtml } from "./play-options";

type TextKey =
  | "title"
  | "artist"
  | "albumArtist"
  | "album"
  | "composer"
  | "show"
  | "episodeId"
  | "grouping"
  | "genre"
  | "comment"
  | "description"
  | "sortTitle"
  | "sortArtist"
  | "sortAlbumArtist"
  | "sortAlbum"
  | "sortComposer"
  | "sortShow";
type NumKey = "year" | "trackNr" | "discNr" | "season" | "episode";
type Key = TextKey | NumKey;

interface Field {
  key: Key;
  label: string;
  num?: boolean;
  /** 数字の項目と同じ幅 (3 つ横に並べる) */
  narrow?: boolean;
  /** 1 本のときだけ出す */
  single?: boolean;
  /** 読みの項目 */
  sort?: TextKey;
  /** この種類のときだけ出す (無ければいつも)。iTunes と同じく、その種類で意味のある項目だけを出す。
   * 種類の違う動画をまとめて選んだときは全部出す。隠した項目の値は変えない */
  kinds?: VideoKind[];
}

const MV: VideoKind[] = ["musicvideo"];
const TV: VideoKind[] = ["tvshow"];
// tr: 使う側で訳す
const F: Record<string, Field> = {
  title: { key: "title", label: "タイトル", single: true, sort: "sortTitle" },
  // ムービーでは「監督」と呼ぶ (iTunes と同じ。iTunesDB では同じアーティストの欄)。iTunes のテレビ番組には無い
  artist: { key: "artist", label: "アーティスト", sort: "sortArtist", kinds: ["movie", "musicvideo"] },
  show: { key: "show", label: "番組", sort: "sortShow", kinds: TV },
  season: { key: "season", label: "シーズン", num: true, kinds: TV },
  episode: { key: "episode", label: "エピソード", num: true, single: true, kinds: TV },
  episodeId: { key: "episodeId", label: "エピソード ID", narrow: true, single: true, kinds: TV },
  album: { key: "album", label: "アルバム", sort: "sortAlbum", kinds: MV },
  albumArtist: { key: "albumArtist", label: "アルバムアーティスト", sort: "sortAlbumArtist", kinds: MV },
  composer: { key: "composer", label: "作曲者", sort: "sortComposer", kinds: MV },
  grouping: { key: "grouping", label: "グループ" },
  genre: { key: "genre", label: "ジャンル" },
  year: { key: "year", label: "年", num: true },
  trackNr: { key: "trackNr", label: "トラック番号", num: true, single: true, kinds: MV },
  discNr: { key: "discNr", label: "ディスク番号", num: true, kinds: MV },
  comment: { key: "comment", label: "コメント" },
};
/** 読みがなのページに並べる項目 (iTunes の「読みがな」と同じ順) */
const SORTED: Field[] = [F.title, F.artist, F.albumArtist, F.album, F.composer, F.show];

/** ムービーとテレビ番組は、iTunes と同じく「シャッフル時にスキップ」「再生位置を記憶」を既定でオンにする */
const longForm = (k: VideoKind) => k !== "musicvideo";

/** リリース日 (UNIX 秒) と日付の入力欄 (YYYY-MM-DD)。日付だけなので UTC で持つ (時差で前の日にならないように) */
const dayValue = (sec: number) => (sec ? new Date(sec * 1000).toISOString().slice(0, 10) : "");
const daySec = (v: string) => (v ? Math.floor(Date.parse(`${v}T00:00:00Z`) / 1000) : 0);

// ---------------- アートワーク ----------------

/** iPod OS で、動画のアートワークを変えられるか */
export function videoArtEditable(): boolean {
  return writable() && !isRockbox() && !!state.snap?.artworkSupported;
}

/** アートワークを変えたあと: 動画の一覧を読み直して (画像の ID が変わる)、iPod の状態と一緒に画面へ */
async function refreshVideos(s: Snapshot) {
  const list = await call<VideoView[]>("list_videos");
  if (list) setVideos(list);
  applySnapshot(s);
}

/** アートワークの操作。選んだらすぐ iPod に書く (情報の編集の保存のボタンを待たない。曲の右クリックのメニューと同じ)。
 * `done` は変えたあとに呼ぶ (情報の編集の画面の画像を描き直す) */
const videoArt = {
  async pick(list: VideoView[], done?: () => void) {
    const f = await openDialog({
      multiple: false,
      title: tr("アートワークにする画像"),
      filters: [{ name: tr("画像"), extensions: ["jpg", "jpeg", "png", "webp", "bmp", "gif"] }],
    });
    if (typeof f !== "string") return;
    const ids = list.map((v) => v.id);
    const s = await withArtProgress(tr("書き込み中…"), () => call<Snapshot>("set_artwork", { ids, path: f }));
    if (!s) return;
    await refreshVideos(s);
    done?.();
    toast(tr("{n} 本の動画のアートワークを設定しました", { n: ids.length }));
  },
  search(list: VideoView[], done?: () => void) {
    const v0 = list[0];
    // ミュージックビデオはそのアルバム (無ければ曲名) のジャケットを探すことが多い
    void searchArtwork(
      list.map((v) => v.id),
      {
        artist: v0.artist || v0.albumArtist,
        album: v0.album || v0.title,
        applied: async (s) => {
          await refreshVideos(s);
          done?.();
        },
      },
    );
  },
  async remove(list: VideoView[], done?: () => void) {
    const s = await withArtProgress(tr("削除中…"), () => call<Snapshot>("remove_artwork", { ids: list.map((v) => v.id) }));
    if (!s) return;
    await refreshVideos(s);
    done?.();
  },
};

/** 右クリックの「アートワーク」 */
export function videoArtMenuItems(list: VideoView[]): MenuItem[] {
  const items: MenuItem[] = [
    { label: tr("ネットで探す…"), action: () => videoArt.search(list) },
    { label: tr("画像ファイルから設定…"), action: () => void videoArt.pick(list) },
  ];
  if (list.some((v) => v.artworkId)) items.push({ sep: true }, { label: tr("削除"), danger: true, action: () => void videoArt.remove(list) });
  return items;
}

// ---------------- 情報の編集 ----------------

/**
 * 動画の情報の編集。複数を選んだときは、値が揃っていない項目は空欄 (複数の値) で出し、
 * 書き換えた項目だけを全部に反映する (タイトル・エピソード・エピソード ID・トラック番号・開始 / 停止は 1 本のときだけ)
 */
export function editVideos(ids: string[]) {
  const list = (vis.videos ?? []).filter((v) => ids.includes(v.id));
  if (!list.length || !writable()) return;
  if (isRockbox()) return editRockboxVideos(list);
  const one = list.length === 1 ? list[0] : null;
  const shownOf = (v: VideoView, k: Key) => {
    const x = v[k];
    return typeof x === "number" ? (x ? String(x) : "") : x;
  };
  const common = (k: Key) => {
    const x = shownOf(list[0], k);
    return list.every((v) => shownOf(v, k) === x) ? x : null;
  };
  const same = <T>(of: (v: VideoView) => T): T | null => (list.every((v) => of(v) === of(list[0])) ? of(list[0]) : null);
  const kindsAttr = (kinds?: VideoKind[]) => (kinds ? ` data-kinds="${kinds.join(" ")}"` : "");
  const multi = (v: string | null) => (v === null ? ` placeholder="${esc(tr("複数の値"))}"` : "");
  const field = (f: Field) => {
    if (f.single && !one) return "";
    const v = common(f.key);
    return `<div class="field${f.num || f.narrow ? " te-num" : ""}"${kindsAttr(f.kinds)}><label for="ve-${f.key}" data-label="${f.key}">${esc(tr(f.label))}</label>
           <input type="text" id="ve-${f.key}" data-k="${f.key}" ${f.num ? 'inputmode="numeric"' : ""} value="${esc(v ?? "")}"${multi(v)}></div>`;
  };
  const fields = (...fs: Field[]) => fs.map(field).join("");

  // ---- 見出し
  const sub = one
    ? one.kind === "tvshow"
      ? [one.show, one.season || one.episode ? tr("シーズン {s}、エピソード {e}", { s: one.season, e: one.episode }) : ""].filter(Boolean).join(" · ")
      : [one.artist, one.album].filter(Boolean).join(" — ")
    : tr("書き換えた項目だけを全部の動画に反映します。空欄の「複数の値」は変更しません。");
  const head = `<div class="te-head">${one ? `<span class="te-art none" aria-hidden="true">${ICON_VIDEO}</span>` : ""}<div class="te-head-text"><h3>${esc(
    one ? one.title : tr("{n} 本の動画の情報を編集", { n: list.length }),
  )}</h3><div class="te-head-sub${one ? "" : " wrap"}">${esc(sub)}</div></div></div>`;

  // ---- タブ (iTunes の「情報を見る」と同じ並び。ファイルは 1 本のときだけ)
  // tr: 使う側で訳す
  const pages: [string, string][] = [
    ["detail", "詳細"],
    ["artwork", "アートワーク"],
    ["desc", "説明"],
    ["options", "オプション"],
    ["yomi", "読みがな"],
    ...(one ? ([["file", "ファイル"]] as [string, string][]) : []),
  ];
  const tabs = `<div class="te-tabs"><div class="seg ve-tabs" role="radiogroup" aria-label="${esc(tr("表示する情報"))}">${pages
    .map(([k, label], i) => `<label class="seg-item"><input type="radio" name="ve-tab" value="${k}" ${i ? "" : "checked"} /><span>${esc(tr(label))}</span></label>`)
    .join("")}</div></div>`;

  // ---- 詳細
  const released = same((v) => v.released);
  const stats = one
    ? `<div class="set-info ve-stats">
         <span>${esc(tr("再生時間"))}</span><span>${esc(fmtTime(one.durationMs - (one.durationMs % 1000)))}</span>
         <span>${esc(tr("再生回数"))}</span><span class="ve-plays"><span class="ve-plays-n">${one.playCount.toLocaleString()}</span>${
           one.playCount || one.lastPlayed ? `<button type="button" class="btn mini" data-x="reset-plays">${esc(tr("リセット"))}</button>` : ""
         }</span>
       </div>`
    : "";
  const detail = `
     <div class="te-grid">${fields(F.title, F.artist, F.show)}</div>
     <div class="te-grid"${kindsAttr(TV)}>${fields(F.season, F.episode, F.episodeId)}
       <div class="field te-num"><label for="ve-released">${esc(tr("リリース日"))}</label><input type="date" id="ve-released" value="${esc(dayValue(released ?? 0))}"></div>
     </div>
     <div class="te-grid">${fields(F.album, F.albumArtist, F.composer, F.grouping)}</div>
     <div class="te-grid te-detail">${fields(F.genre, F.year, F.trackNr, F.discNr)}</div>
     ${canRate() ? `<div class="te-extra">${ratingStarsHtml()}</div>` : ""}
     ${stats}
     <div class="te-grid">${fields(F.comment)}</div>`;

  // ---- アートワーク
  const artPage = state.snap?.artworkSupported
    ? artPageHtml(
        ICON_VIDEO,
        one ? tr("選ぶとすぐ iPod に書き込みます (保存のボタンを押さなくても変わります)。") : tr("選んだ {n} 本の動画に同じ画像を設定します。選ぶとすぐ iPod に書き込みます。", { n: list.length }),
        true,
      )
    : `<p class="te-kind-note muted">${esc(tr("この iPod はアートワークに対応していません"))}</p>`;

  // ---- 説明
  const desc = common("description");
  const descPage = `<textarea class="te-lyrics" id="ve-description" data-k="description" aria-label="${esc(tr("説明"))}"${multi(desc)}>${esc(desc ?? "")}</textarea>`;

  // ---- オプション
  const kind = same((v) => v.kind);
  const options = playOptionsHtml(
    list,
    `<div class="field"><label for="ve-kind">${esc(tr("メディアの種類"))}</label><select id="ve-kind">${kind ? "" : `<option value="" selected>${esc(tr("複数の値"))}</option>`}${VIDEO_KINDS.map(
      ([k, label]) => `<option value="${k}" ${k === kind ? "selected" : ""}>${esc(tr(label))}</option>`,
    ).join("")}</select></div>`,
    one ? one.durationMs : null,
  );

  // ---- 読みがな (表記は詳細の欄を映して見せるだけ。読みを自動で入れる・説明は最初の行に)
  const yomi = `<div class="te-grid">${SORTED.filter((f) => one || !f.single)
    .map((f, n) => {
      const sv = common(f.sort!);
      const v = common(f.key);
      return `<div class="field te-has-sort"${kindsAttr(f.kinds)}><label data-label="${f.key}">${esc(tr(f.label))}</label>
           <input type="text" data-mirror="${f.key}" value="${esc(v ?? "")}" disabled${multi(v)}></div>
         <div class="field te-sort"${kindsAttr(f.kinds)}>${n === 0 ? readingHeadHtml(`ve-${f.sort}`, true) : `<label class="te-sort-spacer" aria-hidden="true">&nbsp;</label>`}
           <input type="text" id="ve-${f.sort}" data-k="${f.sort}" aria-label="${esc(tr("{a}の読み", { a: tr(f.label) }))}" value="${esc(sv ?? "")}" placeholder="${sv === null ? tr("複数の値") : tr("表記どおり")}"></div>`;
    })
    .join("")}</div>`;

  // ---- ファイル (1 本のとき)。映像と音声は開いたときに読む
  const date = (sec: number) => (sec ? new Date(sec * 1000).toLocaleString(lang === "ja" ? "ja-JP" : undefined) : "—");
  const fileRows = one
    ? (
        [
          [tr("種類"), one.filetype],
          [tr("映像"), "…"],
          [tr("音声"), "…"],
          [tr("再生時間"), fmtTime(one.durationMs - (one.durationMs % 1000))],
          [tr("サイズ"), tr("{a} ({n} バイト)", { a: fmtBytes(one.size), n: one.size.toLocaleString() })],
          [tr("ビットレート"), one.bitrate ? `${one.bitrate} kbps` : ""],
          [tr("サンプルレート"), one.sampleRate ? `${(one.sampleRate / 1000).toLocaleString(undefined, { maximumFractionDigits: 3 })} kHz` : ""],
          [tr("場所"), one.ipodPath],
          [tr("追加日"), date(one.dateAdded)],
          [tr("変更日"), date(one.dateModified)],
          [tr("最後に再生"), date(one.lastPlayed)],
        ] as [string, string][]
      )
        .filter(([, v]) => v)
        .map(([k, v], i) => `<span>${esc(k)}</span><span${i === 1 ? ' data-f="video"' : i === 2 ? ' data-f="audio"' : ""}>${esc(v)}</span>`)
        .join("")
    : "";

  const page = (k: string, html: string, cls = "") => `<div class="te-page${cls}${k === "detail" ? "" : " te-off"}" data-page="${k}" role="tabpanel">${html}</div>`;
  const body = `${tabs}<div class="te-pages">
     ${page("detail", detail)}
     ${page("artwork", artPage)}
     ${page("desc", descPage, " te-lyrics-page")}
     ${page("options", options)}
     ${page("yomi", yomi)}
     ${one ? page("file", `<div class="set-info dt-grid ve-file">${fileRows}</div>`) : ""}
   </div>`;

  modal(
    `${head}${body}
     <div class="modal-actions"><p class="te-where">${esc(tr("iPod のデータベースに保存します。PC の元のファイルは変わりません。"))}</p><button class="btn" data-x="cancel">${esc(
       tr("キャンセル"),
     )}</button><button class="btn primary" data-x="ok">${esc(tr("保存"))}</button></div>`,
    (root, close) => {
      const q = <T extends HTMLElement>(sel: string) => root.querySelector<T>(sel);
      const inputs = [...root.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("[data-k]")];
      const touched = new Set<Key>();
      inputs.forEach((i) => i.addEventListener("input", () => touched.add(i.dataset.k as Key)));
      const input = (k: Key) => q<HTMLInputElement>(`[data-k="${k}"]`);
      /** 今の種類では出していない項目 (保存でも送らない) */
      const hiddenField = (el: Element) => !!el.closest("[data-kinds].hidden");

      // ---- タブ
      const seg = q<HTMLElement>(".ve-tabs")!;
      const showPage = (name: string) => {
        seg.querySelector<HTMLInputElement>(`input[value="${name}"]`)!.checked = true;
        root.querySelectorAll<HTMLElement>(".te-page").forEach((p) => p.classList.toggle("te-off", p.dataset.page !== name));
        placeSegThumb(seg);
        // 読みがなの表記は、詳細で書き換えた値を映す
        if (name === "yomi") root.querySelectorAll<HTMLInputElement>("[data-mirror]").forEach((m) => (m.value = input(m.dataset.mirror as Key)?.value ?? m.value));
        if (name === "file") loadFileInfo();
      };
      placeSegThumb(seg, false);
      seg.addEventListener("change", () => showPage(seg.querySelector<HTMLInputElement>("input:checked")!.value));
      bindReadingTools(
        root,
        () =>
          SORTED.map((f) => ({ src: input(f.key)!, sort: input(f.sort!)! })).filter((p) => p.src && p.sort && !hiddenField(p.sort)),
        (el) => touched.add(el.dataset.k as Key),
      );

      // ---- 種類と、種類で決まる項目
      const kindSel = q<HTMLSelectElement>("#ve-kind")!;
      const kindValue = () => kindSel.value as VideoKind | "";
      const opts = bindPlayOptions(root, list, one ? one.durationMs : null, () => showPage("options"));
      const applyKind = () => {
        const k = kindValue() || null;
        root.querySelectorAll<HTMLElement>("[data-kinds]").forEach((el) => el.classList.toggle("hidden", !!k && !el.dataset.kinds!.split(" ").includes(k)));
        root.querySelectorAll<HTMLElement>('[data-label="artist"]').forEach((l) => (l.textContent = k === "movie" ? tr("監督") : tr("アーティスト")));
      };
      applyKind();
      kindSel.addEventListener("change", () => {
        const k = kindValue();
        opts.kindChanged(!k || list.every((v) => v.kind === k) ? null : longForm(k));
        applyKind();
      });
      const first = inputs.find((i) => !hiddenField(i));
      first?.focus();
      if (first instanceof HTMLInputElement) first.select();

      // ---- 評価・再生回数
      const stars = canRate() ? bindRatingStars(root, list.map((v) => v.rating)) : null;
      let resetPlays = false;
      q('[data-x="reset-plays"]')?.addEventListener("click", (e) => {
        resetPlays = true;
        q(".ve-plays-n")!.textContent = "0";
        (e.currentTarget as HTMLButtonElement).replaceWith(Object.assign(document.createElement("span"), { className: "muted", textContent: tr("保存すると 0 に戻します") }));
      });

      // ---- アートワーク (選んだらすぐ iPod に書く)
      const paintArt = async () => {
        const v = one ? vis.videos?.find((x) => x.id === one.id) : null;
        const url = v?.artworkId && state.snap?.artworkSupported ? await invoke<string | null>("get_artwork", { imageId: v.artworkId, large: true }).catch(() => null) : null;
        const big = q<HTMLElement>(".te-art-big");
        if (big) big.innerHTML = url ? `<img src="${url}" alt="">` : `<span class="te-art-none" aria-hidden="true">${ICON_VIDEO}</span>`;
        const small = q<HTMLElement>(".te-head .te-art");
        if (small) small.outerHTML = url ? `<img class="te-art fade" src="${url}" alt="">` : `<span class="te-art none" aria-hidden="true">${ICON_VIDEO}</span>`;
        const rm = q<HTMLButtonElement>('[data-x="art-remove"]');
        if (rm) rm.disabled = !(vis.videos ?? []).some((x) => ids.includes(x.id) && x.artworkId);
      };
      const nowList = () => {
        const l = (vis.videos ?? []).filter((v) => ids.includes(v.id));
        return l.length ? l : list;
      };
      q('[data-x="art-pick"]')?.addEventListener("click", () => void videoArt.pick(nowList(), paintArt));
      q('[data-x="art-search"]')?.addEventListener("click", () => videoArt.search(nowList(), paintArt));
      q('[data-x="art-remove"]')?.addEventListener("click", () => void videoArt.remove(nowList(), paintArt));
      void paintArt();

      // ---- ファイル (開いたときに 1 回だけ映像と音声を読む)
      let fileLoaded = false;
      const loadFileInfo = async () => {
        if (fileLoaded || !one) return;
        fileLoaded = true;
        const f = await invoke<VideoFileInfo | null>("video_file_info", { id: one.id }).catch(() => null);
        const set = (k: string, v: string) => {
          const el = q<HTMLElement>(`[data-f="${k}"]`);
          if (el) el.textContent = v || "—";
        };
        const ch = (n: number) => (n === 1 ? tr("モノラル") : n === 2 ? tr("ステレオ") : n ? `${n} ch` : "");
        set("video", f ? [f.video, f.width ? `${f.width}×${f.height}` : "", f.fps ? `${Math.round(f.fps * 100) / 100} fps` : ""].filter(Boolean).join(" · ") : "");
        set("audio", f?.audio ? [f.audio.toUpperCase(), f.audioRate ? `${(f.audioRate / 1000).toLocaleString(undefined, { maximumFractionDigits: 3 })} kHz` : "", ch(f.channels)].filter(Boolean).join(" · ") : "");
      };

      // ---- 保存
      const okBtn = q<HTMLButtonElement>('[data-x="ok"]')!;
      let saving = false;
      let busy: SaveBusy | null = null;
      const failed = () => {
        saving = false;
        busy?.();
      };
      const save = async () => {
        if (saving) return;
        const edit: VideoEdit = {};
        const set = edit as Record<string, unknown>;
        for (const i of inputs) {
          const k = i.dataset.k as Key;
          const f = Object.values(F).find((x) => x.key === k);
          // 説明は改行を残し、前後の空白と空行だけ除く
          const v = i.value.trim();
          if (!touched.has(k) || hiddenField(i) || v === (common(k) ?? "")) continue;
          if (f?.num) {
            if (v && !/^\d{1,4}$/.test(v)) {
              toast(tr("{a}は数字で入力してください", { a: tr(f.label) }), "error");
              showPage("detail");
              i.focus();
              return;
            }
            set[k] = v ? Number(v) : 0;
          } else {
            set[k] = v;
          }
        }
        if (edit.title === "") {
          toast(tr("タイトルは空にできません"), "error");
          return;
        }
        const relEl = q<HTMLInputElement>("#ve-released")!;
        if (!hiddenField(relEl) && relEl.value !== dayValue(released ?? 0) && (relEl.value || released !== null)) edit.released = daySec(relEl.value);
        const k = kindValue();
        if (k && list.some((v) => v.kind !== k)) edit.kind = k;
        const r = stars?.changed();
        if (r !== undefined) edit.rating = r;
        if (resetPlays) edit.resetPlayCount = true;
        const more = opts.changes();
        if (!more) return;
        Object.assign(edit, more);
        if (!Object.keys(edit).length) return close();
        saving = true;
        busy = busySave(okBtn);
        try {
          const res = await call<{ videos: VideoView[]; snapshot: Snapshot }>("edit_videos", { ids, edit });
          if (!res) return failed();
          await busy.settle();
          setVideos(res.videos);
          applySnapshot(res.snapshot);
          close();
        } catch (e) {
          failed();
          failToast(e, 6000);
        }
      };
      okBtn.addEventListener("click", save);
      root.querySelectorAll("input").forEach((i) => i.addEventListener("keydown", (e) => isEnter(e) && save()));
      q('[data-x="cancel"]')!.addEventListener("click", close);
    },
    "te-modal ve-modal",
  );
}

/** Rockbox: タイトル (ファイル名) と番組名 (Videos の中のフォルダ) だけ。変えるとファイルを移す */
function editRockboxVideos(list: VideoView[]) {
  const one = list.length === 1 ? list[0] : null;
  const ids = list.map((v) => v.id);
  const show = list.every((v) => v.show === list[0].show) ? list[0].show : null;
  modal(
    `<div class="te-head"><span class="te-art none" aria-hidden="true">${ICON_VIDEO}</span><div class="te-head-text"><h3>${esc(
      one ? one.title : tr("{n} 本の動画の情報を編集", { n: list.length }),
    )}</h3></div></div>
     <div class="te-grid">
       ${one ? `<div class="field"><label for="ve-title">${esc(tr("タイトル"))}</label><input type="text" id="ve-title" value="${esc(one.title)}"></div>` : ""}
       <div class="field"><label for="ve-show">${esc(tr("番組"))}</label><input type="text" id="ve-show" value="${esc(show ?? "")}"${show === null ? ` placeholder="${esc(tr("複数の値"))}"` : ""}></div>
     </div>
     <p class="te-kind-note muted">${esc(tr("Rockbox では、タイトルはファイルの名前、番組名は Videos の中のフォルダになります (変えるとファイルを移します)。ほかの情報は Rockbox が読まないので持てません"))}</p>
     <div class="modal-actions"><p class="te-where">${esc(tr("iPod の中の動画のファイルの名前と場所を変えます。"))}</p><button class="btn" data-x="cancel">${esc(
       tr("キャンセル"),
     )}</button><button class="btn primary" data-x="ok">${esc(tr("保存"))}</button></div>`,
    (root, close) => {
      const q = <T extends HTMLElement>(sel: string) => root.querySelector<T>(sel);
      const titleEl = q<HTMLInputElement>("#ve-title");
      const showEl = q<HTMLInputElement>("#ve-show")!;
      const first = titleEl ?? showEl;
      first.focus();
      first.select();
      const okBtn = q<HTMLButtonElement>('[data-x="ok"]')!;
      const save = async () => {
        const edit: VideoEdit = {};
        const t = titleEl?.value.trim();
        if (one && t && t !== one.title) edit.title = t;
        const s = showEl.value.trim();
        if (s !== (show ?? "") || (s && show === null)) edit.show = s;
        if (!Object.keys(edit).length) return close();
        const restore = busySave(okBtn);
        const r = await call<{ videos: VideoView[]; snapshot: Snapshot }>("edit_videos", { ids, edit });
        if (!r) return restore();
        await restore.settle();
        // ファイル名が変わると ID も変わる
        vis.selected.clear();
        setVideos(r.videos);
        applySnapshot(r.snapshot);
        close();
      };
      okBtn.addEventListener("click", save);
      root.querySelectorAll("input").forEach((i) => i.addEventListener("keydown", (e) => isEnter(e) && save()));
      q('[data-x="cancel"]')!.addEventListener("click", close);
    },
    "te-modal",
  );
}
