#!/usr/bin/env bash
# CC Anatomy のリリースを開始する:
#   バージョン反映 → Cargo.lock 更新 → git commit → 注釈付きタグ → push
#
# ビルド・updater 署名・latest.json 生成・GitHub Release 作成は、タグ push を受けた
# GitHub Actions（.github/workflows/release.yml）が行う。リリースノートはタグ注釈で渡す。
#
# 使い方:
#   scripts/release.sh 0.3.0 "リリースノート（省略可）"
#
# 前提:
#   - リポジトリ Secrets に TAURI_SIGNING_PRIVATE_KEY / TAURI_SIGNING_PRIVATE_KEY_PASSWORD 登録済み
#   - gh CLI が o2yama アカウントでログイン済みであること
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:?使い方: scripts/release.sh <version> [notes]}"
NOTES="${2:-CC Anatomy v$VERSION}"
REPO="o2yama/cc-anatomy"

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "エラー: バージョンは x.y.z 形式で指定してください" >&2; exit 1; }
if gh release view "v$VERSION" --repo "$REPO" >/dev/null 2>&1; then
  echo "エラー: v$VERSION は既にリリース済みです" >&2; exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo "エラー: 未コミットの変更があります。リリースに含めるものだけを先にコミットしてください" >&2
  git status --short >&2
  exit 1
fi
# main 以外でタグを作ると push 対象から外れ、「配信開始」と表示だけされる事故になる
[[ "$(git branch --show-current)" == "main" ]] || { echo "エラー: main ブランチで実行してください" >&2; exit 1; }

echo "==> バージョンを $VERSION に更新"
node -e "
const fs = require('fs');
for (const f of ['package.json', 'src-tauri/tauri.conf.json']) {
  const j = JSON.parse(fs.readFileSync(f, 'utf8'));
  j.version = '$VERSION';
  fs.writeFileSync(f, JSON.stringify(j, null, 2) + '\n');
}"
# 行頭アンカーで [package] の version 行だけに一致する（依存側は `xxx = { version = ... }` 形式のため）
sed -i '' "s/^version = \".*\"/version = \"$VERSION\"/" src-tauri/Cargo.toml
# Cargo.lock のバージョンも追従させる（CI での --locked 相当のズレを防ぐ）
cargo metadata --manifest-path src-tauri/Cargo.toml --format-version 1 >/dev/null

echo "==> git commit / tag / push"
git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
# 「変更なし」だけをスキップし、hook 等による commit 失敗は握り潰さず止める
# （旧 HEAD にタグを付けて古いコードを新バージョンとして配信する事故の防止）
if git diff --cached --quiet; then
  echo "（バージョン変更なし・コミットスキップ）"
else
  git commit -m "リリース v$VERSION"
fi
git tag -a "v$VERSION" -m "$NOTES"
# --tags は無関係なローカルタグまで push するため、コミットに紐づく注釈付きタグだけを送る
git push origin main --follow-tags

echo ""
echo "✅ v$VERSION のタグを push しました。ビルドと配信は GitHub Actions が行います:"
echo "   https://github.com/$REPO/actions/workflows/release.yml"
echo "   完了後、インストール済みのアプリは起動時（15秒後）または12時間ごとのチェックで更新を検知します"
