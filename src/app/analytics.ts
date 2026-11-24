// 利用状況で送る出来事。送るものを足したら、設定の「このアプリについて」の説明と README の「安心して使うために」(README.en.md は Safety) を直す。
// 曲名・ファイル名・パス・iPod の名前やシリアル・正確な数 (範囲にする)・エラーの文章は送らない

import { appVersion, state } from "./state";
import type { Snapshot, TransferSummary } from "../domain/types";
import { bucket, sendAnalytics } from "../infra/analytics";

/** 起動。Umami のメインの画面の訪問者はページビューだけを数えるので、ページビューとして送る。版は題名に入れる */
export function analyticsLaunch() {
  sendAnalytics("launch", { url: "/launch", title: `ZenPod ${appVersion}` });
}

/** 開いた iPod のモードと機種 (モードと機種の組み合わせごとに 1 日 1 回) */
export function analyticsIpod(s: Snapshot) {
  const model = s.device.modelName;
  sendAnalytics(`ipod:${s.mode}:${model}`, {
    url: "/ipod",
    name: "ipod",
    data: { mode: s.mode, model, ...(s.rockbox?.version ? { rockbox: s.rockbox.version } : {}) },
  });
}

/** iPod を開くのにかかった時間と曲数 (どちらも範囲。モードごとに 1 日 1 回)。反応の速さが保てているかを見るため */
export function analyticsOpen(s: Snapshot, ms: number) {
  const time = (v: number) => bucket(v, [1000, 3000, 10000, 30000], ["<1s", "1-3s", "3-10s", "10-30s", "30s+"]);
  // Rockbox: どこで時間がかかったか。walk フォルダをたどる・tags タグを読む・other それ以外 (プレイリスト・ジャケットの割り当てなど)、
  // reread キャッシュが使えずタグを読み直した曲の割合、fromdb タグを読まずに Rockbox のデータベースから埋めた曲の割合
  const sc = s.scan;
  const share = (n: number) => (n === 0 ? "0" : bucket(n / Math.max(1, sc!.files), [0.1, 0.9], ["<10%", "10-90%", "90%+"]));
  const scan: Record<string, string> = sc
    ? {
        walk: time(sc.walkMs),
        tags: time(sc.readMs),
        other: time(Math.max(0, ms - sc.walkMs - sc.readMs)),
        reread: share(sc.reread),
        fromdb: share(sc.fromDb),
      }
    : {};
  // 両方のモード: probe 機種を調べた (うち usb USB の問い合わせ・space 容量の問い合わせ)・itdb iTunesDB・artdb ArtworkDB・
  // snap 画面に渡す一覧を作った・rest 測った分の残り (画面とのやり取りなど)。1 秒未満も分ける。
  // spotlight は Mac だけ: Spotlight がこの iPod を索引しているか (HDD を取り合う)
  const tm = s.times;
  const part = (v: number) => bucket(v, [100, 1000, 3000, 10000], ["<0.1s", "0.1-1s", "1-3s", "3-10s", "10s+"]);
  const measured = tm ? tm.probeMs + tm.dbMs + tm.artMs + tm.snapshotMs + (sc ? sc.walkMs + sc.readMs : 0) : 0;
  const times: Record<string, string> = tm
    ? {
        probe: part(tm.probeMs),
        usb: part(tm.usbMs),
        space: part(tm.spaceMs),
        itdb: part(tm.dbMs),
        ...(s.mode === "ipod" ? { artdb: part(tm.artMs) } : {}),
        snap: part(tm.snapshotMs),
        rest: part(Math.max(0, ms - measured)),
        ...(tm.spotlight === null ? {} : { spotlight: tm.spotlight ? "yes" : "no" }),
      }
    : {};
  sendAnalytics(`open:${s.mode}`, {
    url: "/open",
    name: "open",
    data: {
      mode: s.mode,
      time: time(ms),
      tracks: bucket(s.tracks?.length ?? 0, [1000, 5000, 20000], ["<1k", "1k-5k", "5k-20k", "20k+"]),
      ...scan,
      ...times,
    },
  });
}

/** 起動してから iPod について見えたもの (iPod を開かずに終わる人の、どこで止まったかを見るため) */
export const analyticsSeen = { usb: false, drive: false, opened: false, failed: false };

/** はじめの案内を閉じてから、これだけたっても iPod を開いていなければ送る */
export const NO_IPOD_AFTER_MS = 120_000;

/** iPod を開かないまま時間がたった (1 日 1 回)。usb USB で iPod を認識した・drive ドライブが出た・failed 開けなかった・pc PC の音楽フォルダを登録している */
export function analyticsNoIpod() {
  if (analyticsSeen.opened || state.snap) return;
  const yn = (v: boolean) => (v ? "yes" : "no");
  sendAnalytics("noipod", {
    url: "/noipod",
    name: "no_ipod",
    data: {
      usb: yn(analyticsSeen.usb),
      drive: yn(analyticsSeen.drive),
      failed: yn(analyticsSeen.failed),
      pc: yn(state.pc.folders.length > 0),
    },
  });
}

/** 転送の結果 (モードと結果ごとに 1 日 1 回)。曲数は範囲、エラーの文章は送らない */
export function analyticsTransfer(s: TransferSummary) {
  const result = s.saveError ? "save_error" : s.cancelled ? "cancelled" : s.errors.length ? "errors" : "ok";
  const mode = state.snap?.mode ?? "unknown";
  const n = (v: number) => bucket(v, [1, 10, 100, 1000], ["0", "1-9", "10-99", "100-999", "1k+"]);
  sendAnalytics(`transfer:${mode}:${result}`, {
    url: "/transfer",
    name: "transfer",
    data: { mode, result, songs: n(s.added + s.replaced), errors: n(s.errors.length) },
  });
}

// ---- ネットから取ってくる機能の、どこまで進んだか ----
// 使い始めた人が最後 (反映・保存) までたどり着いたか、どこで止まったかを見るため。
// 検索した言葉 (アーティスト名・アルバム名) や見つかったものの名前は送らない。数と時間は範囲

const netTime = (ms: number) => bucket(ms, [1000, 3000, 10000, 30000], ["<1s", "1-3s", "3-10s", "10-30s", "30s+"]);

/** 段階をどこまで進んだか (戻らない) と、検索の結果 */
interface NetFunnel {
  stages: readonly string[];
  /** 進んだ一番先の段階 (stages の添字。-1 は検索まで行っていない) */
  at: number;
  /** 最後の検索の候補の数 */
  found: number;
  /** 最後の検索にかかった時間 */
  ms: number;
  /** 検索で通信に失敗した */
  error: boolean;
  /** 反映した候補の検索先 (itunes / deezer / musicbrainz) */
  source: string;
}

export function netFunnel(stages: readonly string[]): NetFunnel {
  return { stages, at: -1, found: 0, ms: 0, error: false, source: "" };
}

export function netReach(f: NetFunnel, stage: string) {
  f.at = Math.max(f.at, f.stages.indexOf(stage));
}

/** 検索を 1 回した結果を覚える (`res` が null なら通信に失敗した) */
export function netSearched(f: NetFunnel, started: number, res: unknown[] | null) {
  f.ms = performance.now() - started;
  f.error = res === null;
  f.found = res?.length ?? 0;
  netReach(f, f.found ? "results" : "searched");
}

/** 画面を閉じたときに、どこまで進んだかを送る (段階ごとに 1 日 1 回)。検索まで行かなければ送らない */
export function analyticsNet(name: "artwork_search" | "song_info", f: NetFunnel) {
  if (f.at < 0) return;
  const reached = f.stages[f.at];
  sendAnalytics(`${name}:${reached}`, {
    url: `/${name}`,
    name,
    data: {
      reached,
      found: bucket(f.found, [1, 10], ["0", "1-9", "10+"]),
      time: netTime(f.ms),
      error: f.error ? "yes" : "no",
      ...(f.source ? { source: f.source } : {}),
    },
  });
}

/** 表記ゆれの統一で、MusicBrainz に別名を問い合わせた結果 (結果ごとに 1 日 1 回)。アーティスト名は送らない */
export function analyticsAliasLookup(result: "done" | "cancelled" | "error", artists: number, ms: number) {
  sendAnalytics(`alias:${result}`, {
    url: "/alias_lookup",
    name: "alias_lookup",
    data: { result, artists: bucket(artists, [1, 10, 100, 1000], ["0", "1-9", "10-99", "100-999", "1k+"]), time: netTime(ms) },
  });
}

/** 設定から ffmpeg を入れた結果 (結果ごとに 1 日 1 回。はじめの案内から入れた分は onboarding で送る) */
export function analyticsFfmpegInstall(result: "installed" | "failed" | "cancelled") {
  sendAnalytics(`ffmpeg:${result}`, { url: "/ffmpeg_install", name: "ffmpeg_install", data: { result } });
}

/** USB で iPod を認識したが、20 秒たってもドライブとして表示されなかった (1 日 1 回)。原因の多さを見るため。
 * reason: ejected (Windows。取り出したあと挿したまま) / fsck (Mac がディスクを検査中) / denied (Mac。ドライブを読む許可が無い) / none。
 * Linux と、まだ調べていないときは無し */
export function analyticsUsbNoDrive(reason: string | null) {
  sendAnalytics("usbnodrive", { url: "/usb", name: "usb_no_drive", data: { after: "20s", ...(reason ? { reason } : {}) } });
}

/** はじめの案内で選んだもの。ffmpeg: present (もともと入っていた) / installed / failed / later (入れずに次へ) /
 * skipped (そのページで案内を閉じた) / not_reached。rockbox: yes / no / unknown / none。
 * end: done (最後まで) / skip (右上のスキップ) / esc (Esc で閉じた)。at: 閉じたページ (welcome / ffmpeg / pc / rockbox / done) */
export function analyticsOnboarding(data: { ffmpeg: string; rockbox: string; end: string; at: string }) {
  sendAnalytics("onboarding", { url: "/onboarding", name: "onboarding", data });
}
