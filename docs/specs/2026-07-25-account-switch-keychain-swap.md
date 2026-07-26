# アカウント切り替え改修仕様: Keychain スワップ方式（2026-07-25）

## 目的

アカウント切り替えを「PC 全体のログイン情報の書き換え」にする。
現行の setup-token 長期トークン + `.zshrc` への `CLAUDE_CODE_OAUTH_TOKEN` 注入方式は、
ターミナル起動の claude にしか効かず、かつ環境変数が Keychain より優先されるため撤去する。

## 調査で確定した前提（2026-07-25 researcher 調査）

- ライブ資格情報: Keychain service `Claude Code-credentials`、account は `$USER`。
  中身は JSON: `{"claudeAiOauth": {"accessToken": "sk-ant-oat01-...", "refreshToken": "sk-ant-ort01-...", "expiresAt": <unixミリ秒>, "scopes": [...], "subscriptionType": ..., "rateLimitTier": ...}}`
- `~/.claude.json` のトップレベルキー `oauthAccount` にアカウント表示情報（email 等）が入っており、
  Keychain と同時に書き換えないと Claude Code の表示が旧アカウントのまま残る（CCSwitcher が同時更新している）。
- refresh token は **one-time use**。refresh 成功時に旧 refresh token は無効化される。
  → 切り替え前に「現アカウントの最新資格情報をスナップショットへ書き戻す」sync-back が必須。
- access token の更新は Claude Code 自身が refresh token で自動実行する。
  アプリ側で token endpoint を叩く必要はない（期限切れ access token を書き込んでも次回利用時に自動 refresh される）。
- `claude auth login` はローカル v2.1.220 に実在（`claude auth --help` で確認済み）。
  ブラウザ承認完了までブロックするが、exit code での完了判定は未確認
  → 完了検知は Keychain エントリの変化ポーリングで行う。
- **2026-07-25 18:36 実機観測**: 実行中の claude セッションは、access token の期限が
  まだ5時間半ほど残っている段階でも OAuth refresh を実行し、そのたびにライブ Keychain
  （`Claude Code-credentials`）へ書き込む（accessToken・refreshToken・両方の expiresAt が
  ローテートされることを確認）。一方 `oauthAccount`（`~/.claude.json`）はこの refresh では
  変化しない。つまり切り替え後に旧アカウントの claude セッションが残っていると、
  「Keychain=旧アカウントの新トークン / oauthAccount=切り替え先アカウント」という不整合が
  時間の問題で必ず発生する。この状態で素朴に sync-back を行うと、oauthAccount の同一性判定
  （org_id/email）を無条件に信じてしまい、別アカウントのスナップショットへ誤ったトークンを
  書き込む（詳細は「レビュー後の追補」11項）。

## データモデル

- Keychain 追加エントリ: `CC Anatomy-cred-<name>`（service）
  - 中身はライブ資格情報 JSON の完全コピー（`{"claudeAiOauth": {...}}` 全体）
- `~/.claude/cc-anatomy/meta.json` の各アカウントに追加フィールド:
  - `oauth_account`: `~/.claude.json` の `oauthAccount` オブジェクトのコピー（serde_json::Value で保持、構造を決め打ちしない）
  - `has_credentials`: bool（スナップショット登録済みか）
- 既存の監視用長期トークン `CC Anatomy-token-<name>` と `CC Anatomy-active` は**存置**（トレイの使用率監視に必要）。
  - **2026-07-25 ユーザー決定で全廃**: 切り替えが Keychain スワップで簡単になったため、監視用長期トークンによる
    複数アカウントの使用率並列表示という機能自体が不要と判断し、この仕組みを丸ごと廃止した。
    `CC Anatomy-token-<name>` / `CC Anatomy-active` の Keychain エントリ・`accounts.json` 上の関連ロジック
    （`token_svc`・`missing_token`・`add_account_in_terminal`・`claim_pending_account`・
    `accounts_usage_detail`/`AccountUsageDetail`）はすべて削除。使用量は「現在ライブのアカウントのみ」を
    `/api/oauth/usage` `/api/oauth/profile`（ライブ access token 直叩き）で表示する一本道にした。
    起動時マイグレーション `remove_legacy_monitor_tokens()` が旧エントリを一度だけ掃除する（冪等）。
    `accounts.json` の後方互換は維持（旧フィールドが残っていても読める。新規に書き出すことはない）。

## フロー

### A. 現在ログイン中アカウントの取り込み（登録の基本経路）

1. `Claude Code-credentials` と `~/.claude.json` の `oauthAccount` を読む
2. `oauthAccount` の email（無ければ uuid）で既存登録と照合
3. 新規なら meta.json にアカウント追加、既存なら更新。資格情報を `CC Anatomy-cred-<name>` に保存
4. UI: アカウント一覧の上部に「現在のログイン: <email>（未登録なら［取り込む］ボタン）」を常時表示

### B. 新規アカウントの追加（ブラウザ認証）

1. 追加ボタン → Terminal.app で `claude auth login` を起動（既存の setup-token 起動と同じ osascript 流儀）
2. アプリは起動前の `Claude Code-credentials` の内容ハッシュを控え、2秒間隔でポーリング（タイムアウト5分）
3. 変化を検知したら A の取り込み処理を実行
4. 注意: ブラウザでどのアカウントを選ぶかは CLI から強制できない。取り込み後に email を UI 表示して本人に確認させる

### C. 切り替え（「このアカウントに切り替える」）

1. **sync-back**: 現在の `Claude Code-credentials` + `oauthAccount` を読み、登録アカウントと照合。
   - 一致 → そのアカウントのスナップショット（cred + oauth_account）を最新内容で上書き
   - 不一致（未登録アカウントがログイン中）→ 確認ダイアログ「現在のログインは未登録です。切り替えると失われます。取り込みますか？」
2. 切り替え先の `CC Anatomy-cred-<name>` を `Claude Code-credentials` に書き込み
   （`security` CLI。既存エントリは delete → add、または add -U。既存コードの流儀に合わせる）
3. `~/.claude.json` の `oauthAccount` を切り替え先の `oauth_account` に置換。
   **他のキーは一切変更しない**（読み込み → 該当キーのみ置換 → 書き戻し。パース失敗時は中断してエラー表示）
4. `CC Anatomy-active` を更新、トレイ・UI を更新
5. 検証: profile API（長期トークンではなくスワップした accessToken）で疎通確認。
   401 なら「refresh token が失効しています。再ログインしてください」→ B の `claude auth login` 導線を表示
   （access token の期限切れ自体は正常。401 判定は refresh も効かないケースの検出なので、
   検証は oauth/profile を accessToken で叩いて 401 のときのみ警告。ネットワークエラーは警告しない）

### D. 旧方式の撤去（マイグレーション）

1. `.zshrc` からアプリが挿入した `CLAUDE_CODE_OAUTH_TOKEN` の export 行（ヘルパーマーカー行含む）を削除する処理を追加し、アプリ起動時に一度実行（冪等に）
2. 切り替え経路としての setup-token / .zshrc 注入コードは削除
3. 監視用長期トークンの登録機能（`claude setup-token` を Terminal で実行して `CC Anatomy-token-<name>` に保存）は残す。
   UI 上は「使用量監視用トークン（任意）」として切り替えとは別の位置づけにする
   - **2026-07-25 ユーザー決定でこの方針自体を撤回**: 3. の監視用長期トークン登録機能を含め、
     監視用長期トークンの仕組みそのものを全廃した（データモデル節を参照）。
     `add_account_in_terminal` / `claim_pending_account`（setup-token 経路）と、UI の
     「使用量監視用トークン（任意）」セクションは削除済み。1.（.zshrc 注入撤去）と
     2.（切り替え経路としての旧方式撤去）はそのまま有効

## UI（Accounts.tsx / AccountsOverlay）

- アカウント一覧の各行: 名前 / email / 資格情報スナップショット有無 / 「このアカウントに切り替える」ボタン / 「削除」ボタン
  （2026-07-25 ユーザー決定で監視用長期トークンの仕組みを全廃したため、監視トークン列は削除済み。
  行内の「再ログイン」ボタンは2026-07-26 ユーザー決定で削除。下記 M2 追記を参照）
- 現在 PC にログイン中のアカウントをバッジで明示（oauthAccount の email と照合）
- 切り替え後の注記を表示: 「実行中の Claude Code セッションには反映されません。新しく起動したセッションから有効です」
- 使用量表示（ツールバー・メニューバー）はライブアカウントのみの1枚表示に統一（カルーセルは廃止）

## 裁定事項（2026-07-25 builder の指摘への回答）

1. **「選択中(active)」概念は新しい切り替えに統合する（B案）**。
   - 切り替え（Keychain スワップ）成功時に `meta.active` / `CC Anatomy-active` も更新する。
   - 「選択中 = 実際に PC にログイン中のアカウント」に一本化し、表示専用の選択トグルは廃止。
   - `claude_env()` の env var 注入（cmux 起動・タスク抽出時）は、スワップ後はライブ Keychain 自体が正しいアカウントになるため不要。削除し、素の `claude` 呼び出しに戻す。
   - 「`CC Anatomy-active` を壊さない」の意図は「トレイの使用率監視機能を壊さない」こと。active ポインタの更新主体が新しい切り替えに変わるのは意図どおり。トレイの複数アカウント使用率表示（長期トークン利用）は従来どおり動作すること。

2. **データファイルは既存の `accounts.json` を継続使用**（仕様書中の `meta.json` 表記は誤り）。
   既存ユーザーの登録データを消失させないこと。新フィールド（`oauth_account`, `has_credentials`）は後方互換に追加する。

3. **アカウント同一判定は org_id（organizationUuid）を第一キー、email を第二キー**とする。
   - 取り込み時: `oauthAccount` 内の organizationUuid が既存登録の org_id と一致すれば同一アカウントとみなし、そのエントリにスナップショットをマージする（二重登録しない）。
   - organizationUuid が取れない場合のみ email で照合。どちらも取れなければ新規登録。

4. **監視用長期トークンの全廃**（2026-07-25 ユーザー決定）: 切り替えが Keychain スワップで簡単になったため、
   複数アカウントの使用量を並べて見る機能自体を廃止する。使用量は「現在ライブのアカウントのみ」を表示する。
   - `add_account_in_terminal`（setup-token 登録フロー）・`claim_pending_account`・
     `CC Anatomy-token-<name>` の読み書き・`CC Anatomy-active`（`ACTIVE_SVC`）を削除。
     `accounts.json` の `active` フィールドは「ライブ追随の記録専用」（Keychain の裏付け無し）として存置。
   - 起動時マイグレーション `remove_legacy_monitor_tokens()` で旧 Keychain エントリを一度だけ削除（冪等）。
   - `get_rate_limits` / `get_account_profile` / トレイの使用率表示は、ライブ資格情報の access token による
     `/api/oauth/usage` `/api/oauth/profile` に一本化。長期トークン用の `/v1/messages` probe
     （`probe_headers`/`TokenCheck`/`check_oauth_token`/`rate_limits_from_headers`/`usage_summary`）は削除。
     ライブ access token が期限切れの場合は「取得できませんでした（Claude Code を一度使うと更新されます）」と表示する。
   - トレイ: ライブアカウントの使用率のみ表示。他の登録アカウントは名前だけ列挙（使用率・リセット時刻は出さない）。
   - アカウント画面: 監視トークン列・関連バッジを削除。行は「名前 / email / スナップショット有無 / ライブバッジ /
     切り替え / 再ログイン」に。

## レビュー後の追補（2026-07-25 reviewer 指摘への裁定）

旧実装のモジュールヘッダに「swap 方式は実機検証で否定済み」の記録があった。否定理由は
①refresh token ローテーション、②実行中の Claude Code セッションがライブ資格情報を上書きして
切り替え結果を踏み潰す、の2点。①は sync-back で解消。②には以下で対応する（この根拠を
accounts.rs のモジュール doc に「旧結論を覆す理由」として必ず記録すること）。

1. **実行中セッションのガード**（2026-07-25 実装時は下記(a)、同日ユーザー報告を受けて(b)に緩和）:
   - (a) 当初実装: `running_sessions() > 0` の間は切り替え・アカウント追加（`claude auth login`）を
     一律ブロックし、UI で「すべての Claude Code セッションを終了してください」と表示していた。
   - (b) 緩和後（2026-07-25 ユーザー了解の上で変更）: 実運用のユーザー環境ではシェルセッションが
     常時4件程度開いており「0件のタイミング」が実質存在せず、(a)のハードブロックのままでは
     切り替え・再ログイン・アカウント追加のいずれも実行できなくなることが判明した。
     そのため `switch_account` / `start_add_account_login` に `force: bool` を追加し、
     「確認 + force」方式に緩和する: `force=false` かつ外部セッションが1件以上あれば
     `SessionsRunning { count }` を返す。UI はこれを受けて「起動中の Claude Code セッションが
     N 件あります。続行すると、実行中セッションが古いトークンを書き戻して切り替えが
     巻き戻ったり、保存済みアカウントが後で再ログイン必要になる可能性があります。
     全セッション終了を推奨しますが、続行しますか？」と確認し、承諾されたら `force=true` で
     再実行する。**リスク自体は解消されたわけではなく**、警告文で明示した上でユーザーの判断に委ねる。
   - (c) **フロント側のスキップ設定**（2026-07-26 ユーザー承認）: 確認ダイアログに「今後この確認を
     表示しない」チェックボックスを追加し、チェックして続行すると `localStorage`
     （`cc-anatomy.skipSessionsConfirm`）に記録、以後は sessions_running が返っても確認を出さず
     自動で `force=true` 再実行する。根拠: 12時間の実機検証で巻き戻りは未観測、現行 Claude Code は
     Keychain を読み直してから使う挙動が濃厚、かつ sync-back + 同一性検証（11項）の防御があるため
     毎回の確認は過剰と判断。**ずれ検知警告（11項の mismatched 警告）と needs_import
     （未登録ログインの取り込み確認）はこの設定の対象外**とし、データ喪失・安全性に関わるため
     常に表示する。
   - **本アプリ自身の環境診断・タスク抽出の実行中（`diagnostics::is_running()` /
     `actions::is_agent_busy()`）は、(b)の緩和後も引き続きハードブロックのまま**（force でも
     迂回不可）。これらは完了を待てば済む短時間の処理であり、ユーザーのシェルセッションと違って
     いつ終わるかアプリ自身が把握しているため、待たせることに実害が無いため。
2. **Flow B にも sync-back を必須化**: `claude auth login` はライブ資格情報を上書きするため、
   起動前に Flow C と同一の sync-back を実行する。ライブが未登録なら needs_import で止める。
3. **sync-back は best-effort 禁止**: ライブ資格情報 or oauthAccount の読み取りに失敗したら
   切り替え・追加を中断する（黙ってスキップして上書きに進まない）。
4. **書き込み順序**: `~/.claude.json` のパース検証 → sync-back 分の save_meta 確定 →
   Keychain スワップ → oauthAccount 置換 → active 更新。active 更新の失敗は警告に留め、
   切り替え全体を失敗扱いにしない。スワップ後は読み戻し検証（書いた内容と一致するか）を行う。
5. **切り替え後のトークン検証（旧 C-5）は撤回**: スナップショットの access token は期限切れが
   常態で、401 検証は誤検知にしかならない。事前検証は行わず、各アカウント行に常設の
   「再ログイン」導線（`claude auth login` 起動）を置くことで失効時の回復手段とする。
   - **2026-07-26 ユーザー決定で行内「再ログイン」ボタンを削除**: 「＋ アカウントを追加
     （ブラウザでログイン）」と機能的に重複しており冗長と判断。失効時・未取り込み時の回復経路は
     追加フローに一本化する（同一アカウントでログインすれば org_id 照合で既存エントリに
     マージされ、スナップショットが更新される既存挙動をそのまま使う。`startAddAccountLogin`
     自体はコマンドとして残置）。行内アクションは「切り替える」「削除」の2つに簡素化。
6. **「選択中 = ログイン中」の一本化を徹底**: `get_accounts` / 取り込み時にライブの org と一致する
   登録アカウントへ `meta.active` / `CC Anatomy-active` を追随させる。
7. **ファイル書き込みの保全**: `~/.claude.json` / `.zshrc` の書き戻しは元ファイルのパーミッションを
   維持し、symlink の場合はリンク先を書き換える（リンクを実ファイルで置換しない）。
8. **Keychain account 名**: ライブアイテムの `acct` は `$USER` 決め打ちにせず、既存アイテムから
   読んで再利用する。取得できなければ中断。
9. **Flow B の完了検知**: ライブ資格情報のハッシュ変化を完了条件とする。
   （当初は「ハッシュ変化かつ org/email 変化」としたが、同一アカウントの再ログインが
   永久に完了しなくなるため撤回。ハッシュのみだと自動 refresh による誤検知があり得るが、
   その場合も取り込み処理は実質 sync-back になるだけで無害。2026-07-25 再レビュー A 対応）
10. **検証項目の修正**: `~/.claude.json` の保全確認はテキスト diff ではなく意味的比較
    （`jq -S` で正規化して比較）とする。serde_json の書き戻しでキー順・整形が変わるため。
11. **sync-back の同一性検証**（2026-07-25 18:36 実機観測を受けて追加）:
    - `Meta.last_live_hash`（新フィールド、後方互換 default None）に、アプリが最後に把握している
      ライブ資格情報 JSON の SHA-256 を記録する。更新タイミングは
      switch_account（スワップ成功時・target_cred のハッシュ）、import_live_account
      （取り込んだ creds のハッシュ）、sync_back_live_login（書き戻し成功時・現在の creds の
      ハッシュ）の3箇所。
    - sync_back_live_login の冒頭で現在のライブ資格情報の SHA-256 を計算し、`last_live_hash`
      と比較する。
      - 一致 → 前回アプリが把握していた状態から外部書き込みが無い。従来どおり oauthAccount の
        organizationUuid/emailAddress を信じて帰属を決めてよい（profile API は叩かない）。
      - 不一致、または `last_live_hash` が None（初回等） → 「アプリの知らないところでライブが
        書き換わった」とみなし、ライブの access token で `/api/oauth/profile` を叩いて実際の
        持ち主を確認する（`actions::oauth_get_with_token` を流用）。
        - 成功 → 返ってきた email で帰属先スナップショットを決定する。oauthAccount の記載と
          ズレていた場合は oauthAccount 側（org_id 含む）を信用せず、profile の email だけで
          照合する。ズレを検知した場合は戻り値に警告
          （「ライブのログインが実行中セッションにより巻き戻っていました。」）を含め、
          UI に表示する。
        - 失敗（401・ネットワークエラー等、確認不能） → 推測で書き込まず sync-back を中断する
          （切り替え・追加も中止）。エラーメッセージ:
          「ライブ資格情報の持ち主を確認できませんでした。少し待って再試行するか、
          全セッション終了後に再試行してください」。
    - 実装は `resolve_live_owner`（純粋関数、profile 呼び出しを引数として注入）+
      `sync_back_live_login`（Keychain/ファイル IO を伴う実行部）に分離し、
      hash一致・hash不一致+profile一致・hash不一致+profile不一致（ズレ検知）・
      profile確認不能の4パターンをユニットテストで検証する。

## 表示名（display_name）機能（2026-07-26 ユーザー要望で追加）

内部識別子 `name`（Keychain サービス名 `CC Anatomy-cred-<name>` 等の照合キー）は不変のまま、
ユーザーが自由に付けられる表示専用の `display_name: Option<String>` を `StoredAccount`/`Account`
に追加した（後方互換 default None）。表示フォールバック規則は「`display_name` があればそれ、
無ければ `name`」で統一し、Rust 側は `resolve_display_name`、フロント側は `accountLabel()`
に集約する。新コマンド `rename_account(name, display_name)` はトリム後に空文字なら
`display_name` を None に戻す（= 内部識別子表示に戻る）。Accounts.tsx の行内名前表示は
クリックでインライン編集（Enter/blur で保存、Escape でキャンセル）。トレイ・確認ダイアログ・
使用状況ポップオーバーもすべて表示名優先に統一した。

## 制約・非対象

- macOS 限定（既存のアカウント機能と同じ。Windows は対象外のまま）
- 複数マシンで同一アカウントを並行使用した場合の refresh token 競合は対象外（既知の Claude Code 側挙動）
- Claude Code 実行中プロセスへの即時反映はしない

## 検証項目（実装完了の定義）

- [ ] 取り込み → 切り替え → `claude auth status` で切り替え先アカウントが表示される
- [ ] 切り替え後に新規ターミナルで claude を起動し、切り替え先アカウントで動作する
- [ ] A→B→A の往復切り替えで両アカウントとも動作し続ける（sync-back の検証）
- [ ] `.zshrc` の export 行が撤去され、再起動しても再挿入されない
- [ ] `~/.claude.json` の oauthAccount 以外のキーが変更されていない（`jq -S` で正規化して意味的に比較。serde_json の書き戻しでキー順・整形が変わるため単純なテキスト diff では判定しない）
- [ ] トレイの使用率監視が従来どおり動作する（2026-07-25 決定後: ライブアカウントの使用率のみ表示、他の登録アカウントは名前だけ列挙されること）
- [ ] `security add-generic-password -U` で更新した後、Claude Code 本体が承認ダイアログなしでライブ資格情報アイテムを読めるか（ACL が保持されているかの確認。ACL が壊れると毎回 Keychain のアクセス許可ダイアログが出るようになる）
- [ ] 同一アカウントで再ログイン（同じアカウントで `claude auth login` をやり直す）した後、そのアカウントへ「切り替える」操作が正常に完了するか（poll の完了条件をハッシュ変化のみに変更したことの回帰確認。以前は「hash 変化 かつ org/email 変化」を条件にしていたため、同一アカウントの再ログインが永久に完了しない不具合があった）
