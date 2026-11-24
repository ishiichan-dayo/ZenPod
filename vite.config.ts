import { defineConfig } from "vite";

// Tauri 用: 固定ポート / 画面に出す環境変数は VITE_* と TAURI_ENV_* だけ
// (TAURI_* にすると、ビルドの環境にある TAURI_SIGNING_PRIVATE_KEY なども画面のコードから読めてしまう)
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "es2021", outDir: "dist", emptyOutDir: true },
});
