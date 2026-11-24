// はじめの案内

import { t as tr } from "../i18n";
import { invoke, listen } from "../infra/tauri";
import { analyticsOnboarding } from "../app/analytics";
import { state } from "../app/state";
import { fmtBytes } from "../domain/format";
import { LAYOUT_DEFAULT, layoutToDisplay } from "../domain/rockbox-layout";
import type { Snapshot } from "../domain/types";
import { getStored, setStored } from "../infra/prefs";
import { pollDevices } from "./device";
import { esc } from "./dom";
import { call, toast } from "./feedback";
import { ICON_CLOSE, ICON_FOLDER } from "./icons";
import { confirmBox, modal } from "./modal";
import { folderName, pickPcFolder, removePcFolder } from "./pc";
import { ICON_CONVERT, ICON_GEAR, ICON_IPOD, ICON_SEND, rebuildRockboxDb } from "./settings";
import type { FfmpegSettings } from "./settings";
import { renderSidebar } from "./sidebar";

// ---------------- はじめの案内 ----------------

/** はじめの案内の版。上げると、見終わった人にも次の起動でもう一度出す (前の版から更新した人にも見てもらうとき) */
export const ONB_VERSION = "1.2";
type OnbStep = "welcome" | "ffmpeg" | "pc" | "rockbox" | "done";
/** ffmpeg のページは、ffmpeg が見つからず自動で入れられるときだけ出す */
const ONB_STEPS: OnbStep[] = ["welcome", "ffmpeg", "pc", "rockbox", "done"];

const ICON_BULB =
  '<svg viewBox="0 0 24 24"><path d="M9 21h6v-2H9v2Zm3-19a7 7 0 0 0-4 12.74V17a1 1 0 0 0 1 1h6a1 1 0 0 0 1-1v-2.26A7 7 0 0 0 12 2Zm2.85 11.1-.85.6V16h-4v-2.3l-.85-.6a5 5 0 1 1 5.7 0Z"/></svg>';

/** 形の見本 (iPod が無くても出せるよう、架空の曲で) */
export function layoutExample(template: string): string {
  // tr: 訳さない (見本の曲の情報)
  const sample: Record<string, string> = { albumartist: "Future", artist: "Future", album: "I NEVER LIKED YOU", year: "2022", disctrack: "01", track: "01", disc: "1", title: "712PM", genre: "Hip-Hop" };
  return `${template.replace(/\{(\w+)\}/g, (_, k: string) => sample[k] ?? "")}.flac`;
}

/**
 * はじめの案内。初めて起動したときに出す (前の版から使っている人には出さない)。設定の「一般」からもう一度見られる。
 * 言語はシステムに合わせる (設定の「一般」で変えられる)
 */
export async function showOnboarding(start: OnbStep = "welcome") {
  const ff = await call<FfmpegSettings>("get_settings");
  const needFf = !!ff && !ff.ffmpeg && ff.canInstallFfmpeg;
  const steps = ONB_STEPS.filter((s) => s !== "ffmpeg" || needFf);
  let step = Math.max(0, steps.indexOf(start));
  // Rockbox のページの答え (案内の中だけで使う)
  let rbAnswer: "yes" | "no" | "unknown" | null = null;
  // ffmpeg を入れている途中・入れた・失敗した。入れている間に次のページへ進んでも、案内を閉じても続ける
  let ffState: "idle" | "installing" | "done" | "error" = "idle";
  let ffError = "";
  let ffProgress = "";
  let ffPct = 0;
  let rerender: () => void = () => {};
  // ページの下に添えるコツ (電球のアイコン付きの枠)
  const tip = (text: string) => `<p class="onb-tip"><span class="onb-tip-icon" aria-hidden="true">${ICON_BULB}</span><span>${esc(text)}</span></p>`;
  // タイトルとスキップを 1 つの行に (最初と最後のページはスキップを出さない)
  const head = (title: string, cls = "") =>
    `<div class="onb-head ${cls}"><h2>${esc(title)}</h2>${
      step > 0 && step < steps.length - 1 ? `<button class="btn ghost" data-x="onb-skip">${esc(tr("スキップ"))}</button>` : ""
    }</div>`;
  // ようこそと ffmpeg のページの、横に並べる 3 つの枠の 1 つ
  const box = (icon: string, title: string, sub: string) =>
    `<li><span class="onb-feat-icon" aria-hidden="true">${icon}</span><b>${esc(title)}</b><span>${esc(sub)}</span></li>`;
  const page = (st: OnbStep): string => {
    switch (st) {
      case "welcome":
        return `${head(tr("ZenPod へようこそ"))}
          <p>${esc(tr("iPod に音楽を入れて、整理するアプリです。"))}</p>
          <div class="onb-body">
            <ul class="onb-boxes">
              ${box(ICON_SEND, tr("ドラッグで転送"), tr("FLAC なども、iPod で聞ける形に変換します"))}
              ${box(ICON_IPOD, tr("iPod OS と Rockbox の両方に対応"), tr("どちらの iPod も、同じ操作で管理できます"))}
              ${box(ICON_FOLDER, tr("音楽ファイルの整理にも"), tr("PC の曲の情報やファイル名を、まとめて直せます"))}
            </ul>
          </div>`;
      case "ffmpeg": {
        // 1 ページの中で、説明 → 進み具合 → 完了 と切り替える (ページの点は増やさない)
        if (ffState === "installing")
          return `${head(tr("ffmpeg を入れましょう"))}
            <div class="onb-ff-center">
              <div class="onb-ff-label">${esc(tr("ffmpeg をダウンロード中…"))}</div>
              <div class="transfer-bar onb-ff-bar"><div id="onb-ff-fill" style="width: ${ffPct}%"></div></div>
              <div class="onb-ff-size muted" id="onb-ff-pct">${esc(ffProgress)}</div>
              <button class="btn mini" data-x="onb-ff-cancel">${esc(tr("中止"))}</button>
            </div>`;
        if (ffState === "done")
          return `${head(tr("ffmpeg を入れましょう"))}
            <div class="onb-ff-center">
              <div class="onb-done-mark" aria-hidden="true"><svg viewBox="0 0 64 64"><circle class="c" cx="32" cy="32" r="28"/><path class="k" d="M20 33.5 28.5 42 45 24"/></svg></div>
              <div class="onb-ff-label">${esc(tr("完了しました"))}</div>
              <div class="muted">${esc(ff?.nativeFlac ? tr("Ogg / Opus / WMA なども転送できるようになりました。") : tr("FLAC なども転送できるようになりました。"))}</div>
            </div>`;
        const mac = navigator.userAgent.includes("Mac");
        // 分かりにくい人も迷わないよう、入れておくのをすすめる (強制はしない。「あとで」で先へ進める)
        return `${head(tr("ffmpeg を入れましょう"))}
          <p>${esc(tr("ZenPod で曲を転送するには、無料のソフト ffmpeg を入れておくのがおすすめです。"))}</p>
          <div class="onb-body">
          <ul class="onb-boxes">
            ${box(ICON_CONVERT, tr("自動で変換"), ff?.nativeFlac ? tr("Ogg / Opus / WMA なども、iPod で聞ける形にして送ります") : tr("FLAC / Ogg なども、iPod で聞ける形にして送ります"))}
            ${box(ICON_SEND, tr("ボタン 1 つで入る"), mac ? tr("約 30MB をダウンロード。管理者権限は不要です") : tr("約 80MB をダウンロード。管理者権限は不要です"))}
            ${box(ICON_GEAR, tr("あとから消せる"), tr("設定の「変換 (ffmpeg)」で、いつでも削除できます"))}
          </ul>
          <div class="onb-ff-action">
            ${ffState === "error" ? `<div class="onb-ff-error">${esc(ffError)}</div>` : ""}
            <button class="btn primary" data-x="onb-ff-install">${esc(ffState === "error" ? tr("もう一度試す") : tr("ffmpeg を入れる"))}</button>
          </div>
          </div>`;
      }
      case "pc":
        return `${head(tr("PC の音楽フォルダ"))}
          <p>${esc(tr("登録すると、サイドバーから曲を選んで iPod に送れます。登録しなくても、ドラッグで転送できます。"))}</p>
          <div class="onb-folders">
            <div class="onb-folder-box">
              <ul class="onb-folder-list">${state.pc.folders
                .map(
                  (f) =>
                    `<li><span class="onb-folder-icon">${ICON_FOLDER}</span><span class="onb-folder-text"><span>${esc(folderName(f))}</span><span class="muted">${esc(f)}</span></span><button class="btn icon ghost" data-onb-remove="${esc(f)}" title="${esc(tr("外す"))}" aria-label="${esc(tr("外す"))}">${ICON_CLOSE}</button></li>`,
                )
                .join("")}</ul>
              ${state.pc.folders.length ? "" : `<p class="onb-folder-empty muted">${esc(tr("まだ登録していません"))}</p>`}
              <div class="onb-folder-fade" aria-hidden="true"></div>
            </div>
            <button type="button" class="onb-folder-add" data-x="onb-add">＋ ${esc(tr("フォルダを追加…"))}</button>
          </div>
          ${tip(tr("あとからサイドバーの「PC」の「＋」でも追加できます。"))}`;
      case "rockbox": {
        // まず Rockbox を入れているかを聞く。入れている人には、iPod をつないだときに今の並びに合う形を提案する (applyRockboxLayout)
        const opt = (v: "yes" | "no" | "unknown", label: string, sub: string) =>
          `<label class="onb-layout"><input type="radio" name="onb-rb" value="${v}" ${rbAnswer === v ? "checked" : ""}><span><span>${esc(label)}</span><span class="muted">${esc(sub)}</span></span></label>`;
        return `${head("Rockbox")}
          <p>${esc(tr("iPod に Rockbox を入れていますか？"))}</p>
          <div class="onb-layouts" role="radiogroup">
            ${opt("yes", tr("入れている"), tr("つないだときに、今ある曲の並びに合うフォルダの形を提案します"))}
            ${opt("no", tr("入れていない"), tr("このままで使えます"))}
            ${opt("unknown", tr("Rockbox が分からない"), tr("Apple の代わりに iPod に入れて使う、無料のファームウェアです。入れていなければ、このままで使えます"))}
          </div>
          ${
            rbAnswer === "yes"
              ? tip(tr("曲がまだ少ない iPod には「{a}」の形で置きます。あとから設定の「転送」で変えられます。", { a: layoutToDisplay(getStored("rbLayoutDefault") ?? LAYOUT_DEFAULT) }))
              : ""
          }`;
      }
      case "done":
        return `<div class="onb-done-mark" aria-hidden="true"><svg viewBox="0 0 64 64"><circle class="c" cx="32" cy="32" r="28"/><path class="k" d="M20 33.5 28.5 42 45 24"/></svg></div>
          ${head(tr("準備ができました"), "onb-done-title")}
          <p class="onb-done-lead">${esc(tr("iPod をつなぐと、自動で読み込みます。"))}</p>
          <ul class="onb-tips">
            <li>${esc(tr("曲を右クリックすると、情報の編集やプレイリストへの追加ができます"))}</li>
            ${rbAnswer === "yes" ? `<li>${esc(tr("左上の「iPod OS | Rockbox」で、管理のしかたを切り替えられます"))}</li>` : ""}
            <li>${esc(navigator.userAgent.includes("Mac") ? tr("表示の大きさは ⌘ + / ⌘ − で変えられます") : tr("表示の大きさは Ctrl + / Ctrl − で変えられます"))}</li>
            <li>${esc(tr("設定はサイドバーの一番下の「アプリの設定」から開けます"))}</li>
          </ul>`;
    }
  };
  // 利用状況: ffmpeg のページを見たか・入れずに先へ進んだか・最後まで進んだか
  let ffSeen = false;
  let ffPassed = false;
  // 閉じ方 (最後まで / 右上のスキップ)。どちらでもなければ Esc で閉じた
  let endedBy: "done" | "skip" | null = null;
  let reported = false;
  const finish = () => {
    setStored("onboarded", ONB_VERSION);
    if (reported) return;
    reported = true;
    const ffmpeg = !needFf
      ? "present"
      : ffState === "done"
        ? "installed"
        : ffState === "error"
          ? "failed"
          : ffPassed
            ? "later"
            : ffSeen
              ? "skipped"
              : "not_reached";
    analyticsOnboarding({ ffmpeg, rockbox: rbAnswer ?? "none", end: endedBy ?? "esc", at: steps[step] });
  };
  const installFf = async () => {
    ffState = "installing";
    ffProgress = "";
    ffPct = 0;
    rerender();
    const unlisten = await listen<{ received: number; total: number }>("ffmpeg-progress", (ev) => {
      const { received, total } = ev.payload;
      ffPct = total ? Math.floor((received / total) * 100) : 0;
      ffProgress = `${fmtBytes(received)} / ${fmtBytes(total)}`;
      const fill = document.getElementById("onb-ff-fill");
      const label = document.getElementById("onb-ff-pct");
      if (fill) fill.style.width = `${ffPct}%`;
      if (label) label.textContent = ffProgress;
    });
    const r = await invoke<FfmpegSettings>("install_ffmpeg").catch((e) => {
      // 中止したときは Rust が「中止しました」(の訳) を返す → 最初の状態に戻す
      ffError = String(e) === tr("中止しました") ? "" : String(e);
      return null;
    });
    unlisten();
    ffState = r?.ffmpeg ? "done" : ffError ? "error" : "idle";
    // 案内を閉じたあとに終わったときは、結果をトーストで知らせる
    if (!state.onboarding) {
      if (ffState === "done") toast(tr("ffmpeg を入れました"));
      else if (ffState === "error") toast(ffError, "error", 8000);
    }
    rerender();
  };
  state.onboarding = true;
  await modal(
    "",
    (root, close) => {
      // スキップはタイトルの行の右端、戻るは左下、次へは右下
      const html = () => {
        const st = steps[step];
        const last = step === steps.length - 1;
        return `<div class="onb-page onb-${st}">${page(st)}</div>
          <div class="onb-foot">
            ${step > 0 ? `<button class="btn" data-x="onb-back">${esc(tr("戻る"))}</button>` : "<span></span>"}
            <div class="onb-dots">${steps.map((_, i) => `<span class="${i === step ? "on" : ""}"></span>`).join("")}</div>
            <div class="onb-nav">${
              // ffmpeg を入れていない間は、次へを「あとで」にして控えめにする (入れるボタンを目立たせる)
              st === "ffmpeg" && (ffState === "idle" || ffState === "error")
                ? `<button class="btn" data-x="onb-next">${esc(tr("あとで"))}</button>`
                : `<button class="btn primary" data-x="onb-next" ${st === "ffmpeg" && ffState === "installing" ? "disabled" : ""}>${esc(last ? tr("はじめる") : tr("次へ"))}</button>`
            }</div>
          </div>`;
      };
      // モーダルの高さを、中身が一番多いページに合わせて固定する (ページを移っても大きさが変わらない。
      // PC の音楽フォルダの一覧はその高さの残りいっぱいに伸びる)。全ページを描いて測り、描き直す前に戻すので画面には出ない
      const fit = () => {
        const [keepStep, keepRb] = [step, rbAnswer];
        root.style.height = "";
        root.classList.add("onb-measure");
        let max = 0;
        rbAnswer = "yes"; // コツが出る分も含めて測る
        for (step = 0; step < steps.length; step++) {
          root.innerHTML = html();
          max = Math.max(max, root.scrollHeight);
        }
        [step, rbAnswer] = [keepStep, keepRb];
        root.classList.remove("onb-measure");
        root.style.height = `${max}px`;
      };
      const render = () => {
        // フォルダを外したときに、一覧のスクロールの位置を保つ
        const keep = root.querySelector<HTMLElement>(".onb-folder-list")?.scrollTop ?? 0;
        const last = step === steps.length - 1;
        root.innerHTML = html();
        if (steps[step] === "ffmpeg") ffSeen = true;
        root.querySelector('[data-x="onb-next"]')!.addEventListener("click", () => {
          if (steps[step] === "ffmpeg") ffPassed = true;
          if (last) {
            endedBy = "done";
            finish();
            close();
            return;
          }
          step++;
          render();
        });
        root.querySelector('[data-x="onb-back"]')?.addEventListener("click", () => {
          step--;
          render();
        });
        root.querySelector('[data-x="onb-skip"]')?.addEventListener("click", () => {
          endedBy = "skip";
          finish();
          close();
        });
        root.querySelector('[data-x="onb-ff-install"]')?.addEventListener("click", () => void installFf());
        root.querySelector('[data-x="onb-ff-cancel"]')?.addEventListener("click", () => invoke("cancel_ffmpeg_install"));
        root.querySelector('[data-x="onb-add"]')?.addEventListener("click", async () => {
          if (!(await pickPcFolder())) return;
          renderSidebar();
          render();
          // 足したフォルダが見えるよう、一番下まで送る
          const list = root.querySelector<HTMLElement>(".onb-folder-list");
          if (list) {
            list.scrollTop = list.scrollHeight;
            fadeFolders();
          }
        });
        // フォルダの一覧は高さを決めてスクロールさせる。下にまだあるときは下端をぼかす
        const fadeFolders = () => {
          const list = root.querySelector<HTMLElement>(".onb-folder-list");
          if (!list) return;
          const more = list.scrollHeight - list.scrollTop - list.clientHeight > 1;
          root.querySelector(".onb-folder-fade")?.classList.toggle("on", more);
        };
        const folderList = root.querySelector<HTMLElement>(".onb-folder-list");
        if (folderList) folderList.scrollTop = keep;
        folderList?.addEventListener("scroll", fadeFolders, { passive: true });
        fadeFolders();
        root.querySelectorAll<HTMLElement>("[data-onb-remove]").forEach((b) =>
          b.addEventListener("click", async () => {
            await removePcFolder(b.dataset.onbRemove!);
            render();
          }),
        );
        root.querySelectorAll<HTMLInputElement>('input[name="onb-rb"]').forEach((r) =>
          r.addEventListener("change", () => {
            rbAnswer = r.value as "yes" | "no" | "unknown";
            render();
            root.querySelector<HTMLInputElement>(`input[name="onb-rb"][value="${rbAnswer}"]`)?.focus();
          }),
        );
        root.querySelector<HTMLButtonElement>(steps[step] === "ffmpeg" && ffState !== "done" && ffState !== "installing" ? '[data-x="onb-ff-install"]' : '[data-x="onb-next"]')?.focus();
      };
      fit();
      render();
      // ffmpeg を入れ終わったら、そのページを開いているときだけ描き直す
      rerender = () => root.isConnected && steps[step] === "ffmpeg" && render();
      // ウィンドウの大きさで折り返しが変わるので測り直す
      const onResize = () => {
        if (!root.isConnected) return window.removeEventListener("resize", onResize);
        fit();
        render();
      };
      window.addEventListener("resize", onResize);
    },
    "onb-modal",
    // 外側を押しても Esc でも閉じない (右上のスキップか最後のボタンで閉じる)
    false,
    false,
  );
  finish();
  state.onboarding = false;
  // 案内を出している間につながった iPod を、閉じたらすぐ読み込む
  void pollDevices();
}

/** データベースがまだ無い Rockbox の iPod を開いたら、アプリで作るか聞く (iPod ごとに 1 回) */
const dbAsked = new Set<string>();
export async function offerRockboxDb(s: Snapshot) {
  const rb = s.rockbox;
  if (!rb?.dbSupported || rb.dbTracks !== null || s.writableError || !s.tracks.length || dbAsked.has(s.device.root)) return;
  dbAsked.add(s.device.root);
  const ok = await confirmBox(
    tr("Rockbox のデータベースを作りますか？"),
    tr("この iPod にはまだ Rockbox のデータベースがありません。データベースがあると、iPod の「データベース」でアーティスト・アルバムごとに曲を探せます。iPod 本体で作ると曲が多いときは時間がかかりますが、アプリで作ることもできます。"),
    tr("作る"),
  );
  if (ok) await rebuildRockboxDb();
}
