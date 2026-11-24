// アーティスト名の表記ゆれの統一

import { t as tr } from "../../i18n";
import { invoke, listen } from "../../infra/tauri";
import { analyticsAliasLookup } from "../../app/analytics";
import type { Snapshot } from "../../domain/types";
import { esc } from "../dom";
import { call, toast } from "../feedback";
import { busySave, modal } from "../modal";
import { applySnapshot } from "../render";

interface VariantGroup {
  names: { name: string; count: number }[];
  reasons: ("normalize" | "album" | "alias")[];
  suggested: string;
}

// tr: 使う側で訳す
const VARIANT_REASON_LABEL: Record<VariantGroup["reasons"][number], string> = {
  normalize: "表記の違い",
  album: "同じアルバム",
  alias: "MusicBrainz の別名",
};

/**
 * アーティスト名の表記ゆれをまとめる。候補をグループごとに見せ、統一先を選んでから反映する。
 * 同じアーティストかどうかの判断が要るので、確認を省略する設定は設けない
 */
export async function unifyArtists() {
  let res = await call<{ groups: VariantGroup[]; unchecked: number }>("artist_variants");
  if (!res) return;
  // モーダルの中の処理から書き換えるので、変数ではなくオブジェクトで持つ
  const lookup = { running: false, unlisten: null as (() => void) | null, cancelled: false, total: 0 };
  await modal(
    `<h3>${esc(tr("アーティスト名の表記ゆれ"))}</h3>
     <p>${esc(tr("同じアーティストと思われる表記をまとめます。統一先の表記を選んでください。チェックを外したグループは変更しません。iPod 内の情報だけを書き換え、PC の元ファイルは変更しません。"))}</p>
     <div class="uv-net">
       <span class="uv-net-text muted"></span>
       <span class="spacer"></span>
       <button class="btn" data-x="net"></button>
     </div>
     <div class="uv-list"></div>
     <div class="modal-actions">
       <span class="spacer"></span>
       <button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button>
       <button class="btn primary" data-x="ok"></button>
     </div>`,
    (root, close) => {
      const list = root.querySelector<HTMLElement>(".uv-list")!;
      const okBtn = root.querySelector<HTMLButtonElement>('[data-x="ok"]')!;
      const netBtn = root.querySelector<HTMLButtonElement>('[data-x="net"]')!;
      const netText = root.querySelector<HTMLElement>(".uv-net-text")!;
      const groups = () => res!.groups;

      const refresh = () => {
        const n = [...list.querySelectorAll<HTMLInputElement>("[data-g]")].filter((b) => b.checked).length;
        okBtn.textContent = tr("{n} グループを統一", { n: n.toLocaleString() });
        okBtn.disabled = n === 0 || lookup.running;
      };
      const renderNet = () => {
        if (lookup.running) return;
        const u = res!.unchecked;
        netText.textContent = u
          ? tr("MusicBrainz で未確認のアーティスト: {n} 組 (約 {a} 分)", { n: u.toLocaleString(), a: Math.max(1, Math.ceil((u * 1.1) / 60)) })
          : tr("MusicBrainz の別名も確認済みです");
        netBtn.textContent = tr("ネットで別名も探す");
        netBtn.disabled = u === 0;
      };
      const render = () => {
        list.innerHTML = groups().length
          ? groups()
              .map(
                (g, gi) => `<div class="uv-group">
            <label class="uv-head"><input type="checkbox" data-g="${gi}" checked>
              <span class="uv-reasons">${g.reasons.map((r) => `<span class="uv-reason">${esc(tr(VARIANT_REASON_LABEL[r]))}</span>`).join("")}</span></label>
            ${g.names
              .map(
                (n, ni) => `<label class="uv-name"><input type="radio" name="uv-${gi}" value="${ni}" ${n.name === g.suggested ? "checked" : ""}>
                <span class="uv-label" title="${esc(n.name)}">${esc(n.name)}</span><span class="muted">${n.count.toLocaleString()} ${esc(tr("曲"))}</span></label>`,
              )
              .join("")}
          </div>`,
              )
              .join("")
          : `<div class="uv-empty muted">${esc(res!.unchecked ? tr("表記ゆれは見つかりませんでした。「ネットで別名も探す」を使うと、「米津玄師」と「Kenshi Yonezu」のような、別の文字で書いた表記も調べられます") : tr("表記ゆれは見つかりませんでした"))}</div>`;
        list.querySelectorAll("[data-g]").forEach((b) => b.addEventListener("change", refresh));
        renderNet();
        refresh();
      };

      const finish = () => {
        if (lookup.running) {
          lookup.cancelled = true;
          invoke("cancel_alias_lookup");
        }
        lookup.unlisten?.();
        close();
      };
      netBtn.addEventListener("click", async () => {
        if (lookup.running) {
          lookup.cancelled = true;
          invoke("cancel_alias_lookup");
          return;
        }
        lookup.running = true;
        lookup.cancelled = false;
        lookup.total = 0;
        const started = performance.now();
        netBtn.textContent = tr("中止");
        netText.textContent = tr("MusicBrainz に問い合わせ中…");
        refresh();
        lookup.unlisten = await listen<{ done: number; total: number }>("alias-progress", (ev) => {
          netText.textContent = tr("MusicBrainz に問い合わせ中… {n} / {a}", { n: ev.payload.done.toLocaleString(), a: ev.payload.total.toLocaleString() });
          lookup.total = ev.payload.total;
        });
        const got = await call<number>("lookup_artist_aliases");
        analyticsAliasLookup(lookup.cancelled ? "cancelled" : got === null ? "error" : "done", lookup.total, performance.now() - started);
        lookup.unlisten();
        lookup.unlisten = null;
        lookup.running = false;
        if (!root.isConnected) return;
        const next = await call<{ groups: VariantGroup[]; unchecked: number }>("artist_variants");
        if (next) res = next;
        render();
      });

      root.querySelector('[data-x="cancel"]')!.addEventListener("click", finish);
      okBtn.addEventListener("click", async () => {
        const edits = groups()
          .map((g, gi) => {
            if (!list.querySelector<HTMLInputElement>(`[data-g="${gi}"]`)!.checked) return null;
            const ni = Number(list.querySelector<HTMLInputElement>(`input[name="uv-${gi}"]:checked`)?.value ?? 0);
            return { from: g.names.map((n) => n.name), to: g.names[ni].name };
          })
          .filter((e) => e !== null);
        const restore = busySave(okBtn);
        const r = await call<{ count: number; snapshot: Snapshot }>("unify_artists", { edits });
        if (!r) {
          restore();
          refresh();
          return;
        }
        applySnapshot(r.snapshot);
        await restore.settle();
        toast(tr("{n} 曲のアーティスト名を統一しました", { n: r.count.toLocaleString() }));
        finish();
      });
      render();
    },
    "wide",
  );
  // Esc や背景クリックで閉じたときも問い合わせを止める
  if (lookup.running) invoke("cancel_alias_lookup");
  lookup.unlisten?.();
}
