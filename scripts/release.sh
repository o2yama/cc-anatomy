#!/usr/bin/env bash
# CC Anatomy のリリース一式を行う:
#   バージョン反映 → 署名付きビルド → latest.json 生成 → git commit/tag/push → GitHub Release 作成
#
# 使い方:
#   scripts/release.sh 0.1.2 "リリースノート（省略可）"
#
# 前提:
#   - 署名鍵 ~/.tauri/cc-anatomy.key が存在すること（紛失すると更新配信不能）
#   - gh CLI が o2yama アカウントでログイン済みであること
#   - dmg 生成が Finder を使うため、フォアグラウンドのターミナルで実行すること
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:?使い方: scripts/release.sh <version> [notes]}"
NOTES="${2:-CC Anatomy v$VERSION}"
KEY_PATH="$HOME/.tauri/cc-anatomy.key"
REPO="o2yama/cc-anatomy"
ASSET="cc-anatomy_${VERSION}_universal.app.tar.gz"

[[ -f "$KEY_PATH" ]] || { echo "エラー: 署名鍵 $KEY_PATH がありません" >&2; exit 1; }
# universal ビルドには両アーキテクチャの Rust ターゲットが必要
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target list --installed | grep -qx "$t" || rustup target add "$t"
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "エラー: バージョンは x.y.z 形式で指定してください" >&2; exit 1; }
if gh release view "v$VERSION" --repo "$REPO" >/dev/null 2>&1; then
  echo "エラー: v$VERSION は既にリリース済みです" >&2; exit 1
fi

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

echo "==> 署名付きビルド"
# CLI が TAURI_SIGNING_PRIVATE_KEY_PATH を解釈しないため鍵の中身を直接渡す。
# 鍵はパスワードなし生成（--ci）だが、PASSWORD 未設定だと TTY プロンプトを試みて
# 非対話環境で落ちるため空文字を明示する
TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY_PATH")" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" \
npm run tauri build -- --target universal-apple-darwin

BUNDLE="src-tauri/target/universal-apple-darwin/release/bundle"
TARBALL="$BUNDLE/macos/CC Anatomy.app.tar.gz"
SIG_FILE="$TARBALL.sig"
DMG="$BUNDLE/dmg/CC Anatomy_${VERSION}_universal.dmg"
for f in "$TARBALL" "$SIG_FILE" "$DMG"; do
  [[ -f "$f" ]] || { echo "エラー: ビルド成果物がありません: $f" >&2; exit 1; }
done

echo "==> latest.json 生成"
mkdir -p tmp/release
cp "$TARBALL" "tmp/release/$ASSET"
node -e "
const fs = require('fs');
const manifest = {
  version: '$VERSION',
  notes: process.argv[1],
  pub_date: new Date().toISOString().replace(/\.\d+Z$/, 'Z'),
  // universal binary のため両アーキテクチャに同一アセットを配信する
  platforms: Object.fromEntries(['darwin-aarch64', 'darwin-x86_64'].map(k => [k, {
    signature: fs.readFileSync('$SIG_FILE', 'utf8').trim(),
    url: 'https://github.com/$REPO/releases/download/v$VERSION/$ASSET'
  }]))
};
fs.writeFileSync('tmp/release/latest.json', JSON.stringify(manifest, null, 2) + '\n');
" "$NOTES"

echo "==> git commit / tag / push"
git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "リリース v$VERSION" || echo "（バージョン変更なし・コミットスキップ）"
git tag "v$VERSION"
git push origin main --tags

echo "==> GitHub Release 作成"
gh release create "v$VERSION" \
  "tmp/release/$ASSET" \
  "tmp/release/latest.json" \
  "$DMG" \
  --repo "$REPO" \
  --title "CC Anatomy v$VERSION" \
  --notes "$NOTES"

echo ""
echo "✅ v$VERSION を配信しました: https://github.com/$REPO/releases/tag/v$VERSION"
echo "   インストール済みのアプリは起動時（15秒後）または12時間ごとのチェックで更新を検知します"
