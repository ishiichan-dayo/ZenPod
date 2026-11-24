#!/usr/bin/env bash
# 先行版 (GitHub のプレリリース) を、ビルドし直さずに正式版に切り替える。
#
#   scripts/promote.sh 2.8.14      中身を見せて、確かめてから切り替える
#   scripts/promote.sh 2.8.14 -y   確かめずに切り替える
#
# 先行版は正式版と同じ番号で作る (`gh workflow run release ... -f early=true`)。アプリ・インストーラ・署名はそのまま使い、
# 次の 2 つだけを変える:
#   1. latest.json の `early: true` を外して上げ直す。アプリは early の付いた latest.json を普通の更新確認では無視する
#      (src/ui/update.ts)。latest.json そのものには署名が無い (署名は添付ごとに中に入っている) ので書き換えてよい
#   2. リリースをプレリリースから Latest にする。起動時の確認先 releases/latest/download/latest.json と、
#      製品のサイトのダウンロードのボタン (GitHub API の最新版) がこの版になる
# 途中で止まっても、もう一度実行すれば続きから進む。gh は配布リポジトリに書ける人でログインしておくこと
set -euo pipefail

REPO=ishiichan-dayo/ZenPod
V="${1:?版を指定してください (例: scripts/promote.sh 2.8.14)}"
V="${V#v}"
TAG="v$V"
YES="${2:-}"

if [[ ! "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "版の形が違います: $V" >&2
  exit 1
fi

read -r PRE DRAFT < <(gh release view "$TAG" -R "$REPO" --json isPrerelease,isDraft --jq '"\(.isPrerelease) \(.isDraft)"')
LATEST=$(gh release view -R "$REPO" --json tagName --jq .tagName)
if [[ "$DRAFT" == true ]]; then
  echo "$TAG はまだ下書きです (リリースのワークフローが終わっていない)" >&2
  exit 1
fi
if [[ "$PRE" == false && "$LATEST" == "$TAG" ]]; then
  echo "$TAG はもう正式版です"
  exit 0
fi
if [[ "$PRE" == false ]]; then
  echo "$TAG はプレリリースではありませんが、Latest は $LATEST です。手で確かめてください" >&2
  exit 1
fi
# 今の正式版より新しい版だけ (古い先行版を切り替えると、全員に古い版を案内してしまう)
if [[ "$(printf '%s\n%s\n' "${LATEST#v}" "$V" | sort -V | tail -1)" != "$V" || "${LATEST#v}" == "$V" ]]; then
  echo "$TAG は今の正式版 ($LATEST) より新しくありません" >&2
  exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
gh release download "$TAG" -R "$REPO" -p latest.json -D "$TMP"

# early を外し、公開日を今にする。版とダウンロード先 (この版のタグの URL) も確かめる
EARLY=$(node - "$TMP/latest.json" "$V" "https://github.com/$REPO/releases/download/$TAG/" <<'EOF'
const fs = require("fs");
const [file, v, base] = process.argv.slice(2);
const json = JSON.parse(fs.readFileSync(file, "utf8"));
const fail = (msg) => { console.error(msg); process.exit(1); };
if (json.version !== v) fail(`latest.json の版が ${json.version} です (${v} のはず)`);
const urls = Object.entries(json.platforms ?? {});
if (!urls.length) fail("latest.json に platforms がありません");
for (const [name, p] of urls) {
  if (!p.url?.startsWith(base)) fail(`${name} のダウンロード先がこの版のタグではありません: ${p.url}`);
  if (!p.signature) fail(`${name} に署名がありません`);
}
const early = json.early === true;
delete json.early;
json.pub_date = new Date().toISOString();
fs.writeFileSync(file, JSON.stringify(json, null, 2) + "\n");
console.log(early);
EOF
)

echo "== $TAG を正式版にします (今の正式版: $LATEST)"
echo "-- 全員に案内されるリリースノート:"
node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).notes)' "$TMP/latest.json"
echo "--"
if [[ "$YES" != "-y" ]]; then
  read -r -p "切り替えますか？ [y/N] " ans
  [[ "$ans" == y || "$ans" == Y ]] || { echo "やめました"; exit 1; }
fi

# 先に latest.json を直す (プレリリースのままなら、普通の更新確認には出ないので、順番の途中で止まっても困らない)
if [[ "$EARLY" == true ]]; then
  gh release upload "$TAG" "$TMP/latest.json" -R "$REPO" --clobber
  echo "latest.json の early を外しました"
fi
gh release edit "$TAG" -R "$REPO" --prerelease=false --latest >/dev/null
echo "リリースを Latest にしました"

# 起動時の確認先がこの版になったか (GitHub の転送が切り替わるまで少し待つことがある)
URL="https://github.com/$REPO/releases/latest/download/latest.json"
for _ in $(seq 1 10); do
  got=$(curl -fsSL "$URL" | node -e 'const j = JSON.parse(require("fs").readFileSync(0, "utf8")); console.log(`${j.version} ${j.early === true}`)' || true)
  if [[ "$got" == "$V false" ]]; then
    echo "確認: $URL は $V (early なし)"
    exit 0
  fi
  sleep 3
done
echo "$URL がまだ $V になっていません (今: ${got:-読めない})。少し待って curl で確かめてください" >&2
exit 1
