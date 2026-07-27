# Apple 署名・公証のセットアップ手順

`.github/workflows/release.yml` は Apple 系の Secrets（`APPLE_CERTIFICATE` 等）が
GitHub リポジトリに登録されていれば自動的に macOS ビルドへコード署名・公証を行い、
未登録なら従来どおり未署名ビルドを続行する（詳細は release.yml 冒頭のコメント参照）。

このドキュメントは、その Secrets を用意するために**大津山さんが手元で1回だけ行う作業**の手順。
参照した一次情報: https://v2.tauri.app/distribute/sign/macos/ 、
https://github.com/tauri-apps/tauri-action/blob/dev/examples/publish-to-auto-release-universal-macos-app-with-signing-certificate.yml

## 前提

- Apple Developer Program（年額有料。$99/年）への登録が必要
- macOS 上で作業する（Keychain Access を使うため）

## 1. Developer ID Application 証明書を作成する

1. https://developer.apple.com/account/resources/certificates/list を開く
2. 「＋」→ **Developer ID Application** を選択（配布用の署名証明書。App Store 用の証明書とは別物なので選び間違えないこと）
3. 画面の指示に従い CSR（証明書署名要求）を作る:
   - Keychain Access.app → メニューの「Keychain Access」→「証明書アシスタント」→「認証局に証明書を要求...」
   - メールアドレス・通称を入力し「ディスクに保存」を選択して CSR ファイルを書き出す
4. 作成した CSR を developer.apple.com のフォームにアップロードし、証明書（`.cer`）をダウンロード
5. ダウンロードした `.cer` をダブルクリックして Keychain（ログインキーチェーン）にインストールする
   - インストールすると、対応する秘密鍵とペアで「証明書」カテゴリに表示される

## 2. .p12 として書き出し、base64 化する

1. Keychain Access.app で先ほどインストールした証明書（`Developer ID Application: <名前> (<Team ID>)`）を選択
2. 右クリック →「書き出す...」→ ファイル形式 `.p12 証明書` を選び、パスワード（`APPLE_CERTIFICATE_PASSWORD` として使う）を設定して保存
3. base64 化する:

   ```bash
   openssl base64 -A -in /path/to/certificate.p12 -out certificate-base64.txt
   ```

4. `certificate-base64.txt` の中身が `APPLE_CERTIFICATE` Secret の値になる

## 3. App用パスワードを発行する（公証用）

公証（notarization）には Apple ID ベースの方法と App Store Connect API キーの方法があるが、
release.yml は前者（Apple ID + App用パスワード）を使う想定。

1. https://appleid.apple.com にサインイン
2. 「サインインとセキュリティ」→「App用パスワード」→「App用パスワードを生成」
3. 任意のラベル（例: `cc-anatomy-notarize`）を付けて生成されたパスワードを控える（`APPLE_PASSWORD` になる。通常の Apple ID パスワードではない点に注意）
4. Team ID は https://developer.apple.com/account の「メンバーシップの詳細」（Membership details）ページに表示される（`APPLE_TEAM_ID`）

（代替: App Store Connect の API キー方式を使う場合は `APPLE_API_ISSUER` / `APPLE_API_KEY` /
`APPLE_API_KEY_PATH` を使う。https://appstoreconnect.apple.com/access/api で発行できるが、
release.yml は現状この方式には対応していないため、使う場合はワークフロー側の変更が別途必要）

## 4. GitHub Secrets に登録する

このリポジトリ（`o2yama/cc-anatomy`）に対して、`gh` CLI で登録する:

```bash
gh secret set APPLE_CERTIFICATE < certificate-base64.txt
gh secret set APPLE_CERTIFICATE_PASSWORD    # プロンプトで入力（2で設定した .p12 のパスワード）
gh secret set KEYCHAIN_PASSWORD             # プロンプトで入力（任意の文字列。CI の一時キーチェーン用で Apple とは無関係）
gh secret set APPLE_ID                      # プロンプトで入力（Apple ID のメールアドレス）
gh secret set APPLE_PASSWORD                # プロンプトで入力（3で発行した App用パスワード）
gh secret set APPLE_TEAM_ID                 # プロンプトで入力
```

`gh secret set NAME` は値を渡さず実行すると標準入力から読むので、プロンプトでの貼り付け（Enter → Ctrl+D）か、
`echo -n "値" | gh secret set NAME` の形で登録するとよい。値がターミナル履歴やシェル変数に残らないよう、
`.p12` の base64 ファイルや控えたパスワードは登録後に削除しておく。

登録済みか確認:

```bash
gh secret list
```

## 5. 動作確認

1. 通常どおり `scripts/release.sh` でタグを push し、release ワークフローを実行する
2. ワークフローのログで「Apple Developer Certificate をインポート」ステップが実行されている（スキップされていない）ことを確認する
3. リリースされた `.dmg` をダウンロードし、展開した `.app` に対して:

   ```bash
   # コード署名の検証（Gatekeeper が受理するか）
   spctl -a -vv /Applications/CC\ Anatomy.app

   # 公証（notarization）が付与されているか（stapler でチケットが埋め込まれているか）
   xcrun stapler validate /Applications/CC\ Anatomy.app
   ```

   `spctl` が `accepted` かつ `source=Notarized Developer ID` と表示され、
   `stapler validate` が成功すれば、初回起動時の「開発元を確認できません」表示は出なくなる
4. 別の Mac（このビルドをしていない環境）でダウンロード→起動し、警告なしで開けることを確認する

## トラブルシューティング

- 「Apple Developer Certificate をインポート」ステップがスキップされる → `APPLE_CERTIFICATE` が登録されていない、または空。`gh secret list` で確認
- `security: SecKeychainItemImport: ... The specified item could not be found in the keychain` → `.p12` のパスワードと `APPLE_CERTIFICATE_PASSWORD` が一致していない
- 公証が失敗する（`xcrun notarytool` 相当のエラー） → `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` の組み合わせを確認。App用パスワードは通常の Apple ID パスワードとは別物なので取り違えに注意
- 署名はされるが `spctl` で拒否される → 公証（3〜4）が未完了の可能性。tauri-action のログで notarization ステップの結果を確認する
