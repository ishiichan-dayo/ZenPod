// iTunes の「情報を見る」の「オプション」(iPod OS の曲と動画で共通): イコライザ・音量の調整・開始 / 停止・
// 再生位置を記憶・シャッフル時にスキップ。メディアの種類の欄は使う側で作ってイコライザの横に置く

import { t as tr } from "../../i18n";
import { fmtDur } from "../../domain/format";
import { EQ_PRESETS } from "../../domain/visual";
import { esc } from "../dom";
import { toast } from "../feedback";

/** 今の値 (iTunesDB の値のまま。音量は -255〜255、位置はミリ秒、イコライザの 0 はなし) */
export interface PlayOptions {
  skipShuffle: boolean;
  rememberPosition: boolean;
  volume: number;
  startMs: number;
  stopMs: number;
  eq: number;
}

export type PlayOptionsEdit = Partial<PlayOptions>;

/** 音量の調整: iTunesDB の -255〜255 と画面の -100〜100% */
const volPct = (raw: number) => Math.round((raw * 100) / 255);
const volRaw = (pct: number) => Math.round((pct * 255) / 100);

/** 再生位置 (ミリ秒) を "1:23" / "1:02:03" に。端数があれば ".5" のように付ける */
export function fmtTime(ms: number): string {
  const frac = ms % 1000;
  const base = ms >= 3600000 ? `${Math.floor(ms / 3600000)}:${String(Math.floor(ms / 60000) % 60).padStart(2, "0")}:${String(Math.floor(ms / 1000) % 60).padStart(2, "0")}` : fmtDur(ms - frac);
  return frac ? `${base}.${String(frac).padStart(3, "0").replace(/0+$/, "")}` : base;
}

/** "1:23" / "1:02:03" / "83" / "1:23.5" をミリ秒に。読めなければ null */
function parseTime(s: string): number | null {
  const m = s.trim().match(/^(?:(\d+):)?(?:(\d+):)?(\d+)(?:\.(\d{1,3}))?$/);
  if (!m) return null;
  const [h, mi] = m[2] !== undefined ? [Number(m[1]), Number(m[2])] : [0, Number(m[1] ?? 0)];
  const sec = Number(m[3]);
  if (m[1] !== undefined && sec >= 60) return null;
  return ((h * 60 + mi) * 60 + sec) * 1000 + Number((m[4] ?? "").padEnd(3, "0") || 0);
}

const same = <T>(list: PlayOptions[], of: (o: PlayOptions) => T): T | null => (list.every((o) => of(o) === of(list[0])) ? of(list[0]) : null);

/** オプションのページ。`kindField` はメディアの種類の欄 (イコライザと横に並べる)。
 * `durationMs` は 1 つだけのときの長さ (開始 / 停止はそのときだけ出す) */
export function playOptionsHtml(list: PlayOptions[], kindField: string, durationMs: number | null): string {
  const eq = same(list, (o) => o.eq);
  const vol = same(list, (o) => o.volume);
  const one = durationMs !== null ? list[0] : null;
  return `
     <div class="te-grid te-detail">
       ${kindField}
       <div class="field"><label for="po-eq">${esc(tr("イコライザ"))}</label><select id="po-eq">${eq === null ? `<option value="" selected>${esc(tr("複数の値"))}</option>` : ""}<option value="0" ${eq === 0 ? "selected" : ""}>${esc(
         tr("なし"),
       )}</option>${EQ_PRESETS.map(([n, name]) => `<option value="${n}" ${n === eq ? "selected" : ""}>${esc(name)}</option>`).join("")}</select></div>
     </div>
     <div class="field"><label for="po-volume">${esc(tr("音量の調整"))}</label>
       <div class="po-volume"><span class="muted">-100%</span><input type="range" id="po-volume" min="-100" max="100" step="1" value="${volPct(vol ?? 0)}"><span class="muted">+100%</span><output class="po-volume-val"></output></div></div>
     ${
       one && durationMs !== null
         ? `<div class="po-times">
         <label class="check"><input type="checkbox" id="po-start-on" ${one.startMs ? "checked" : ""}> ${esc(tr("開始"))}</label><input type="text" id="po-start" value="${esc(fmtTime(one.startMs))}">
         <label class="check"><input type="checkbox" id="po-stop-on" ${one.stopMs ? "checked" : ""}> ${esc(tr("停止"))}</label><input type="text" id="po-stop" value="${esc(fmtTime(one.stopMs || durationMs))}">
       </div>`
         : ""
     }
     <label class="check"><input type="checkbox" id="po-resume"> ${esc(tr("再生位置を記憶"))}</label>
     <label class="check"><input type="checkbox" id="po-skip"> ${esc(tr("シャッフル時にスキップ"))}</label>`;
}

/** オプションのページの動き。`invalid` は入力が正しくないときに呼ぶ (そのページを開く) */
export function bindPlayOptions(root: HTMLElement, list: PlayOptions[], durationMs: number | null, invalid: () => void) {
  const q = <T extends HTMLElement>(sel: string) => root.querySelector<T>(sel);
  const one = durationMs !== null ? list[0] : null;

  // 再生位置を記憶・シャッフル時にスキップ: 値が揃っていなければ「複数の値」(どちらでもない印)
  const flags = (["skip", "resume"] as const).map((name) => {
    const el = q<HTMLInputElement>(`#po-${name}`)!;
    const key = name === "skip" ? ("skipShuffle" as const) : ("rememberPosition" as const);
    const all = same(list, (o) => o[key]);
    const reset = () => {
      el.checked = !!all;
      el.indeterminate = all === null;
    };
    reset();
    let changed = false;
    el.addEventListener("change", () => (changed = true));
    return {
      key,
      // 種類を変えたら、触っていなければその種類の既定にする (保存のときも Rust が同じように戻す)。null は今の値に戻す
      kindChanged(def: boolean | null) {
        if (changed) return;
        if (def === null) return reset();
        el.checked = def;
        el.indeterminate = false;
      },
      value: () => (changed && !el.indeterminate ? el.checked : undefined),
    };
  });

  const vol = same(list, (o) => o.volume);
  const volEl = q<HTMLInputElement>("#po-volume")!;
  const volOut = q<HTMLElement>(".po-volume-val")!;
  let volTouched = false;
  const paintVol = () => {
    const n = Number(volEl.value);
    volOut.textContent = !volTouched && vol === null ? tr("複数の値") : n ? `${n > 0 ? "+" : ""}${n}%` : tr("なし");
  };
  volEl.addEventListener("input", () => {
    volTouched = true;
    paintVol();
  });
  paintVol();
  const eqSel = q<HTMLSelectElement>("#po-eq")!;
  const times = (["start", "stop"] as const).map((name) => {
    const on = q<HTMLInputElement>(`#po-${name}-on`);
    const txt = q<HTMLInputElement>(`#po-${name}`);
    const sync = () => txt && (txt.disabled = !on!.checked);
    on?.addEventListener("change", sync);
    sync();
    return { name, on, txt };
  });

  return {
    /** 種類を変えたとき。`def` はその種類の既定 (null は今の値に戻す) */
    kindChanged(def: boolean | null) {
      flags.forEach((f) => f.kindChanged(def));
    },
    /** 変えた項目だけ。入力が正しくなければ知らせて null */
    changes(): PlayOptionsEdit | null {
      const out: PlayOptionsEdit = {};
      for (const f of flags) {
        const v = f.value();
        if (v !== undefined) out[f.key] = v;
      }
      if (volTouched) out.volume = volRaw(Number(volEl.value));
      if (eqSel.value !== "" && list.some((o) => o.eq !== Number(eqSel.value))) out.eq = Number(eqSel.value);
      if (one && durationMs !== null) {
        for (const t of times) {
          if (!t.on || !t.txt) continue;
          let ms = 0;
          if (t.on.checked) {
            const p = parseTime(t.txt.value);
            if (p === null || p > durationMs) {
              toast(tr("{a}の位置は 1:23 の形で、{len} までで入力してください", { a: t.name === "start" ? tr("開始") : tr("停止"), len: fmtTime(durationMs) }), "error");
              invalid();
              t.txt.focus();
              return null;
            }
            // 停止の位置を終わりにしたときは「最後まで」(0) と同じ
            ms = t.name === "stop" && p >= durationMs - 999 ? 0 : p;
          }
          const key = t.name === "start" ? "startMs" : "stopMs";
          if (ms !== one[key]) out[key] = ms;
        }
      }
      return out;
    },
  };
}
