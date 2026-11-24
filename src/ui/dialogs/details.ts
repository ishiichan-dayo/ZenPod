// 曲の詳細情報

import { lang, t as tr } from "../../i18n";
import { fmtBytes, fmtDur } from "../../domain/format";
import { esc } from "../dom";
import { call, toast } from "../feedback";
import { modal } from "../modal";
import { findTrack } from "../player";

interface FileDetails {
  path: string;
  fileSize: number;
  modified: number;
  container: string;
  codec: string;
  lossless: boolean | null;
  durationMs: number;
  audioBitrate: number | null;
  overallBitrate: number | null;
  sampleRate: number | null;
  bitDepth: number | null;
  channels: number | null;
  extra: [string, string][];
  transfer: string;
  tags: { kind: string; items: [string, string][] }[];
  pictures: { kind: string; mime: string; width: number; height: number; bytes: number }[];
  /** タグの読み取りで出たエラー (読めなくても形式と音質は出る) */
  tagError: string | null;
  /** 読めない項目を飛ばして読めたときのお知らせ (エラーではない) */
  tagNote: string | null;
  /** タグの構造 (名前と大きさだけ)。タグを読めない曲の報告に貼ってもらう */
  structure: string;
}

interface DbDetails {
  dbid: string;
  ipodPath: string;
  filetype: string;
  size: number;
  lengthMs: number;
  bitrate: number;
  sampleRate: number;
  mediatype: number;
  compilation: boolean;
  hasArtwork: boolean;
  rating: number;
  playCount: number;
  skipCount: number;
  dateAdded: number;
  dateModified: number;
  lastPlayed: number;
  lastSkipped: number;
}

// tr: 使う側で訳す
const MEDIA_TYPES: Record<number, string> = {
  1: "音楽",
  2: "ビデオ",
  4: "ポッドキャスト",
  6: "ビデオポッドキャスト",
  8: "オーディオブック",
  0x20: "ミュージックビデオ",
  0x40: "テレビ番組",
};

/** 曲のファイルの形式・音質・タグの中身と、iPod の曲なら iTunesDB の記録を表示する (1 曲) */
export async function showDetails(id: string) {
  const t = findTrack(id);
  if (!t) return;
  const date = (sec: number) => (sec ? new Date(sec * 1000).toLocaleString(lang === "ja" ? "ja-JP" : undefined) : "—");
  const khz = (hz: number) => `${(hz / 1000).toLocaleString(undefined, { maximumFractionDigits: 3 })} kHz`;
  const ch = (n: number) => (n === 1 ? tr("モノラル") : n === 2 ? tr("ステレオ") : `${n} ch`);
  const bytes = (n: number) => tr("{a} ({n} バイト)", { a: fmtBytes(n), n: n.toLocaleString() });
  const rows = (items: [string, string | null | undefined][]) =>
    items
      .filter(([, v]) => v !== null && v !== undefined && v !== "")
      .map(([k, v]) => `<span>${esc(k)}</span><span>${esc(v!)}</span>`)
      .join("");
  const section = (title: string, body: string) =>
    body ? `<div class="dt-sec"><div class="dt-title">${esc(title)}</div><div class="set-info dt-grid">${body}</div></div>` : "";

  type Reply = { file: FileDetails | null; fileError: string | null; fileStructure?: string; db: DbDetails | null };
  const render = (r: Reply) => {
    const f = r.file;
    const db = r.db;
    let html = "";
    if (f?.tagError) html += `<p class="dt-error">${esc(tr("タグの読み取りでエラーが出ました: {a}", { a: f.tagError }))}</p>`;
    if (f?.tagNote) html += `<p class="dt-error dt-note">${esc(f.tagNote)}</p>`;
    if (f) {
      const summary = [
        f.codec || f.container,
        f.audioBitrate ? `${f.audioBitrate} kbps` : "",
        f.sampleRate ? khz(f.sampleRate) : "",
        f.bitDepth ? `${f.bitDepth} bit` : "",
        f.channels ? ch(f.channels) : "",
      ].filter(Boolean);
      html += `<div class="dt-summary">${summary.map((s) => `<span>${esc(s)}</span>`).join("")}${
        f.lossless === null ? "" : `<span class="dt-badge${f.lossless ? " lossless" : ""}">${f.lossless ? tr("可逆") : tr("非可逆")}</span>`
      }</div>`;
      html += section(
        tr("形式"),
        rows([
          [tr("形式"), f.container],
          [tr("コーデック"), f.codec],
          [tr("長さ"), fmtDur(f.durationMs)],
          [tr("ビットレート (音声)"), f.audioBitrate ? `${f.audioBitrate} kbps` : null],
          [tr("ビットレート (全体)"), f.overallBitrate && f.overallBitrate !== f.audioBitrate ? `${f.overallBitrate} kbps` : null],
          [tr("サンプルレート"), f.sampleRate ? `${khz(f.sampleRate)} (${f.sampleRate.toLocaleString()} Hz)` : null],
          [tr("ビット深度"), f.bitDepth ? `${f.bitDepth} bit` : null],
          [tr("チャンネル"), f.channels ? ch(f.channels) : null],
          ...f.extra,
          ...(db ? [] : ([[tr("iPod への転送"), f.transfer]] as [string, string][])),
        ]),
      );
      html += section(
        tr("ファイル"),
        rows([
          [tr("場所"), f.path],
          [tr("サイズ"), bytes(f.fileSize)],
          [tr("更新日時"), date(f.modified)],
        ]),
      );
    } else {
      html += `<p class="dt-error">${esc(tr("ファイルを読めませんでした: {a}", { a: r.fileError ?? "" }))}</p>`;
    }
    if (db) {
      const stars = db.rating ? "★".repeat(Math.round(db.rating / 20)) : tr("なし");
      html += section(
        tr("iPod のデータベース"),
        rows([
          [tr("種類"), db.filetype],
          [tr("メディアの種類"), MEDIA_TYPES[db.mediatype] ? tr(MEDIA_TYPES[db.mediatype]) : `0x${db.mediatype.toString(16)}`],
          [tr("長さ"), fmtDur(db.lengthMs)],
          [tr("ビットレート"), db.bitrate ? `${db.bitrate} kbps` : null],
          [tr("サンプルレート"), db.sampleRate ? khz(db.sampleRate) : null],
          [tr("サイズ"), bytes(db.size)],
          [tr("追加日"), date(db.dateAdded)],
          [tr("更新日"), date(db.dateModified)],
          [tr("最後に再生"), date(db.lastPlayed)],
          [tr("再生回数"), db.playCount.toLocaleString()],
          [tr("スキップ回数"), db.skipCount.toLocaleString()],
          [tr("最後にスキップ"), db.lastSkipped ? date(db.lastSkipped) : null],
          [tr("評価"), stars],
          [tr("コンピレーション"), db.compilation ? tr("はい") : tr("いいえ")],
          [tr("アートワーク"), db.hasArtwork ? tr("あり") : tr("なし")],
          [tr("iPod 内のパス"), db.ipodPath],
          [tr("データベース ID"), db.dbid],
        ]),
      );
    }
    if (f?.pictures.length) {
      html += section(
        tr("埋め込み画像"),
        f.pictures
          .map(
            (p) =>
              `<span>${esc(p.kind)}</span><span>${esc(
                [p.mime.replace("image/", "").toUpperCase(), p.width ? `${p.width}×${p.height}` : "", fmtBytes(p.bytes)]
                  .filter(Boolean)
                  .join(" · "),
              )}</span>`,
          )
          .join(""),
      );
    }
    for (const tag of f?.tags ?? []) {
      html += `<details class="dt-sec dt-tags"${tag.items.length <= 40 ? " open" : ""}>
        <summary class="dt-title">${esc(tr("タグ: {a}", { a: tag.kind }))} <span class="dt-count">${esc(tr("{n} 項目", { n: tag.items.length }))}</span></summary>
        <div class="set-info dt-grid">${
          tag.items.length
            ? tag.items.map(([k, v]) => `<span class="dt-key">${esc(k)}</span><span>${esc(v)}</span>`).join("")
            : `<span>${esc(tr("（空）"))}</span><span></span>`
        }</div></details>`;
    }
    // タグを読めなかったときは「なし」と出さない (無いのではなく読めていない)
    if (f && !f.tags.length && !f.tagError) html += section(tr("タグ"), rows([["", tr("なし")]]));
    // タグを読めないときは開いておく (報告に貼ってもらう。「テキストでコピー」にも入る)
    // ファイルごと読めなかったときも出す
    const structure = f ? f.structure : r.fileStructure;
    if (structure) {
      html += `<details class="dt-sec dt-tags"${!f || f.tagError ? " open" : ""}>
        <summary class="dt-title">${esc(tr("タグの構造"))} <span class="dt-count">${esc(tr("名前と大きさだけ"))}</span></summary>
        <pre class="dt-pre">${esc(structure)}</pre></details>`;
    }
    return html;
  };

  await modal(
    `<h3>${esc(t.artist ? `${t.title} — ${t.artist}` : t.title)}</h3>
     <div class="dt-body"><p>${esc(tr("読み込み中…"))}</p></div>
     <div class="modal-actions"><button class="btn" data-x="copy">${esc(tr("テキストでコピー"))}</button><button class="btn primary" data-x="close">${esc(tr("閉じる"))}</button></div>`,
    (root, close) => {
      const body = root.querySelector<HTMLElement>(".dt-body")!;
      const closeBtn = root.querySelector<HTMLButtonElement>('[data-x="close"]')!;
      closeBtn.addEventListener("click", close);
      closeBtn.focus();
      root.querySelector('[data-x="copy"]')!.addEventListener("click", () => {
        // 見出しと「項目<TAB>値」の行にして貼り付けやすくする
        const lines: string[] = [root.querySelector("h3")!.textContent ?? ""];
        body.querySelectorAll<HTMLElement>(".dt-error, .dt-summary, .dt-sec").forEach((sec) => {
          if (sec.classList.contains("dt-error")) {
            lines.push("", sec.textContent ?? "");
            return;
          }
          if (sec.classList.contains("dt-summary")) {
            lines.push("", [...sec.children].map((c) => c.textContent).join(" · "));
            return;
          }
          const title = sec.querySelector(".dt-title")!;
          const pre = sec.querySelector("pre");
          // タグの構造: 見出し (後ろの説明は付けない) と、中身をそのまま
          if (pre) {
            lines.push("", `[${title.firstChild!.textContent!.trim()}]`, pre.textContent ?? "");
            return;
          }
          lines.push("", `[${title.textContent!.trim()}]`);
          const cells = [...sec.querySelectorAll<HTMLElement>(".dt-grid > span")];
          for (let i = 0; i + 1 < cells.length; i += 2) lines.push(`${cells[i].textContent}\t${cells[i + 1].textContent}`);
        });
        navigator.clipboard.writeText(lines.join("\n")).then(
          () => toast(tr("コピーしました")),
          () => toast(tr("コピーできませんでした"), "error"),
        );
      });
      const args = t.pcPath ? { path: t.pcPath } : { id };
      call<Reply>("track_details", args).then((r) => {
        if (!body.isConnected) return;
        if (!r) {
          close();
          return;
        }
        body.innerHTML = render(r);
      });
    },
    "wide",
  );
}
