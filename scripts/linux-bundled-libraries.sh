#!/usr/bin/env bash
# AppImage に入れた Ubuntu のライブラリ (GTK・WebKitGTK・GStreamer・FFmpeg のライブラリなど) の
# パッケージ・版・ライセンスの一覧と、各パッケージの著作権表示 (/usr/share/doc/<パッケージ>/copyright) を 1 つのファイルにまとめる。
# リリースのビルド (build.yml の build-linux) で、AppImage を作った Ubuntu の上で動かし、リリースに添付する。
#
#   scripts/linux-bundled-libraries.sh <AppImage> <出力のファイル>
set -euo pipefail

APPIMAGE=$(readlink -f "$1")
OUT=$(readlink -f "$2")
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

(cd "$WORK" && "$APPIMAGE" --appimage-extract >/dev/null)

declare -A PKGS=()
UNKNOWN=()
while IFS= read -r -d '' f; do
  base=$(basename "$f")
  # dpkg -S に「*/名前」を渡すと、その名前で終わるパスを持つパッケージを探す (「/名前」だと完全なパスとして扱う)。
  # 見つかった最初のパッケージの名前 (「名前:amd64」のアーキテクチャは外す)
  pkg=$(dpkg -S "*/$base" 2>/dev/null | grep -v '^diversion' | head -n 1 | cut -d: -f1 || true)
  if [ -n "$pkg" ]; then
    PKGS["$pkg"]=1
  else
    UNKNOWN+=("${f#"$WORK"/squashfs-root/}")
  fi
done < <(find "$WORK/squashfs-root/usr/lib" -type f \( -name '*.so' -o -name '*.so.*' \) -print0)

. /etc/os-release

{
  echo "ZenPod Linux AppImage: bundled libraries"
  echo "========================================"
  echo
  echo "The ZenPod AppImage includes the following libraries from ${PRETTY_NAME}."
  echo "Each is distributed under its own license, shown below together with its copyright file."
  echo "ZenPod itself is licensed under GPL-3.0-or-later (see LICENSE and THIRD_PARTY_LICENSES.md)."
  echo
  echo "Source code: the source of each package at the exact version listed below is available from the"
  echo "Ubuntu archive (https://launchpad.net/ubuntu/+source/<source package>/<source version>, or"
  echo "\`apt-get source <source package>=<source version>\` on ${PRETTY_NAME})."
  echo "If you cannot get it there, ask on https://github.com/ishiichan-dayo/ZenPod/issues and the developer"
  echo "will provide it. This offer is valid for at least three years after the release that included it."
  echo
  echo "AppImage に入れている Ubuntu のライブラリと、その版・ライセンス・著作権表示です。"
  echo "ソースコードは上の Ubuntu のアーカイブから、同じ版のものを入手できます。入手できないときは Issues でお知らせください。"
  echo
  echo "Package / Version / Source package / Source version / Licenses"
  echo "--------------------------------------------------------------"
  for pkg in $(printf '%s\n' "${!PKGS[@]}" | sort); do
    IFS=$'\t' read -r ver src srcver < <(dpkg-query -W -f '${Version}\t${source:Package}\t${source:Version}\n' "$pkg" | head -n 1)
    lic=$(grep -h '^License:' "/usr/share/doc/$pkg/copyright" 2>/dev/null | sed 's/^License: *//' | sort -u | paste -sd ';' - || true)
    echo "- $pkg $ver (source: $src $srcver) — ${lic:-see copyright file}"
  done
  if [ "${#UNKNOWN[@]}" -gt 0 ]; then
    echo
    echo "Files not found in any installed package:"
    printf -- '- %s\n' "${UNKNOWN[@]}"
  fi
  # 著作権表示は元のパッケージ (source package) ごとに 1 回
  declare -A SEEN=()
  for pkg in $(printf '%s\n' "${!PKGS[@]}" | sort); do
    src=$(dpkg-query -W -f '${source:Package}\n' "$pkg" | head -n 1)
    [ -n "${SEEN[$src]:-}" ] && continue
    SEEN["$src"]=1
    echo
    echo "================================================================================"
    echo "$src (copyright file of $pkg)"
    echo "================================================================================"
    cat "/usr/share/doc/$pkg/copyright" 2>/dev/null || echo "(no copyright file)"
  done
} > "$OUT"

echo "${#PKGS[@]} packages, ${#UNKNOWN[@]} unknown files -> $OUT"
