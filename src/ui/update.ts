// アップデート (GitHub Releases の latest.json を確認)

import { lang, t as tr } from "../i18n";
import { check, checkEarly, relaunch } from "../infra/tauri";
import type { Update } from "../infra/tauri";
import { appVersion, state } from "../app/state";
import { fmtBytes } from "../domain/format";
import { esc } from "./dom";
import { toast } from "./feedback";
import { modal } from "./modal";
import { showUpdateButton } from "./titlebar";

/**
 * 新しい版を確認する。`manual` は設定のボタンから (結果をトーストで出す)。
 * 先行版 (報告してくれた人に先に試してもらう小さな修正。GitHub のプレリリース) は、`early` (設定のアップデートの最後の
 * 「先行版」の「確認」) のときだけ確かめる (`checkEarly`。先行版の後に普通の版が出ていればそちらになる)。
 * 先行版は正式版と同じ番号で作り、確かめてもらったら scripts/promote.sh で latest.json の `early` を外して正式版にする。
 * 2.8.10 までの先行版は普通のリリースで、latest.json の `early` で見分けていた
 */
export async function checkForUpdate(manual: boolean, early = false) {
  let update: Update | null = null;
  try {
    update = early ? await checkEarly() : await check();
  } catch (e) {
    if (manual) toast(tr("アップデートを確認できませんでした: {a}", { a: String(e) }), "error", 6000);
    return;
  }
  if (update && update.rawJson.early === true && !early) update = null;
  if (!update) {
    if (manual) toast(tr("最新版です (バージョン {a})", { a: appVersion }));
    return;
  }
  offerUpdate(update);
}

/** 新しい版を案内する。「後で」で閉じても、タイトルバーのボタンからもう一度開ける (再起動するまで) */
export function offerUpdate(u: Update) {
  showUpdateButton(u.version, () => showUpdateCard(u));
  showUpdateCard(u);
}

/**
 * リリースノート (タグのメッセージ) から、画面の言語の部分だけを HTML にする。
 * ノートは日本語の後に "New in x.y.z" から始まる英語が続く形。先頭の "ZenPod x.y.z" (1.2.1 までは "iPod Sync x.y.z") は題名なので出さない。
 * "- " で始まる行は箇条書き、それ以外の行は見出しにする
 */
function releaseNotesHtml(body: string): string {
  const lines = body.replace(/\r/g, "").split("\n").map((l) => l.trim());
  const en = lines.findIndex((l) => /^New in \d/.test(l));
  const part = en < 0 ? lines : lang === "ja" ? lines.slice(0, en) : lines.slice(en);
  let html = "";
  let list = false;
  for (const l of part) {
    if (!l || /^(ZenPod|iPod Sync) v?\d/.test(l)) continue;
    // tr: 訳さない (箇条書きの記号)
    const item = l.match(/^[-・*]\s*(.*)$/);
    if (item && !list) html += "<ul>";
    if (!item && list) html += "</ul>";
    list = !!item;
    html += item ? `<li>${esc(item[1])}</li>` : `<h4>${esc(l)}</h4>`;
  }
  return list ? html + "</ul>" : html;
}

let updateShown = false;

/** 新しい版の案内 (画面中央のモーダル)。閉じるのは「後で」か「更新して再起動」だけ (Esc・外側のクリックでは閉じない。気づかずに閉じないように) */
export function showUpdateCard(u: Update) {
  if (updateShown) return;
  updateShown = true;
  const notes = releaseNotesHtml(u.body ?? "");
  modal(
    `<h3>${esc(tr("新しいバージョン {a} があります", { a: u.version }))}</h3>
     <div class="uc-sub muted">${esc(tr("現在のバージョン"))} ${esc(u.currentVersion)}</div>
     ${notes ? `<div class="uc-notes">${notes}</div>` : ""}
     <div class="uc-progress hidden"><div class="uc-bar"><div class="uc-fill"></div></div><span class="uc-pct"></span></div>
     <div class="modal-actions">
       <span class="spacer"></span>
       <button class="btn" data-x="later">${esc(tr("後で"))}</button>
       <button class="btn primary" data-x="install">${esc(tr("更新して再起動"))}</button>
     </div>`,
    (root, close) => {
      root.querySelector('[data-x="later"]')!.addEventListener("click", close);
      const install = root.querySelector<HTMLButtonElement>('[data-x="install"]')!;
      install.addEventListener("click", () => installUpdate(u, root));
      install.focus();
    },
    "update-modal",
    false,
    false,
  ).then(() => (updateShown = false));
}

async function installUpdate(u: Update, card: HTMLElement) {
  if (state.transferring) {
    toast(tr("転送中は更新できません。転送が終わってから更新してください"), "error");
    return;
  }
  const btns = card.querySelectorAll<HTMLButtonElement>(".modal-actions button");
  btns.forEach((b) => (b.disabled = true));
  const prog = card.querySelector<HTMLElement>(".uc-progress")!;
  const fill = card.querySelector<HTMLElement>(".uc-fill")!;
  const pct = card.querySelector<HTMLElement>(".uc-pct")!;
  prog.classList.remove("hidden");
  let total = 0;
  let got = 0;
  try {
    await u.downloadAndInstall((ev) => {
      if (ev.event === "Started") total = ev.data.contentLength ?? 0;
      else if (ev.event === "Progress") {
        got += ev.data.chunkLength;
        const p = total ? Math.min(100, (got / total) * 100) : 0;
        fill.style.width = `${p.toFixed(0)}%`;
        pct.textContent = total ? `${p.toFixed(0)}%` : fmtBytes(got);
      } else if (ev.event === "Finished") {
        pct.textContent = tr("インストール中…");
      }
    });
    await relaunch();
  } catch (e) {
    toast(tr("更新に失敗しました: {a}", { a: String(e) }), "error", 8000);
    btns.forEach((b) => (b.disabled = false));
    prog.classList.add("hidden");
  }
}
