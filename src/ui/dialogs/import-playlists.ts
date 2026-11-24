// iTunes / ミュージックの XML と .m3u / .m3u8 の取り込み、スマートプレイリストの XML とプレイリストの .m3u8 の書き出し

import { t as tr } from "../../i18n";
import { invoke, openDialog } from "../../infra/tauri";
import { writable } from "../../app/selectors";
import { state } from "../../app/state";
import { splFieldLabel } from "../../domain/smart";
import type { Snapshot } from "../../domain/types";
import { esc } from "../dom";
import { call, failToast, toast, withArtProgress } from "../feedback";
import { ICON_PL, ICON_SMART } from "../icons";
import { busySave, modal } from "../modal";
import { applySnapshot } from "../render";
import { startTransfer } from "../transfer";

// ---------------- iTunes / ミュージックの XML ----------------

interface ImportCandidate {
  index: number;
  name: string;
  smart: boolean;
  total: number;
  matched: number;
  missing: string[];
  missingCount: number;
  /** 見つからない曲のうち、PC にファイルがあって転送できるもの (場所) */
  transferable: string[];
  exists: boolean;
  /** スマートプレイリストを条件ごと取り込む。false なら iTunes が入れた曲のまま */
  asSmart: boolean;
  /** 開いている iPod で判定できないので、外して取り込む条件の項目 */
  droppedFields: number[];
}

interface ImportSummary {
  playlists: number;
  tracks: number;
  missing: number;
}

/**
 * プレイリストを取り込む。iTunes / ミュージックの「ライブラリを書き出す」「プレイリストを書き出す」の XML と、
 * .m3u / .m3u8 (いくつでも)。見つからない曲が PC にあれば、先に転送してから取り込める
 */
export async function importPlaylists() {
  if (!writable()) return;
  const sel = await openDialog({
    multiple: true,
    title: tr("取り込むプレイリスト (iTunes / ミュージックの XML、.m3u / .m3u8)"),
    filters: [{ name: tr("プレイリスト"), extensions: ["xml", "m3u", "m3u8"] }],
  });
  if (!sel) return;
  const paths = Array.isArray(sel) ? sel : [sel];
  if (!paths.length) return;
  const list = await withArtProgress(tr("読み込み中…"), () => call<ImportCandidate[]>("playlist_import_preview", { paths }));
  if (!list) return;
  if (!list.length) {
    toast(tr("取り込めるプレイリストがありません (ライブラリやミュージックなどの組み込みのものは取り込みません)"), "info", 6000);
    return;
  }
  const picked = new Set(list.map((c) => c.index));
  const rowHtml = (c: ImportCandidate) => {
    const icon = c.smart ? ICON_SMART : ICON_PL;
    const count = c.asSmart
      ? tr("条件ごと")
      : c.missingCount
        ? tr("{a} / {b} 曲", { a: c.matched, b: c.total })
        : tr("{n} 曲", { n: c.matched });
    return `<div class="imp-row" data-i="${c.index}">
      <label class="imp-main">
        <input type="checkbox" data-i="${c.index}" ${picked.has(c.index) ? "checked" : ""} />
        <span class="imp-icon">${icon}</span>
        <span class="imp-name">${esc(c.name)}</span>
        ${c.exists ? `<span class="imp-badge">${esc(tr("上書き"))}</span>` : ""}
        <span class="imp-count muted">${esc(count)}</span>
      </label>
      ${
        c.droppedFields.length
          ? `<div class="imp-note">${esc(tr("この iPod では判定できない条件 ({a}) を外して取り込みます", { a: c.droppedFields.map(splFieldLabel).join(tr("、")) }))}</div>`
          : ""
      }
      ${
        c.missingCount
          ? `<button type="button" class="imp-missing-btn" data-miss="${c.index}" aria-expanded="false">${esc(
              c.asSmart
                ? c.transferable.length
                  ? tr("iTunes で入っていた曲のうち iPod に無い曲 {n} (PC から転送できる曲 {m})", { n: c.missingCount, m: c.transferable.length })
                  : tr("iTunes で入っていた曲のうち iPod に無い曲 {n}", { n: c.missingCount })
                : c.transferable.length
                  ? tr("見つからない曲 {n} (PC から転送できる曲 {m})", { n: c.missingCount, m: c.transferable.length })
                  : tr("見つからない曲 {n}", { n: c.missingCount }),
            )}</button>
             <ul class="imp-missing hidden">${c.missing.map((m) => `<li>${esc(m)}</li>`).join("")}${
               c.missingCount > c.missing.length ? `<li class="muted">${esc(tr("ほか {n} 曲", { n: c.missingCount - c.missing.length }))}</li>` : ""
             }</ul>`
          : ""
      }
    </div>`;
  };
  const note = tr("スマートプレイリストは条件ごと取り込み、iPod の曲から選び直します。");
  await modal(
    `<h3>${esc(tr("プレイリストを取り込む"))}</h3>
     <p>${esc(tr("iPod の曲と、曲名・アーティスト・アルバムで結び付けます。iPod に無い曲は入りません。"))}${esc(note)}</p>
     <label class="imp-all"><input type="checkbox" id="imp-all" checked />${esc(tr("すべて選ぶ"))}<span class="muted">${esc(tr("{n} 個", { n: list.length }))}</span></label>
     <div class="imp-list">${list.map(rowHtml).join("")}</div>
     <label class="check imp-transfer hidden"><input type="checkbox" id="imp-xfer" checked /><span id="imp-xfer-text"></span></label>
     <div class="modal-actions"><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok"></button></div>`,
    (m, close) => {
      const okBtn = m.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      const all = m.querySelector<HTMLInputElement>("#imp-all")!;
      const xfer = m.querySelector<HTMLInputElement>("#imp-xfer")!;
      /** 選んだプレイリストの、転送できる曲 (同じファイルは 1 回) */
      const toSend = () => [...new Set(list.filter((c) => picked.has(c.index)).flatMap((c) => c.transferable))];
      const sync = () => {
        const n = toSend().length;
        m.querySelector(".imp-transfer")!.classList.toggle("hidden", !n);
        m.querySelector("#imp-xfer-text")!.textContent = tr("見つからない曲のうち、PC にある {n} 曲を先に iPod へ転送してから取り込む", { n });
        okBtn.textContent = n && xfer.checked ? tr("転送して {n} 個を取り込む", { n: picked.size }) : tr("{n} 個を取り込む", { n: picked.size });
        okBtn.disabled = !picked.size;
        all.checked = picked.size === list.length;
        all.indeterminate = picked.size > 0 && picked.size < list.length;
      };
      xfer.addEventListener("change", sync);
      sync();
      m.querySelector(".imp-list")!.addEventListener("change", (e) => {
        const cb = e.target as HTMLInputElement;
        const i = Number(cb.dataset.i);
        if (cb.checked) picked.add(i);
        else picked.delete(i);
        sync();
      });
      m.querySelector(".imp-list")!.addEventListener("click", (e) => {
        const b = (e.target as HTMLElement).closest<HTMLButtonElement>("[data-miss]");
        if (!b) return;
        const ul = b.nextElementSibling as HTMLElement;
        const open = ul.classList.toggle("hidden") === false;
        b.setAttribute("aria-expanded", String(open));
      });
      all.addEventListener("change", () => {
        if (all.checked) list.forEach((c) => picked.add(c.index));
        else picked.clear();
        m.querySelectorAll<HTMLInputElement>(".imp-list input[type=checkbox]").forEach((cb) => (cb.checked = all.checked));
        sync();
      });
      m.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
      const indexes = () => [...picked];
      const runImport = async (): Promise<boolean> => {
        const r = await call<{ snapshot: Snapshot; summary: ImportSummary }>("playlist_import", { paths, indexes: indexes() });
        if (!r) return false;
        applySnapshot(r.snapshot);
        const s = r.summary;
        toast(
          s.missing
            ? tr("{n} 個のプレイリストを取り込みました (見つからない曲 {m} 曲は入れていません)", { n: s.playlists, m: s.missing })
            : tr("{n} 個のプレイリストを取り込みました", { n: s.playlists }),
          "info",
          5000,
        );
        return true;
      };
      let saving = false;
      okBtn.addEventListener("click", async () => {
        if (saving || !picked.size) return;
        const send = xfer.checked ? toSend() : [];
        if (send.length) {
          // 先に転送して、終わったら (transfer-done) 取り込む。送った曲はタグで結び付く
          close();
          state.afterTransfer = async () => void (await runImport());
          if (!(await startTransfer(send, null))) state.afterTransfer = null;
          return;
        }
        saving = true;
        const restore = busySave(okBtn);
        const ok = await runImport();
        saving = false;
        if (!ok) {
          restore();
          return;
        }
        await restore.settle();
        close();
      });
    },
    "imp-modal",
    false,
  );
}

/** スマートプレイリストを iTunes / ミュージックで読み込める XML に書き出す */
export async function exportSmartXml(ids: string[]) {
  if (!ids.length) return;
  const one = ids.length === 1 ? state.snap?.playlists.find((p) => p.id === ids[0]) : null;
  // 保存のダイアログは Rust が開く (name は最初に入れておくファイル名。やめたら false)
  const name = one?.name ?? tr("スマートプレイリスト");
  let saved: boolean;
  try {
    saved = await invoke<boolean>("export_smart_xml", { ids, name });
  } catch (e) {
    failToast(e, 6000);
    return;
  }
  if (!saved) return;
  toast(tr("書き出しました。iTunes / ミュージックの「ファイル → ライブラリ → プレイリストを読み込む」で、条件ごと読み込めます"), "info", 7000);
}

/** プレイリストを .m3u8 に書き出す。1 つならファイル、いくつもならフォルダを選ぶ (ダイアログは Rust が開く) */
export async function exportM3u8(ids: string[]) {
  if (!ids.length) return;
  const one = ids.length === 1 ? state.snap?.playlists.find((p) => p.id === ids[0]) : null;
  let n: number;
  try {
    n = await invoke<number>("export_m3u8", { ids, name: one?.name ?? tr("プレイリスト") });
  } catch (e) {
    failToast(e, 6000);
    return;
  }
  if (!n) return;
  toast(tr("{n} 個のプレイリストを書き出しました。ほかの iPod では「プレイリストを取り込む」で読み込めます", { n }), "info", 7000);
}
