// 画面の状態 (アプリ全体で 1 つ) と、一覧の行の高さなどの決まった値

import { getVersion } from "../infra/tauri";
import type { DeviceEntry, LibMode, Snapshot, SortKey, Track } from "../domain/types";

export const AUDIO_EXTS = ["mp3", "m4a", "m4b", "mp4", "wav", "aif", "aiff", "flac", "ape", "wv", "ogg", "oga", "opus", "wma", "aac"];

export const state = {
  snap: null as Snapshot | null,
  view: "library" as string, // "library" or playlist id
  sort: { key: "artist" as SortKey, asc: true },
  search: "",
  rows: [] as Track[],
  trackMap: new Map<string, Track>(),
  selected: new Set<string>(),
  anchor: -1,
  /** クリックした行 (カーソル)。チェックはしない。チェックが無いときは、この行が操作の対象になる */
  focused: null as string | null,
  /** Shift で範囲をチェックするときの、範囲より前からのチェック (範囲を縮めたら外せるように) */
  checkBase: new Set<string>(),
  transferring: false,
  /** プレイリストの並び替えのパネルで編集中の下書き (配布リポジトリの Issue #22)。保存するまで iPod には書かない。
   * sort は選んだ並べ方 (ドラッグで動かしたら null)、keep は「あとから足した曲もこの並べ方で入れる」 */
  reorder: null as { id: string; ids: string[]; sort: { key: SortKey; asc: boolean } | null; keep: boolean; dirty: boolean } | null,
  /** 読み込み中のモード (切り替え・開き直し)。読み込み中でなければ null */
  openingMode: null as LibMode | null,
  /** 読み込み中にタグを読み直している曲の数 (初めて Rockbox モードで開くとき) */
  openProgress: null as [number, number] | null,
  /** 初めて開く途中で、読めた曲だけの仮の一覧を出している (state.snap は仮のもの。変更・再生はできない) */
  partialOpen: false,
  /** 仮の一覧は前回開いたときのもの (Rockbox。iPod の中をたどり終わるまで) */
  partialCached: false,
  /** 転送中に新しく入った曲の数 (サイドバーの「ミュージック」の曲数に足して、転送の終わりを待たずに増やす) */
  transferAdded: 0,
  /** チェックしてから転送した PC の曲 (転送できたものは、終わったらチェックを外す) */
  transferChecked: new Set<string>(),
  ejectedRoot: null as string | null,
  /** 自動オープンに失敗した iPod (手動で開くまで再試行しない) */
  failedRoot: null as string | null,
  opening: false,
  /** 見つかった iPod (未接続の案内に「開く」ボタンを出す) */
  devices: [] as DeviceEntry[],
  /** 起動後、iPod を 1 回探し終えたか (それまでは「探しています」と出す) */
  connectChecked: false,
  /** iPod が USB でつながった時刻 (ドライブとして見えるまで「準備を待っています」と出す)。0 = つながっていない */
  usbSince: 0,
  /** USB で認識したのにドライブが出ない理由 (Mac だけ。"fsck" ディスクを検査中 / "denied" 読む許可が無い / "none") */
  driveWaitReason: null as string | null,
  /** 開発版の Ctrl + Shift + D: USB で認識したがドライブが出ない状態を真似る (案内の見た目の確認用) */
  devNoDrive: false,
  /** 開発版の Ctrl + Shift + S: 読み込み中の表示 (スケルトン) を出したままにする (見た目の確認用) */
  devSkeleton: false,
  /** 取り出したあと、まだ USB につながっている (外されるまで「準備を待っています」と出さない) */
  usbEjected: false,
  /** pollDevices の実行中 (早めの確認と定期の確認が重ならないように) */
  polling: false,
  /** はじめの案内を出している間 (iPod を見つけても、案内を閉じるまで読み込まない) */
  onboarding: false,
  /** 転送が終わったら続けて行うこと (プレイリストの取り込みで、見つからない曲を先に送ったとき) */
  afterTransfer: null as (() => Promise<void>) | null,
  /** 取り出しの完了待ち中 (ドライブが消えても「取り外されました」と扱わない) */
  ejecting: false,
  /** 設定ページを表示中 */
  settingsOpen: false,
  /** 転送中の PC の曲 (場所 → 転送中 / 終わった)。転送が終わって iPod の曲を読み直したら空にする */
  sending: new Map<string, "working" | "done">(),
  /** 一覧の表示: 曲ごと / アルバムごと (ジャケット) */
  layout: "tracks" as Layout,
  /** アルバムの一覧で、矢印キーで選んでいるカードの番号 (-1 = なし。マウスで操作したら消す) */
  gridFocus: -1,
  /** アーティスト表示で選んでいるアーティスト (albumArtist || artist)。null = 先頭 */
  artist: null as string | null,
  /** アーティスト表示: 左の一覧 (検索で絞り込んだあと)、右の行の位置 */
  artists: [] as ArtistEntry[],
  rowLayout: null as RowLayout | null,
  /** ヘッダーに曲数を出しているか (合計時間・容量を出すかの判定) */
  countShown: false,
  /** アルバム表示で開いているアルバム (albumKey)。null = アルバムの一覧 */
  openAlbum: null as string | null,
  /** 設定ページで選んでいる項目 */
  settingsCat: "general" as SettingsCat,
  /** PC の音楽フォルダ: 登録一覧・読み取り済みの曲・読み取り中のフォルダ */
  pc: {
    /** 登録したフォルダ (起動時に設定から読む) */
    folders: [] as string[],
    tracks: new Map<string, Track[]>(),
    /** 曲 ID ("pc:<場所>") → 曲 (再生・編集用) */
    byId: new Map<string, Track>(),
    scanning: null as string | null,
    progress: "",
  },
};

export const ROW_H = 30;
/** アーティスト表示の見出しの高さ: アーティスト名 / アルバム */
export const ARTIST_HEAD_H = 64;
/** アルバムの見出しは下寄せ。下の余白 (16px) を広げたら、上の余白が変わらないようここも増やす */
export const ALBUM_HEAD_H = 76;
/** アーティスト表示で、前のアルバムの最後の曲と次のアルバムの見出しの間 */
export const ALBUM_GAP = 16;

export type Layout = "tracks" | "albums" | "artists";

interface ArtistEntry {
  key: string;
  tracks: Track[];
}

/**
 * アーティスト表示の行の位置。行と行の間にアルバムの見出しが入るので、行ごとの top を持つ。
 * blocks は見出し (アーティスト名・アルバム) で、行ではない (クリックしても行の選択にならない)
 */
export interface RowLayout {
  tops: number[];
  height: number;
  blocks: { top: number; h: number; kind: "artist" | "album"; tracks: Track[] }[];
}
export const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

/**
 * 設定ページ。変更はその場で反映する (保存ボタンなし)。
 * FireWire GUID だけは iPod の保存に関わるので「適用」を押したときに反映する
 */
export type SettingsCat = "general" | "transfer" | "convert" | "yomi" | "ipod" | "about" | "debug";


export let appVersion = "";

export async function loadAppVersion() {
  appVersion = await getVersion().catch(() => "");
}
