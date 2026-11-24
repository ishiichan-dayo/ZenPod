// 動画と写真の型 (Rust の domain/video.rs・domain/photo.rs の serde に合わせる)

export type VideoKind = "movie" | "musicvideo" | "tvshow";

export interface VideoView {
  id: string;
  title: string;
  kind: VideoKind;
  artist: string;
  album: string;
  albumArtist: string;
  genre: string;
  composer: string;
  grouping: string;
  comment: string;
  year: number;
  trackNr: number;
  discNr: number;
  /** リリース日 (UNIX 秒、0 = なし) */
  released: number;
  /** 並べ替え用の読み (空 = 表記のまま) */
  sortTitle: string;
  sortArtist: string;
  sortAlbum: string;
  sortAlbumArtist: string;
  sortComposer: string;
  sortShow: string;
  /** テレビ番組の番組名 */
  show: string;
  season: number;
  episode: number;
  /** エピソード ID ("S1E02" など自由な文字) */
  episodeId: string;
  /** 放送局 */
  network: string;
  description: string;
  /** 評価 (0〜100、★ 1 つが 20) */
  rating: number;
  /** シャッフルで飛ばす */
  skipShuffle: boolean;
  /** 再生した位置を覚える */
  rememberPosition: boolean;
  /** 音量の調整 (-255〜255) */
  volume: number;
  /** 再生を始める・止める位置 (ミリ秒。止める位置の 0 は最後まで) */
  startMs: number;
  stopMs: number;
  /** イコライザのプリセットの番号 (0 = なし) */
  eq: number;
  durationMs: number;
  size: number;
  dateAdded: number;
  /** ArtworkDB の画像 id (0 = なし) */
  artworkId: number;
  /** アートワークを利用者が選んだ (一覧にも 1 コマではなくこの画像を出す) */
  customArt: boolean;
  playCount: number;
  lastPlayed: number;
  dateModified: number;
  /** iTunesDB の種類の文字・ビットレート・サンプルレート・iPod の中の場所 (iPod OS だけ) */
  filetype: string;
  bitrate: number;
  sampleRate: number;
  ipodPath: string;
  /** Rockbox の動画の、iPod のルートからの場所 */
  rbPath?: string;
}

export interface PhotoView {
  id: string;
  width: number;
  height: number;
  /** 撮った日時 (UNIX 秒、0 = 不明) */
  date: number;
  size: number;
  /** ファイル名 (Rockbox だけ) */
  name?: string;
}

export interface PhotoAlbum {
  id: string;
  name: string;
  /** アルバムに直接入っている写真 */
  photoIds: string[];
  /** 親のアルバム (Rockbox のフォルダの階層だけ) */
  parent?: string;
}

export interface PhotoLibrary {
  photos: PhotoView[];
  albums: PhotoAlbum[];
  /** 写真を置けるか (iPod OS で写真を表示できない機種は false) */
  supported: boolean;
}

/** 動画の情報の編集 (無い項目は変えない。文字の空欄は消す) */
export interface VideoEdit {
  title?: string;
  kind?: VideoKind;
  artist?: string;
  album?: string;
  albumArtist?: string;
  genre?: string;
  composer?: string;
  year?: number;
  trackNr?: number;
  discNr?: number;
  sortTitle?: string;
  sortArtist?: string;
  sortAlbum?: string;
  sortAlbumArtist?: string;
  sortComposer?: string;
  sortShow?: string;
  grouping?: string;
  comment?: string;
  /** リリース日 (UNIX 秒、0 で消す) */
  released?: number;
  show?: string;
  season?: number;
  episode?: number;
  episodeId?: string;
  network?: string;
  description?: string;
  rating?: number;
  skipShuffle?: boolean;
  rememberPosition?: boolean;
  volume?: number;
  startMs?: number;
  stopMs?: number;
  /** イコライザ (0 で外す) */
  eq?: number;
  /** 再生回数と最後に再生した日時を 0 に戻す */
  resetPlayCount?: boolean;
}

/** 動画のファイルの中身 (情報の編集の「ファイル」のページ) */
export interface VideoFileInfo {
  video: string;
  width: number;
  height: number;
  fps: number;
  audio: string;
  audioRate: number;
  channels: number;
}

/** iTunes のイコライザのプリセット (iTunesDB には "#!#番号#!#" で書く)。名前は iTunes の英語のまま */
export const EQ_PRESETS: [number, string][] = [
  [100, "Acoustic"],
  [101, "Bass Booster"],
  [102, "Bass Reducer"],
  [103, "Classical"],
  [104, "Dance"],
  [105, "Deep"],
  [106, "Electronic"],
  [107, "Flat"],
  [108, "Hip-Hop"],
  [109, "Jazz"],
  [110, "Latin"],
  [111, "Loudness"],
  [112, "Lounge"],
  [113, "Piano"],
  [114, "Pop"],
  [115, "R&B"],
  [116, "Rock"],
  [117, "Small Speakers"],
  [118, "Spoken Word"],
  [119, "Treble Booster"],
  [120, "Treble Reducer"],
  [121, "Vocal Booster"],
];

/** 種類の表示の順と名前 */
// tr: 使う側で訳す
export const VIDEO_KINDS: [VideoKind, string][] = [
  ["movie", "ムービー"],
  ["tvshow", "テレビ番組"],
  ["musicvideo", "ミュージックビデオ"],
];

/** 一覧の並び: 種類 → (テレビ番組は番組・シーズン・話数、ミュージックビデオはアーティスト) → 題名。名前の無い番組・アーティストは最後 */
export function sortVideos(list: VideoView[]): VideoView[] {
  const order = (k: VideoKind) => VIDEO_KINDS.findIndex(([x]) => x === k);
  const coll = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  const group = (v: VideoView) => (v.kind === "tvshow" ? v.show : v.kind === "musicvideo" ? v.artist : "");
  return [...list].sort(
    (a, b) =>
      order(a.kind) - order(b.kind) ||
      Number(!group(a)) - Number(!group(b)) ||
      coll.compare(group(a), group(b)) ||
      a.season - b.season ||
      a.episode - b.episode ||
      coll.compare(a.title, b.title),
  );
}

/**
 * 「ビデオ」の左の一覧で選んでいるところ (iPod の「ビデオ」のメニューと同じ階層)。
 * "" = すべて、"k:<種類>"、"a:<アーティスト>" (ミュージックビデオ)、"s:<番組名>" (テレビ番組)。名前で持つので読み直しても変わらない
 */
export type VideoNode = string;

export function videoInNode(v: VideoView, node: VideoNode): boolean {
  const val = node.slice(2);
  if (node.startsWith("k:")) return v.kind === val;
  if (node.startsWith("a:")) return v.kind === "musicvideo" && v.artist === val;
  if (node.startsWith("s:")) return v.kind === "tvshow" && v.show === val;
  return true;
}

/** 種類の下に並べる名前 (ミュージックビデオはアーティスト、テレビ番組は番組)。名前の無いものは最後 */
export function videoGroups(list: VideoView[], kind: VideoKind): { name: string; count: number }[] {
  const by = new Map<string, number>();
  for (const v of list) {
    if (v.kind !== kind) continue;
    const name = kind === "tvshow" ? v.show : v.artist;
    by.set(name, (by.get(name) ?? 0) + 1);
  }
  const coll = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  return [...by].map(([name, count]) => ({ name, count })).sort((a, b) => Number(!a.name) - Number(!b.name) || coll.compare(a.name, b.name));
}

/** 検索に合うか (題名・番組・アーティスト) */
export function videoMatches(v: VideoView, query: string): boolean {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return true;
  const hay = `${v.title} ${v.show} ${v.artist}`.toLowerCase();
  return terms.every((w) => hay.includes(w));
}
