// スマートプレイリストの条件の画面

import { lang, t as tr } from "../../i18n";
import { isRockbox, writable } from "../../app/selectors";
import { state } from "../../app/state";
import { fmtBytes, fmtTotal } from "../../domain/format";
import { SPL_BOOL_OP_LABEL, SPL_CHOICES, SPL_FIELD_GROUPS, SPL_LIMIT_TYPES, SPL_MEDIA_LABEL, SPL_NOT_IN_ROCKBOX, SPL_OP_LABEL, SPL_SORTS, SPL_UNITS, splFieldLabel, splOps, splToNodes, splToRules, splToday, splType } from "../../domain/smart";
import type { SplCond, SplDef, SplMedia, SplNode, SplOp, SplPrefs, SplPreview } from "../../domain/smart";
import type { Snapshot } from "../../domain/types";
import { exportM3u8, exportSmartXml, importPlaylists } from "./import-playlists";
import { esc, isEnter } from "../dom";
import { call, reduceMotion } from "../feedback";
import { ICON_CLOSE } from "../icons";
import { showMenu } from "../menu";
import { busySave, modal } from "../modal";
import { applySnapshot } from "../render";
import { placeSegThumb } from "../segments";
import { newPlaylist } from "../selection-actions";
import { selectView } from "../sidebar";

/** 項目を選んだときの初めの値 */
function splDefault(field: number): SplCond {
  const type = splType(field) ?? "text";
  const c: SplCond = { kind: "cond", field, op: splOps(type)[0], a: "", b: "", unit: 86400 };
  switch (type) {
    case "num":
      c.a = c.b = "0";
      break;
    case "bitrate":
      c.a = c.b = "128";
      break;
    case "samplerate":
      c.a = c.b = "44100";
      break;
    case "size":
      c.a = c.b = "5";
      break;
    case "time":
      c.a = c.b = "3:00";
      break;
    case "rating":
      c.a = c.b = "5";
      break;
    case "date":
      c.a = c.b = splToday();
      c.unit = 86400;
      break;
    case "playlist":
      c.a = splPlaylistChoices()[0]?.[0] ?? "0";
      break;
    default:
      c.a = SPL_CHOICES[type]?.[0][0] ?? "";
  }
  return c;
}

/** 「プレイリストが…」で選べるプレイリスト (値は iTunes と同じ符号付きの 64 ビット) */
function splPlaylistChoices(): [string, string][] {
  return (state.snap?.playlists ?? [])
    .filter((p) => !p.master && !p.podcast)
    .map((p) => [BigInt.asIntN(64, BigInt(p.id)).toString(), p.name]);
}

/** その項目の値を、今開いている iPod の曲が持っていないか (判定では「なし」・0 として数える) */
function splAway(field: number): boolean {
  return SPL_FIELD_GROUPS[SPL_FIELD_GROUPS.length - 1][1].includes(field) || (isRockbox() && SPL_NOT_IN_ROCKBOX.includes(field));
}

function splFieldOptions(sel: number): string {
  return SPL_FIELD_GROUPS.map(
    ([head, fields]) =>
      `<optgroup label="${esc(tr(head))}">${fields
        .map((f) => `<option value="${f}" ${f === sel ? "selected" : ""}>${esc(splFieldLabel(f))}</option>`)
        .join("")}</optgroup>`,
  ).join("");
}

function splOptions(list: [string, string][], sel: string, translate = true): string {
  return list.map(([v, l]) => `<option value="${esc(v)}" ${v === sel ? "selected" : ""}>${esc(translate ? tr(l) : l)}</option>`).join("");
}

/** 星 (評価)。押した星と同じ星をもう一度押すと 0 (なし) */
function splStarsHtml(k: "a" | "b", v: string): string {
  const n = Number(v) || 0;
  return `<span class="spl-stars" role="radiogroup" aria-label="${esc(tr("評価"))}">${[1, 2, 3, 4, 5]
    .map((i) => `<button type="button" class="spl-star${i <= n ? " on" : ""}" data-star="${i}" data-k="${k}" role="radio" aria-checked="${i === n}" aria-label="${"★".repeat(i)}">★</button>`)
    .join("")}</span>`;
}

/** 値の欄 (項目の種類と比較で変わる) */
function splValueHtml(c: SplCond): string {
  const type = splType(c.field) ?? "text";
  const two = c.op === "range";
  const box = (k: "a" | "b", v: string, kind: string, extra = "") => `<input type="${kind}" data-k="${k}" value="${esc(v)}" ${extra} />`;
  const pair = (one: (k: "a" | "b", v: string) => string, suffix = "") =>
    two ? `${one("a", c.a)}<span class="spl-to muted">〜</span>${one("b", c.b)}${suffix}` : `${one("a", c.a)}${suffix}`;
  const unit = (u: string) => `<span class="spl-unit muted">${esc(u)}</span>`;
  const num = (k: "a" | "b", v: string) => box(k, v, "number", `min="0" step="1"`);
  switch (type) {
    case "text":
      return box("a", c.a, "text", `spellcheck="false" placeholder="${esc(tr("文字を入力"))}"`);
    case "num":
      return pair(num);
    case "bitrate":
      return pair(num, unit("kbps"));
    case "samplerate":
      return pair(num, unit("Hz"));
    case "size":
      return pair((k, v) => box(k, v, "number", `min="0" step="any"`), unit("MB"));
    case "time":
      return pair((k, v) => box(k, v, "text", `placeholder="m:ss" inputmode="numeric"`));
    case "rating":
      return pair(splStarsHtml);
    case "date":
      if (c.op === "inLast" || c.op === "notInLast")
        return `${box("a", c.a, "number", `min="1" step="1"`)}<select data-k="unit">${splOptions(
          SPL_UNITS.map(([s, l]) => [String(s), l]),
          String(c.unit),
        )}</select>`;
      return pair((k, v) => box(k, v, "date"));
    case "bool":
    case "boolNot":
      return "";
    case "playlist": {
      const list = splPlaylistChoices();
      // 消えたプレイリストを指していたら、そのまま残す
      if (!list.some(([v]) => v === c.a)) list.unshift([c.a, tr("(見つからないプレイリスト)")]);
      return `<select data-k="a">${splOptions(list, c.a, false)}</select>`;
    }
    default:
      return `<select data-k="a">${splOptions(SPL_CHOICES[type] ?? [], c.a)}</select>`;
  }
}

/** すべて / いずれか (条件が 2 つ以上のときだけ出す) */
function splMatchSeg(any: boolean, path: string): string {
  const name = `spl-any-${path || "root"}`;
  return `<div class="seg spl-match" role="radiogroup" aria-label="${esc(tr("条件の合わせ方"))}" data-gpath="${path}">
    <label class="seg-item"><input type="radio" name="${name}" value="0" data-k="any" ${any ? "" : "checked"} /><span>${esc(tr("すべてに一致"))}</span></label>
    <label class="seg-item"><input type="radio" name="${name}" value="1" data-k="any" ${any ? "checked" : ""} /><span>${esc(tr("いずれかに一致"))}</span></label>
  </div>`;
}


function splDelButton(label: string): string {
  return `<button type="button" class="spl-del" data-act="remove" title="${esc(label)}" aria-label="${esc(label)}">${ICON_CLOSE}</button>`;
}

/** 条件の並び (グループの中も同じ形)。path は並びの場所 ("" が一番上、"2" は 3 つ目のグループ)。
 *  グループの中では、すべて / いずれかの切り替えはグループの見出しの行に出す */
function splListHtml(items: SplNode[], any: boolean, path: string): string {
  const at = (i: number) => (path ? `${path}.${i}` : String(i));
  const rows = items
    .map((n, i) => {
      const p = at(i);
      if (n.kind === "group")
        return `<div class="spl-group" data-path="${p}">
          <div class="spl-group-head">${
            n.items.length > 1 ? splMatchSeg(n.any, p) : `<span class="spl-group-title">${esc(tr("@spl:グループ"))}</span>`
          }${splDelButton(tr("このグループを削除"))}</div>
          ${splListHtml(n.items, n.any, p)}
        </div>`;
      if (n.kind === "raw")
        return `<div class="spl-row" data-path="${p}"><span class="spl-raw muted">${esc(tr("ZenPod では編集できない条件 (そのまま残します)"))}</span>${splDelButton(tr("この条件を削除"))}</div>`;
      const type = splType(n.field) ?? "text";
      const ops = splOps(type)
        .map((o) => {
          const label = (type === "bool" || type === "boolNot" ? SPL_BOOL_OP_LABEL[o] : null) ?? SPL_OP_LABEL[o];
          return `<option value="${o}" ${o === n.op ? "selected" : ""}>${esc(tr(label))}</option>`;
        })
        .join("");
      const away = splAway(n.field);
      return `<div class="spl-row${away ? " away" : ""}" data-path="${p}">
        <select class="spl-field" data-k="field" aria-label="${esc(tr("項目"))}">${splFieldOptions(n.field)}</select>
        <select class="spl-op" data-k="op" aria-label="${esc(tr("比べ方"))}">${ops}</select>
        <span class="spl-val">${splValueHtml(n)}</span>
        ${splDelButton(tr("この条件を削除"))}
      </div>`;
    })
    .join("");
  return `${items.length > 1 && !path ? splMatchSeg(any, path) : ""}
    <div class="spl-list">${rows}</div>
    <div class="spl-adds" data-gpath="${path}">
      <button type="button" class="btn ghost mini" data-act="add">＋ ${esc(tr("条件を追加"))}</button>
      ${path ? "" : `<button type="button" class="btn ghost mini" data-act="group">＋ ${esc(tr("グループを追加"))}</button>`}
    </div>`;
}

/** 新しく作るときのひな形 */
interface SplTemplate {
  name: string;
  rules: () => SplNode[];
  prefs?: Partial<SplPrefs>;
}
// tr: 使う側で訳す
const SPL_TEMPLATES: SplTemplate[] = [
  {
    name: "よく聴く曲",
    rules: () => [{ ...splDefault(0x16), op: "gt", a: "0" }],
    prefs: { checkLimits: true, limitType: 3, limitValue: 25, limitSort: 0x19, reverseSort: false },
  },
  { name: "最近追加した曲", rules: () => [{ ...splDefault(0x10), op: "inLast", a: "2", unit: 604800 }] },
  { name: "評価の高い曲", rules: () => [{ ...splDefault(0x19), op: "gt", a: "3" }] },
  { name: "まだ聴いていない曲", rules: () => [{ ...splDefault(0x16), op: "is", a: "0" }] },
  {
    name: "しばらく聴いていない曲",
    rules: () => [
      { ...splDefault(0x16), op: "gt", a: "0" },
      { ...splDefault(0x17), op: "notInLast", a: "3", unit: 2628000 },
    ],
  },
];

/** スマートプレイリストを作る (id なし) / 条件を編集する */
export async function openSmartEditor(id?: string) {
  if (!writable()) return;
  const pl = id ? state.snap?.playlists.find((p) => p.id === id) : null;
  let def: SplDef;
  if (id) {
    const d = await call<SplDef>("smart_playlist", { id });
    if (!d) return;
    def = d;
  } else {
    def = {
      media: "music",
      rules: { any: false, items: [] },
      prefs: { liveUpdate: true, checkRules: true, checkLimits: false, limitType: 3, limitSort: 2, reverseSort: false, limitValue: 25, matchChecked: false },
    };
  }
  const root = { any: def.rules.any, items: splToNodes(def.rules.items) };
  if (!root.items.length) root.items.push(splDefault(0x04));
  // 「チェックマークの付いた曲だけ」は画面に出さない (ZenPod ではチェックを見る・変える画面が無く、使う場面がほぼ無い)。
  // iTunes でオンにしてあれば、そのまま保存する
  const prefs = { ...def.prefs };
  let media = def.media;
  // iTunes で「条件を使う」を外してあるときだけ、そのスイッチを出す
  const showRulesSwitch = !prefs.checkRules;
  const sortValue = (s: number, r: boolean) => `${s}:${r ? 1 : 0}`;
  const sorts: [string, string][] = SPL_SORTS.map(([s, r, l]) => [sortValue(s, r), l]);
  // 知らない選択方法は、そのまま残す
  if (!sorts.some(([v]) => v === sortValue(prefs.limitSort, prefs.reverseSort))) sorts.push([sortValue(prefs.limitSort, prefs.reverseSort), tr("(その他)")]);
  const oldIds = new Set(state.snap?.playlists.map((p) => p.id));
  const switchRow = (sid: string, on: boolean, label: string, hint: string) =>
    `<label class="check switch-row spl-switch"><span class="switch-text">${esc(label)}<span class="check-hint">${esc(hint)}</span></span><input type="checkbox" role="switch" class="switch" id="${sid}" ${on ? "checked" : ""}></label>`;
  const limitSort = `<select id="spl-lim-sort" aria-label="${esc(tr("選び方"))}">${splOptions(sorts, sortValue(prefs.limitSort, prefs.reverseSort))}</select>`;
  const limitVal = `<input type="number" id="spl-lim-val" min="1" step="1" value="${prefs.limitValue}" aria-label="${esc(tr("上限"))}" />`;
  const limitType = `<select id="spl-lim-type" aria-label="${esc(tr("単位"))}">${splOptions(
    SPL_LIMIT_TYPES.map(([v, l]) => [String(v), l]),
    String(prefs.limitType),
  )}</select>`;
  // 日本語は「ランダム で 25 曲 まで」、英語は「25 songs selected by random」
  const limitLine =
    lang === "ja"
      ? `${limitSort}<span>${esc(tr("で"))}</span>${limitVal}${limitType}<span>${esc(tr("まで"))}</span>`
      : `${limitVal}${limitType}<span>${esc(tr("選び方"))}</span>${limitSort}`;

  await modal(
    `<div class="spl-head">
       <h3>${esc(id ? tr("スマートプレイリストを編集") : tr("新規スマートプレイリスト"))}</h3>
       <label class="spl-label" for="spl-name">${esc(tr("名前"))}</label>
       <input type="text" id="spl-name" class="spl-name" value="${esc(pl?.name ?? tr("スマートプレイリスト"))}" />
     </div>
     <div class="spl-body">
     ${
       id
         ? ""
         : `<div class="spl-templates"><span class="spl-label">${esc(tr("ひな形"))}</span>${SPL_TEMPLATES.map(
             (t, i) => `<button type="button" class="spl-chip" data-tpl="${i}">${esc(tr(t.name))}</button>`,
           ).join("")}</div>`
     }
     <div class="spl-section spl-chips" role="radiogroup" aria-label="${esc(tr("対象"))}">
       <span class="spl-label">${esc(tr("対象"))}</span>${(Object.keys(SPL_MEDIA_LABEL) as SplMedia[])
         .map((m) => `<button type="button" class="spl-chip${m === media ? " on" : ""}" data-media="${m}" role="radio" aria-checked="${m === media}">${esc(tr(SPL_MEDIA_LABEL[m]))}</button>`)
         .join("")}
     </div>
     <div class="spl-section">
       <div class="spl-label">${esc(tr("条件"))}</div>
       ${showRulesSwitch ? switchRow("spl-rules-on", prefs.checkRules, tr("条件を使う"), tr("iTunes で外してあります。外したままだと、すべての曲が入ります")) : ""}
       <div id="spl-rules" class="spl-rules"></div>
       <div id="spl-away" class="spl-away hidden">${esc(
         isRockbox()
           ? tr("薄く表示した項目の情報 (再生回数・ラブなど) は Rockbox の曲には無いので、「なし」・0 として数えます")
           : tr("iTunes だけの情報 (ラブ・購入した項目など) は iPod に入っていないので、「なし」として数えます"),
       )}</div>
     </div>
     <div class="spl-section">
       ${switchRow("spl-lim-on", prefs.checkLimits, tr("曲の数を絞る"), tr("条件に合う曲が多いときに、決めた数・長さ・大きさまでにします"))}
       <div id="spl-limit" class="spl-limit${prefs.checkLimits ? "" : " hidden"}">${limitLine}</div>
       ${switchRow("spl-live", prefs.liveUpdate, tr("ライブアップデート"), tr("曲を足したり消したりしたときに、中身を作り直します"))}
     </div>
     </div>
     <div class="spl-foot">
       <div id="spl-preview" class="spl-preview" aria-live="polite"></div>
       <div class="modal-actions"><button class="btn" data-x="cancel">${esc(tr("キャンセル"))}</button><button class="btn primary" data-x="ok">${esc(id ? tr("保存") : tr("作成"))}</button></div>
     </div>`,
    (m, close) => {
      const q = <T extends HTMLElement>(sel: string) => m.querySelector<T>(sel)!;
      const rulesEl = q<HTMLElement>("#spl-rules");
      const nameEl = q<HTMLInputElement>("#spl-name");
      const okBtn = q<HTMLButtonElement>('[data-x="ok"]');
      let nameTouched = !!id;
      const render = () => {
        rulesEl.innerHTML = splListHtml(root.items, root.any, "");
        rulesEl.classList.toggle("off", !prefs.checkRules);
        rulesEl.querySelectorAll<HTMLElement>(".seg").forEach((s) => placeSegThumb(s, false));
        q<HTMLElement>("#spl-away").classList.toggle("hidden", !rulesEl.querySelector(".spl-row.away"));
      };
      /** data-path ("0.2.1") の行がある並びと、その中の位置 */
      const locate = (path: string): { list: SplNode[]; i: number } => {
        const idx = path.split(".").map(Number);
        let list = root.items;
        for (const i of idx.slice(0, -1)) list = (list[i] as { items: SplNode[] }).items;
        return { list, i: idx[idx.length - 1] };
      };
      /** data-gpath ("" は一番上) のグループ */
      const group = (gpath: string): { any: boolean; items: SplNode[] } => {
        if (!gpath) return root;
        const { list, i } = locate(gpath);
        return list[i] as { any: boolean; items: SplNode[] };
      };
      const currentDef = (): SplDef => ({
        media,
        rules: { any: root.any, items: splToRules(root.items) },
        prefs: { ...prefs },
      });
      // 条件に合う曲 (入力のたびに少し待ってから数える。古い結果は捨てる)
      let seq = 0;
      let timer = 0;
      const preview = () => {
        clearTimeout(timer);
        timer = window.setTimeout(async () => {
          const my = ++seq;
          const r = await call<SplPreview>("preview_smart_playlist", { id: id ?? null, def: currentDef() });
          if (my !== seq || !r || !m.isConnected) return;
          const byId = new Map(state.snap?.tracks.map((t) => [t.id, t]));
          const names = (r.sample ?? []).map((tid) => byId.get(tid)?.title).filter((t): t is string => !!t);
          const more = r.count - names.length;
          q<HTMLElement>("#spl-preview").innerHTML = `
            <div class="spl-count"><b>${esc(tr("{n} 曲", { n: r.count }))}</b>${
              r.count ? `<span class="muted">${esc(fmtTotal(r.lengthMs))} · ${esc(fmtBytes(r.size))}</span>` : ""
            }</div>
            <div class="spl-sample muted">${
              r.count
                ? esc(names.join(tr("、"))) + (more > 0 ? esc(tr(" ほか {n} 曲", { n: more })) : "")
                : esc(tr("いまの条件に合う曲はありません"))
            }</div>`;
        }, 150);
      };
      const touched = () => preview();
      render();
      preview();

      const onValue = (e: Event, structural: boolean) => {
        const el = e.target as HTMLInputElement | HTMLSelectElement;
        const k = el.dataset.k;
        if (!k) return;
        if (k === "any") {
          const g = el.closest<HTMLElement>("[data-gpath]");
          if (g) group(g.dataset.gpath!).any = el.value === "1";
          if (g) placeSegThumb(g);
          touched();
          return;
        }
        const row = el.closest<HTMLElement>(".spl-row[data-path]");
        if (!row) return;
        const { list, i } = locate(row.dataset.path!);
        const n = list[i];
        if (n.kind !== "cond") return;
        if (k === "field") list[i] = splDefault(Number(el.value));
        else if (k === "op") {
          const op = el.value as SplOp;
          // 「以内」に変えたら数は 1 から。戻したら日付に戻す
          const wasLast = n.op === "inLast" || n.op === "notInLast";
          const isLast = op === "inLast" || op === "notInLast";
          if (wasLast !== isLast && splType(n.field) === "date") n.a = n.b = isLast ? "1" : splToday();
          n.op = op;
        } else if (k === "unit") n.unit = Number(el.value);
        else if (k === "a" || k === "b") n[k] = el.value;
        if (structural && (k === "field" || k === "op")) {
          render();
          // 項目を変えたら、続けて値を入れられるようにする
          if (k === "field") rulesEl.querySelector<HTMLElement>(`.spl-row[data-path="${row.dataset.path}"] .spl-val [data-k=a]`)?.focus();
        }
        touched();
      };
      rulesEl.addEventListener("change", (e) => onValue(e, true));
      rulesEl.addEventListener("input", (e) => {
        const k = (e.target as HTMLElement).dataset.k;
        if (k === "a" || k === "b") onValue(e, false);
      });
      rulesEl.addEventListener("click", (e) => {
        const t = e.target as HTMLElement;
        // 星
        const star = t.closest<HTMLElement>(".spl-star");
        if (star) {
          const row = star.closest<HTMLElement>(".spl-row[data-path]")!;
          const { list, i } = locate(row.dataset.path!);
          const n = list[i];
          if (n.kind === "cond") {
            const k = star.dataset.k as "a" | "b";
            const v = String(Number(star.dataset.star));
            n[k] = n[k] === v ? "0" : v;
            render();
            touched();
          }
          return;
        }
        const btn = t.closest<HTMLButtonElement>("[data-act]");
        if (!btn) return;
        const act = btn.dataset.act;
        let focusPath: string | null = null;
        if (act === "remove") {
          const row = btn.closest<HTMLElement>("[data-path]")!;
          const { list, i } = locate(row.dataset.path!);
          list.splice(i, 1);
          // 中が空になったグループは消す
          const parent = row.parentElement?.closest<HTMLElement>(".spl-group");
          if (parent && !list.length) {
            const up = locate(parent.dataset.path!);
            up.list.splice(up.i, 1);
          }
          if (!root.items.length) root.items.push(splDefault(0x04));
        } else {
          const gpath = btn.closest<HTMLElement>("[data-gpath]")!.dataset.gpath!;
          const g = group(gpath);
          g.items.push(act === "group" ? { kind: "group", any: false, items: [splDefault(0x04)] } : splDefault(0x04));
          focusPath = (gpath ? `${gpath}.` : "") + String(g.items.length - 1);
        }
        render();
        touched();
        // 足した行を見えるところに出して、項目を選べるようにする
        if (focusPath !== null) {
          const added = rulesEl.querySelector<HTMLElement>(`[data-path="${focusPath}"]`);
          added?.scrollIntoView({ block: "nearest", behavior: reduceMotion ? "auto" : "smooth" });
          added?.querySelector<HTMLElement>("[data-k=field]")?.focus({ preventScroll: true });
        }
      });

      // 対象
      m.querySelectorAll<HTMLElement>("[data-media]").forEach((b) =>
        b.addEventListener("click", () => {
          media = b.dataset.media as SplMedia;
          m.querySelectorAll<HTMLElement>("[data-media]").forEach((x) => {
            x.classList.toggle("on", x === b);
            x.setAttribute("aria-checked", String(x === b));
          });
          touched();
        }),
      );
      // ひな形 (名前を変えていなければ名前も替える)
      m.querySelectorAll<HTMLElement>("[data-tpl]").forEach((b) =>
        b.addEventListener("click", () => {
          const t = SPL_TEMPLATES[Number(b.dataset.tpl)];
          root.any = false;
          root.items = t.rules();
          Object.assign(prefs, { checkLimits: false, limitType: 3, limitValue: 25, limitSort: 2, reverseSort: false }, t.prefs ?? {});
          q<HTMLInputElement>("#spl-lim-on").checked = prefs.checkLimits;
          q<HTMLInputElement>("#spl-lim-val").value = String(prefs.limitValue);
          q<HTMLSelectElement>("#spl-lim-type").value = String(prefs.limitType);
          q<HTMLSelectElement>("#spl-lim-sort").value = sortValue(prefs.limitSort, prefs.reverseSort);
          q<HTMLElement>("#spl-limit").classList.toggle("hidden", !prefs.checkLimits);
          if (!nameTouched) nameEl.value = tr(t.name);
          m.querySelectorAll<HTMLElement>("[data-tpl]").forEach((x) => x.classList.toggle("on", x === b));
          render();
          touched();
        }),
      );
      nameEl.addEventListener("input", () => (nameTouched = true));

      const readPrefs = () => {
        if (showRulesSwitch) {
          prefs.checkRules = q<HTMLInputElement>("#spl-rules-on").checked;
          rulesEl.classList.toggle("off", !prefs.checkRules);
        }
        prefs.checkLimits = q<HTMLInputElement>("#spl-lim-on").checked;
        q<HTMLElement>("#spl-limit").classList.toggle("hidden", !prefs.checkLimits);
        prefs.limitValue = Math.max(1, Math.round(Number(q<HTMLInputElement>("#spl-lim-val").value) || 1));
        prefs.limitType = Number(q<HTMLSelectElement>("#spl-lim-type").value);
        const [s, r] = q<HTMLSelectElement>("#spl-lim-sort").value.split(":");
        prefs.limitSort = Number(s);
        prefs.reverseSort = r === "1";
        prefs.liveUpdate = q<HTMLInputElement>("#spl-live").checked;
        touched();
      };
      for (const sel of ["#spl-rules-on", "#spl-lim-on", "#spl-lim-val", "#spl-lim-type", "#spl-lim-sort", "#spl-live"]) {
        const el = m.querySelector<HTMLElement>(sel);
        el?.addEventListener("change", readPrefs);
        el?.addEventListener("input", readPrefs);
      }

      let saving = false;
      const save = async () => {
        if (saving) return;
        const name = nameEl.value.trim();
        if (!name) {
          nameEl.focus();
          return;
        }
        saving = true;
        const restore = busySave(okBtn);
        const s = await call<Snapshot>("save_smart_playlist", { id: id ?? null, name, def: currentDef() });
        saving = false;
        if (!s) {
          restore();
          return;
        }
        await restore.settle();
        applySnapshot(s);
        const created = id ? null : s.playlists.find((p) => !oldIds.has(p.id));
        close();
        if (created) selectView(created.id);
      };
      okBtn.addEventListener("click", save);
      nameEl.addEventListener("keydown", (e) => isEnter(e) && save());
      q<HTMLElement>('[data-x="cancel"]').addEventListener("click", close);
      if (!id) {
        nameEl.focus();
        nameEl.select();
      }
    },
    "smart-modal",
    false,
  );
}

/** サイドバーの「＋」: 普通のプレイリストかスマートプレイリストか */
export function openNewPlaylistMenu(btn: HTMLElement) {
  const r = btn.getBoundingClientRect();
  const pls = state.snap?.playlists ?? [];
  const smart = pls.filter((p) => p.smart);
  // 普通のプレイリスト (スマートプレイリストは条件ごと XML で。ライブラリ全体とポッドキャストは外す)
  const plain = pls.filter((p) => !p.master && !p.smart && !p.podcast);
  showMenu(r.left, r.bottom + 4, [
    { label: tr("新規プレイリスト…"), action: () => newPlaylist() },
    { label: tr("新規スマートプレイリスト…"), action: () => openSmartEditor() },
    { sep: true },
    { label: tr("プレイリストを取り込む…"), action: () => importPlaylists() },
    {
      label: tr("プレイリストをすべて .m3u8 で書き出す…"),
      disabled: !plain.length,
      action: () => exportM3u8(plain.map((p) => p.id)),
    },
    {
      label: tr("スマートプレイリストを iTunes 用に書き出す…"),
      disabled: !smart.length,
      action: () => exportSmartXml(smart.map((p) => p.id)),
    },
  ]);
}
