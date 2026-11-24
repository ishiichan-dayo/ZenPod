// 動画と写真の状態 (iPod から読んだ一覧・表示中のアルバム・選んだもの) と、読み込み

import { invoke } from "../infra/tauri";
import { state } from "./state";
import { videoInNode } from "../domain/visual";
import type { PhotoAlbum, PhotoLibrary, VideoNode, VideoView } from "../domain/visual";
import { getStored } from "../infra/prefs";

export const vis = {
  /** iPod の動画 (まだ読んでいなければ null) */
  videos: null as VideoView[] | null,
  /** iPod の写真とアルバム (まだ読んでいなければ null) */
  photos: null as PhotoLibrary | null,
  /** 写真の表示で選んでいるアルバム (null = すべての写真) */
  album: null as string | null,
  /** ビデオの表示で左の一覧から選んでいるところ ("" = すべてのビデオ) */
  videoNode: "" as VideoNode,
  /** 一覧でチェックしたもの (表示中の画面の中だけ) */
  selected: new Set<string>(),
  /** Shift で範囲を選ぶときの起点 */
  anchor: null as string | null,
  /** 起点を置いたときのチェック (Shift での範囲チェックは、これに範囲を足したもの。曲の一覧と同じ) */
  checkBase: new Set<string>(),
  /** カーソル (クリックしたもの。チェックとは別) */
  focused: null as string | null,
  /** 読み込み中 */
  loadingVideos: false,
  loadingPhotos: false,
  /** 読んだ iPod (別の iPod・モードに替わったら読み直す) */
  loadedFor: "",
};

export const VIDEO_VIEW = "videos";
export const PHOTO_VIEW = "photos";

export function isVideoView(): boolean {
  return state.view === VIDEO_VIEW;
}
export function isPhotoView(): boolean {
  return state.view === PHOTO_VIEW;
}
/** 曲の一覧の代わりに動画か写真を出している */
export function isVisualView(): boolean {
  return isVideoView() || isPhotoView();
}

function deviceKey(): string {
  const s = state.snap;
  return s ? `${s.device.root}|${s.mode}` : "";
}

/** iPod が替わったら (閉じた・別の iPod・モードの切り替え) 一覧を捨てる。替わっていれば true */
export function resetVisualIfChanged(): boolean {
  const key = deviceKey();
  if (key === vis.loadedFor) return false;
  vis.loadedFor = key;
  vis.videos = null;
  vis.photos = null;
  vis.album = null;
  vis.videoNode = "";
  vis.selected.clear();
  vis.anchor = null;
  return true;
}

export async function loadVideos(): Promise<void> {
  if (!state.snap || vis.loadingVideos) return;
  vis.loadingVideos = true;
  const key = deviceKey();
  try {
    const list = await invoke<VideoView[]>("list_videos");
    if (key === deviceKey()) setVideos(list);
  } catch {
    if (key === deviceKey()) vis.videos = [];
  } finally {
    vis.loadingVideos = false;
  }
}

export async function loadPhotos(): Promise<void> {
  if (!state.snap || vis.loadingPhotos) return;
  vis.loadingPhotos = true;
  const key = deviceKey();
  try {
    const lib = await invoke<PhotoLibrary>("photo_library");
    if (key === deviceKey()) setPhotos(lib);
  } catch {
    if (key === deviceKey()) vis.photos = { photos: [], albums: [], supported: false };
  } finally {
    vis.loadingPhotos = false;
  }
}

/** 写真の一覧を入れ替える。iPod OS は保存のたびに ID が振り直され、Rockbox はフォルダの名前を変えると ID が変わるので、
 * 開いているアルバムは名前の道筋で探し直す (見つからなければ、見つかる一番深い親) */
export function setPhotos(lib: PhotoLibrary) {
  const path = vis.album ? albumChain(vis.album).map((a) => a.name) : [];
  vis.photos = lib;
  vis.album = null;
  for (const name of path) {
    const next: PhotoAlbum | undefined = childAlbums(vis.album).find((a) => a.name === name);
    if (!next) break;
    vis.album = next.id;
  }
  if (isPhotoView()) {
    const ids = new Set(lib.photos.map((p) => p.id));
    for (const id of [...vis.selected]) if (!ids.has(id)) vis.selected.delete(id);
  }
}

export function setVideos(list: VideoView[]) {
  vis.videos = list;
  // 選んでいた番組・アーティストが無くなった (名前や種類を変えた・消した) ら、その種類へ (種類は空でも左に出ている)
  if (/^[as]:/.test(vis.videoNode) && !list.some((v) => videoInNode(v, vis.videoNode))) {
    vis.videoNode = vis.videoNode.startsWith("a:") ? "k:musicvideo" : "k:tvshow";
  }
  if (isVideoView()) {
    const ids = new Set(list.map((v) => v.id));
    for (const id of [...vis.selected]) if (!ids.has(id)) vis.selected.delete(id);
  }
}

/** 写真の見方: サムネイル (アルバムはフォルダ) / リスト (左にアルバムの一覧)。設定に覚える */
export function photoLayout(): "grid" | "list" {
  return getStored("photoLayout") === "list" ? "list" : "grid";
}

export function albumById(id: string | null) {
  return id ? vis.photos?.albums.find((a) => a.id === id) : undefined;
}

const albumCollator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** あるアルバムの中のアルバム (null = 一番上)。名前の順 */
export function childAlbums(parent: string | null): PhotoAlbum[] {
  return (vis.photos?.albums ?? []).filter((a) => (a.parent ?? null) === parent).sort((a, b) => albumCollator.compare(a.name, b.name));
}

/** 一番上からそのアルバムまでの道筋 */
export function albumChain(id: string): PhotoAlbum[] {
  const out: PhotoAlbum[] = [];
  let a = albumById(id);
  while (a && out.length < 64) {
    out.unshift(a);
    a = albumById(a.parent ?? null);
  }
  return out;
}

/** アルバムの道筋の名前 ("旅行 / 2024") */
export function albumLabel(id: string): string {
  return albumChain(id)
    .map((a) => a.name)
    .join(" / ");
}

/** 写真の表示に出す写真 (選んでいるアルバムのもの。撮った順) */
export function photosShown() {
  const lib = vis.photos;
  if (!lib) return [];
  if (!vis.album) return [...lib.photos].sort((a, b) => a.date - b.date);
  const album = lib.albums.find((a) => a.id === vis.album);
  const byId = new Map(lib.photos.map((p) => [p.id, p]));
  return (album?.photoIds ?? []).map((id) => byId.get(id)).filter((p) => !!p);
}
