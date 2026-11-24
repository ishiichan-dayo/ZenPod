// アプリの設定 (一般・転送・変換・読み・iPod・このアプリについて)

import { t as tr } from "../i18n";
import { invoke, listen } from "../infra/tauri";
import { analyticsFfmpegInstall } from "../app/analytics";
import { isRockbox, writable } from "../app/selectors";
import { $, appVersion, state } from "../app/state";
import type { SettingsCat } from "../app/state";
import { fmtBytes } from "../domain/format";
import type { DeviceInfo, Snapshot } from "../domain/types";
import { getStored, pref, prefStore, setPref, setStored, setStoredLocal } from "../infra/prefs";
import { applyTheme, changeZoom, renderZoomSetting, themePref } from "./appearance";
import type { Theme } from "./appearance";
import { DEV_ACTIONS, devCanRun, devSettingsHtml } from "./dev";
import { generateReadings } from "./dialogs/readings";
import { unifyArtists } from "./dialogs/unify-artists";
import { esc } from "./dom";
import { busyToast, call, failToast, toast } from "./feedback";
import { closeMenu } from "./menu";
import type { MenuItem } from "./menu";
import { busySave, confirmBox } from "./modal";
import { applySnapshot, renderAll, switchLangLive } from "./render";
import { artSizeHtml, bindArtSize } from "./art-size";
import { renderLayoutCard } from "./rockbox-layout";
import { renderRows } from "./rows";
import { CONVERT_LOSSLESS, CONVERT_LOSSY, convertOptions, duplicatePolicy } from "./transfer";
import type { DuplicatePolicy } from "./transfer";
import { checkForUpdate } from "./update";

export interface FfmpegSettings {
  ffmpeg: string | null;
  /** 設定で選んだ ffmpeg の場所 (null = 自動で探す) */
  ffmpegChosen: string | null;
  ffmpegInstalledByApp: boolean;
  canInstallFfmpeg: boolean;
  /** ffmpeg が無くても OS の機能 (macOS の afconvert) で FLAC を変換できる */
  nativeFlac: boolean;
}

/** 虫 (Lucide の bug アイコン、ISC ライセンス)。設定の「デバッグ」(開発版だけ) */
const ICON_BUG =
  '<svg class="icon-line" viewBox="0 0 24 24" aria-hidden="true"><path d="m8 2 1.88 1.88"/><path d="M14.12 3.88 16 2"/><path d="M9 7.13v-1a3.003 3.003 0 1 1 6 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6"/><path d="M12 20v-9"/><path d="M6.53 9C4.6 8.8 3 7.1 3 5"/><path d="M6 13H2"/><path d="M3 21c0-2.1 1.7-3.9 3.8-4"/><path d="M20.97 5c0 2.1-1.6 3.8-3.5 4"/><path d="M22 13h-4"/><path d="M17.2 17c2.1.1 3.8 1.9 3.8 4"/></svg>';

/** 歯車 (Lucide の settings アイコン、ISC ライセンス)。線で描くので小さくても形が分かる */
export const ICON_GEAR =
  '<svg class="icon-line" viewBox="0 0 24 24" aria-hidden="true"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>';
export const ICON_SEND = '<svg viewBox="0 0 24 24"><path d="M5 20h14v-2H5v2Zm7-16-6 6h4v6h4v-6h4l-6-6Z" transform="rotate(180 12 12)"/></svg>';
export const ICON_CONVERT = '<svg viewBox="0 0 24 24"><path d="M7 7h11l-3-3 1.4-1.4L21.8 8l-5.4 5.4L15 12l3-3H7V7Zm10 10H6l3 3-1.4 1.4L2.2 16l5.4-5.4L9 12l-3 3h11v2Z"/></svg>';
const ICON_YOMI = '<svg viewBox="0 0 24 24"><path d="M4 5h16v2H4V5Zm0 4h10v2H4V9Zm0 4h16v2H4v-2Zm0 4h10v2H4v-2Z"/></svg>';
export const ICON_IPOD = '<svg viewBox="0 0 24 24"><path d="M7 2h10a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2Zm1 2v6h8V4H8Zm4 8a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Zm0 2.25a1.25 1.25 0 1 1 0 2.5 1.25 1.25 0 0 1 0-2.5Z"/></svg>';

const ICON_INFO = '<svg viewBox="0 0 24 24"><path d="M11 7h2v2h-2V7Zm0 4h2v6h-2v-6Zm1-9a10 10 0 1 0 0 20 10 10 0 0 0 0-20Zm0 18a8 8 0 1 1 0-16 8 8 0 0 1 0 16Z"/></svg>';

const RELEASES_URL = "https://github.com/ishiichan-dayo/ZenPod";
const KOFI_URL = "https://ko-fi.com/ishiichan_dayo";

const SETTINGS_CATS: { id: SettingsCat; label: string; icon: string }[] = [
  { id: "general", label: "一般", icon: ICON_GEAR },
  { id: "transfer", label: "転送", icon: ICON_SEND },
  { id: "convert", label: "変換 (ffmpeg)", icon: ICON_CONVERT },
  { id: "yomi", label: "読み", icon: ICON_YOMI },
  { id: "ipod", label: "iPod", icon: ICON_IPOD },
  { id: "about", label: "このアプリについて", icon: ICON_INFO },
  // 開発版だけ: 確認用の操作のボタン (devSettingsHtml)
  ...(import.meta.env.DEV ? [{ id: "debug" as SettingsCat, label: "デバッグ", icon: ICON_BUG }] : []),
];

/** 設定の「匿名の利用状況を送る」の説明: 送るもの (README の「安心して使うために」/ README.en.md の Safety と合わせる) */
// tr: 使う側で訳す
const ANALYTICS_SENT = [
  "アプリのバージョン・言語・OS",
  "開いた iPod のモード (iPod OS / Rockbox)・機種・曲数のおおまかな範囲",
  "開くのにかかった時間とその内訳 (Rockbox のデータベースを使えた割合と、Mac では Spotlight が iPod を索引しているかを含む)",
  "はじめの案内で選んだこと (ffmpeg を入れたか・Rockbox を使っているか)",
  "iPod をつないでもドライブとして表示されなかったこと (その理由: 取り出し済みか、Mac ではディスクの検査中か・読む許可が無いか)",
  "iPod を開かないまま時間がたったこと (USB やドライブとして iPod が見えていたか)",
  "転送の結果 (成功したか・曲数とエラーの数のおおまかな範囲)",
  "ネットで探す機能 (アートワーク・曲情報・アーティストの別名) をどこまで使ったか (検索したか・反映したか・候補の数とかかった時間のおおまかな範囲・どのサービスの候補を選んだか)",
  "設定から ffmpeg を入れた結果",
];
/** 盾 (Lucide の shield-check、ISC ライセンス) と ✕ */
const ICON_SHIELD = '<svg class="icon-line" viewBox="0 0 24 24" aria-hidden="true"><path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/></svg>';
const ICON_NO = '<svg class="icon-line" viewBox="0 0 24 24" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg>';
/** 送らないもの */
// tr: 使う側で訳す
const ANALYTICS_NOT_SENT = [
  "名前・メールアドレス・PC やユーザーの名前",
  "ファイルの場所・曲名",
  "iPod の名前やシリアル番号",
  "通信で届く IP アドレス (保存しません)",
];

/** 設定の左の一覧と、選んだ項目の内容だけを表示 */

export function renderSettingsNav() {
  const item = (c: (typeof SETTINGS_CATS)[number]) =>
    `<li class="side-item ${state.settingsCat === c.id ? "active" : ""}" data-cat="${c.id}">${c.icon}<span class="label">${esc(tr(c.label))}</span></li>`;
  // 「デバッグ」(開発版だけ) はサイドメニューの下端に置く
  $("set-nav").innerHTML = SETTINGS_CATS.filter((c) => c.id !== "debug").map(item).join("");
  const foot = $("set-nav-foot");
  const bottom = SETTINGS_CATS.filter((c) => c.id === "debug");
  foot.classList.toggle("hidden", !bottom.length);
  foot.querySelector("ul")!.innerHTML = bottom.map(item).join("");
  document.querySelectorAll<HTMLElement>("#settings-body .set-section").forEach((el) => {
    el.classList.toggle("hidden", el.dataset.cat !== state.settingsCat);
  });
}

export async function openSettings() {
  closeMenu();
  state.settingsOpen = true;
  const settings = await call<FfmpegSettings>("get_settings");
  const check = (key: Parameters<typeof pref>[0], label: string, hint = "") =>
    `<label class="check switch-row"><span class="switch-text">${label}${
      hint ? `<span class="check-hint">${hint}</span>` : ""
    }</span><input type="checkbox" role="switch" class="switch" data-pref="${key}" ${pref(key) ? "checked" : ""}></label>`;
  const theme = themePref();
  $("settings-body").innerHTML = `
    <section class="set-section" data-cat="general">
    <h2 class="set-head">${esc(tr("一般"))}</h2>
    <div class="set-card">
      <div class="set-title">${esc(tr("言語 / Language"))}</div>
      <div class="seg" role="radiogroup" aria-label="${esc(tr("言語 / Language"))}">
        ${(
          [
            ["system", tr("システムに合わせる")],
            ["ja", tr("日本語")],
            ["en", "English"],
          ] as [string, string][]
        )
          .map(
            ([v, label]) =>
              `<label class="seg-item"><input type="radio" name="set-lang" value="${v}" ${v === (getStored("lang") ?? "system") ? "checked" : ""}><span>${esc(label)}</span></label>`,
          )
          .join("")}
      </div>
    </div>
    <div class="set-card">
      <div class="set-title">${esc(tr("外観"))}</div>
      <div class="seg" role="radiogroup" aria-label="${esc(tr("外観"))}">
        ${(
          [
            ["system", tr("システムに合わせる")],
            ["light", tr("ライト")],
            ["dark", tr("ダーク")],
          ] as [Theme, string][]
        )
          .map(
            ([v, label]) =>
              `<label class="seg-item"><input type="radio" name="set-theme" value="${v}" ${v === theme ? "checked" : ""}><span>${label}</span></label>`,
          )
          .join("")}
      </div>
    </div>
    <div class="set-card">
      <div class="set-title">${esc(tr("表示の大きさ"))}</div>
      <div class="field-row set-item set-zoom">
        <button class="btn icon" id="set-zoom-out" title="${esc(tr("縮小"))}" aria-label="${esc(tr("縮小"))}">−</button>
        <span id="set-zoom-value" class="set-zoom-value" aria-live="polite"></span>
        <button class="btn icon" id="set-zoom-in" title="${esc(tr("拡大"))}" aria-label="${esc(tr("拡大"))}">＋</button>
        <span class="spacer"></span>
        <button class="btn" id="set-zoom-reset">${esc(tr("100% に戻す"))}</button>
      </div>
      <div class="check-hint">${esc(navigator.userAgent.includes("Mac") ? tr("⌘ + / ⌘ − / ⌘ 0 でも変えられます") : tr("Ctrl + / Ctrl − / Ctrl 0 でも変えられます"))}</div>
    </div>
    <div class="set-card">
      <div class="set-title">${esc(tr("アップデート"))}</div>
      <div class="field-row set-item set-update-row">
        <span class="muted">${esc(tr("バージョン {a}", { a: appVersion }))}</span>
        <span class="spacer"></span>
        <button class="btn" id="set-update-check">${esc(tr("アップデートを確認"))}</button>
      </div>
      ${check("autoUpdate", tr("起動時にアップデートを確認する"))}
      <div class="set-tidy-row">
        <div><div>${esc(tr("先行版"))}</div><div class="check-hint">${esc(tr("報告を受けて直した不具合などを、次のアップデートより先に試せる版です"))}</div></div>
        <button class="btn" id="set-update-early">${esc(tr("確認"))}</button>
      </div>
    </div>
    </section>
    <section class="set-section" data-cat="transfer">
    <h2 class="set-head">${esc(tr("転送"))}</h2>
    <div class="set-card">
      <div class="set-dup">
        <label for="set-dup">${esc(tr("iPod に同じ曲があるとき"))}<span class="check-hint">${esc(tr("タイトル・アーティスト・アルバム・トラック番号が同じで、長さの差が 2 秒未満の曲"))}</span></label>
        <select id="set-dup" data-store="duplicates">
          ${(
            [
              ["skip", tr("スキップする")],
              ["replace", tr("上書きする (再生回数・評価・プレイリストはそのまま)")],
              ["keep", tr("両方残す")],
            ] as [DuplicatePolicy, string][]
          )
            .map(([v, label]) => `<option value="${v}" ${v === duplicatePolicy() ? "selected" : ""}>${label}</option>`)
            .join("")}
        </select>
      </div>
      ${check("artwork", tr("ジャケット画像も転送する"), tr("曲に埋め込まれた画像か、同じフォルダの cover.jpg など"))}
      ${check("shrinkArtwork", tr("埋め込み画像を縮小する"), tr("iPod に入れるコピーだけを縮めた JPEG にします (PC のファイルは変わりません)。Rockbox ではアートワークの表示が速くなります"))}
      <div class="set-dup set-art-size ${pref("shrinkArtwork") ? "" : "hidden"}">
        <label for="set-art-size">${esc(tr("縮小する大きさ (長い辺)"))}</label>
        ${artSizeHtml("set-art-size")}
      </div>
      ${check("autoReading", tr("日本語の曲名・アーティスト名に読みを付ける"), tr("日本語の曲名が iPod で正しい順に並びます (日本語以外の曲名は変わりません)"))}
    </div>
    <div class="set-card">
      <div class="set-title">${esc(tr("ポッドキャスト"))}</div>
      ${check("podcastSyncOnConnect", tr("iPod をつないだら、ポッドキャストの同期を確かめる"), tr("送る新しい回か、消す聞き終わった回があるときだけ、確認の一覧を出します"))}
    </div>
    <div class="set-card" id="set-layout"></div>
    </section>
    <section class="set-section" data-cat="convert">
    <h2 class="set-head">${esc(tr("変換 (ffmpeg)"))}</h2>
    <div class="set-card">
      <div class="set-title">${esc(tr("変換先の形式"))}</div>
      <div class="set-convert">
        <label for="set-conv-lossless">${esc(tr("可逆圧縮の曲 (FLAC / APE など)"))}</label>
        <select id="set-conv-lossless" data-store="convertLossless">
          ${convertOptions(CONVERT_LOSSLESS, getStored("convertLossless") ?? "alac")}
        </select>
        <label for="set-conv-lossy">${esc(tr("非可逆の曲 (Ogg / Opus / WMA など)"))}</label>
        <select id="set-conv-lossy" data-store="convertLossy">
          ${convertOptions(CONVERT_LOSSY, getStored("convertLossy") ?? "aac:256")}
        </select>
        <label for="set-conv-video">${esc(tr("動画 (iPod OS)"))}</label>
        <select id="set-conv-video" data-store="videoSize">
          ${(
            [
              ["640", tr("640×480 (標準。テレビにつないでもきれい)")],
              ["320", tr("320×240 (容量が小さく、変換が速い)")],
            ] as [string, string][]
          )
            .map(([v, label]) => `<option value="${v}" ${v === (getStored("videoSize") ?? "640") ? "selected" : ""}>${esc(label)}</option>`)
            .join("")}
        </select>
      </div>
      <div class="hint set-convert-hint">
        <p>${esc(tr("MP3 / AAC / ALAC / WAV / AIFF の曲は変換せずにそのまま転送します。容量は 4 分の曲 1 曲あたりの目安です。"))}</p>
        <p>${esc(tr("動画は、iPod でそのまま再生できる形 (H.264 / MPEG-4・640×480 まで) ならそのまま送り、それ以外は大きさに収めて変換します。Rockbox には 320×240 の MPEG-2 にして送ります。"))}</p>
      </div>
      ${check("confirmConvert", tr("変換して転送する前に、変換後の形式を確認する"))}
    </div>
    <div class="set-card">
      <div class="set-title">ffmpeg</div>
      <!-- 上: 今の状態 (枠の中)。下: 場所の設定 -->
      <div id="set-ffmpeg-status" class="set-ff-status"></div>
      <!-- 場所は「参照」で開くダイアログ (Rust が開き、ffmpeg か確かめる) でだけ選ぶ。手で入力はできない -->
      <label class="set-ff-path-label" for="set-ffmpeg">${esc(tr("ffmpeg の場所"))}</label>
      <div class="field-row">
        <input type="text" id="set-ffmpeg" readonly placeholder="${esc(tr("自動で探します"))}" value="${esc(settings?.ffmpegChosen ?? "")}">
        <button class="btn" id="set-ffmpeg-browse">${esc(tr("参照"))}</button>
        <button class="btn${settings?.ffmpegChosen ? "" : " hidden"}" id="set-ffmpeg-reset">${esc(tr("自動に戻す"))}</button>
      </div>
      <div class="hint">${esc(tr("変換に使う ffmpeg を「参照」で選べます。選ばなければ自動で探します。"))}</div>
    </div>
    </section>
    <section class="set-section" data-cat="yomi">
    <h2 class="set-head">${esc(tr("読み"))}</h2>
    <div class="set-card">
      ${check("confirmReadings", tr("「読みを自動生成」の前に変更内容を確認する"))}
    </div>
    </section>
    <section class="set-section" data-cat="ipod">
    <h2 class="set-head">iPod</h2>
    <div class="set-card" id="set-device"></div>
    <div class="set-card muted" id="set-device-none">${esc(tr("iPod を接続すると、機種・場所・FireWire GUID を確認できます。"))}</div>
    <div class="set-card" id="set-tidy">
      <div class="set-title">${esc(tr("ライブラリの整理"))}</div>
      <div class="set-tidy-row" id="set-tidy-yomi">
        <div><div>${esc(tr("全曲に読みを付ける"))}</div><div class="check-hint">${esc(tr("日本語の曲名・アーティスト名に読みを付けて、iPod で正しい順に並ぶようにします (日本語以外は変わりません)"))}</div></div>
        <button class="btn" data-tidy="yomi">${esc(tr("実行"))}</button>
      </div>
      <div class="set-tidy-row">
        <div><div>${esc(tr("アーティスト名の表記ゆれをまとめる"))}</div><div class="check-hint">${esc(tr("同じアーティストの違う表記 (「YOASOBI」と「Yoasobi」のような大文字・小文字の違いや、「米津玄師」と「Kenshi Yonezu」のような別の文字での表記) を見つけて、1 つにそろえます"))}</div></div>
        <button class="btn" data-tidy="artists">${esc(tr("実行"))}</button>
      </div>
      <div class="hint">${esc(tr("サイドバーの「ミュージック」を右クリックしても使えます。"))}</div>
    </div>
    </section>
    <section class="set-section" data-cat="about">
    <h2 class="set-head">${esc(tr("このアプリについて"))}</h2>
    <div class="set-card">
      <div class="set-title">ZenPod</div>
      <div class="muted">${esc(tr("バージョン {a}", { a: appVersion }))}</div>
      <div class="set-tidy-row">
        <div><div>${esc(tr("配布ページ"))}</div><div class="check-hint">${esc(tr("最新版のダウンロードと、使い方・対応機種の説明"))}</div></div>
        <button class="btn" data-url="${RELEASES_URL}">${esc(tr("開く"))}</button>
      </div>
      <div class="set-tidy-row">
        <div><div>${esc(tr("不具合の報告・要望"))}</div><div class="check-hint">${esc(tr("GitHub の Issues に書き込めます。iPod の機種と OS を書いていただけると助かります"))}</div></div>
        <button class="btn" data-url="${RELEASES_URL}/issues">${esc(tr("開く"))}</button>
      </div>
      <div class="set-tidy-row">
        <div><div>${esc(tr("ライセンス"))}</div><div class="check-hint">${esc(tr("ZenPod は GPL-3.0 のオープンソースです。使っているライブラリのライセンスの一覧を開きます"))}</div></div>
        <button class="btn" data-url="${RELEASES_URL}/blob/main/THIRD_PARTY_LICENSES.md">${esc(tr("開く"))}</button>
      </div>
      <div class="set-item set-analytics">${check("analytics", tr("匿名の利用状況を送る"))}
        <!-- 送らないもの (個人情報) を枠で目立たせていつも出し、送るものは開いて見る。スイッチの label の外に置く -->
        <div class="check-hint set-analytics-lead">${esc(tr("使っている人の数や動作の速さを知ってアプリの改善に役立てるため、1 日 1 回送ります。"))}</div>
        <div class="set-analytics-private">
          <div class="set-analytics-head">${ICON_SHIELD}${esc(tr("個人情報は送りません"))}</div>
          <ul class="set-analytics-no">${ANALYTICS_NOT_SENT.map((x) => `<li>${ICON_NO}<span>${esc(tr(x))}</span></li>`).join("")}</ul>
          <p class="check-hint">${esc(tr("同じ人を 1 人と数えるための ID はランダムに作ったもので、あなたや PC とは結び付きません。送り先は開発者が管理する日本のサーバーです。"))}</p>
        </div>
        <details class="set-analytics-sent">
          <summary>${esc(tr("送るもの ({n} 項目)", { n: ANALYTICS_SENT.length }))}</summary>
          <ul class="check-hint">${ANALYTICS_SENT.map((x) => `<li>${esc(tr(x))}</li>`).join("")}</ul>
        </details>
      </div>
    </div>
    <div class="set-card">
      <div class="set-title">${esc(tr("応援"))}</div>
      <div class="set-tidy-row">
        <div class="check-hint">${esc(tr("ZenPod は個人で開発している無料のアプリです。気に入っていただけたら、Ko-fi で応援していただけるとうれしいです。"))}</div>
        <button class="btn" data-url="${KOFI_URL}">${esc(tr("Ko-fi で応援する"))}</button>
      </div>
    </div>
    </section>${import.meta.env.DEV ? devSettingsHtml() : ""}`;
  renderSettingsNav();
  const root = $("settings-body");
  root.querySelectorAll<HTMLButtonElement>("[data-url]").forEach((b) =>
    b.addEventListener("click", () => call("open_url", { url: b.dataset.url })),
  );
  // 開発版だけ: デバッグのボタン。設定を閉じてから実行する (結果が設定の後ろに隠れないように)
  root.querySelectorAll<HTMLButtonElement>("[data-dev]").forEach((b) =>
    b.addEventListener("click", () => {
      const a = DEV_ACTIONS.find((x) => x.key === b.dataset.dev);
      if (!a || !devCanRun(a)) return;
      closeSettings();
      a.run();
    }),
  );
  root.querySelectorAll<HTMLButtonElement>("[data-tidy]").forEach((b) =>
    b.addEventListener("click", () => (b.dataset.tidy === "yomi" ? generateReadings(null) : unifyArtists())),
  );
  // 言語: 保存してから画面を読み直す (すべての文言を作り直すため)
  root.querySelectorAll<HTMLInputElement>('input[name="set-lang"]').forEach((r) =>
    r.addEventListener("change", async () => {
      // 読み直さずにその場で切り替え、設定の画面を今の言語で作り直す (開いていた項目とスクロールの位置はそのまま)
      const body = $("settings-body");
      const top = body.scrollTop;
      body.classList.add("set-fade");
      await new Promise((ok) => setTimeout(ok, 150));
      await switchLangLive(r.value === "system" ? null : r.value);
      await openSettings();
      body.scrollTop = top;
      void body.offsetWidth;
      body.classList.remove("set-fade");
    }),
  );
  root.querySelector("#set-zoom-out")!.addEventListener("click", () => changeZoom(-1, false));
  root.querySelector("#set-zoom-in")!.addEventListener("click", () => changeZoom(1, false));
  root.querySelector("#set-zoom-reset")!.addEventListener("click", () => changeZoom(0, false));
  renderZoomSetting();
  root.querySelectorAll<HTMLInputElement>('input[name="set-theme"]').forEach((r) =>
    r.addEventListener("change", () => {
      setStored("theme", r.value === "system" ? null : r.value);
      applyTheme();
    }),
  );
  root.querySelectorAll<HTMLInputElement>("[data-pref]").forEach((b) =>
    b.addEventListener("change", () => setPref(b.dataset.pref!, b.checked)),
  );
  // 縮小する大きさは、縮小がオンのときだけ出す
  bindArtSize(root, "set-art-size");
  root.querySelector<HTMLInputElement>('[data-pref="shrinkArtwork"]')!.addEventListener("change", (e) =>
    root.querySelector(".set-art-size")!.classList.toggle("hidden", !(e.currentTarget as HTMLInputElement).checked),
  );
  root.querySelectorAll<HTMLSelectElement>("select[data-store]").forEach((sel) =>
    sel.addEventListener("change", () => setStored(sel.dataset.store!, sel.value)),
  );

  const statusEl = root.querySelector<HTMLElement>("#set-ffmpeg-status")!;
  // Mac は配布元が違い (1 ファイルの GPL 版)、FLAC は OS の機能で変換できる
  const mac = navigator.userAgent.includes("Mac");
  const ffStatus = (st: FfmpegSettings | null) => {
    const found = st?.ffmpeg ?? null;
    if (found) {
      statusEl.innerHTML = `<div class="set-ff-box"><div class="set-ok">${esc(tr("✓ 使用中: {a}", { a: found }))}</div>${
        st?.ffmpegInstalledByApp
          ? `<div class="set-ff-foot"><span class="set-ff-note">${esc(mac ? tr("このアプリが自動で入れたものです (約 70MB)。") : tr("このアプリが自動で入れたものです (約 150MB)。"))}</span><button class="btn mini" id="set-ff-remove">${esc(tr("削除"))}</button></div>`
          : ""
      }</div>`;
      statusEl.querySelector("#set-ff-remove")?.addEventListener("click", async () => {
        const detail = st?.nativeFlac
          ? tr("Ogg / Opus / WMA などの転送ができなくなります。FLAC は Mac の機能で変換を続けます。あとで入れ直せます。")
          : tr("FLAC / Ogg などの転送と、ALAC の曲の再生ができなくなります。あとで入れ直せます。");
        const ok = await confirmBox(tr("ffmpeg を削除しますか？"), detail, tr("削除"), true);
        if (!ok) return;
        const r = await call<FfmpegSettings>("uninstall_ffmpeg");
        if (r) ffStatus(r);
      });
      return;
    }
    const missing = st?.nativeFlac
      ? tr("ffmpeg が見つかりません。Ogg / Opus / WMA などの転送に必要です (FLAC は Mac の機能で変換できます)。")
      : tr("ffmpeg が見つかりません。FLAC / Ogg などの転送と、ALAC の曲の再生に必要です。");
    const size = mac
      ? tr("約 30MB をダウンロードし、約 70MB を使います。管理者権限は不要です。")
      : tr("約 80MB をダウンロードし、約 150MB を使います。管理者権限は不要です。");
    statusEl.innerHTML = `<div class="set-warn">⚠ ${esc(missing)}${
      st?.canInstallFfmpeg
        ? `<div class="set-ff-actions"><button class="btn primary" id="set-ff-install">${esc(tr("ffmpeg を自動で入れる"))}</button><span class="set-ff-note">${esc(size)}</span></div>`
        : mac
          ? `<div class="set-ff-note">${esc(tr("Mac では Homebrew で入れられます:"))} <code>brew install ffmpeg</code></div>`
          : `<div class="set-ff-note">${esc(tr("Linux ではディストリビューションのパッケージで入れられます:"))} <code>sudo apt install ffmpeg</code></div>`
    }</div>`;
    statusEl.querySelector("#set-ff-install")?.addEventListener("click", installFfmpeg);
  };
  const installFfmpeg = async () => {
    // 1 行目: 文言と受け取った量 / 2 行目: 横いっぱいのバー / 3 行目: 配布元の説明と中止
    statusEl.innerHTML = `<div class="set-ff-box">
      <div class="set-ff-head"><span>${esc(tr("ffmpeg をダウンロード中…"))}</span><span id="set-ff-pct" class="set-ff-pct"></span></div>
      <div class="transfer-bar"><div id="set-ff-fill"></div></div>
      <div class="set-ff-foot">
        <span class="set-ff-note">${esc(
          mac
            ? tr("配布元: martin-riedl.de (GPL 版、Apple の公証済み)。ダウンロード後に配布元のチェックサムと照合します。")
            : tr("配布元: BtbN/FFmpeg-Builds (GitHub、LGPL 版)。ダウンロード後に配布元のチェックサムと照合します。"),
        )}</span>
        <button class="btn mini" id="set-ff-cancel">${esc(tr("中止"))}</button>
      </div>
    </div>`;
    statusEl.querySelector("#set-ff-cancel")!.addEventListener("click", () => invoke("cancel_ffmpeg_install"));
    const unlisten = await listen<{ received: number; total: number }>("ffmpeg-progress", (ev) => {
      const { received, total } = ev.payload;
      const pct = total ? Math.floor((received / total) * 100) : 0;
      const fill = document.getElementById("set-ff-fill");
      const label = document.getElementById("set-ff-pct");
      if (fill) fill.style.width = `${pct}%`;
      if (label) label.textContent = `${fmtBytes(received)} / ${fmtBytes(total)}`;
    });
    let cancelled = false;
    const r = await invoke<FfmpegSettings>("install_ffmpeg").catch((e) => {
      // 中止したときは Rust が「中止しました」(の訳) を返す
      cancelled = String(e) === tr("中止しました");
      if (!cancelled) failToast(e, 8000);
      return null;
    });
    unlisten();
    analyticsFfmpegInstall(r?.ffmpeg ? "installed" : cancelled ? "cancelled" : "failed");
    if (r?.ffmpeg) toast(tr("ffmpeg を入れました"));
    ffStatus(r ?? (await call<FfmpegSettings>("get_settings")));
  };
  ffStatus(settings);
  const ffInput = root.querySelector<HTMLInputElement>("#set-ffmpeg")!;
  const ffReset = root.querySelector<HTMLButtonElement>("#set-ffmpeg-reset")!;
  // 選んだ場所は Rust が prefs.json に書く。画面は表示を合わせるだけ
  const showChosen = (r: FfmpegSettings) => {
    ffInput.value = r.ffmpegChosen ?? "";
    ffReset.classList.toggle("hidden", !r.ffmpegChosen);
    setStoredLocal("ffmpegPath", r.ffmpegChosen);
    ffStatus(r);
  };
  root.querySelector("#set-ffmpeg-browse")!.addEventListener("click", async () => {
    // ファイルのダイアログは Rust が開き、ffmpeg と確かめてから覚える (やめたら null)
    const r = await call<FfmpegSettings | null>("pick_ffmpeg");
    if (r) showChosen(r);
  });
  ffReset.addEventListener("click", async () => {
    const r = await call<FfmpegSettings>("reset_ffmpeg_path");
    if (r) showChosen(r);
  });

  for (const [id, label, early] of [
    ["#set-update-check", tr("アップデートを確認"), false],
    ["#set-update-early", tr("確認"), true],
  ] as const) {
    root.querySelector(id)!.addEventListener("click", async (e) => {
      const b = e.currentTarget as HTMLButtonElement;
      b.disabled = true;
      b.textContent = tr("確認中…");
      await checkForUpdate(true, early);
      b.disabled = false;
      b.textContent = label;
    });
  }
  // 一覧は隠すだけ (並べ直さない)。閉じるときに、開いている間に変わったものがあれば作り直す
  settingsBase = settingsKey();
  showSettingsPane();
  $("set-back").focus();
}

/** 設定を開いた時点の、一覧に効くものの状態 (設定・iPod の中身 (snapshot は読み直すたびに別のもの)・PC の曲) */
let settingsBase: [string, Snapshot | null] = ["", null];
function settingsKey(): [string, Snapshot | null] {
  return [`${JSON.stringify(prefStore)}|${state.pc.byId.size}|${state.pc.tracks.size}|${state.pc.scanning}`, state.snap];
}

/** 設定を閉じるときのフェードアウトの終わりを待つタイマー */
let settingsCloseTimer = 0;

export function showSettingsPane() {
  // 設定は一覧の上に重ねるモーダル (一覧は後ろに残す)
  const el = $("settings");
  if (state.settingsOpen) {
    clearTimeout(settingsCloseTimer);
    el.classList.remove("closing", "hidden");
    renderSettingsDevice();
    return;
  }
  if (el.classList.contains("hidden") || el.classList.contains("closing")) return;
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
    el.classList.add("hidden");
    return;
  }
  // 薄くしながら沈めてから隠す (CSS の .settings-page.closing の 0.2 秒)
  el.classList.add("closing");
  settingsCloseTimer = window.setTimeout(() => {
    el.classList.remove("closing");
    el.classList.add("hidden");
  }, 200);
}

/** 設定ページの iPod の欄 (接続中のときだけ) */
export function renderSettingsDevice() {
  renderLayoutCard();
  const el = document.getElementById("set-device");
  if (!el) return;
  const d = state.snap?.device;
  el.classList.toggle("hidden", !d);
  document.getElementById("set-device-none")?.classList.toggle("hidden", !!d);
  // ライブラリの整理は iPod があるときだけ。書き込めない・転送中は押せない
  document.getElementById("set-tidy")?.classList.toggle("hidden", !d);
  document.getElementById("set-tidy-yomi")?.classList.toggle("hidden", isRockbox());
  document.querySelectorAll<HTMLButtonElement>("#set-tidy [data-tidy]").forEach((b) => (b.disabled = !writable() || state.transferring));
  if (!d) {
    el.innerHTML = "";
    return;
  }
  // 入力中に描き直さない
  if (el.dataset.root === d.root && el.contains(document.activeElement)) return;
  el.dataset.root = d.root;
  el.innerHTML = `
    <div class="set-info">
      <span>${esc(tr("モデル"))}</span><span>${esc(d.modelName)}${d.modelNumber ? ` (${esc(d.modelNumber)})` : ""}</span>
      <span>${esc(tr("場所"))}</span><span>${esc(d.root)}</span>
      <span>${esc(tr("DB の署名"))}</span><span>${d.checksum === "hash58" ? tr("hash58 (FireWire GUID が必要)") : d.checksum === "none" ? tr("不要") : tr("非対応")}</span>
    </div>
    <div class="field" id="set-guid-field"></div>`;
  renderGuidField(el.querySelector<HTMLElement>("#set-guid-field")!, d, !d.firewireId);
}

// tr: 使う側で訳す
const GUID_SOURCE: Record<string, string> = {
  sysInfoExtended: "iPod の SysInfoExtended から自動で取得",
  sysInfo: "iPod の SysInfo から自動で取得",
  usb: "USB のシリアル番号から自動で取得",
  manual: "手で入力した値",
};

/**
 * FireWire GUID の欄。取れているときは読み取り専用で出す (違う値にすると iPod で曲が表示されなくなるため)。
 * 「変更…」で確認してから入力欄にする。取れていないときは最初から入力欄
 */
function renderGuidField(el: HTMLElement, d: DeviceInfo, editing: boolean) {
  const hint = tr("データベースの署名に使う 16 桁の ID です。");
  if (!editing) {
    el.innerHTML = `<label>FireWire GUID</label>
      <div class="field-row guid-row">
        <code class="guid-value">${esc(d.firewireId ?? "")}</code>
        <button class="btn" data-x="copy">${esc(tr("コピー"))}</button>
        <button class="btn" data-x="edit">${esc(tr("変更"))}</button>
      </div>
      <div class="hint">${esc(tr("{source}。{hint}手で変える必要はありません。", { source: tr(GUID_SOURCE[d.firewireIdSource ?? ""] ?? ""), hint }))}</div>`;
    el.querySelector('[data-x="copy"]')!.addEventListener("click", () =>
      navigator.clipboard.writeText(d.firewireId ?? "").then(
        () => toast(tr("FireWire GUID をコピーしました")),
        () => toast(tr("コピーできませんでした"), "error"),
      ),
    );
    el.querySelector('[data-x="edit"]')!.addEventListener("click", async () => {
      const ok = await confirmBox(
        tr("FireWire GUID を変更しますか？"),
        tr("自動で取得した値はふつう正しい値です。違う値にして保存すると、iPod のデータベースの署名が合わなくなり、iPod で曲が表示されなくなります (曲のファイルは消えません)。\n\n修理で基板を交換した iPod など、自動で取得した値が合わないと分かっているときだけ変更してください。"),
        tr("変更する"),
        true,
      );
      if (ok) renderGuidField(el, d, true);
    });
    return;
  }
  el.innerHTML = `<label for="set-guid">FireWire GUID</label>
    <div class="field-row guid-row">
      <input type="text" id="set-guid" class="guid-value" placeholder="000A2700XXXXXXXX" value="${esc(d.firewireId ?? "")}">
      <button class="btn primary" id="set-guid-apply" disabled>${esc(tr("保存"))}</button>
      ${d.firewireId ? `<button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button>` : ""}
    </div>
    <div class="hint">${hint}${d.firewireId ? "" : tr("自動で取得できなかったので、分かっている場合だけ入力してください。")}</div>`;
  const input = el.querySelector<HTMLInputElement>("#set-guid")!;
  const btn = el.querySelector<HTMLButtonElement>("#set-guid-apply")!;
  input.focus();
  input.addEventListener("input", () => {
    btn.disabled = !input.value.trim() || input.value.trim().toUpperCase() === (d.firewireId ?? "");
  });
  el.querySelector('[data-x="cancel"]')?.addEventListener("click", () => renderGuidField(el, d, false));
  btn.addEventListener("click", async () => {
    const restore = busySave(btn);
    const s = await call<Snapshot>("set_firewire_id", { hex: input.value.trim() });
    if (!s) {
      restore();
      return;
    }
    await restore.settle();
    applySnapshot(s);
    toast(tr("FireWire GUID を設定しました"));
  });
}

export function closeSettings() {
  state.settingsOpen = false;
  // 何も変わっていなければ、並べ直さずに一覧を出すだけ (2 万曲で 80ms ほど並べ替え直していた)
  const [key, snap] = settingsKey();
  if (key !== settingsBase[0] || snap !== settingsBase[1]) return renderAll();
  showSettingsPane();
  renderRows();
}

/**
 * iPod 全体の整理 (たまにしか使わない操作)。サイドバーの「ミュージック」の右クリックと、
 * 設定の「iPod」にある「ライブラリの整理」から使う
 */
export function tidyMenuItems(): MenuItem[] {
  const off = !writable() || state.transferring;
  return [
    { header: tr("ライブラリの整理") },
    // 読みは iTunesDB の並べ替え用の項目なので iPod OS だけ
    ...(isRockbox() ? [] : [{ label: tr("全曲に読みを付ける…"), disabled: off, action: () => generateReadings(null) }]),
    { label: tr("アーティスト名の表記ゆれをまとめる…"), disabled: off, action: unifyArtists },
    ...(isRockbox() && state.snap?.rockbox?.dbSupported
      ? [{ label: tr("Rockbox のデータベースを作り直す"), disabled: off, action: () => rebuildRockboxDb() }]
      : []),
  ];
}

/**
 * Rockbox のデータベースを PC で作り直す。本体で作ると曲が多いときは何時間もかかるが、PC なら数秒〜数分で済む。
 * 曲を変えたときは保存のたびに自動で作り直すので、これは初めて使う iPod や、本体の DB がおかしいとき用
 */
export async function rebuildRockboxDb() {
  const busy = busyToast(tr("Rockbox のデータベースを作っています…"));
  let r: { built: { tracks: number; keptStats: number; skipped: number }; snapshot: Snapshot } | null;
  try {
    r = await call("rebuild_rockbox_db");
  } finally {
    busy.done();
  }
  if (!r) return;
  applySnapshot(r.snapshot);
  const b = r.built;
  const parts = [tr("Rockbox のデータベースを作りました ({n} 曲)", { n: b.tracks.toLocaleString() })];
  if (b.skipped) parts.push(tr("{n} 曲はタグを読めない形式なので、iPod の「データベース → 今すぐ更新」で入ります", { n: b.skipped }));
  toast(parts.join(" / "), "info", b.skipped ? 10000 : 3500);
}
