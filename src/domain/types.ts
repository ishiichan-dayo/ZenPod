// 画面とバックエンド (zenpod-app) がやり取りする型。名前と形は Rust 側の serde (camelCase) に合わせる


export interface Track {
  id: string;
  title: string;
  artist: string;
  album: string;
  albumArtist: string;
  genre: string;
  durationMs: number;
  trackNr: number;
  discNr: number;
  year: number;
  size: number;
  bitrate: number;
  playCount: number;
  rating: number;
  filetype: string;
  dateAdded: number;
  artworkId: number;
  sortTitle: string;
  sortArtist: string;
  sortAlbum: string;
  sortAlbumArtist: string;
  composer: string;
  /** PC のフォルダの曲 (iPod の曲では undefined) */
  pcPath?: string;
  /** PC の曲のリリースの種類 (タグの MEDIATYPE など)。iPod の曲には無い */
  releaseType?: string;
  /** Rockbox モードの曲の、iPod のルートからの場所 ("/Music/..."。iPod OS の曲には無い) */
  rbPath?: string;
  /** PC の曲のタグを読めなかった理由 (読めた曲・iPod の曲には無い) */
  readError?: string;
  /** PC の曲にジャケットが埋め込まれているか (iPod の曲は artworkId で見る) */
  hasPicture?: boolean;
  /** 分類 (iPod の曲だけ)。Rockbox では置き場所 (Podcasts / Audiobooks) で決まる */
  kind?: MediaKind;
}

export type MediaKind = "music" | "podcast" | "audiobook";
// tr: 使う側で訳す
export const KIND_LABEL: Record<MediaKind, string> = { music: "音楽", podcast: "ポッドキャスト", audiobook: "オーディオブック" };

export interface Playlist {
  id: string;
  name: string;
  trackIds: string[];
  master: boolean;
  smart: boolean;
  podcast: boolean;
}

export interface DeviceInfo {
  root: string;
  modelNumber: string | null;
  modelName: string;
  firewireId: string | null;
  firewireIdSource: "sysInfoExtended" | "sysInfo" | "usb" | "manual" | null;
  usbProductId: number | null;
  checksum: "none" | "hash58" | "unsupported";
  /** iPod OS で写真を表示できる機種か (型番から) */
  photos: boolean;
  /** iPod OS で動画を再生できる機種か (型番から。分からなければ true) */
  video: boolean;
  /** iPod shuffle なら iTunesSD の形式 (v1 = 1G / 2G、v2 = 3G / 4G) */
  shuffle: "v1" | "v2" | null;
  totalBytes: number;
  freeBytes: number;
}

export interface Snapshot {
  device: DeviceInfo;
  name: string;
  tracks: Track[];
  playlists: Playlist[];
  writableError: string | null;
  /** 容量の内訳 (バイト)。使用量からこれらを引いた残りが「その他」 */
  usage: { music: number; photos: number; videos?: number; artwork: number };
  /** 曲の管理のしかた: iPod OS (iTunesDB) / Rockbox (iPod の中のファイルそのもの) */
  mode: LibMode;
  /** Rockbox モードのときの Rockbox の設定 */
  rockbox: {
    dbAutoupdate: boolean;
    /** アプリで Rockbox のデータベースを作れる版か */
    dbSupported: boolean;
    /** iPod にあるデータベースの曲の数。無ければ null */
    dbTracks: number | null;
    /** 最後にデータベースを作り直せなかった理由 */
    dbError: string | null;
    /** 入っている Rockbox の版 ("4.0" など) */
    version: string | null;
    /** 「自動で続きから再生」(Automatic Resume) がオンか */
    autoresume: boolean;
  } | null;
  /** Rockbox が入っている iPod か (.rockbox がある)。無ければモードの切り替えを出さない */
  hasRockbox: boolean;
  artworkSupported: boolean;
  /** Rockbox モードで開いたときの読み込みの内訳 (open_device の結果だけ) */
  scan?: { walkMs: number; readMs: number; files: number; reread: number; fromDb: number };
  /** 開いたときだけ: 開くのにかかった時間の内訳 (ミリ秒) と、Mac で Spotlight がこの iPod を索引しているか */
  times?: { probeMs: number; usbMs: number; spaceMs: number; dbMs: number; artMs: number; snapshotMs: number; spotlight: boolean | null };
}

export interface DeviceEntry {
  root: string;
  label: string;
}

export interface Progress {
  /** 転送し始めた曲のジャケット (data URL)。"working" のときだけ */
  art?: string | null;
  index: number;
  total: number;
  file: string;
  path: string;
  status: "working" | "converting" | "added" | "duplicate" | "replaced" | "error";
  message: string | null;
  /** 動画の変換の進み具合 (0〜1)。"converting" のときだけ */
  percent?: number;
}

export interface TransferSummary {
  added: number;
  duplicates: number;
  replaced: number;
  errors: string[];
  cancelled: boolean;
  saveError: string | null;
  /** 送った動画・写真の数 (added には入らない) */
  videos: number;
  photos: number;
}

export type SortKey =
  | "order"
  | "title"
  | "artist"
  | "album"
  | "albumArtist"
  | "genre"
  | "composer"
  | "year"
  | "trackNr"
  | "duration"
  | "size"
  | "bitrate"
  | "filetype"
  | "fileName"
  | "dateAdded"
  | "plays"
  | "rating"
  | "mediaKind"
  | "onIpod"
  | "artwork";

export type LibMode = "ipod" | "rockbox";

// ---------------- PC の音楽フォルダ ----------------

export interface PcTrack {
  path: string;
  title: string;
  artist: string;
  album: string;
  albumArtist: string;
  genre: string;
  composer: string;
  durationMs: number;
  trackNr: number;
  discNr: number;
  year: number;
  size: number;
  /** リリースの種類 ("album" / "single" など、小文字。不明なら空) */
  releaseType: string;
  /** 並べ替え用の読み (ファイルのソート用タグ)。古い版の読み取り結果には無い */
  sortTitle?: string;
  sortArtist?: string;
  sortAlbum?: string;
  sortAlbumArtist?: string;
  /** タグを読めなかった理由 (読めた曲には無い) */
  readError?: string;
  /** ジャケットが埋め込まれているか */
  hasPicture?: boolean;
}

export function pcTrackToTrack(p: PcTrack): Track {
  return {
    id: `pc:${p.path}`,
    title: p.title,
    artist: p.artist,
    album: p.album,
    albumArtist: p.albumArtist,
    genre: p.genre,
    durationMs: p.durationMs,
    trackNr: p.trackNr,
    discNr: p.discNr,
    year: p.year,
    size: p.size,
    bitrate: 0,
    playCount: 0,
    rating: 0,
    filetype: "",
    dateAdded: 0,
    artworkId: 0,
    sortTitle: p.sortTitle ?? "",
    sortArtist: p.sortArtist ?? "",
    sortAlbum: p.sortAlbum ?? "",
    sortAlbumArtist: p.sortAlbumArtist ?? "",
    composer: p.composer ?? "",
    pcPath: p.path,
    releaseType: p.releaseType ?? "",
    readError: p.readError,
    hasPicture: p.hasPicture,
  };
}
