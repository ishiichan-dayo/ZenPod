// Tauri の API の入口。画面のほかのモジュールは @tauri-apps を直接 import しない
// ファイルを書く先・ffmpeg・PC の音楽フォルダ・開く iPod のダイアログは Rust のコマンドが開く (画面から場所を渡さない)。
// ここのダイアログは、読むだけのファイル (転送する曲・取り込むプレイリスト・ジャケットの画像) を選ぶときだけ使う

import { Channel, invoke } from "@tauri-apps/api/core";
import type { DownloadEvent, Update } from "@tauri-apps/plugin-updater";
export { invoke };
export { listen } from "@tauri-apps/api/event";
export { getCurrentWebview } from "@tauri-apps/api/webview";
export { getCurrentWindow } from "@tauri-apps/api/window";
export { getVersion } from "@tauri-apps/api/app";
export { open as openDialog } from "@tauri-apps/plugin-dialog";
export { check, type Update } from "@tauri-apps/plugin-updater";
export { relaunch } from "@tauri-apps/plugin-process";

/**
 * 先行版を含めて一番新しい版を確かめる (設定の「先行版」の「確認」)。先行版は GitHub のプレリリースで、
 * updater の確認先 (releases/latest) には出ないので Rust のコマンドで確かめる (src-tauri の commands/update.rs)。
 * 画面のほかの部分からは普通の更新と同じに扱えるよう、updater の Update と同じ形にして返す
 */
export async function checkEarly(): Promise<Update | null> {
  const u = await invoke<{ version: string; currentVersion: string; body: string | null } | null>("check_early_update");
  if (!u) return null;
  return {
    ...u,
    body: u.body ?? undefined,
    rawJson: {},
    downloadAndInstall: async (onEvent?: (ev: DownloadEvent) => void) => {
      const ch = new Channel<DownloadEvent>();
      if (onEvent) ch.onmessage = onEvent;
      await invoke("install_early_update", { onEvent: ch });
    },
  } as unknown as Update;
}
