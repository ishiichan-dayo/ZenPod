// 一覧の下の帯 (曲の数・選んだ曲の形式)

import { t as tr } from "../i18n";
import { invoke } from "../infra/tauri";
import { $, state } from "../app/state";
import { fmtBytes, fmtTotal } from "../domain/format";
import { esc } from "./dom";
import { findTrack } from "./player";

/** ヘッダーの曲数の下に、表示中の曲の合計時間と容量を出す */
export function renderStatus() {
  // アーティスト表示はヘッダーの曲数が全アーティストの分なので、合計もそれに合わせる
  const rows = state.layout === "artists" && state.rowLayout ? state.artists.flatMap((a) => a.tracks) : state.rows;
  const ms = rows.reduce((a, t) => a + t.durationMs, 0);
  const bytes = rows.reduce((a, t) => a + t.size, 0);
  // 曲数が出ていない (接続待ち・読み込み中) ときは出さない
  const shown = rows.length > 0 && state.countShown;
  $("view-stats").textContent = shown ? `${fmtTotal(ms)} · ${fmtBytes(bytes)}` : "";
}

interface AudioSummary {
  codec: string;
  lossless: boolean | null;
  bitrate: number | null;
  sampleRate: number | null;
  bitDepth: number | null;
  channels: number | null;
}

/** 曲 ID → 形式の要約 (読めなかった曲は null)。音声の中身は編集で変わらないので iPod を開き直すまで使う */
export const summaryCache = new Map<string, AudioSummary | null>();
let formatTimer = 0;
let formatSeq = 0;
/** フッターの右端に、カーソルのある曲 (クリックした行) の形式を出す。チェックは関係ない。
 *  カーソルが止まってから読む (矢印キーで連続して動かしたとき用) */
export function renderFormat() {
  clearTimeout(formatTimer);
  const id = state.focused;
  const el = $("status-format");
  if (!id || !findTrack(id)) {
    el.classList.add("hidden");
    return;
  }
  const seq = ++formatSeq;
  const show = () => {
    const f = summaryCache.get(id);
    if (seq !== formatSeq) return;
    if (!f) {
      el.classList.add("hidden");
      return;
    }
    const parts = [
      f.codec,
      f.bitrate ? `${f.bitrate} kbps` : "",
      f.sampleRate ? `${(f.sampleRate / 1000).toLocaleString(undefined, { maximumFractionDigits: 3 })} kHz` : "",
      f.bitDepth ? `${f.bitDepth} bit` : "",
      f.channels ? (f.channels === 1 ? tr("モノラル") : f.channels === 2 ? tr("ステレオ") : `${f.channels} ch`) : "",
    ].filter(Boolean);
    el.innerHTML =
      esc(parts.join(" · ")) +
      (f.lossless === null ? "" : ` <span class="dt-badge${f.lossless ? " lossless" : ""}">${f.lossless ? tr("可逆") : tr("非可逆")}</span>`);
    el.dataset.id = id;
    el.classList.remove("hidden");
  };
  if (summaryCache.has(id)) {
    show();
    return;
  }
  formatTimer = window.setTimeout(async () => {
    const r = await invoke<(AudioSummary | null)[]>("audio_summaries", { ids: [id] }).catch(() => null);
    if (!r) return;
    summaryCache.set(id, r[0]);
    show();
  }, 150);
}
