#!/usr/bin/env bash
# リリース済みの universal dmg に合わせて Homebrew cask を更新する。
# aarch64 → universal への一度きりの移行（URL・arch 制約）も冪等に処理する。
#
# 使い方:
#   scripts/update-cask.sh 0.3.0
#   → ~/MyProjects/homebrew-tap で差分を確認して commit & push する
set -euo pipefail

VERSION="${1:?使い方: scripts/update-cask.sh <version>}"
CASK="$HOME/MyProjects/homebrew-tap/Casks/cc-anatomy.rb"
URL="https://github.com/o2yama/cc-anatomy/releases/download/v${VERSION}/CC.Anatomy_${VERSION}_universal.dmg"

[[ -f "$CASK" ]] || { echo "エラー: cask がありません: $CASK" >&2; exit 1; }

echo "==> dmg を取得して sha256 を計算: $URL"
# 一時ファイルは案件内 tmp/（gitignore 対象）に置く運用ルールに合わせる
TMP="$(cd "$(dirname "$0")/.." && pwd)/tmp/cask-update"
mkdir -p "$TMP"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL -o "$TMP/app.dmg" "$URL"
SHA="$(shasum -a 256 "$TMP/app.dmg" | awk '{print $1}')"

sed -i '' \
  -e "s/^  version \".*\"/  version \"$VERSION\"/" \
  -e "s/^  sha256 \".*\"/  sha256 \"$SHA\"/" \
  -e "s/_aarch64\.dmg/_universal.dmg/" \
  -e "/depends_on arch: :arm64/d" \
  "$CASK"

echo "✅ cask を v$VERSION（universal, sha256=$SHA）に更新しました"
echo "   cd ~/MyProjects/homebrew-tap && git diff で確認して commit & push してください"
