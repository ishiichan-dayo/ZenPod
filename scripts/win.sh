#!/usr/bin/env bash
# WSL から Windows 版をビルドして、Windows 側ですぐ動作確認するためのスクリプト。
#
#   scripts/win.sh dev        開発モード: 画面は WSL の Vite から読み込む (HTML/CSS/TS の変更は即反映)
#   scripts/win.sh run        リリース版 (画面を埋め込み) をビルドして起動
#   scripts/win.sh installer  インストーラ (NSIS) を作ってダウンロードフォルダに置く
#   scripts/win.sh stop       起動中の開発版を終了
#
# dev / run はインストール版とは別アプリ「ZenPod Dev」として動く (設定・キャッシュは別)。
# 事前に scripts/setup-win-toolchain.sh を一度実行しておくこと。
set -euo pipefail

cd "$(dirname "$0")/.."
MODE="${1:-dev}"
TARGET=x86_64-pc-windows-msvc
TOOLCHAIN="${IPOD_SYNC_WIN_TOOLCHAIN:-$HOME/.local/share/ipod-sync/win-toolchain}"
# 開発モードの画面を読む Vite。ensure_vite が、このフォルダの Vite が動いているポートに決める
VITE_PORT=1420
VITE_URL="http://127.0.0.1:$VITE_PORT"
EXE_NAME="zenpod-dev.exe"
DEV_IDENTIFIER="app.ipodsync.desktop.dev"
export PATH="$TOOLCHAIN/bin:$HOME/.cargo/bin:/usr/lib/llvm-18/bin:$PATH"

die() { echo "error: $*" >&2; exit 1; }

command -v cargo-xwin >/dev/null && [ -x "$TOOLCHAIN/bin/lld-link" ] \
  || die "Windows 向けの道具がありません。先に scripts/setup-win-toolchain.sh を実行してください"
command -v cmd.exe >/dev/null || die "Windows の cmd.exe が見つかりません (WSL で実行してください)"

# Windows 側のパス (UNC パスの警告を避けるため C: で実行)
win() { (cd /mnt/c && cmd.exe /c "$@" 2>/dev/null | tr -d '\r'); }
LOCALAPPDATA_WIN=$(win echo %LOCALAPPDATA%)
DEV_DIR=$(wslpath -u "$LOCALAPPDATA_WIN")/ZenPodDev
DOWNLOADS=$(wslpath -u "$(win echo %USERPROFILE%)")/Downloads

# インストール版と混ざらないよう、識別子と名前を変える (tauri.conf.json に上書きマージされる)。
# windows は配列ごと置き換わるので、tauri.windows.conf.json と同じ項目 (decorations も) を書く
dev_config() {
  local dev_url="$1"
  cat <<EOF
{"identifier":"app.ipodsync.desktop.dev","productName":"ZenPod Dev",
 "build":{"devUrl":"$dev_url"},
 "app":{"windows":[{"title":"ZenPod Dev","width":1120,"height":740,"minWidth":820,"minHeight":520,"dragDropEnabled":true,"decorations":false}]}}
EOF
}

# まず普通に閉じる (強制終了すると WebView の localStorage が書き込まれず、設定や登録フォルダが消える)。
# 5 秒待っても終わらなければ強制終了
stop_dev() {
  win taskkill /IM "$EXE_NAME" >/dev/null || return 0
  for _ in $(seq 1 10); do
    win tasklist /FI "IMAGENAME eq $EXE_NAME" /NH | grep -qi "$EXE_NAME" || return 0
    sleep 0.5
  done
  win taskkill /IM "$EXE_NAME" /F >/dev/null || true
}

# 前回の WebView2 (同じデータフォルダを使う msedgewebview2.exe) が終わるまで待つ。
# 終了処理中に次を起動すると WebView2 が落ちて、ウィンドウが真っ黒になる
wait_webview_exit() {
  (cd /mnt/c && timeout 20 powershell.exe -NoProfile -NonInteractive -Command "
    for (\$i = 0; \$i -lt 30; \$i++) {
      \$p = Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | Where-Object { \$_.CommandLine -like '*$DEV_IDENTIFIER*' }
      if (-not \$p) { exit 0 }
      Start-Sleep -Milliseconds 300
    }" </dev/null >/dev/null 2>&1) || true
}

launch() {
  local exe_win
  wait_webview_exit
  exe_win=$(wslpath -w "$DEV_DIR/$EXE_NAME")
  echo "==> 起動: $exe_win"
  # cmd.exe の start は WSL から空の引数 "" を渡せず固まるので PowerShell で起動する
  # ZENPOD_DEV_READ_DELAY_MS (1 曲のタグを読むたびに待つミリ秒)・ZENPOD_DEV_WALK_DELAY_MS (Rockbox で 1 フォルダを読むたびに待つミリ秒) を渡すと、遅い iPod を真似られる (開発版だけ)
  (cd /mnt/c && timeout 30 powershell.exe -NoProfile -NonInteractive -Command \
    "\$env:ZENPOD_DEV_READ_DELAY_MS='${ZENPOD_DEV_READ_DELAY_MS:-}'; \$env:ZENPOD_DEV_WALK_DELAY_MS='${ZENPOD_DEV_WALK_DELAY_MS:-}'; Start-Process -FilePath '${exe_win//\'/\'\'}'" </dev/null >/dev/null 2>&1) \
    || die "起動に失敗しました: $exe_win"
}

# そのポートの Vite が、このフォルダの画面を配っているか (ほかの作業フォルダの Vite は、このフォルダのファイルを 403 で断る)
vite_is_ours() {
  [ "$(curl -s -m 3 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$1/@fs$PWD/package.json")" = 200 ]
}

# WSL の中でそのポートを待ち受けているものがあるか (mirrored のネットワークでは、空いたポートへの curl が
# Windows 側に回されて返事が来ないことがあるので、curl では調べない)
port_in_use() {
  ss -ltnH "sport = :$1" 2>/dev/null | grep -q .
}

# 1420 番から順に、このフォルダの Vite が動いているポートを使う。無ければ空いているポートで起動する
# (別の作業フォルダの Vite が 1420 番で動いたままだと、開発版がそちらの古い画面を読んでしまうため)
ensure_vite() {
  local port free=""
  for port in $(seq 1420 1429); do
    if port_in_use "$port" && vite_is_ours "$port"; then
      VITE_PORT=$port VITE_URL="http://127.0.0.1:$port"
      echo "==> Vite は起動済み ($VITE_URL)"
      return
    fi
    if [ -z "$free" ] && ! port_in_use "$port"; then free=$port; fi
  done
  [ -n "$free" ] || die "Vite を起動できる空いたポートがありません (1420〜1429)"
  [ "$free" = 1420 ] || echo "==> 1420 番はほかの作業フォルダの Vite が使っているので、$free 番で起動します"
  VITE_PORT=$free VITE_URL="http://127.0.0.1:$free"
  echo "==> Vite を起動 ($VITE_URL, ログ: target/win-dev/vite.log)"
  mkdir -p target/win-dev
  nohup npx vite --host 127.0.0.1 --port "$VITE_PORT" --strictPort > target/win-dev/vite.log 2>&1 &
  for _ in $(seq 1 30); do
    vite_is_ours "$VITE_PORT" && return
    sleep 0.5
  done
  die "Vite が起動しません (target/win-dev/vite.log を確認)"
}

case "$MODE" in
  dev)
    ensure_vite
    echo "==> Windows 版 (debug) をビルド"
    # custom-protocol を付けない debug ビルドは devUrl (WSL の Vite) から画面を読む
    TAURI_CONFIG="$(dev_config "$VITE_URL")" \
      cargo xwin build -p zenpod --target "$TARGET"
    stop_dev
    mkdir -p "$DEV_DIR"
    cp "target/$TARGET/debug/zenpod.exe" "$DEV_DIR/$EXE_NAME"
    # debug ビルドはコンソールアプリなので黒い窓が一緒に開き、閉じるとアプリも終了する。
    # PE ヘッダの Subsystem を GUI (2) に書き換えてコンソールを出さない
    python3 - "$DEV_DIR/$EXE_NAME" <<'PY'
import struct, sys
with open(sys.argv[1], "r+b") as f:
    f.seek(0x3C); pe = struct.unpack("<I", f.read(4))[0]
    f.seek(pe); assert f.read(4) == b"PE\0\0", "PE ヘッダが見つかりません"
    f.seek(pe + 24 + 68); f.write(struct.pack("<H", 2))
PY
    launch
    echo "    画面 (src/ 以下) の変更は保存すると自動で反映されます。Rust を変えたら再実行してください。"
    ;;
  run)
    echo "==> Windows 版 (release, 画面埋め込み) をビルド"
    npx tauri build --runner cargo-xwin --target "$TARGET" --no-bundle \
      --config "$(dev_config "$VITE_URL")"
    stop_dev
    mkdir -p "$DEV_DIR"
    cp "target/$TARGET/release/ZenPod.exe" "$DEV_DIR/$EXE_NAME"
    launch
    ;;
  installer)
    [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || [ ! -f "$HOME/.tauri/ipod-sync.key" ] || {
      export TAURI_SIGNING_PRIVATE_KEY="$(cat "$HOME/.tauri/ipod-sync.key")"
      export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$HOME/.tauri/ipod-sync.key.password" 2>/dev/null || true)"
    }
    echo "==> インストーラをビルド"
    npx tauri build --runner cargo-xwin --target "$TARGET" --bundles nsis
    VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
    SRC="target/$TARGET/release/bundle/nsis/ZenPod_${VERSION}_x64-setup.exe"
    cp "$SRC" "$DOWNLOADS/"
    echo "==> $(wslpath -w "$DOWNLOADS")\\$(basename "$SRC")"
    ;;
  stop)
    stop_dev
    echo "==> 開発版を終了しました"
    ;;
  *)
    die "使い方: scripts/win.sh [dev|run|installer|stop]"
    ;;
esac
