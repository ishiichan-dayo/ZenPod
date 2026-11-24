// 曲情報をネット (MusicBrainz) で取得して反映する

import { lang, t as tr } from "../../i18n";
import { invoke } from "../../infra/tauri";
import { analyticsNet, netFunnel, netReach, netSearched } from "../../app/analytics";
import { pcPool } from "../../app/selectors";
import { state } from "../../app/state";
import { fmtDur } from "../../domain/format";
import { releaseDir } from "../../domain/track-order";
import type { Snapshot, Track } from "../../domain/types";
import { getStored, setStored } from "../../infra/prefs";
import { esc, isEnter } from "../dom";
import { call, toast } from "../feedback";
import { busySave, modal } from "../modal";
import { rescanCurrentPc } from "../pc";
import { findTrack } from "../player";
import { applySnapshot } from "../render";

interface ReleaseCandidate {
  id: string;
  title: string;
  artist: string;
  date: string;
  country: string;
  trackCount: number;
  formats: string;
  disambiguation: string;
  thumbUrl: string;
}

type TagField =
  | "title"
  | "artist"
  | "album"
  | "albumArtist"
  | "year"
  | "trackNr"
  | "discNr"
  | "sortTitle"
  | "sortArtist"
  | "sortAlbum"
  | "sortAlbumArtist";

/** 曲情報をネットで取得するときの表記: 元の表記 / ローマ字・英語 / 表記はそのまま読みだけローマ字・英語 */
type Naming = "original" | "latin" | "latinSort";

interface TagChange {
  id: string;
  trackTitle: string;
  field: TagField;
  before: string;
  after: string;
  /** 波ダッシュなど見た目がほぼ同じ記号の違いだけ (初期状態でチェックを外す) */
  minor: boolean;
}

// tr: 使う側で訳す
const TAG_FIELD_LABEL: Record<TagField, string> = {
  title: "タイトル",
  artist: "アーティスト",
  album: "アルバム",
  albumArtist: "アルバムアーティスト",
  year: "年",
  trackNr: "トラック番号",
  discNr: "ディスク番号",
  sortTitle: "タイトルの読み",
  sortArtist: "アーティストの読み",
  sortAlbum: "アルバムの読み",
  sortAlbumArtist: "アルバムアーティストの読み",
};

/**
 * 2 つの文字列の文字単位の差分を HTML にする: [変更前 (消える文字を <del>), 変更後 (入る文字を <ins>)]。
 * 曲名程度の長さなので単純な LCS で十分
 */
function diffHtml(a: string, b: string): [string, string] {
  const x = [...a];
  const y = [...b];
  const dp = Array.from({ length: x.length + 1 }, () => new Array<number>(y.length + 1).fill(0));
  for (let i = x.length - 1; i >= 0; i--)
    for (let j = y.length - 1; j >= 0; j--)
      dp[i][j] = x[i] === y[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  // 共通部分がわずか (英語 → カタカナなど丸ごと別の表記) なら、空白だけ一致のようなまだらな表示にせず全体を強調する
  const run = (tag: string, text: string) => (text ? `<${tag}>${esc(text)}</${tag}>` : "");
  if (dp[0][0] < Math.max(x.length, y.length) * 0.4) return [run("del", a), run("ins", b)];
  let before = "";
  let after = "";
  let i = 0;
  let j = 0;
  // 連続する同じ種類の文字は 1 つのタグにまとめる
  let del = "";
  let ins = "";
  const flush = () => {
    before += run("del", del);
    after += run("ins", ins);
    del = ins = "";
  };
  while (i < x.length || j < y.length) {
    if (i < x.length && j < y.length && x[i] === y[j]) {
      flush();
      before += esc(x[i]);
      after += esc(y[j]);
      i++;
      j++;
    } else if (j < y.length && (i >= x.length || dp[i][j + 1] >= dp[i + 1][j])) {
      ins += y[j++];
    } else {
      del += x[i++];
    }
  }
  flush();
  return [before, after];
}

/** 差分表の変更前・変更後のセル。変更前が空なら「（なし）」 */
function diffCells(before: string, after: string): [string, string] {
  const [b, a] = diffHtml(before, after);
  return [before ? b : `<span class="muted">${esc(tr("（なし）"))}</span>`, a];
}

/**
 * 曲情報をネット (MusicBrainz) で取得して正式な表記に直す。
 * アルバムの候補を選ぶ → 曲ごとの差分を確認 (行ごとに除外・書き換え可) → 反映
 */
interface ReleaseTrack {
  disc: number;
  nr: number;
  title: string;
  artist: string;
  lengthMs: number;
}

interface TagDiffResult {
  changes: TagChange[];
  unmatched: string[];
  /** 曲ごとの対応 (track はリリースの曲目 tracks の添字。null = 対応なし) */
  matches: { id: string; track: number | null }[];
  tracks: ReleaseTrack[];
  discCount: number;
  /** ローマ字・英語の表記の出どころ: "titles" (曲名も) / "artists" (アーティスト名だけ) / "" (元の表記) */
  latin: string;
}

export async function fetchTagsOnline(ids: string[]) {
  // 検索欄に入れる名前や同じアルバムの曲は、アルバム名の入っている曲から (情報の無い曲と一緒に選ばれていることがある)
  const first = ids.map((id) => findTrack(id)).find((t) => t?.album) ?? findTrack(ids[0]);
  if (!first) return;
  // PC のフォルダの曲なら、反映先は元のファイルのタグ
  const pc = !!first.pcPath;
  const pool = pc ? pcPool() : state.snap!.tracks;
  // アーティスト表記が揺れていても同じアルバムとして扱うため、アルバム名だけで集める
  // (別アーティストの同名アルバムが混ざっても、曲目と対応しない曲は変更されない)
  // PC の曲は同じフォルダの曲に限る (シングルのタグに誤ってアルバム名が入っていることがある)
  const d = releaseDir(first);
  // 選んだ曲 (アルバム名が空の曲も) はいつも対象に含める
  const albumIds = first.album
    ? [...new Set([...ids, ...pool.filter((t) => t.album === first.album && releaseDir(t) === d).map((t) => t.id)])]
    : ids;
  const widerAlbum = albumIds.length > ids.length;
  let generation = 0;
  const funnel = netFunnel(["searched", "results", "chosen", "saved"]);
  // 表記の選び方 (前回選んだもの。無ければ英語の画面はローマ字・英語、日本語の画面は元の表記)
  const storedNaming = getStored("tagNaming");
  let naming: Naming =
    storedNaming === "original" || storedNaming === "latin" || storedNaming === "latinSort"
      ? storedNaming
      : lang === "ja"
        ? "original"
        : "latin";
  const namingHint = () =>
    naming === "latin"
      ? tr("曲名・アーティスト名などを、MusicBrainz に登録されたローマ字・英語の表記にします。日本語など、ラテン文字以外で書かれた曲に使います。")
      : naming === "latinSort"
        ? tr("表記は元のまま、並べ替え用の読みにローマ字・英語を入れます。iPod でアルファベット順に並びます。")
        : tr("MusicBrainz に登録された元の表記 (発売されたときの表記) にします。");
  // 「候補に戻る」で検索し直さないよう、直前の結果を持っておく
  let last: { key: string; res: ReleaseCandidate[] } | null = null;

  await modal(
    `<h3>${esc(tr("曲情報をネットで取得"))}</h3>
     <div class="tf-body"></div>`,
    (root, close) => {
      const body = root.querySelector<HTMLElement>(".tf-body")!;
      const cancel = () => {
        generation++;
        close();
      };

      // ---- 1. アルバムの候補 ----
      const showSearch = (artist: string, album: string, all: boolean) => {
        body.innerHTML = `
          <div class="tf-naming">
            <span class="tf-naming-label">${esc(tr("名前の表記"))}</span>
            <div class="seg" role="radiogroup" aria-label="${esc(tr("名前の表記"))}">
              ${(
                [
                  ["original", tr("元の表記")],
                  ["latin", tr("ローマ字・英語")],
                  ["latinSort", tr("読みだけローマ字・英語")],
                ] as [Naming, string][]
              )
                .map(
                  ([v, label]) =>
                    `<label class="seg-item"><input type="radio" name="tf-naming" value="${v}" ${v === naming ? "checked" : ""}><span>${esc(label)}</span></label>`,
                )
                .join("")}
            </div>
          </div>
          <div class="tf-naming-hint check-hint">${esc(namingHint())}</div>
          <div class="as-bar">
            <input type="text" id="tf-artist" placeholder="${esc(tr("アーティスト"))}" value="${esc(artist)}">
            <input type="text" id="tf-album" placeholder="${esc(tr("アルバム"))}" value="${esc(album)}">
            <button class="btn" id="tf-go">${esc(tr("検索"))}</button>
          </div>
          <div class="tf-list"></div>
          <div class="hint">${esc(tr("MusicBrainz から探します（アーティスト名とアルバム名が送信されます）。同じアルバムでも版 (通常盤・限定盤・配信など) で曲目が違うことがあるので、曲数を目安に選んでください。"))}</div>
          <div class="modal-actions">
            ${
              widerAlbum
                ? `<label class="check as-scope"><input type="checkbox" id="tf-all" ${all ? "checked" : ""}> ${esc(tr("同じアルバム名の {n} 曲すべてが対象", { n: albumIds.length }))}</label>`
                : `<span class="muted as-scope">${esc(tr("{n} 曲が対象", { n: ids.length }))}</span>`
            }
            <span class="spacer"></span>
            <button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button>
          </div>`;
        const list = body.querySelector<HTMLElement>(".tf-list")!;
        const artistIn = body.querySelector<HTMLInputElement>("#tf-artist")!;
        const albumIn = body.querySelector<HTMLInputElement>("#tf-album")!;
        const target = () => (body.querySelector<HTMLInputElement>("#tf-all")?.checked ? albumIds : ids);
        body.querySelector('[data-x="cancel"]')!.addEventListener("click", cancel);
        body.querySelectorAll<HTMLInputElement>('input[name="tf-naming"]').forEach((r) =>
          r.addEventListener("change", () => {
            naming = r.value as Naming;
            setStored("tagNaming", naming);
            body.querySelector(".tf-naming-hint")!.textContent = namingHint();
          }),
        );

        const run = async () => {
          const gen = ++generation;
          // 情報の無い曲は、わかる名前を入れてから探してもらう (アルバム名が無いと MusicBrainz で絞れない)
          if (!albumIn.value.trim()) {
            list.innerHTML = `<div class="as-empty">${esc(tr("アルバム名を入れて検索してください。見つけたアルバムの曲目から、曲ごとに対応する曲を選んで反映できます。"))}</div>`;
            (artistIn.value.trim() ? albumIn : artistIn).focus();
            return;
          }
          const key = `${artistIn.value.trim()}\u0000${albumIn.value.trim()}`;
          let res = last?.key === key ? last.res : null;
          if (!res) {
            list.innerHTML = `<div class="as-empty">${esc(tr("検索中…（数秒かかります）"))}</div>`;
            const started = performance.now();
            res = await call<ReleaseCandidate[]>("search_releases", { artist: artistIn.value, album: albumIn.value });
            if (gen !== generation) return;
            netSearched(funnel, started, res);
            if (res) last = { key, res };
          }
          if (!res?.length) {
            list.innerHTML = `<div class="as-empty">${esc(tr("見つかりませんでした。アーティスト名やアルバム名を変えて検索してください。"))}</div>`;
            return;
          }
          list.innerHTML = res
            .map((c, i) => {
              const meta = [c.date, c.country, c.formats, c.trackCount ? tr("{n} 曲", { n: c.trackCount }) : "", c.disambiguation]
                .filter(Boolean)
                .map(esc)
                .join(" · ");
              return `<button class="tf-cand" data-i="${i}">
                <img src="${esc(c.thumbUrl)}" alt="" loading="lazy">
                <span class="tf-cand-text"><span class="tf-cand-title">${esc(c.title)}</span>
                <span class="tf-cand-artist">${esc(c.artist)}</span>
                <span class="tf-cand-meta muted">${meta}</span></span>
              </button>`;
            })
            .join("");
          // ジャケットが無い版は画像を隠す (WebView が直接読めない場合はバックエンド経由で取得)
          list.querySelectorAll<HTMLImageElement>(".tf-cand img").forEach((img, i) =>
            img.addEventListener(
              "error",
              async () => {
                const url = await invoke<string | null>("fetch_image", { url: res![i].thumbUrl }).catch(() => null);
                if (url) img.src = url;
                else img.classList.add("tf-noimg");
              },
              { once: true },
            ),
          );
          list.querySelectorAll<HTMLElement>(".tf-cand").forEach((b) =>
            b.addEventListener("click", () =>
              showDiff(res![Number(b.dataset.i)], target(), artistIn.value, albumIn.value, target() === albumIds),
            ),
          );
        };
        body.querySelector("#tf-go")!.addEventListener("click", run);
        [artistIn, albumIn].forEach((el) => el.addEventListener("keydown", (e) => isEnter(e) && run()));
        run();
      };

      // ---- 2. 差分の確認 ----
      // assign: 手で選んだ曲の対応 (曲 ID → 曲目の添字、null = 反映しない)。
      // keep: 対応を選び直して作り直すときに引き継ぐ、チェックと書き換えた値 (曲 ID + 項目 → 値) とスクロール位置
      type Keep = { checked: Map<string, boolean>; values: Map<string, string>; scroll: number };
      const showDiff = async (
        c: ReleaseCandidate,
        target: string[],
        artist: string,
        album: string,
        all: boolean,
        assign: Record<string, number | null> = {},
        keep?: Keep,
      ) => {
        const gen = ++generation;
        netReach(funnel, "chosen");
        if (!keep)
          body.innerHTML = `<div class="as-empty tf-loading">${esc(
            naming === "original" ? tr("「{a}」の曲目を取得中…", { a: c.title }) : tr("「{a}」の曲目とローマ字・英語の表記を取得中…", { a: c.title }),
          )}</div>`;
        const d = pc
          ? await call<TagDiffResult>("preview_release_pc", {
              tracks: target.map((id) => findTrack(id)).filter((t): t is Track => !!t),
              releaseId: c.id,
              naming,
              assign,
            })
          : await call<TagDiffResult>("preview_release", {
              ids: target,
              releaseId: c.id,
              naming,
              assign,
            });
        if (gen !== generation) return;
        if (!d) {
          // 選び直しに失敗したときは、元の画面のまま選べるように戻す
          if (keep) body.querySelectorAll<HTMLSelectElement>(".tf-assign").forEach((s) => (s.disabled = false));
          else showSearch(artist, album, all);
          return;
        }
        const changes = d.changes;
        const changeKey = (x: TagChange) => `${x.id}\u0000${x.field}`;
        const checked = changes.map((x) => keep?.checked.get(changeKey(x)) ?? !x.minor);
        const values = changes.map((x) => keep?.values.get(changeKey(x)) ?? x.after);

        // 複数の曲で同じ変更 (年・アルバム名・アーティスト表記など) は「アルバム全体」に 1 行でまとめる。
        // 曲名とトラック番号は曲ごとに違うので常に曲ごとに出す
        const commonKey = (x: TagChange) => `${x.field}\u0000${x.before}\u0000${x.after}`;
        const keyCount = new Map<string, number>();
        for (const x of changes) keyCount.set(commonKey(x), (keyCount.get(commonKey(x)) ?? 0) + 1);
        const isCommon = (x: TagChange) =>
          x.field !== "title" && x.field !== "sortTitle" && x.field !== "trackNr" && (keyCount.get(commonKey(x)) ?? 0) >= 2;
        const common = new Map<string, number[]>();
        const perTrack = new Map<string, number[]>();
        changes.forEach((x, i) => {
          const [map, k] = isCommon(x) ? [common, commonKey(x)] : [perTrack, x.id];
          map.set(k, [...(map.get(k) ?? []), i]);
        });
        const fieldOrder: TagField[] = [
          "album",
          "sortAlbum",
          "albumArtist",
          "sortAlbumArtist",
          "artist",
          "sortArtist",
          "year",
          "discNr",
          "title",
          "sortTitle",
          "trackNr",
        ];
        const byField = (a: number, b: number) =>
          fieldOrder.indexOf(changes[a].field) - fieldOrder.indexOf(changes[b].field);

        // 1 行 = 同じ値に揃える変更のまとまり (indices)
        const rowHtml = (indices: number[], note = "") => {
          const x = changes[indices[0]];
          const [before, after] = diffCells(x.before, values[indices[0]]);
          return `<div class="tf-row" data-idx="${indices.join(",")}">
            <input type="checkbox" class="tf-chk" ${checked[indices[0]] ? "checked" : ""}>
            <span class="tf-field">${esc(tr(TAG_FIELD_LABEL[x.field]))}${x.minor ? `<span class="tf-minor" title="${esc(tr("～ と 〜 のような、見た目がほぼ同じ記号の違いだけです"))}">${esc(tr("記号のみ"))}</span>` : ""}</span>
            <span class="tf-before" title="${esc(x.before)}">${before}</span>
            <span class="tf-arrow">→</span>
            <span class="tf-after" title="${esc(tr("{a}（クリックで編集）", { a: values[indices[0]] }))}">${after}</span>
            <span class="tf-note muted">${note}</span>
          </div>`;
        };
        const trackTitle = (id: string) => findTrack(id)?.title ?? changes.find((x) => x.id === id)?.trackTitle ?? "";
        const commonRows = [...common.values()].sort((a, b) => byField(a[0], b[0]));
        // 曲はすべて出す (変更の無い曲・対応しなかった曲も、対応する曲目を選び直せるように)。
        // 並びはリリースの曲順、対応しなかった曲は最後
        const matchOf = new Map(d.matches.map((m) => [m.id, m.track]));
        const order = (id: string) => matchOf.get(id) ?? Number.MAX_SAFE_INTEGER;
        const trackGroups = d.matches
          .map((m, i) => ({ id: m.id, i }))
          .sort((a, b) => order(a.id) - order(b.id) || a.i - b.i)
          .map(({ id }) => [id, perTrack.get(id) ?? []] as [string, number[]]);
        const trackLabel = (r: ReleaseTrack) =>
          `${d.discCount > 1 ? `${r.disc}-` : ""}${String(r.nr).padStart(2, "0")}  ${r.title}${r.lengthMs ? ` (${fmtDur(r.lengthMs)})` : ""}`;
        const assignSelect = (id: string) => {
          const cur = matchOf.get(id) ?? null;
          return `<select class="tf-assign" data-id="${esc(id)}" title="${esc(tr("この曲に対応する曲目"))}">
            <option value="" ${cur === null ? "selected" : ""}>${esc(tr("（反映しない）"))}</option>
            ${d.tracks.map((r, j) => `<option value="${j}" ${cur === j ? "selected" : ""}>${esc(trackLabel(r))}</option>`).join("")}
          </select>`;
        };
        const unmatchedCount = d.matches.filter((m) => m.track === null).length;

        const nTracks = new Set(changes.map((x) => x.id)).size;
        body.innerHTML = `
          <p>${esc(tr("「{a}」と比べました。", { a: c.date ? `${c.title} (${c.date})` : c.title }))} ${esc(
            changes.length
              ? tr("{n} 曲に変更があります。チェックを外した行は変更しません。変更後の値はクリックで書き換えられます。", { n: nTracks })
              : tr("変更はありません。"),
          )}</p>
          ${
            naming !== "original" && d.latin === "artists"
              ? `<p class="tf-latin-note">${esc(tr("この版にはローマ字・英語の曲目が MusicBrainz に登録されていないため、曲名とアルバム名は元の表記のままです (アーティスト名だけローマ字・英語にしました)。"))}</p>`
              : ""
          }
          ${
            unmatchedCount
              ? `<p class="tf-unmatched">${esc(tr("{n} 曲は曲目と対応しませんでした。曲ごとの右の欄で、対応する曲目を選ぶと反映できます。", { n: unmatchedCount }))}</p>`
              : ""
          }
          ${
            changes.length || unmatchedCount
              ? `<div class="tf-diff">
            ${
              commonRows.length
                ? `<div class="tf-section"><div class="tf-section-head">${esc(tr("アルバム全体"))}</div>
                ${commonRows.map((idx) => rowHtml(idx, tr("{n} 曲", { n: idx.length }))).join("")}</div>`
                : ""
            }
            ${trackGroups
              .map(([id, idx]) => {
                const t = findTrack(id);
                const note = matchOf.get(id) == null ? tr("曲目を選ぶと反映できます") : idx.length ? "" : tr("変更なし");
                return `<div class="tf-section tf-track ${idx.length ? "" : "tf-track-empty"}">
                <div class="tf-section-head">
                  <label class="tf-track-label">${
                    // 変更の無い曲も曲名の位置を揃える (見えないチェックボックスを置く)
                    idx.length ? `<input type="checkbox" class="tf-track-chk">` : `<input type="checkbox" class="tf-chk-space" tabindex="-1" aria-hidden="true" disabled>`
                  }
                    ${t?.trackNr ? `<span class="tf-nr">${t.trackNr}</span>` : ""}<span class="tf-track-title" title="${esc(trackTitle(id))}">${esc(trackTitle(id))}</span>
                    ${t?.durationMs ? `<span class="tf-track-len muted">${fmtDur(t.durationMs)}</span>` : ""}</label>
                  ${note ? `<span class="tf-track-note muted">${esc(note)}</span>` : ""}
                  <span class="spacer"></span>
                  ${assignSelect(id)}
                </div>
                ${idx.sort(byField).map((i) => rowHtml([i])).join("")}</div>`;
              })
              .join("")}
          </div>`
              : ""
          }
          ${
            // 変更が無ければ書き換えの説明は要らない
            changes.length
              ? `<div class="hint">${
                  pc
                    ? tr("PC の元のファイルのタグを書き換えます。iPod に転送済みの曲は変わりません。")
                    : tr("iPod 内の情報だけを書き換え、PC の元ファイルは変更しません。表記を変えた項目は読みを付け直します。")
                }${changes.some((x) => x.minor) ? tr("「記号のみ」(～ と 〜 のような違い) は最初はチェックを外しています。") : ""}</div>`
              : ""
          }
          <div class="modal-actions">
            <button class="btn" data-x="back">${esc(tr("← 候補に戻る"))}</button>
            <span class="spacer"></span>
            ${
              changes.length
                ? `<button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok"></button>`
                : `<button class="btn primary" data-x="cancel">${esc(tr("閉じる"))}</button>`
            }
          </div>`;
        body.querySelector('[data-x="back"]')!.addEventListener("click", () => showSearch(artist, album, all));
        body.querySelector('[data-x="cancel"]')!.addEventListener("click", cancel);
        // 曲の対応を選び直したら、チェックと書き換えた値を引き継いで作り直す
        const diffEl = body.querySelector<HTMLElement>(".tf-diff");
        if (diffEl && keep) diffEl.scrollTop = keep.scroll;
        body.querySelectorAll<HTMLSelectElement>(".tf-assign").forEach((sel) =>
          sel.addEventListener("change", () => {
            const next = { ...assign, [sel.dataset.id!]: sel.value === "" ? null : Number(sel.value) };
            const kept: Keep = { checked: new Map(), values: new Map(), scroll: diffEl?.scrollTop ?? 0 };
            changes.forEach((x, i) => {
              kept.checked.set(changeKey(x), checked[i]);
              kept.values.set(changeKey(x), values[i]);
            });
            body.querySelectorAll<HTMLSelectElement>(".tf-assign").forEach((s) => (s.disabled = true));
            showDiff(c, target, artist, album, all, next, kept);
          }),
        );
        if (!changes.length) {
          body.querySelector<HTMLButtonElement>('[data-x="cancel"]')!.focus();
          return;
        }

        const okBtn = body.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
        const rowIdx = (row: Element) => (row as HTMLElement).dataset.idx!.split(",").map(Number);
        const refresh = () => {
          const n = checked.filter(Boolean).length;
          okBtn.textContent = tr("{n} 件を反映", { n: n.toLocaleString() });
          okBtn.disabled = n === 0;
          body.querySelectorAll(".tf-row").forEach((row) => row.classList.toggle("off", !checked[rowIdx(row)[0]]));
          // 曲の見出しのチェック: 全部 / 一部 / なし
          body.querySelectorAll<HTMLElement>(".tf-track").forEach((sec) => {
            const rows = [...sec.querySelectorAll(".tf-row")];
            const on = rows.filter((r) => checked[rowIdx(r)[0]]).length;
            const box = sec.querySelector<HTMLInputElement>(".tf-track-chk");
            if (!box) return;
            box.checked = on === rows.length;
            box.indeterminate = on > 0 && on < rows.length;
          });
        };
        body.querySelectorAll<HTMLElement>(".tf-row").forEach((row) => {
          const idx = rowIdx(row);
          row.querySelector<HTMLInputElement>(".tf-chk")!.addEventListener("change", (e) => {
            idx.forEach((i) => (checked[i] = (e.target as HTMLInputElement).checked));
            refresh();
          });
          // 変更後の値: クリックで入力欄に切り替え、確定したら差分表示に戻す
          const afterEl = row.querySelector<HTMLElement>(".tf-after")!;
          afterEl.addEventListener("click", () => {
            if (afterEl.querySelector("input")) return;
            afterEl.innerHTML = `<input type="text" value="${esc(values[idx[0]])}">`;
            const input = afterEl.querySelector("input")!;
            input.focus();
            input.select();
            const done = () => {
              idx.forEach((i) => (values[i] = input.value.trim()));
              const [before, after] = diffCells(changes[idx[0]].before, values[idx[0]]);
              row.querySelector(".tf-before")!.innerHTML = before;
              afterEl.innerHTML = after;
              afterEl.title = tr("{a}（クリックで編集）", { a: values[idx[0]] });
              idx.forEach((i) => (checked[i] = true));
              row.querySelector<HTMLInputElement>(".tf-chk")!.checked = true;
              refresh();
            };
            input.addEventListener("blur", done, { once: true });
            input.addEventListener("keydown", (e) => {
              if (isEnter(e)) input.blur();
            });
          });
        });
        body.querySelectorAll<HTMLElement>(".tf-track").forEach((sec) =>
          sec.querySelector<HTMLInputElement>(".tf-track-chk")?.addEventListener("change", (e) => {
            const on = (e.target as HTMLInputElement).checked;
            sec.querySelectorAll(".tf-row").forEach((row) => {
              rowIdx(row).forEach((i) => (checked[i] = on));
              row.querySelector<HTMLInputElement>(".tf-chk")!.checked = on;
            });
            refresh();
          }),
        );
        refresh();

        okBtn.addEventListener("click", async () => {
          const byId = new Map<string, Record<string, string | number>>();
          for (const [i, x] of changes.entries()) {
            if (!checked[i]) continue;
            const v = values[i];
            const num = x.field === "year" || x.field === "trackNr" || x.field === "discNr";
            if (num && v && !/^\d{1,4}$/.test(v)) {
              toast(tr("{a}は数字で入力してください", { a: tr(TAG_FIELD_LABEL[x.field]) }), "error");
              return;
            }
            if (x.field === "title" && !v) {
              toast(tr("タイトルは空にできません"), "error");
              return;
            }
            const edit = byId.get(x.id) ?? {};
            edit[x.field] = num ? Number(v || 0) : v;
            byId.set(x.id, edit);
          }
          const restore = busySave(okBtn);
          if (pc) {
            const n = await call<number>("apply_pc_tag_edits", {
              edits: [...byId].map(([id, edit]) => ({ path: findTrack(id)!.pcPath!, edit })),
            });
            // 一部だけ失敗した場合もエラー表示のあとで読み直す
            rescanCurrentPc();
            if (n === null) {
              restore();
              refresh();
              return;
            }
            netReach(funnel, "saved");
            await restore.settle();
            toast(tr("{n} 曲のファイルを更新しました", { n: n.toLocaleString() }));
            cancel();
            return;
          }
          const r = await call<{ count: number; snapshot: Snapshot }>("apply_tag_edits", {
            edits: [...byId].map(([id, edit]) => ({ id, edit })),
          });
          if (!r) {
            restore();
            refresh();
            return;
          }
          netReach(funnel, "saved");
          applySnapshot(r.snapshot);
          await restore.settle();
          toast(tr("{n} 曲の情報を更新しました", { n: r.count.toLocaleString() }));
          cancel();
        });
      };

      showSearch(first.albumArtist || first.artist, first.album, true);
    },
    "wide",
  );
  analyticsNet("song_info", funnel);
}
