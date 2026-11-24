// 複数の曲のファイル名の変更 (タグのパーツを組み合わせる)

import { t as tr } from "../../i18n";
import { pcPool } from "../../app/selectors";
import { state } from "../../app/state";
import { NAME_PARTS, NAME_PRESETS, localizePattern, nameFromPattern, splitFileName } from "../../domain/file-name";
import type { Snapshot, Track } from "../../domain/types";
import { getStored, setStored } from "../../infra/prefs";
import { esc } from "../dom";
import { call, toast } from "../feedback";
import { busySave, modal } from "../modal";
import { renamablePath, rescanCurrentPc } from "../pc";
import { findTrack, player, stopPlayer } from "../player";
import { applySnapshot } from "../render";
import { tokenEditor, tokensHtml } from "../token-editor";

export async function renamePcFiles(ids: string[]) {
  const tracks = ids.map((id) => findTrack(id)).filter((t): t is Track => !!renamablePath(t));
  if (!tracks.length) return;
  // PC の曲か、Rockbox モードの曲 (iPod の中のファイル) か
  const onIpod = !tracks[0].pcPath;
  const pathOf = (t: Track) => renamablePath(t)!;
  let pattern = localizePattern(getStored("renamePattern") ?? NAME_PRESETS[0]);
  let pad = getStored("renamePad") !== "0";
  // 行ごとのチェック (外した行は変えない)。パターンを変えても外した行は外したまま
  const excluded = new Set<string>();

  await modal(
    `<h3>${esc(tr("{n} 曲のファイル名を変更", { n: tracks.length.toLocaleString() }))}</h3>
     <p>${esc(tr("タグの項目を組み合わせてファイル名を付けます。拡張子はそのままです。ファイル名に使えない文字は、見た目の近い文字 (？ ／ ： など) に置き換えます。"))}</p>
     <div class="rn-parts" id="rn-parts"></div>
     <div class="tok-editor rn-pattern" id="rn-pattern" contenteditable="plaintext-only" spellcheck="false" role="textbox" aria-label="${esc(tr("ファイル名"))}"></div>
     <div class="tok-hint muted">${esc(tr("部品はドラッグで足したり並べ替えたりできます。欄の外へ出すと外れます。"))}</div>
     <div class="rn-options">
       <span class="muted">${esc(tr("よく使う形:"))}</span>
       ${NAME_PRESETS.map(localizePattern).map((p) => `<button type="button" class="rn-preset" data-preset="${esc(p)}">${tokensHtml(p, NAME_PARTS.map(([k]) => tr(k)))}</button>`).join("")}
     </div>
     <div class="rn-list">
       <div class="rn-list-bar"><label class="check"><input type="checkbox" id="rn-pad" ${pad ? "checked" : ""}> ${esc(tr("曲順を 2 桁にそろえる (01)"))}</label></div>
       <div class="rd-wrap"><table class="rd-table rn-table">
         <thead><tr><th class="rd-chk"><input type="checkbox" id="rn-all"></th><th>${esc(tr("今の名前"))}</th><th>${esc(tr("新しい名前"))}</th><th></th></tr></thead>
         <tbody></tbody>
       </table></div>
     </div>
     <div class="modal-actions">
       <span class="spacer"></span>
       <button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button>
       <button class="btn primary" data-x="ok"></button>
     </div>`,
    (root, close) => {
      const tbody = root.querySelector("tbody")!;
      const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      const allBox = root.querySelector<HTMLInputElement>("#rn-all")!;
      type Row = { t: Track; stem: string; ext: string; name: string; problem: string; key: string };
      let rows: Row[] = [];

      const compute = () => {
        // 同じフォルダにある、今回変えないファイルの名前 (ぶつからないか調べる)
        const inBatch = new Set(tracks.map((t) => pathOf(t).toLowerCase()));
        const pool = onIpod ? (state.snap?.tracks ?? []).filter((t) => t.rbPath) : pcPool();
        const others = new Set(
          pool
            .map((t) => pathOf(t).toLowerCase())
            .filter((p) => !inBatch.has(p)),
        );
        rows = tracks.map((t) => {
          const { dir, stem, ext } = splitFileName(pathOf(t));
          const { name, missing } = nameFromPattern(pattern, t, pad);
          let problem = "";
          if (!name) problem = tr("名前が空になります");
          else if (missing.length) problem = tr("{a} が空です", { a: missing.map((k) => tr(k)).join(tr("・")) });
          else if (others.has((dir + name + ext).toLowerCase())) problem = tr("同じ名前のファイルがあります");
          return { t, stem, ext, name, problem, key: (dir + name + ext).toLowerCase() };
        });
        // 名前が変わらない曲はそのまま。変わる曲が、変わらない曲やほかの変わる曲と同じ名前になるときだけ外す
        const count = new Map<string, number>();
        for (const r of rows) count.set(r.key, (count.get(r.key) ?? 0) + 1);
        for (const r of rows)
          if (!r.problem && r.name !== r.stem && (count.get(r.key) ?? 0) > 1) r.problem = tr("ほかの曲と同じ名前になります");
      };
      const active = (r: Row) => !r.problem && r.name !== r.stem && !excluded.has(r.t.id);
      const render = () => {
        compute();
        tbody.innerHTML = rows
          .map((r, i) => {
            const same = r.name === r.stem;
            const disabled = !!r.problem || same;
            return `<tr class="${disabled ? "rn-off" : ""}">
              <td class="rd-chk"><input type="checkbox" data-i="${i}" ${active(r) ? "checked" : ""} ${disabled ? "disabled" : ""}></td>
              <td class="rd-before" title="${esc(r.stem + r.ext)}">${esc(r.stem)}<span class="muted">${esc(r.ext)}</span></td>
              <td class="rd-track" title="${esc(r.name + r.ext)}">${r.name ? `${esc(r.name)}<span class="muted">${esc(r.ext)}</span>` : ""}</td>
              <td class="rn-state">${r.problem ? `<span class="rn-problem">${esc(r.problem)}</span>` : same ? `<span class="muted">${esc(tr("変更なし"))}</span>` : ""}</td>
            </tr>`;
          })
          .join("");
        tbody.querySelectorAll<HTMLInputElement>("input[data-i]").forEach((b) =>
          b.addEventListener("change", () => {
            const id = rows[Number(b.dataset.i)].t.id;
            if (b.checked) excluded.delete(id);
            else excluded.add(id);
            refresh();
          }),
        );
        refresh();
      };
      const refresh = () => {
        const selectable = rows.filter((r) => !r.problem && r.name !== r.stem);
        const n = selectable.filter((r) => !excluded.has(r.t.id)).length;
        okBtn.textContent = tr("{n} 曲の名前を変更", { n: n.toLocaleString() });
        okBtn.disabled = n === 0;
        allBox.checked = n > 0 && n === selectable.length;
        allBox.indeterminate = n > 0 && n < selectable.length;
      };

      const editor = tokenEditor(
        root.querySelector<HTMLElement>("#rn-pattern")!,
        root.querySelector<HTMLElement>("#rn-parts")!,
        NAME_PARTS.map(([k]) => tr(k)),
        pattern,
        (v) => {
          pattern = v;
          render();
        },
      );
      root.querySelectorAll<HTMLElement>("[data-preset]").forEach((b) =>
        b.addEventListener("click", () => {
          pattern = b.dataset.preset!;
          editor.set(pattern);
          render();
        }),
      );
      root.querySelector<HTMLInputElement>("#rn-pad")!.addEventListener("change", (e) => {
        pad = (e.target as HTMLInputElement).checked;
        render();
      });
      allBox.addEventListener("change", () => {
        for (const r of rows) {
          if (r.problem || r.name === r.stem) continue;
          if (allBox.checked) excluded.delete(r.t.id);
          else excluded.add(r.t.id);
        }
        render();
      });
      root.querySelector('[data-x="cancel"]')!.addEventListener("click", close);
      okBtn.addEventListener("click", async () => {
        const target = rows.filter(active);
        if (!target.length) return;
        const restore = busySave(okBtn);
        if (target.some((r) => r.t.id === player.current)) stopPlayer();
        if (onIpod) {
          const snap = await call<Snapshot>("rename_rockbox_files", { renames: target.map((x) => ({ id: x.t.id, name: x.name })) });
          if (!snap) {
            restore();
            refresh();
            return;
          }
          setStored("renamePattern", pattern);
          setStored("renamePad", pad ? "1" : "0");
          applySnapshot(snap);
          await restore.settle();
          toast(tr("{n} 曲のファイル名を変更しました", { n: target.length.toLocaleString() }));
          close();
          return;
        }
        const r = await call<string[]>("rename_pc_files", {
          renames: target.map((x) => ({ path: x.t.pcPath!, name: x.name })),
        });
        if (!r) {
          restore();
          refresh();
          return;
        }
        setStored("renamePattern", pattern);
        setStored("renamePad", pad ? "1" : "0");
        await restore.settle();
        toast(tr("{n} 曲のファイル名を変更しました", { n: r.length.toLocaleString() }));
        close();
        rescanCurrentPc();
      });
      render();
      editor.focus();
    },
    "wide",
  );
}
