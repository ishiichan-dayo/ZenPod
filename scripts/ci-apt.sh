#!/usr/bin/env bash
# CI (GitHub Actions の Ubuntu) で apt のパッケージを入れる。
#
#   scripts/ci-apt.sh <パッケージ>...
#
# ランナーの既定の Azure のミラー (azure.archive.ubuntu.com) が極端に遅くなることがある
# (2026-10-23 に 38 分止まり、2026-10-28 には 1 日に 3 回 10 分の上限に当たった。いつもは 1 分ほど)。
# 時間内に終わらなければ、公式のミラー (archive.ubuntu.com) に替えてやり直す
set -euo pipefail

opts=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30)

attempt() {
  timeout 120 sudo apt-get "${opts[@]}" update &&
    timeout 300 sudo apt-get "${opts[@]}" install -y --no-install-recommends "$@"
}

if attempt "$@"; then
  exit 0
fi

echo "::warning::apt が時間内に終わらないので、公式のミラーに替えてやり直します"
for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
  [[ -f "$f" ]] && sudo sed -i 's|http://azure.archive.ubuntu.com/ubuntu|http://archive.ubuntu.com/ubuntu|g' "$f"
done
# 途中で止めたので、半端に入ったパッケージを片付けてから
sudo dpkg --configure -a || true
attempt "$@"
