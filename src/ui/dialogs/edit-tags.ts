// 曲情報の編集

import { t as tr } from "../../i18n";
import { isRockbox, writable } from "../../app/selectors";
import { state } from "../../app/state";
import { invoke } from "../../infra/tauri";
import { artSizeHtml, bindArtSize } from "../art-size";
import { fmtBytes } from "../../domain/format";
import { KIND_LABEL } from "../../domain/types";
import type { MediaKind, Snapshot, Track } from "../../domain/types";
import { fetchTagsOnline } from "./fetch-tags";
import { bindShrinkOnSet, pickArtwork, removeArtwork, searchArtwork, shrinkArtwork, shrinkOnSetHtml } from "./artwork-search";
import { bindPlayOptions, playOptionsHtml } from "./play-options";
import type { PlayOptions } from "./play-options";
import { esc, isEnter } from "../dom";
import { ICON_LIB } from "../icons";
import { call, failToast, replayClass, toast } from "../feedback";
import { busySave, modal } from "../modal";
import type { SaveBusy } from "../modal";
import { rescanCurrentPc } from "../pc";
import { findTrack } from "../player";
import { applySnapshot } from "../render";
import { artCache, pcArtCache } from "../artwork-cells";
import { placeSegThumb } from "../segments";
import { canRate } from "../selection-actions";

type TagKey =
  | "title"
  | "artist"
  | "album"
  | "albumArtist"
  | "genre"
  | "composer"
  | "year"
  | "trackNr"
  | "discNr"
  | "sortTitle"
  | "sortArtist"
  | "sortAlbum"
  | "sortAlbumArtist";

const TAG_FIELDS: { key: TagKey; label: string; num?: boolean; single?: boolean }[] = [
  { key: "title", label: "タイトル", single: true },
  { key: "artist", label: "アーティスト" },
  { key: "albumArtist", label: "アルバムアーティスト" },
  { key: "album", label: "アルバム" },
  { key: "genre", label: "ジャンル" },
  { key: "composer", label: "作曲者" },
  { key: "year", label: "年", num: true },
  { key: "trackNr", label: "トラック番号", num: true, single: true },
  { key: "discNr", label: "ディスク番号", num: true },
];

/** 読み (並べ替え用) の項目と、その読みの元になる表記の項目 */
const SORT_FIELDS: { key: TagKey; of: TagKey; label: string; single?: boolean }[] = [
  { key: "sortTitle", of: "title", label: "タイトル", single: true },
  { key: "sortArtist", of: "artist", label: "アーティスト" },
  { key: "sortAlbumArtist", of: "albumArtist", label: "アルバムアーティスト" },
  { key: "sortAlbum", of: "album", label: "アルバム" },
];

/**
 * 曲情報の編集。複数曲のときは、値が揃っていない項目は空欄 (複数の値) で表示し、
 * 書き換えた項目だけを全曲に反映する
 */
export async function editTags(ids: string[]) {
  const tracks = ids.map((id) => findTrack(id)).filter((t): t is Track => !!t);
  // PC のフォルダの曲は元のファイルのタグを書き換える (iPod が読み取り専用でも編集できる)
  const pc = tracks.length > 0 && tracks.every((t) => !!t.pcPath);
  if (!tracks.length || (!pc && !writable())) return;
  const single = tracks.length === 1;
  const fields = TAG_FIELDS.filter((f) => single || !f.single);
  const sortFields = SORT_FIELDS.filter((f) => single || !f.single);
  const sortOf = new Map(sortFields.map((f) => [f.of, f]));
  // ネットで取得は 1 曲か、同じアルバムの曲のとき。アルバム名が空の曲 (情報の無い曲) は、
  // 探したアルバムの曲目から曲ごとに選んで反映するので、一緒に選んでいてもよい
  const albums = new Set(tracks.filter((t) => t.album).map((t) => `${t.album}\u0000${t.albumArtist || t.artist}`));
  const canFetch = single || albums.size <= 1;
  const shown = (t: Track, k: TagKey) => {
    const v = t[k];
    return typeof v === "number" ? (v ? String(v) : "") : v;
  };
  const common = (k: TagKey) => {
    const v = shown(tracks[0], k);
    return tracks.every((t) => shown(t, k) === v) ? v : null;
  };
  // 保存先の説明は保存のボタンの横に 1 つだけ出す (歌詞の書き込み先は情報と違うので、1 曲のときは両方を書く)
  const where = pc
    ? tr("PC の元のファイルに保存します。iPod に転送済みの曲は変わりません。")
    : isRockbox()
      ? tr("iPod の中の曲のファイルに保存します。PC の元のファイルは変わりません。")
      : single
        ? tr("曲名などは iPod のデータベースに、歌詞は iPod の中の曲のファイルに保存します。PC の元のファイルは変わりません。")
        : tr("iPod のデータベースに保存します。PC の元のファイルは変わりません。");
  const t0 = tracks[0];
  // iTunes の「オプション」(iPod OS の曲だけ)。今の値は開くときに iTunesDB から読む
  const optRows = !pc && !isRockbox() ? await invoke<(PlayOptions | null)[]>("track_options", { ids }).catch(() => null) : null;
  const playOpts = optRows?.every((o) => o) ? (optRows as PlayOptions[]) : null;
  const fetchBtn = canFetch
    ? `<button class="btn te-fetch" data-x="fetch" title="${esc(tr("MusicBrainz でアルバムを探して、曲名・番号などを入れます"))}">${esc(tr("曲情報をネットで取得"))}</button>`
    : "";
  const head = single
    ? `<div class="te-head">${artSlot(t0)}<div class="te-head-text"><h3>${esc(t0.title)}</h3><div class="te-head-sub">${esc(
        [t0.artist, t0.album].filter(Boolean).join(" — "),
      )}</div></div>${fetchBtn}</div>`
    : `<div class="te-head"><div class="te-head-text"><h3>${esc(tr("{n} 曲の情報を編集", { n: tracks.length.toLocaleString() }))}</h3><div class="te-head-sub wrap">${esc(
        tr("書き換えた項目だけを全曲に反映します。空欄の「複数の値」は変更しません。"),
      )}</div></div>${fetchBtn}</div>`;
  // アートワーク: 選んだらすぐ書き込む (保存のボタンを待たない。右クリックのメニューと同じ)。
  // ネットで探すのは 1 曲か、同じアルバムの曲のとき (右クリックのメニューと同じ)
  const artOk = pc || !!state.snap?.artworkSupported;
  const oneAlbum = albums.size === 1 && tracks.every((t) => t.album);
  const artPage = artOk
    ? artPageHtml(
        ICON_LIB,
        pc
          ? tr("選ぶとすぐ PC の元のファイルに埋め込みます (保存のボタンを押さなくても変わります)。")
          : single
            ? tr("選ぶとすぐ iPod に書き込みます (保存のボタンを押さなくても変わります)。")
            : tr("選んだ {n} 曲に同じ画像を設定します。選ぶとすぐ iPod に書き込みます。", { n: tracks.length.toLocaleString() }),
        single || oneAlbum,
        // 曲のファイルの埋め込み画像の大きさと縮小。PC の曲と Rockbox の曲は設定する画像を縮めるかも選べる (iPod OS は埋め込まない)
        { shrink: true, pc },
      )
    : "";
  // tr: 使う側で訳す
  const pages: [string, string][] = [
    ["info", "情報"],
    ...(artOk ? ([["artwork", "アートワーク"]] as [string, string][]) : []),
    ...(single ? ([["lyrics", "歌詞"]] as [string, string][]) : []),
    ...(playOpts ? ([["options", "オプション"]] as [string, string][]) : []),
  ];
  const tabs =
    pages.length > 1
      ? `<div class="te-tabs"><div class="seg" role="radiogroup" aria-label="${esc(tr("表示する情報"))}">${pages
          .map(([k, label], i) => `<label class="seg-item"><input type="radio" name="te-tab" value="${k}" ${i ? "" : "checked"} /><span>${esc(tr(label))}</span></label>`)
          .join("")}</div></div>`
      : "";
  const field = (f: (typeof TAG_FIELDS)[number], extraClass = "") => {
    const v = common(f.key);
    return `<div class="field${f.num ? " te-num" : ""}${extraClass}"><label for="te-${f.key}">${esc(tr(f.label))}</label>
           <input type="text" id="te-${f.key}" data-k="${f.key}" ${f.num ? 'inputmode="numeric"' : ""} value="${esc(v ?? "")}" ${v === null ? `placeholder="${esc(tr("複数の値"))}"` : ""}></div>`;
  };
  // 曲を見分ける項目 (表記と読みを横に並べる)。読みの見出しは最初の行だけに出し、その横に自動入力と説明
  const named = fields.filter((f) => sortOf.has(f.key));
  const namedHtml = named
    .map((f, n) => {
      const sf = sortOf.get(f.key)!;
      const sv = common(sf.key);
      const label = n === 0 ? readingHeadHtml(`te-${sf.key}`, !pc) : `<label class="te-sort-spacer" aria-hidden="true">&nbsp;</label>`;
      return `${field(f, " te-has-sort")}<div class="field te-sort">${label}
           <input type="text" id="te-${sf.key}" data-k="${sf.key}" aria-label="${esc(tr("{a}の読み", { a: tr(f.label) }))}" value="${esc(sv ?? "")}" placeholder="${sv === null ? tr("複数の値") : tr("表記どおり")}"></div>`;
    })
    .join("");
  const detailHtml = fields
    .filter((f) => !sortOf.has(f.key))
    .map((f) => field(f))
    .join("");
  await modal(
    `${head}${tabs}
     <div class="te-pages"><div class="te-page" data-page="info" role="tabpanel">
     <div class="te-grid">${namedHtml}</div>
     <div class="te-sec-title">${esc(tr("詳細"))}</div>
     <div class="te-grid te-detail">${detailHtml}</div>
     ${pc || (playOpts && !canRate()) ? "" : `<div class="te-sec-title">${esc(tr("iPod"))}</div>${kindRatingHtml(tracks, !playOpts)}`}
     </div>${artOk ? `<div class="te-page te-off" data-page="artwork" role="tabpanel">${artPage}</div>` : ""}${
       single
         ? `<div class="te-page te-lyrics-page te-off" data-page="lyrics" role="tabpanel">
       <textarea class="te-lyrics" aria-label="${esc(tr("歌詞"))}" spellcheck="false" disabled placeholder="${esc(tr("読み込み中…"))}"></textarea>
     </div>`
         : ""
     }${
       playOpts
         ? `<div class="te-page te-off" data-page="options" role="tabpanel">${playOptionsHtml(playOpts, kindSelectHtml(tracks, tr("メディアの種類")), single ? t0.durationMs : null)}</div>`
         : ""
     }</div>
     <div class="modal-actions"><p class="te-where">${esc(where)}</p><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok">${esc(tr("保存"))}</button></div>`,
    (root, close) => {
      const inputs = [...root.querySelectorAll<HTMLInputElement>("[data-k]")];
      const extra = pc ? null : bindKindRating(root, tracks);
      const touched = new Set<TagKey>();
      inputs.forEach((i) => i.addEventListener("input", () => touched.add(i.dataset.k as TagKey)));
      inputs[0]?.focus();
      inputs[0]?.select();
      // 歌詞 (1 曲のときだけ): 開いたときに曲のファイルから読む。読めるまでは書き換えられない
      const lyricsEl = root.querySelector<HTMLTextAreaElement>(".te-lyrics");
      let lyricsLoaded: string | null = null;
      if (lyricsEl) {
        invoke<string | null>("track_lyrics", { id: tracks[0].id }).then(
          (v) => {
            lyricsLoaded = v ?? "";
            lyricsEl.value = lyricsLoaded;
            lyricsEl.placeholder = tr("歌詞はありません");
            lyricsEl.disabled = false;
          },
          (e) => (lyricsEl.placeholder = tr("歌詞を読めませんでした: {e}", { e: String(e) })),
        );
      }
      // タブの切り替え (一覧の表示の切り替えと同じ部品。左右の矢印キーでも移れる)
      const seg = root.querySelector<HTMLElement>(".te-tabs .seg");
      const showPage = (page: string) => {
        if (!seg) return;
        seg.querySelector<HTMLInputElement>(`input[value="${page}"]`)!.checked = true;
        root.querySelectorAll<HTMLElement>(".te-page").forEach((p) => p.classList.toggle("te-off", p.dataset.page !== page));
        placeSegThumb(seg);
        if (page === "lyrics" && lyricsEl && !lyricsEl.disabled) lyricsEl.focus();
        // 大きい画像は、アートワークのタブを初めて開いたときに読む
        if (page === "artwork" && !artPainted) {
          artPainted = true;
          void paintArt();
        }
      };
      if (seg) {
        placeSegThumb(seg, false);
        seg.addEventListener("change", () => showPage(seg.querySelector<HTMLInputElement>("input:checked")!.value));
      }
      // ---- オプション (種類を変えたら、触っていない「再生位置を記憶」「シャッフル時にスキップ」はその種類の既定に)
      const opts = playOpts ? bindPlayOptions(root, playOpts, single ? t0.durationMs : null, () => showPage("options")) : null;
      root.querySelector<HTMLSelectElement>("#te-kind")?.addEventListener("change", (e) => {
        const k = (e.currentTarget as HTMLSelectElement).value as MediaKind | "";
        opts?.kindChanged(!k || tracks.every((t) => (t.kind ?? "music") === k) ? null : k !== "music");
      });
      loadHeadArt(root, tracks[0]);
      // ---- アートワーク (変えたら大きい画像と見出しの画像を描き直す)
      const paintArt = async () => {
        const now = ids.map((id) => findTrack(id)).filter((t): t is Track => !!t);
        const big = root.querySelector<HTMLElement>(".te-art-big");
        if (!big) return;
        // 複数の曲は、全部が同じ画像のときだけ出す (PC の曲は画像を見分けられないので 1 曲のときだけ)
        const t = now[0];
        let url: string | null = null;
        if (t && pc && single) url = await invoke<string | null>("pc_album_art", { path: t.pcPath }).catch(() => null);
        else if (t && !pc && t.artworkId && now.every((x) => x.artworkId === t.artworkId))
          url = await invoke<string | null>("get_artwork", { imageId: t.artworkId, large: true }).catch(() => null);
        big.innerHTML = url ? `<img src="${url}" alt="">` : `<span class="te-art-none" aria-hidden="true">${ICON_LIB}</span>`;
        const small = root.querySelector<HTMLElement>(".te-head .te-art");
        if (small && t) {
          const s = t.pcPath ? await invoke<string | null>("pc_artwork", { path: t.pcPath }).catch(() => null) : url;
          if (t.pcPath) pcArtCache.set(t.pcPath, s);
          small.outerHTML = s ? `<img class="te-art fade" src="${s}" alt="">` : `<span class="te-art none" aria-hidden="true">${ICON_LIB}</span>`;
        }
        // 画像の有無をまだ読んでいない PC の曲もあるので、無いと分かっている曲だけのときに押せなくする
        const rm = root.querySelector<HTMLButtonElement>('[data-x="art-remove"]');
        if (rm) rm.disabled = pc ? now.every((x) => x.hasPicture === false) : !now.some((x) => x.artworkId);
        await paintEmbedded();
      };
      // 1 曲のときは埋め込み画像の大きさを出し、選んだ大きさで縮めなくてよければ縮小を押せなくする (複数の曲は縮める曲だけ縮める)
      // 設定する画像を縮めるかのチェックがあれば (PC の曲・Rockbox の曲) それと一緒に、無ければ大きさの欄だけを動かす
      const artSizeOf = root.querySelector("#te-art-shrink")
        ? bindShrinkOnSet(root, pc, "te-art", () => void paintEmbedded())
        : root.querySelector("#te-art-size")
          ? bindArtSize(root, "te-art-size", () => void paintEmbedded())
          : null;
      const paintEmbedded = async () => {
        const sizeEl = root.querySelector<HTMLElement>(".te-art-size");
        const shrinkBtn = root.querySelector<HTMLButtonElement>('[data-x="art-shrink"]');
        if (!sizeEl || !single || !artSizeOf) return;
        type Embedded = { width: number; height: number; bytes: number; fits: boolean } | null;
        const e = await (pc
          ? invoke<Embedded>("pc_embedded_artwork", { path: tracks[0].pcPath, size: artSizeOf() })
          : invoke<Embedded>("embedded_artwork", { id: ids[0], size: artSizeOf() })
        ).catch(() => null);
        sizeEl.textContent = e ? tr("埋め込み画像 {w}×{h} · {s}", { w: e.width, h: e.height, s: fmtBytes(e.bytes) }) : tr("埋め込み画像なし");
        if (shrinkBtn) shrinkBtn.disabled = !e || e.fits;
      };
      let artPainted = false;
      root.querySelector('[data-x="art-pick"]')?.addEventListener("click", () => void pickArtwork(ids, paintArt));
      root.querySelector('[data-x="art-search"]')?.addEventListener("click", () => void searchArtwork(ids, undefined, paintArt));
      root.querySelector('[data-x="art-remove"]')?.addEventListener("click", () => void removeArtwork(ids, paintArt));
      root.querySelector('[data-x="art-shrink"]')?.addEventListener("click", () => void shrinkArtwork(ids, paintArt, artSizeOf?.()));
      // 保存している間は、保存のボタンに回る印を出して押せなくする (タグの書き込み・Rockbox のファイルの移動は時間がかかる)
      const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      let saving = false;
      let busy: SaveBusy | null = null;
      /** 保存できなかった: ボタンを戻す */
      const failed = () => {
        saving = false;
        busy?.();
      };
      const save = async () => {
        if (saving) return;
        const edit: Partial<Record<TagKey | "lyrics", string | number>> = {};
        for (const i of inputs) {
          const k = i.dataset.k as TagKey;
          const f = TAG_FIELDS.find((x) => x.key === k) ?? { key: k, label: tr("読み"), num: false };
          const v = i.value.trim();
          if (!touched.has(k) || v === (common(k) ?? "")) continue;
          if (f.num) {
            if (v && !/^\d{1,4}$/.test(v)) {
              toast(tr("{a}は数字で入力してください", { a: tr(f.label) }), "error");
              i.focus();
              return;
            }
            edit[k] = v ? Number(v) : 0;
          } else {
            edit[k] = v;
          }
        }
        // 歌詞は行末の空白と前後の空行を除いて比べる (保存するときも同じように整える)
        const tidy = (s: string) => s.replace(/\r\n?/g, "\n").split("\n").map((l) => l.trimEnd()).join("\n").replace(/^\n+|\n+$/g, "");
        if (lyricsEl && lyricsLoaded !== null && tidy(lyricsEl.value) !== tidy(lyricsLoaded)) edit.lyrics = tidy(lyricsEl.value);
        if (edit.title === "") {
          toast(tr("タイトルは空にできません"), "error");
          return;
        }
        const play = opts ? opts.changes() : {};
        if (!play) return;
        const more = { ...(extra?.changes() ?? {}), ...play };
        if (!Object.keys(edit).length && !Object.keys(more).length) {
          close();
          return;
        }
        saving = true;
        busy = busySave(okBtn);
        // 保存の後の描き直しなどで例外が出ても、「保存中…」のまま止めない (理由は画面に出す)
        try {
          if (pc) {
            const paths = tracks.map((t) => t.pcPath!);
            const n = await call<number>("edit_pc_tags", { paths, edit });
            // 一部だけ失敗した場合もエラー表示のあとで読み直す
            rescanCurrentPc();
            if (n === null) return failed();
            await busy.settle();
            if (!single) toast(tr("{n} 曲のファイルを更新しました", { n: n.toLocaleString() }));
            close();
            return;
          }
          const r = await call<{ count: number; snapshot: Snapshot }>("edit_tags", { ids, edit: { ...edit, ...more } });
          if (!r) return failed();
          applySnapshot(r.snapshot);
          await busy.settle();
          if (!single) toast(tr("{n} 曲の情報を更新しました", { n: r.count.toLocaleString() }));
          if (more.kind && more.kind !== "music") hintAutoresume();
          close();
        } catch (e) {
          failed();
          failToast(e, 6000);
        }
      };
      inputs.forEach((i) => i.addEventListener("keydown", (e) => isEnter(e) && save()));
      const input = (k: TagKey) => root.querySelector<HTMLInputElement>(`[data-k="${k}"]`);
      bindReadingTools(
        root,
        () => sortFields.map((f) => ({ src: input(f.of)!, sort: input(f.key)! })).filter((p) => p.src && p.sort),
        (el) => touched.add(el.dataset.k as TagKey),
      );
      root.querySelector('[data-x="fetch"]')?.addEventListener("click", () => {
        close();
        fetchTagsOnline(tracks.map((t) => t.id));
      });
      root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
      root.querySelector('[data-x="ok"]')!.addEventListener("click", save);
    },
    "te-modal",
  );
}

/** 読みの説明 (「?」を押すと開く)。`autoNote` は表記を変えたときに読みを付け直すとき (iPod の曲・動画) */
export function readingHelpHtml(autoNote: boolean): string {
  return `<div id="te-help" class="te-help hidden" role="dialog" aria-label="${esc(tr("読みについて"))}">
       <div class="te-help-title">${esc(tr("読み (ソート名 / Sort name) について"))}</div>
       <ul>
         <li>${esc(tr("iPod は曲・アーティスト・アルバムを、表記ではなくこの読みの順に並べます。空欄なら表記のまま並びます。"))}</li>
         <li>${esc(tr("日本語の漢字の名前は、文字だけでは読み方が決まりません。カタカナの読みを入れると、五十音順の正しい位置に並びます。"))}<br>${esc(tr("例: 米津玄師 → ヨネヅケンシ (アルファベット順に並べたいときは Yonezu Kenshi)"))}</li>
         <li>${esc(tr("英語などラテン文字の名前は、ふつうは空欄のままで構いません。「The Beatles」を B の位置に並べたいときは「Beatles」と入れます。"))}</li>
         <li>${esc(tr("「自動で入れる」は、日本語 (漢字・かな) の名前にカタカナの読みを作ります。それ以外の名前はそのままです。"))}</li>
         ${autoNote ? `<li>${esc(tr("表記を変えて読みを変えなかった項目は、保存するときに読みを付け直します。"))}</li>` : ""}
         <li>${esc(tr("iTunes / ミュージック App の「並べ替え」(Sort Name / Sort Artist / Sort Album) と同じものです。"))}</li>
       </ul>
     </div>`;
}

/** 読みの見出しの「?」(説明を開く) と「自動で入れる」の動き。`pairs` は今表示している表記と読みの欄の組。
 * 自動で入れた読みの欄は `filled` に渡す (変えた項目として覚えるため) */
export function bindReadingTools(root: HTMLElement, pairs: () => { src: HTMLInputElement; sort: HTMLInputElement }[], filled: (sort: HTMLInputElement) => void) {
  const note = root.querySelector<HTMLElement>(".te-note");
  let noteTimer = 0;
  const helpBtn = root.querySelector<HTMLButtonElement>('[data-x="yomi-help"]');
  // 読みの説明: 「?」の下に重ねて出す (ほかの要素を押し下げない)。外を押すと閉じる
  const helpEl = root.querySelector<HTMLElement>("#te-help");
  const setHelp = (open: boolean) => {
    helpEl?.classList.toggle("hidden", !open);
    helpBtn?.setAttribute("aria-expanded", String(open));
    helpBtn?.classList.toggle("active", open);
  };
  helpBtn?.addEventListener("click", (e) => {
    e.preventDefault();
    setHelp(helpEl!.classList.contains("hidden"));
  });
  root.addEventListener("mousedown", (e) => {
    const t = e.target as HTMLElement;
    if (!t.closest("#te-help, [data-x='yomi-help']")) setHelp(false);
  });
  // 読みの自動入力: 今の表記 (書き換えた値を含む) から作る。値が揃っていない項目 (空欄) は触らない。
  // 何が変わったか分かるよう、変えた欄を光らせて、結果を下の説明の位置に出す
  root.querySelector('[data-x="yomi"]')?.addEventListener("click", async () => {
    const targets = pairs().filter((p) => p.src.value.trim() !== "");
    const texts = targets.map((p) => p.src.value.trim());
    const r = texts.length ? await call<(string | null)[]>("make_readings", { texts }) : [];
    if (!r || !note) return;
    let changed = 0; // 読みを入れた / 書き換えた
    let cleared = 0; // 読みが要らないので空欄にした
    let plain = 0; // もともと空欄で、読みも要らない
    let kept = 0; // 今の読みと同じ
    targets.forEach((p, n) => {
      const el = p.sort;
      // 読みが要らない表記 (英数字だけなど) は空欄 = 表記どおりに並ぶ
      const v = r[n] ?? "";
      if (el.value.trim() === v) {
        if (v) kept++;
        else plain++;
        return;
      }
      el.value = v;
      filled(el);
      replayClass(el, "flash");
      if (v) changed++;
      else cleared++;
    });
    const parts = [
      changed ? tr("{n} 項目に読みを入れました", { n: changed }) : "",
      cleared ? tr("{n} 項目は読みが要らないので空欄にしました", { n: cleared }) : "",
      kept ? tr("{n} 項目は今の読みのままです", { n: kept }) : "",
      plain ? tr("{n} 項目は英数字などで読みが要りません", { n: plain }) : "",
    ].filter(Boolean);
    note.textContent = parts.length
      ? `${parts.join(tr("。"))}${tr("。")}${changed || cleared ? tr("保存すると反映します。") : tr("変更はありません。")}`
      : tr("読みを作れる項目がありません (表記が空か、複数の値です)。");
    // ボタンの下に浮かせて出し、少したったら消す (ほかの要素を動かさない)
    note.classList.add("show");
    clearTimeout(noteTimer);
    noteTimer = window.setTimeout(() => note.classList.remove("show"), 5000);
  });
}

/** 読みの列の見出し (「読み」・「?」・「自動で入れる」と説明)。`id` は最初の読みの欄 */
export function readingHeadHtml(id: string, autoNote: boolean): string {
  return `<div class="te-sort-head"><label for="${id}" title="${esc(tr("並べ替え用の読み (Sort name)"))}">${esc(tr("読み"))}</label><button type="button" class="help-btn" data-x="yomi-help" aria-expanded="false" aria-controls="te-help" title="${esc(tr("読み (Sort name) とは"))}">?</button><span class="te-yomi"><button type="button" class="btn mini" data-x="yomi" title="${esc(tr("表記から読みを作って読みの欄に入れます (保存するまで反映しません)"))}">${esc(tr("自動で入れる"))}</button><span class="te-note" role="status" aria-live="polite"></span></span>${readingHelpHtml(autoNote)}</div>`;
}

/**
 * アートワークのタブ (大きい画像と、その下に操作)。画像は `.te-art-big` に描く。`search` はネットで探すを出すか。
 * `embedded` は埋め込み画像の大きさ (`.te-art-size`) と縮小のボタン、設定する画像を縮めるかのチェック
 * (PC の曲と Rockbox の曲。ネットで探すの画面と同じ部品)。iPod OS の曲は大きさの欄だけ
 */
export function artPageHtml(icon: string, note: string, search: boolean, embedded?: { shrink: boolean; pc: boolean }): string {
  const onSet = embedded ? shrinkOnSetHtml(embedded.pc, false, "te-art") : "";
  return `<div class="te-art-page">
       <div class="te-art-big"><span class="te-art-none" aria-hidden="true">${icon}</span></div>
       ${embedded?.shrink ? `<p class="te-art-size muted"></p>` : ""}
       <div class="te-art-actions">
         <button type="button" class="btn" data-x="art-pick">${esc(tr("画像ファイルから設定"))}</button>
         ${search ? `<button type="button" class="btn" data-x="art-search">${esc(tr("ネットで探す"))}</button>` : ""}
         ${embedded?.shrink ? `<button type="button" class="btn" data-x="art-shrink">${esc(tr("縮小する"))}</button>` : ""}
         <button type="button" class="btn danger" data-x="art-remove">${esc(tr("削除"))}</button>
       </div>
       ${
         embedded?.shrink
           ? onSet || `<div class="as-shrink"><span>${esc(tr("縮小する大きさ (長い辺)"))}</span>${artSizeHtml("te-art-size")}</div>`
           : ""
       }
       <p class="te-kind-note muted">${esc(note)}</p>
     </div>`;
}

/** 見出しのジャケットの場所。覚えてある画像はすぐ出し、まだなら読み込み中の四角にして `loadHeadArt` で読む */
function artSlot(t: Track): string {
  const known = t.pcPath ? pcArtCache.get(t.pcPath) : t.artworkId && state.snap?.artworkSupported ? artCache.get(t.artworkId) : null;
  if (known) return `<img class="te-art" src="${known}" alt="">`;
  return `<span class="te-art none" aria-hidden="true">${ICON_LIB}</span>`;
}

async function loadHeadArt(root: HTMLElement, t: Track) {
  const el = root.querySelector<HTMLElement>("span.te-art");
  if (!el) return;
  let url: string | null = null;
  if (t.pcPath) {
    if (pcArtCache.has(t.pcPath)) return;
    url = await invoke<string | null>("pc_artwork", { path: t.pcPath }).catch(() => null);
    pcArtCache.set(t.pcPath, url);
  } else if (t.artworkId && state.snap?.artworkSupported && !artCache.has(t.artworkId)) {
    url = await invoke<string | null>("get_artwork", { imageId: t.artworkId, large: false }).catch(() => null);
    artCache.set(t.artworkId, url);
  }
  if (url && el.isConnected) el.outerHTML = `<img class="te-art fade" src="${url}" alt="">`;
}

/** 情報の編集の、分類と評価の欄 (iPod の曲だけ) */
/** 分類の選択欄。`label` は見出し (オプションのページでは iTunes と同じ「メディアの種類」) */
function kindSelectHtml(tracks: Track[], label: string): string {
  const kinds = new Set(tracks.map((t) => t.kind ?? "music"));
  const kind = kinds.size === 1 ? [...kinds][0] : null;
  const opts = (Object.keys(KIND_LABEL) as MediaKind[])
    .map((k) => `<option value="${k}" ${k === kind ? "selected" : ""}>${esc(tr(KIND_LABEL[k]))}</option>`)
    .join("");
  return `<div class="field"><label for="te-kind">${esc(label)}</label><select id="te-kind">${kind ? "" : `<option value="" selected>${esc(tr("複数の値"))}</option>`}${opts}</select></div>`;
}

/** 情報のページの iPod の欄。分類はオプションのページが無いとき (Rockbox) だけここに置く */
function kindRatingHtml(tracks: Track[], withKind: boolean): string {
  const kindNote = isRockbox()
    ? tr("Rockbox では、ポッドキャストは Podcasts、オーディオブックは Audiobooks のフォルダに置きます (分類を変えるとファイルを移します)")
    : tr("ポッドキャストとオーディオブックは、iPod で再生した位置を覚え、シャッフルでは飛ばします");
  const stars = canRate() ? ratingStarsHtml() : "";
  return `<div class="te-extra">
      ${withKind ? kindSelectHtml(tracks, tr("分類")) : ""}
      ${stars}
    </div>
    ${withKind ? `<p class="te-kind-note muted">${esc(kindNote)}</p>` : ""}`;
}

/** 評価の欄 (★ 5 つ) */
export function ratingStarsHtml(): string {
  return `<div class="field"><label>${esc(tr("評価"))}</label><div class="te-stars" role="radiogroup" aria-label="${esc(tr("評価"))}">${[1, 2, 3, 4, 5]
    .map((n) => `<button type="button" class="te-star" data-star="${n}" role="radio" aria-label="${"★".repeat(n)}">★</button>`)
    .join("")}<span class="te-star-note muted"></span></div></div>`;
}

/** 評価の欄の動き。`ratings` は編集するものの今の評価 (揃っていなければ「複数の値」)。changed() は押して変えたときだけ値を返す */
export function bindRatingStars(root: HTMLElement, ratings: number[]) {
  const first = ratings[0];
  let rating: number | null = ratings.every((r) => r === first) ? first : null;
  let ratingTouched = false;
  const note = root.querySelector<HTMLElement>(".te-star-note");
  const paint = () => {
    const n = rating === null ? 0 : Math.round(rating / 20);
    root.querySelectorAll<HTMLElement>(".te-star").forEach((b) => {
      const on = Number(b.dataset.star) <= n;
      b.classList.toggle("on", on);
      b.setAttribute("aria-checked", String(Number(b.dataset.star) === n));
    });
    if (note) note.textContent = rating === null ? tr("複数の値") : n ? "" : tr("なし");
  };
  root.querySelectorAll<HTMLElement>(".te-star").forEach((b) =>
    b.addEventListener("click", () => {
      const n = Number(b.dataset.star);
      // 今と同じ星を押したら評価を消す
      rating = rating !== null && Math.round(rating / 20) === n ? 0 : n * 20;
      ratingTouched = true;
      paint();
    }),
  );
  paint();
  return {
    changed(): number | undefined {
      return ratingTouched && rating !== null ? rating : undefined;
    },
  };
}

/** 分類と評価の欄の動き。changes() は変えた項目だけ返す */
function bindKindRating(root: HTMLElement, tracks: Track[]) {
  const sel = root.querySelector<HTMLSelectElement>("#te-kind")!;
  const stars = bindRatingStars(root, tracks.map((t) => t.rating));
  return {
    changes(): { kind?: MediaKind; rating?: number } {
      const out: { kind?: MediaKind; rating?: number } = {};
      const k = sel.value as MediaKind | "";
      if (k && tracks.some((t) => (t.kind ?? "music") !== k)) out.kind = k;
      const r = stars.changed();
      if (r !== undefined) out.rating = r;
      return out;
    },
  };
}

/** Rockbox でポッドキャスト・オーディオブックを入れたのに「自動で続きから再生」がオフなら、1 回だけ案内する */
let autoresumeHinted = false;
export function hintAutoresume() {
  const rb = state.snap?.rockbox;
  if (!isRockbox() || !rb || rb.autoresume || autoresumeHinted) return;
  autoresumeHinted = true;
  toast(
    tr("続きから聞くには、iPod の Rockbox で「Settings → General Settings → Automatic Resume」の「Automatic Resume」を Yes にしてください"),
    "info",
    12000,
  );
}
