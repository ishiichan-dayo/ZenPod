#!/usr/bin/env bash
# WSL (Ubuntu) から Windows 版をクロスビルドするための道具を用意する。sudo 不要。
#   - Rust の Windows ターゲット (x86_64-pc-windows-msvc)
#   - cargo-xwin (MSVC の CRT / Windows SDK を自動取得してリンク)
#   - lld-link / makensis (apt のパッケージをダウンロードしてユーザー領域に展開)
# 展開先: ~/.local/share/ipod-sync/win-toolchain
set -euo pipefail

DEST="${IPOD_SYNC_WIN_TOOLCHAIN:-$HOME/.local/share/ipod-sync/win-toolchain}"
ROOT="$DEST/root"
BIN="$DEST/bin"
export PATH="$HOME/.cargo/bin:$PATH"

echo "==> Rust の Windows ターゲット"
rustup target add x86_64-pc-windows-msvc

echo "==> cargo-xwin"
command -v cargo-xwin >/dev/null || cargo install cargo-xwin --locked

echo "==> lld-link / makensis を $DEST に展開"
mkdir -p "$ROOT" "$BIN" "$DEST/debs"
LLVM_VER=$(clang --version 2>/dev/null | sed -nE 's/.*clang version ([0-9]+).*/\1/p' | head -1)
LLVM_VER=${LLVM_VER:-18}
(
  cd "$DEST/debs"
  apt-get download "lld-$LLVM_VER" "liblld-$LLVM_VER" nsis nsis-common
  for f in *.deb; do dpkg -x "$f" "$ROOT"; done
)

LLVM_BIN="$ROOT/usr/lib/llvm-$LLVM_VER/bin"
cat > "$BIN/lld-link" <<EOF
#!/bin/sh
LD_LIBRARY_PATH="$ROOT/usr/lib/x86_64-linux-gnu:$ROOT/usr/lib/llvm-$LLVM_VER/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}" exec "$LLVM_BIN/lld-link" "\$@"
EOF
cat > "$BIN/makensis" <<EOF
#!/bin/sh
NSISDIR="$ROOT/usr/share/nsis" exec "$ROOT/usr/bin/makensis" "\$@"
EOF
# clang は argv[0] が clang-cl だと MSVC 互換モードになる
ln -sf "$(command -v "clang-$LLVM_VER" || command -v clang)" "$BIN/clang-cl"
chmod +x "$BIN/lld-link" "$BIN/makensis"

"$BIN/lld-link" --version
"$BIN/makensis" -VERSION && echo
echo "==> 完了: $BIN"
