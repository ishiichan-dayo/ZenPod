// 読みの自動生成 (プレビューして選んで反映)

import { t as tr } from "../../i18n";
import type { Snapshot } from "../../domain/types";
import { pref, setPref } from "../../infra/prefs";
import { esc } from "../dom";
import { call, toast } from "../feedback";
import { busySave, modal } from "../modal";
import { applySnapshot } from "../render";

interface ReadingChange {
  id: string;
  trackTitle: string;
  field: "title" | "artist" | "album" | "albumArtist" | "composer";
  source: string;
  before: string;
  after: string;
}

// tr: 使う側で訳す
const READING_FIELD_LABEL: Record<ReadingChange["field"], string> = {
  title: "タイトル",
  artist: "アーティスト",
  album: "アルバム",
  albumArtist: "アルバムアーティスト",
  composer: "作曲者",
};

/** 読みの自動生成。設定で確認を省略していなければ、差分を見せてから反映する */
export async function generateReadings(ids: string[] | null) {
  const overwrite = ids !== null;
  if (!pref("confirmReadings")) {
    const r = await call<{ count: number; snapshot: Snapshot }>("generate_readings", { ids, overwrite });
    if (!r) return;
    applySnapshot(r.snapshot);
    toast(r.count ? tr("{n} 曲に読みを付けました", { n: r.count }) : tr("読みを付ける曲はありませんでした"));
    return;
  }
  const changes = await call<ReadingChange[]>("preview_readings", { ids, overwrite });
  if (!changes) return;
  if (!changes.length) {
    toast(tr("読みを付ける (変更する) 曲はありませんでした"));
    return;
  }
  const trackCount = new Set(changes.map((c) => c.id)).size;
  await modal(
    `<h3>${esc(tr("読みの変更を確認"))}</h3>
     <p>${esc(tr("{n} 曲・{c} 件の読みを変更します。チェックを外した行は変更しません。変更後の読みは直接書き換えられます。", { n: trackCount.toLocaleString(), c: changes.length.toLocaleString() }))}</p>
     <div class="rd-wrap">
       <table class="rd-table">
         <thead><tr>
           <th class="rd-chk"><input type="checkbox" id="rd-all" checked></th>
           <th>${esc(tr("曲"))}</th><th>${esc(tr("項目"))}</th><th>${esc(tr("表記"))}</th><th>${esc(tr("現在の読み"))}</th><th>${esc(tr("変更後"))}</th>
         </tr></thead>
         <tbody>
         ${changes
           .map(
             (c, i) => `<tr>
             <td class="rd-chk"><input type="checkbox" data-i="${i}" checked></td>
             <td class="rd-track" title="${esc(c.trackTitle)}">${esc(c.trackTitle)}</td>
             <td class="rd-field">${esc(tr(READING_FIELD_LABEL[c.field]))}</td>
             <td class="rd-src" title="${esc(c.source)}">${esc(c.source)}</td>
             <td class="rd-before">${c.before ? esc(c.before) : `<span class="muted">${esc(tr("（なし）"))}</span>`}</td>
             <td class="rd-after"><input type="text" data-v="${i}" value="${esc(c.after)}"></td>
           </tr>`,
           )
           .join("")}
         </tbody>
       </table>
     </div>
     <div class="modal-actions">
       <label class="check rd-skip"><input type="checkbox" id="rd-skip"> ${esc(tr("次回から確認せずに反映する"))}</label>
       <span class="spacer"></span>
       <button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button>
       <button class="btn primary" data-x="ok"></button>
     </div>`,
    (root, close) => {
      const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      const boxes = [...root.querySelectorAll<HTMLInputElement>("tbody input[type=checkbox]")];
      const all = root.querySelector<HTMLInputElement>("#rd-all")!;
      const refresh = () => {
        const n = boxes.filter((b) => b.checked).length;
        okBtn.textContent = tr("{n} 件を反映", { n: n.toLocaleString() });
        okBtn.disabled = n === 0;
        all.checked = n === boxes.length;
        all.indeterminate = n > 0 && n < boxes.length;
      };
      boxes.forEach((b) => b.addEventListener("change", refresh));
      all.addEventListener("change", () => {
        boxes.forEach((b) => (b.checked = all.checked));
        refresh();
      });
      refresh();
      root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
      okBtn.addEventListener("click", async () => {
        const edits = boxes
          .filter((b) => b.checked)
          .map((b) => {
            const i = Number(b.dataset.i);
            const v = root.querySelector<HTMLInputElement>(`[data-v="${i}"]`)!.value.trim();
            return { id: changes[i].id, field: changes[i].field, value: v };
          });
        const restore = busySave(okBtn);
        const r = await call<{ count: number; snapshot: Snapshot }>("apply_readings", { edits });
        if (!r) {
          restore();
          return;
        }
        if (root.querySelector<HTMLInputElement>("#rd-skip")!.checked) setPref("confirmReadings", false);
        applySnapshot(r.snapshot);
        await restore.settle();
        toast(tr("{n} 曲の読みを更新しました", { n: r.count.toLocaleString() }));
        close();
      });
    },
    "wide",
  );
}
