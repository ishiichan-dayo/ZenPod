// アートワークをまとめて探す (配布リポジトリの Issue #17)。選んだ曲のアルバムを 1 つずつ出し、ネットの候補から選んでいく。
// 一番よさそうな候補 (検索の 1 番目) を選んだ状態で出すので、よければ Enter で決めて次へ進める。
// 書き込みは裏で順に行い (待たずに次のアルバムへ)、次のアルバムは見ている間に先に探しておく

import { t as tr } from "../../i18n";
import { albumArtistName, albumKey } from "../../domain/track-order";
import type { Snapshot, Track } from "../../domain/types";
import { esc, isEnter } from "../dom";
import { call, toast } from "../feedback";
import { ICON_CLOSE } from "../icons";
import { modal } from "../modal";
import { findTrack } from "../player";
import { applySnapshot } from "../render";
import {
  artDestNote,
  bindShrinkOnSet,
  pcPathsOf,
  refreshPcArtwork,
  renderCandidates,
  sameAlbumIds,
  searchArtworkOnline,
  searchStatus,
  shrinkOnSet,
  shrinkOnSetHtml,
  skeletonCards,
} from "./artwork-search";
import type { ArtCandidate, SearchResult } from "./artwork-search";

interface AlbumJob {
  /** そのアルバムの曲 (選んでいない曲も含めて、アルバム全体に付ける) */
  ids: string[];
  album: string;
  artist: string;
  hasArt: boolean;
  /** 検索の経過と、それまでに届いた候補 (先に探しているあいだも覚えておき、そのアルバムを出したら見せる) */
  progress?: string;
  found?: ArtCandidate[];
  /** 決めた候補 (「戻る」で戻ったら、それを選んだ状態で出す) */
  chosen?: string;
}

/** 選んだ曲が 2 つ以上のアルバムにまたがるか (「アルバムごとにネットで探す…」を出すか) */
export function spansAlbums(ids: string[]): boolean {
  const keys = new Set<string>();
  for (const id of ids) {
    const t = findTrack(id);
    if (t?.album) keys.add(albumKey(t));
    if (keys.size > 1) return true;
  }
  return false;
}

/** 選んだ曲をアルバムに分ける。アルバム名の無い曲は探しようがないので数だけ返す */
function albumJobs(ids: string[]): { jobs: AlbumJob[]; noAlbum: number } {
  const seen = new Set<string>();
  const jobs: AlbumJob[] = [];
  let noAlbum = 0;
  for (const id of ids) {
    if (seen.has(id)) continue;
    const t = findTrack(id);
    if (!t) continue;
    if (!t.album) {
      seen.add(id);
      noAlbum++;
      continue;
    }
    const group = sameAlbumIds(t);
    group.forEach((x) => seen.add(x));
    seen.add(id);
    const tracks = group.map((x) => findTrack(x)).filter((x): x is Track => !!x);
    // PC の曲は埋め込みの画像の有無 (まだ読んでいない曲は無いものとして扱う)、iPod の曲は ArtworkDB の画像
    const hasArt = tracks.some((x) => (x.pcPath ? x.hasPicture === true : x.artworkId > 0));
    jobs.push({ ids: group, album: t.album, artist: albumArtistName(tracks) ?? (t.albumArtist || t.artist), hasArt });
  }
  return { jobs, noAlbum };
}

export async function searchArtworkBulk(ids: string[]) {
  const { jobs, noAlbum } = albumJobs(ids);
  if (!jobs.length) return toast(tr("アルバム名のある曲を選んでください"));
  const pc = !!findTrack(ids[0])?.pcPath;
  // ジャケットの無いアルバムだけを順に出す (全部にあるときは全部)
  let withArt = jobs.every((j) => j.hasArt);
  /** 決めた・スキップしたアルバム (チェックを切り替えても、もう出さない) */
  const handled = new Set<AlbumJob>();
  /** これから出すアルバム (今出しているものを含む) */
  const queue = () => jobs.filter((j) => !handled.has(j) && (withArt || !j.hasArt));
  let cur: AlbumJob | null = null;
  // 書き込み: 裏で 1 つずつ。終わった数と失敗した数を数え、全部終わったら知らせる
  let pending = 0;
  /** 書き込めたアルバム (戻って選び直したアルバムも 1 つと数える) */
  const applied = new Set<AlbumJob>();
  let chain: Promise<void> = Promise.resolve();
  let finished = false;
  const searches = new Map<AlbumJob, Promise<SearchResult | null>>();
  let paintProgress = (_j: AlbumJob) => {};
  const run = (j: AlbumJob, artist: string, album: string) =>
    searchArtworkOnline(artist, album, (m, found) => {
      j.progress = m;
      if (found?.length) j.found = found;
      paintProgress(j);
    });
  const searchOf = (j: AlbumJob, artist = j.artist, album = j.album) => {
    if (artist !== j.artist || album !== j.album) return run(j, artist, album);
    if (!searches.has(j)) searches.set(j, run(j, artist, album));
    return searches.get(j)!;
  };

  let paintStatus = () => {};
  const report = () => {
    if (!finished || pending) return;
    if (applied.size) toast(tr("{n} アルバムのアートワークを設定しました", { n: applied.size.toLocaleString() }));
  };
  const write = (j: AlbumJob, c: ArtCandidate) => {
    pending++;
    paintStatus();
    chain = chain.then(async () => {
      const url = c.fullUrl;
      if (pc) {
        const paths = pcPathsOf(j.ids);
        const n = await call<number>("apply_pc_artwork_url", { paths, url, shrink: shrinkOnSet(j.ids) });
        refreshPcArtwork(paths, n !== null);
        if (n !== null) applied.add(j);
      } else {
        const s = await call<Snapshot>("apply_artwork_url", { ids: j.ids, url, shrink: shrinkOnSet(j.ids) });
        if (s) {
          applySnapshot(s);
          applied.add(j);
        }
      }
      pending--;
      paintStatus();
      report();
    });
  };

  await modal(
    `<div class="ab-top"><h3>${esc(tr("アルバムごとにアートワークを探す"))}</h3><button class="btn icon ghost" data-x="cancel" title="${esc(tr("閉じる"))} (Esc)" aria-label="${esc(tr("閉じる"))}">${ICON_CLOSE}</button></div>
     <div class="ab-head"><span class="ab-pos"></span><span class="ab-name"></span></div>
     <div class="as-bar">
       <input type="text" id="as-artist" placeholder="${esc(tr("アーティスト"))}">
       <input type="text" id="as-album" placeholder="${esc(tr("アルバム"))}">
       <button class="btn" id="as-go">${esc(tr("検索"))}</button>
     </div>
     <div id="as-info" class="as-info"></div>
     <div id="as-grid" class="as-grid" tabindex="-1"></div>
     <div class="hint">${esc(tr("iTunes・Deezer・MusicBrainz から探します（アーティスト名とアルバム名が各サービスに送信されます）。"))} ${esc(
       tr("一番よさそうな候補を選んであります。← → で選び直し、Enter で決めて次のアルバムへ進みます。"),
     )} ${esc(artDestNote(pc, false))}${noAlbum ? ` ${esc(tr("アルバム名の無い {n} 曲は飛ばします。", { n: noAlbum.toLocaleString() }))}` : ""}</div>
     ${shrinkOnSetHtml(pc, false)}
     <div class="modal-actions">
       <label class="check as-scope"><input type="checkbox" id="ab-all" ${withArt ? "checked" : ""}> ${esc(tr("設定済みのアルバムも探し直す"))}</label>
       <span class="ab-status muted"></span>
       <span class="spacer"></span>
       <button class="btn" data-x="back" disabled>${esc(tr("戻る"))}</button>
       <button class="btn" data-x="skip">${esc(tr("スキップ"))}</button>
       <button class="btn primary" data-x="ok" disabled>${esc(tr("これにする"))}</button>
     </div>`,
    (root, close) => {
      const q = <T extends HTMLElement>(sel: string) => root.querySelector<T>(sel)!;
      const grid = q<HTMLElement>("#as-grid");
      const infoEl = q<HTMLElement>("#as-info");
      const okBtn = q<HTMLButtonElement>('[data-x="ok"]');
      const artistIn = q<HTMLInputElement>("#as-artist");
      const albumIn = q<HTMLInputElement>("#as-album");
      const allBox = q<HTMLInputElement>("#ab-all");
      bindShrinkOnSet(root, pc);
      let selected: ArtCandidate | null = null;
      /** 自分で候補を選んだか (選んでいなければ、届いた候補の 1 番目を選び直す) */
      let picked = false;
      let generation = 0;
      let gen = 0;
      /** 届いた候補を並べる */
      const paintFound = (list: ArtCandidate[], searching = false) => {
        if (!picked || !list.some((c) => c.fullUrl === selected?.fullUrl)) {
          const before = list.find((c) => c.fullUrl === cur?.chosen);
          picked = !!before;
          selected = before ?? list[0] ?? null;
        }
        okBtn.disabled = !selected;
        const first = !grid.querySelector(".as-card");
        void renderCandidates(
          grid,
          list,
          () => gen === generation,
          (c) => {
            selected = c;
            picked = true;
            okBtn.disabled = false;
          },
          selected?.fullUrl,
          searching,
        );
        // 最初に並んだら、Enter で決められるよう候補の一覧へ (検索語を打っている途中なら奪わない)
        if (first && !(document.activeElement instanceof HTMLInputElement)) grid.focus();
      };
      paintProgress = (j) => {
        if (j !== cur) return;
        if (j.found?.length) paintFound(j.found, true);
        if (j.progress) searchStatus(infoEl, j.progress, true);
      };
      paintStatus = () => {
        q<HTMLElement>(".ab-status").textContent = pending ? tr("書き込み中… 残り {n} アルバム", { n: pending }) : "";
      };

      const show = async (j: AlbumJob, artist = j.artist, album = j.album) => {
        gen = ++generation;
        const mine = gen;
        cur = j;
        selected = null;
        picked = false;
        okBtn.disabled = true;
        paintPos();
        q<HTMLElement>(".ab-name").textContent = [j.album, j.artist].filter(Boolean).join(" — ");
        artistIn.value = artist;
        albumIn.value = album;
        grid.innerHTML = skeletonCards();
        grid.dataset.key = "";
        searchStatus(infoEl, tr("検索中…"), true);
        // 先に探していれば、そこまでの経過と届いた候補から出す
        if (artist === j.artist && album === j.album && searches.has(j)) paintProgress(j);
        else {
          j.progress = undefined;
          j.found = undefined;
        }
        const res = await searchOf(j, artist, album);
        if (mine !== generation) return;
        searchStatus(infoEl, "", false);
        // 見ている間に、次のアルバムを先に探しておく
        const next = queue().find((x) => x !== j);
        if (next) void searchOf(next);
        const cands = res?.candidates ?? [];
        if (res?.artistAliases.length) infoEl.textContent = tr("別名でも検索: {a}", { a: res.artistAliases.join(tr("、")) });
        if (!cands.length) {
          grid.innerHTML = `<div class="as-empty">${esc(tr("見つかりませんでした。アーティスト名やアルバム名を変えて検索するか、スキップしてください。"))}</div>`;
          grid.focus();
          return;
        }
        paintFound(cands);
      };

      /** 何番目か: 片付けた数 + 1 / 片付けた数 + 残り (今のアルバムを含む) */
      const paintPos = () => {
        const left = queue().length + (cur && !queue().includes(cur) ? 1 : 0);
        q<HTMLElement>(".ab-pos").textContent = tr("{a} / {n}", { a: (handled.size + 1).toLocaleString(), n: (handled.size + left).toLocaleString() });
      };

      /** 片付けた順 (「戻る」で 1 つ前のアルバムをもう一度出す) */
      const history: AlbumJob[] = [];
      const backBtn = q<HTMLButtonElement>('[data-x="back"]');

      /** 次のアルバムへ (`done` は今のアルバムを片付けたか)。無ければ閉じる */
      const advance = (done: boolean) => {
        if (cur && done) {
          handled.add(cur);
          history.push(cur);
        }
        backBtn.disabled = !history.length;
        const next = queue().find((x) => x !== cur);
        if (next) return void show(next);
        generation++;
        close();
      };

      /** 1 つ前のアルバムに戻る。決めていたアルバムも選び直せる (選び直したら、その画像で書き直す) */
      const back = () => {
        const prev = history.pop();
        if (!prev) return;
        handled.delete(prev);
        backBtn.disabled = !history.length;
        void show(prev);
      };

      allBox.addEventListener("change", () => {
        withArt = allBox.checked;
        // 今のアルバムを外したら次へ (片付けたことにはしない。オンに戻せばまた出る)。
        // ほかに出すアルバムが無ければ、閉じずに今のアルバムを出したままにする
        if (cur && !queue().includes(cur) && queue().some((x) => x !== cur)) advance(false);
        else paintPos();
        // 続けて Enter で決められるよう、候補の一覧へ戻す
        grid.focus();
      });
      okBtn.addEventListener("click", () => {
        if (!cur || !selected) return;
        // 戻って同じ候補のまま決め直したときは、書き込み済みなら書き直さない
        if (!(cur.chosen === selected.fullUrl && applied.has(cur))) write(cur, selected);
        cur.chosen = selected.fullUrl;
        advance(true);
      });
      q('[data-x="skip"]').addEventListener("click", () => advance(true));
      backBtn.addEventListener("click", back);
      q('[data-x="cancel"]').addEventListener("click", () => {
        generation++;
        close();
      });
      const research = () => cur && void show(cur, artistIn.value, albumIn.value);
      q("#as-go").addEventListener("click", research);
      [artistIn, albumIn].forEach((el) => el.addEventListener("keydown", (e) => isEnter(e) && research()));
      // 候補の上では ← → で選び直し、Enter で決める (入力欄とボタンの上ではふだんどおり)
      root.addEventListener("keydown", (e) => {
        const t = e.target as HTMLElement;
        if (t.closest("input, textarea, select") || (t.closest("button") && !t.closest(".as-card"))) return;
        if (isEnter(e)) {
          e.preventDefault();
          okBtn.click();
          return;
        }
        if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
        const cards = [...grid.querySelectorAll<HTMLElement>(".as-card")];
        const i = cards.findIndex((c) => c.classList.contains("selected"));
        const n = cards[Math.max(0, Math.min(cards.length - 1, i + (e.key === "ArrowRight" ? 1 : -1)))];
        if (!n) return;
        e.preventDefault();
        n.click();
        n.scrollIntoView({ block: "nearest" });
        grid.focus();
      });
      advance(false);
    },
    "wide art-pick",
  );
  finished = true;
  report();
}
