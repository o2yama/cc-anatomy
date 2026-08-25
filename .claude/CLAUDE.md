# cc-dashboard（アプリ名: CC Anatomy）

Claude Code の環境と活動状況を「解剖」して可視化するデスクトップアプリ（Tauri v2 + React + TypeScript）。
2026-07-12 に cc-dashboard → **CC Anatomy** に改名（identifier: com.o2yama.cc-anatomy）。フォルダ名は据え置き。

## ゴール

**最終ゴール（2026-07-25 再定義）**: PC の環境を Claude Code が最も効果的に動けるようにセットアップすること。PC のスペック・環境情報・リソース状況・セキュリティ観点・既存データ・ユーザーの役割を踏まえて環境を整える。メニューバーから見える要素は現状維持のまま、アプリ本体の機能を拡張していく。

その手段・土台として、`~/.claude` / `~/.claude-mem` に散らばる Claude Code の活動データを1つのダッシュボードで俯瞰する：

1. ディレクトリ（プロジェクト）ごとのセッション履歴一覧と「何をやったか」のサマリー
2. Skills / Agents のインベントリ
3. （将来）hooks・plugins・rules などサブシステムの俯瞰

## データソースの構造メモ

- `sdk_sessions`: content_session_id（= jsonl ファイル名の UUID）, memory_session_id, project(basename), user_prompt, started_at_epoch(ms)
- `session_summaries`: memory_session_id で sdk_sessions と JOIN。**1セッションに複数行**（prompt_number ごと）
- 全セッション49,145件に対しサマリー4,112件。サマリー無しセッションは user_prompt で代替表示
- jsonl: 1行1イベント。`type:"user"/"assistant"` 行の `message.content` が会話本体。attachment / meta 行はスキップ
- Skills: `~/.claude/skills/*/SKILL.md`（YAML frontmatter の name/description）
- Agents: `~/.claude/agents/*.md`（同上）
- claude-mem DB（`~/.claude-mem/claude-mem.db`）は**読み取り専用で開く**。書き込み厳禁

## 開発コマンド

- `npm run tauri dev` — 開発起動
- `npm run tauri build` — 配布ビルド（macOS 配布物は `-- --target universal-apple-darwin`）
- `scripts/release.sh <version> ["ノート"]` — リリース開始（バージョン反映→注釈付きタグ push。ビルドと配信は GitHub Actions）
- `cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored` — 実資格情報を使う使用量取得の e2e 検証

## 現在有効な設計決定（結論のみ・経緯は docs/dev-log.md）

- サマリー表示は claude-mem SQLite から読み、jsonl はドリルダウン時のみ遅延読み
- macOS は universal binary 1本配布、Windows は監視機能のみ（アカウント切替・環境診断・右クリックメニューは macOS 限定）
- リリースビルドは GitHub Actions（release.yml、`max-parallel: 1` 必須）。Windows は windows-latest 未検証のため matrix 未投入
- 自動アップデートは tauri-plugin-updater 経由。署名鍵 `~/.tauri/cc-anatomy.key` 紛失で更新配信が不能になるため要バックアップ。リリースは必ず `scripts/release.sh` で行う
- ライブ資格情報は `credentials.rs` に抽象化（macOS = Keychain、Windows/Linux = `~/.claude/.credentials.json`）
- claude CLI / cmux 起動は絶対パスで解決（GUI アプリは .zshrc の PATH を継承しない）
- 環境診断は `claude -p` headless を read-only 許可リストで実行、修正は Terminal.app で対話起動（配布アプリ自身はユーザーファイルに書き込まない）
- **アカウント切替は Keychain スワップ方式**（2026-07-25 実装、`accounts.rs` 参照）: ライブ Keychain の `Claude Code-credentials` をアカウント別スナップショット（`CC Anatomy-cred-<name>`）で保管し、切替時にライブ Keychain エントリ自体を書き換えて PC 全体のログインを差し替える。`~/.claude.json` の `oauthAccount` も同時に置換（他のキーは触らない）。refresh token が one-time use のため、切替・追加（`claude auth login`）の直前に必ず sync-back（現在ログイン中アカウントの最新資格情報をスナップショットへ書き戻す）を行う。外部セッション（シェルの claude セッション）が1件以上ある場合は「確認 + force」方式でユーザーに続行確認を挟む（ハードブロックにするとユーザー環境で機能自体が使えなくなったため2026-07-25に緩和。本アプリ自身の診断/タスク抽出プロセスはこれとは別に常時ハードブロック）。sync-back はベストエフォートにせず、読み取り失敗や部分適用の失敗（ロールバックも失敗した場合）は `meta.inconsistent` フラグで検出し、明示的な取り込み/再ログインまで以後の sync-back を止める。さらに `meta.last_live_hash`（SHA-256）で前回把握したライブ資格情報からの外部書き換えを検知し、ズレていれば `/api/oauth/profile` で実際の持ち主を確認してから sync-back する（実行中セッションの自動 refresh によるなりすまし帰属を防止）
  - **旧方式（2026-07-25 撤去済み）**: `claude setup-token` の長期トークンを Keychain に保管し `CLAUDE_CODE_OAUTH_TOKEN` 経由で `.zshrc` 注入する方式。環境変数がターミナル限定で PC 全体のログインを書き換えられない上、`CLAUDE_CODE_OAUTH_TOKEN` は Keychain より優先されるため新方式と競合し撤去。旧 `.zshrc` 注入ブロックはアプリ起動時に自動撤去される
- **監視用長期トークンは 2026-07-25 に全廃 → 翌 2026-07-26 に任意機能として復活**（2026-08-22 にコードと Keychain の実地確認で判明）: 07-25 に「複数アカウントの使用量を並べて見る機能自体を廃止する」と決めて `CC Anatomy-token-<name>` / `CC Anatomy-active`・setup-token 登録フロー・`accounts_usage_detail` 等を削除したが、**翌日 07-26 に監視用長期トークンが任意機能として復活しており、現在も稼働している**（`actions.rs` 冒頭コメント参照）。実地確認（2026-08-22）: Keychain に `CC Anatomy-token-share1/2/3` が実在し、`has_monitor_token()` は3件とも true を返す。**起動時マイグレーション `remove_legacy_monitor_tokens()` は現在のコードに存在しない**（07-25 当時の記述が残っていたもの）。使用量取得の優先順位は `resolve_usage_source_order()`（`accounts.rs`）が持ち、**ライブ**は `/api/oauth/usage` →（失敗時）監視トークンで `/v1/messages` → スナップショットで `/api/oauth/usage`、**非ライブは監視トークンの `/v1/messages` が最優先** → スナップショット。したがって「使用量は現在ライブのアカウントのみ・一本道」という旧記述も誤り。`/v1/messages` は `max_tokens:1` の実リクエストなので**使用量を消費する**。`accounts.json` は旧フィールドが残っていても読める後方互換を維持

### 未検証事項

- アカウント切替（Keychain スワップ方式）: 取り込み→切替→`claude auth status`確認、A→B→A往復、`.zshrc`撤去、`~/.claude.json`の`oauthAccount`以外不変、Keychain ACL 保持、同一アカウント再ログイン後の切替、「確認+force」フロー・profile 確認による同一性検証、いずれも実機の Keychain 書き換えを伴うため未検証（詳細は `docs/specs/2026-07-25-account-switch-keychain-swap.md` の検証項目）
- 監視用長期トークン（2026-07-26 復活分）の残存期限。`docs/dev-log.md` には `claude setup-token` は「サブスク用・1年・ローテートなし」とあるが、現物3本の発行日・失効日は未確認

## 既知の落とし穴

- **Ice（メニューバー管理アプリ）が新規トレイアイコンを画面外（x=-8000台）に飛ばす**。隠しセクションにも出ないことがある。Ice 再起動 or レイアウト設定で表示側に割り当てて解決
- アプリ更新時は .app 差し替えだけでは反映されない。実行中プロセスの kill → open -a 再起動まで必要
- tauri build の .dmg 生成はバックグラウンドシェルからだと失敗する（Finder 操作が絡む）。フォアグラウンドで実行する
- 配布物は無署名・aarch64 のみ。初回起動は「プライバシーとセキュリティ → このまま開く」が必要
- **Tauri v2 の WebView はデフォルトで `dragDropEnabled: true`**（ネイティブのファイルドロップ処理が
  HTML5 の dragstart/dragover/drop イベントを横取りし、webview 内で発火しなくなる）。
  HTML5 Drag and Drop を使う画面（アカウント一覧の並び替え等）がある限り、
  `src-tauri/tauri.conf.json` の `app.windows[0]` に `"dragDropEnabled": false` が必須
  （v0.4.0 でこの設定漏れにより D&D 並び替えが実機で全く動かないバグを出した。patch で修正）

---

### 2026-08-25 の決定

- **スナップショットの refresh token 自動更新機能を実装**（未リリース・実機未検証）: Keychain 実物確認で `claudeAiOauth.refreshTokenExpiresAt` の存在を確認し、refresh token は発行から約30日で失効すると判明。非ライブのスナップショットを30日放置すると切り替え後に再ログインが必要になるため、期限20日前から OS ネイティブダイアログ（「<email>の認証情報の期限が切れます。自動で更新しますか？」はい/いいえ）で確認し、「はい」でアプリ自身が `https://console.anthropic.com/v1/oauth/token` に POST してスナップショットを更新する。「いいえ」はメインウィンドウ前面化+アカウントモーダル表示（`open-accounts` イベント）。60秒ループから1サイクル1件・アカウント単位24時間スロットル（accounts.json の `refresh_prompted_at` + プロセス内第二スロットル）。macOS 限定
- **規約リスクをユーザーが受容済み**: このエンドポイント・client_id（Claude Code の公開値）のアプリからの直接利用は Anthropic Consumer Terms 上のグレー〜違反領域で、予告なしの資格情報失効リスクがある（2026-01 サーバー側ブロック強化・2026-02 規約明文化）。通知のみの安全案を提示した上で、ユーザーが自動 refresh を選択（2026-08-25）。調査記録: `tmp/2026-08-25-token-refresh-research.md`（git 管理外）
- **ライブアカウントは絶対に refresh 対象にしない**: Claude Code 本体の自動 refresh と one-time use の refresh token を取り合い資格情報が壊れるため。判定は「非ライブと確定できなければ対象外」に倒す（`confirmed_non_live`: org_id 空・ライブ org 不明は対象外。さらにライブの refreshToken との一致もチェック）
- **HTTP 成功後は新トークンが唯一の有効な資格情報**: 書き込み前に TOCTOU 再検証（スナップショットの refreshToken がフェーズ1と一致するか）を行い、読み取り→整形→書き込みの全体をリトライ対象にし、既存スナップショットが読めなければ最小構成 JSON を新規構築してでも書く。全滅時のみ「再ログインが必要」
- **`AccountOpGuard::acquire` を compare_exchange 化**: 従来は store(true) でアカウント操作同士（switch_account と本機能等）が相互排他になっていなかった（レビュー R3）。既存の switch_account / start_add_account_login にも同じ排他が効くようになる
- **実機未検証**: refresh POST の実レスポンス形状（`refresh_token_expires_in` の有無は未確認・保守側フォールバック25日）、Keychain 書き換え、ダイアログ表示、「いいえ」導線のモーダル表示。既知の残課題: `refreshTokenExpiresAt` を持たない旧スナップショットは対象外のまま・トークンが `security` の argv 経由で `ps` に露出する既存制約の増幅

### 2026-08-22 の決定

- **v0.5.4（同日午後）: v0.5.3 の段階的バックオフを撤去し、照会間隔を5分にした**。v0.5.3 配信後「少し時間が経つと『取得が一時的に制限されています』が必ず出る。ただし普通に使えている」という報告を受けた。バックオフがほぼ常時オンになり、その間 `live_error` が RateLimited 固定になる一方、数値は監視トークン経由で毎分更新されていたため、**新鮮な値に対して「最新でない可能性」と表示していた**。詳細と実測は `docs/dev-log.md`
- **429 の性質（実測）**: リクエスト**頻度**によるもので、そのアカウントの使用量の枠とは**無関係**（5h 枠 100% のアカウントが 200 を返し、16% のアカウントが 429 を返した）。枠は**アカウント単位**で狭く、8秒間隔で3回叩けば 429 になる。「アカウントが上限に張り付いているから」という仮説は検証して否定済み。「claude セッションが枠を共有している」は未検証の仮説
- **スロットルは「成功時のキャッシュ」と「失敗も含む試行間隔」の2つが要る**: `usage_cache.fetched_at` は成功時にしか更新されないため、それだけに依存すると **429 が続く間だけスロットルが消える**（いちばん必要な場面で効かない）。`USAGE_LAST_ATTEMPT` を別の状態として持つ。**`fetched_at` を失敗時に更新して代用してはならない**（注記の古さ判定が壊れる）
- **「ゲートで塞いだ」ことを失敗理由として表示しない**: HTTP を1回も打っていないのに「取得が一時的に制限されています」と言う、ログイン済みが型レベルで確定している分岐で「Claude Code でログインしてください」と言う、という嘘を実際に2つ作り込んだ。`USAGE_LAST_ERROR` に直近の実際の失敗理由を保持して返す
- **注記は「表示中の値が実際に古いとき」だけ出す**: 判定を `live_error`（＝直叩きが失敗したか）で行うと、別ソースが新鮮な値を取れているときに嘘になる。`now - fetched_at` を見る
- **`docs/usage-polling-spec.md` は v0.5.3 時点の記述で、v0.5.4 を反映していない**（冒頭に注意書きあり）。全面改訂まではこの CLAUDE.md と `docs/dev-log.md` を優先すること

- **使用量取得まわりの現行仕様は `docs/usage-polling-spec.md` に集約**（2026-08-22 作成）: 定期実行の間隔・全 HTTP 経路とそれぞれが使うトークン・トークン4種の有効期限と更新主体・Keychain とファイルの読み書き・バックオフのゲート範囲・キャッシュ閾値・切り替えの処理順を `file:line` と［確認］/［未確認］付きで記載。CLAUDE.md の記述と実装がずれていた場合はこちらが実地確認済みの正
- **`/v1/messages` はバックオフの対象外**（`accounts.rs` の `UsageSource::MonitorToken` 経路）: 429 バックオフ中も監視トークン経由の推論リクエストは止まらない。バックオフ中のライブは毎分1回 `max_tokens:1` の実リクエストを投げ続ける。意図的な設計ではなく、429 対応時に見落としていた箇所
- **アカウント切り替え時の refresh token 巻き戻しリスク**: 切り替えは Keychain の `Claude Code-credentials` をスナップショット JSON でまるごと上書きする（refresh token 込み）。refresh token は one-time use のため、スナップショット取得後に Claude Code がそれを消費していた場合、書き戻すとそのアカウントは refresh 不能になり再ログインが必要になる。防止策が切り替え直前の sync-back だが、持ち主を確認できないと飛ばされる。**2026-08-22 の変更で 429 も「持ち主未確認だが続行可」側に倒れるようになった**ため、続行を選ぶとこのリスクを踏みうる（そうしないと切り替え自体が不能になるため意図的）

- **使用量が更新されない不具合を修正**（未リリース）: 症状「アカウント切替後も使用量が更新されない・全アカウント token 切れ表示・復活日時が古いまま」の実体は **HTTP 429 を「token 期限切れ」と誤分類していたバグ**だった（`oauth_get_checked_blocking` が HTTP ステータスを 401 しか見ず、本文に `error` フィールドがあれば全部 Expired と判定していた。429 の本文は `{"error":{"type":"rate_limit_error"}}`）。429 を起こしていたのはアプリ自身で、60秒サイクルごとに約4リクエスト/分（ライブは同じ1分に2回）を12日間連続で出していた。修正内容: ①HTTP ステータス優先の純粋関数 `classify_oauth_response` で 429/401/`authentication_error`/`rate_limit_error` を分離 ②ライブの二重取得を廃止（`get_accounts_usage` が `UsageBatch { accounts, live_error }` を返し、`fetch_raw_status` は `live_usage_summary()` を**ライブがバッチに存在しないときだけ**呼ぶ）③キャッシュ閾値をライブ45秒・非ライブ600秒に分離 ④429 で指数バックオフ（5→10→20→40→60分）。詳細と根拠は `docs/dev-log.md`
- **バックオフ状態は `actions.rs`（全プラットフォーム共通）に置く**: macOS 限定の `accounts.rs` に置くと、Windows/Linux の唯一の使用量取得経路（`tray::fetch_raw_status` のフォールバック）がバックオフの管轄外になる。`accounts_stub.rs` への複製実装も不要になる
- **`live_usage_summary()` のフォールバックは条件付きで必ず残す**: ライブがバッチに存在しないケース（CC Anatomy 未登録の初回起動・org_id 不一致・`has_credentials=false`・**Windows/Linux 全体**）で使用量表示が全滅し、「Claude Code でログインしてください」という誤案内が出る。無条件に削除してはいけない（一度やって退行を出した）
- **429 は `OwnerError::NetworkError` に落とす（`Other` にしない）**: `Other` だと「持ち主未確認でも切り替える」確認ダイアログが出ず切り替え自体が失敗する。修正前は 429 が Expired に化けていたおかげで続行できていたため、`Other` にすると退行になる
- **非ライブアカウントの使用量は原理的に最新化できない**: スナップショットの access token は最後にライブだった時刻から約8時間で切れ、refresh token は Claude Code 本体しか触らない設計のため。ただし「監視用長期トークン」方式（`claude setup-token` の長期トークンで `/v1/messages` のレスポンスヘッダ `anthropic-ratelimit-unified-*` から読む）にはこの制約が無く、**この方式は 2026-07-26 に復活していて現在も動いている**（非ライブの使用量が取れているのはこの経路のおかげ）。2026-08-22 当初「全廃済み」と記述したのは誤りで、実地確認により訂正
- **レート制限は `/api/oauth/usage` 側に固有**: 同時刻に同じライブトークンで `/v1/messages` を叩くと 200 が返り、ヘッダから同じ数値が取れる（2026-08-22 実測）
- **未対応（スコープ外）**: `stale` / `fetched_at` の UI 表示。バックエンドは返しているのにトレイ（`compact_usage_segments`）もフロント（`App.tsx`）も見ておらず、取得失敗時にキャッシュ値を最新のように描く。「常に最新」を構造上保証できない経路がある以上、鮮度表示は本来必須
- **実機検証済み（2026-08-22 10:26〜10:50、開発ビルド）**: ①非ライブの照会間隔600秒（10:38:45→10:48:45 でちょうど600秒後に再照会。ライブは60秒ごと）②429 でバックオフ突入し5分間まったく照会しない（2回とも再現）③バックオフ明けに自動復帰 ④**429 中に `claude -p` の裏起動が発火しない**（バックオフ中、アプリの子プロセスがゼロであることを `ps` で確認）
- **レート制限の実態（2026-08-22 実測）**: `/api/oauth/usage` は**バースト5リクエスト程度で 429 に落ちる**（3〜5回目で切り替わるのを2回観測）。修正前の約4リクエスト/分・12日連続で張り付いていたのは必然だった
- **実機未検証**: 429 時のトレイ文言（「取得が一時的に制限されています（最新でない可能性）」）の実表示、Windows 実機でのフォールバック経路。残存事項の一覧は `tmp/2026-08-22-residual.md`（git 管理外）
- **プロセス上の未達**: ルールでは大きいタスクに Codex クロスベンダーレビューが必須だが、Codex が使用量上限（8/27 復帰）のため Claude 側レビュー3巡で代替した。ユーザー承認済み

### 2026-07-31 の決定

- **ドロワー内ドキュメント編集機能を実装・main マージ済み**: プロジェクト概要でファイル名クリック → 右ドロワー（`DetailDrawer`）内の `DocEditor.tsx`（CodeMirror 6）で CLAUDE.md 等を直接編集・Cmd+S 保存できる。保存は `write_doc`（`env.rs`）の楽観ロック（modified_epoch）で外部変更と競合検出し、conflict 時は専用 UI で再読込を促す。path が null / truncated のコンテンツは読み取り専用。日本語 IME はスパイク（SpikeEditor、削除済み）で CodeMirror 6 の問題なしを確認してから採用
- **未検証事項**: 実機 UI での編集→保存、IME 入力、conflict 検出 UI、未保存離脱ガードの動作確認（ビルド・型チェック・cargo check は通過済み）
- **ドキュメントAI分析機能を実装**（macOS 限定）: エディタ右上の ✨ ボタン（「AIに分析・改善してもらう」）→ `doc_analysis.rs` が `claude -p`（sonnet, stream-json, `--permission-mode dontAsk`, `--max-turns 40`, timeout 600s）を read-only 許可リスト `Read,Glob,Grep,WebFetch(domain:code.claude.com|docs.anthropic.com)` で起動し、ファイル種別に応じた Anthropic 公式ドキュメント（memory/skills/sub-agents/settings）を実行時に WebFetch して照合した改善提案をドロワー内パネルに表示。認証は claude CLI の優先順位そのまま（= ログイン中アカウント。サブスク/API 両対応。`--bare` は OAuth を読まないため不採用）。エディタの未保存バッファを正として分析（上限10万字）。`--setting-sources user --strict-mcp-config` で cwd プロジェクトの settings/hooks/MCP を遮断。stdin 書き込みは専用スレッド（20万字級でのパイプ相互デッドロック防止）。`doc_analysis::is_running()` をアカウント切替の `ensure_app_not_busy()` に登録済み（本アプリ自身のプロセスは常時ハードブロックの不変条件を維持）。グローバルスコープ文書（rules 等）は projectDir を渡さず文書単体+公式照合モードで分析。Opus レビュー1巡（ブロッカー1+要修正4+軽微10）対応済み
- **AI分析の未検証事項**: 実機での `claude -p` 実起動・WebFetch ドメイン許可の実効・進捗ストリーミング表示・キャンセル・分析中アカウント切替のブロック動作
- **セッションタブ廃止**: プロジェクト選択ペインは概要ビューのみに一本化（SessionList/SessionCard 撤去。全体検索と TranscriptDrawer は残存）
- **リリース前 Codex レビュー（クロスベンダー）で9件検出・修正**: 保存中追加入力の dirty 消失／「破棄」の初回ロード巻き戻り／未保存ガード迂回（サイドバー切替・再読込は対応、トレイ終了・updater 再起動経路は未対応で残存）／readDoc 競合／楽観ロックの秒精度（ミリ秒化。check→rename 間の完全排他は未対応で残存）／保存時パーミッション消失（0600→0644。引き継ぎ実装）／アカウント操作と AI 分析・診断の TOCTOU（`ACCOUNT_OP_IN_PROGRESS` + RAII ガード）／headless 硬化（`--tools` 追加 + `~/.ssh` 等の Read 明示拒否）／アプリ終了時の子プロセス孤児化（終了ハンドラで kill_running）
- **使用量ポップオーバーをトレイと同一表示に**: トレイの取得ロジックを `tray.rs::fetch_raw_status()` に抽出共有し、新コマンド `get_usage_overview` で同一データをフロントへ供給（数値のズレを構造的に排除）。表示はトレイの行構成・32ドットゲージ（丸め規則同一、JS/Rust 全数比較で一致確認済み）・「（HH:MM 復活）」フォーマット・他アカウント行+切替（confirm+force フロー共有）・ステータス更新を DOM で再現。`get_rate_limits`/`get_account_profile` コマンドと RateLimits/AccountProfile ベースの旧表示は撤去。expired の区別文言もトレイと同一化（旧「Claude Code を一度使うと取得できます」案内は消滅）。Opus レビュー1巡（ブロッカー1+要修正3+軽微8）対応済み。未検証: 実機でのポップオーバー表示（1行に収まるかは概算のみ）・切替フロー・accounts-updated 購読の実効

### 2026-07-25 の決定

- **ゴール再定義**: 「Claude Code の活動データを1ダッシュボードで俯瞰する」から「PC の環境を Claude Code が最も効果的に動けるようにセットアップする」に最終ゴールを拡大（ユーザー決定）。可視化（ダッシュボード）は最終ゴールの手段・土台という位置づけに変わる。メニューバーから見える要素は現状維持
- **アカウント切替方式の変更**: 「setup-token 長期トークン + `CLAUDE_CODE_OAUTH_TOKEN` 注入」方式から「Keychain の `Claude Code-credentials` をアカウント別スナップショットでスワップし PC 全体のログインを書き換える」方式に変更・**実装済み**（詳細は上記「現在有効な設計決定」および `docs/specs/2026-07-25-account-switch-keychain-swap.md`）。実機での Keychain 書き換え検証は未実施
- **監視用長期トークンの全廃**（※この決定は翌 2026-07-26 に覆され、任意機能として復活している。現行仕様は上記「現在有効な設計決定」を参照）: 切替が Keychain スワップで簡単になったため、複数アカウントの使用量を並べて見る機能自体を廃止。使用量は現在ライブのアカウントのみ表示する方式に変更・実装（詳細は `docs/specs/2026-07-25-account-switch-keychain-swap.md`）
- **セッションガードの緩和**: 実運用でシェルセッションが常時複数開いており「0件」を前提としたハードブロックでは切替・追加・再ログインが一切できなくなったため、「確認 + force」方式に緩和・**実装済み**
- **sync-back の同一性検証**: 実機観測でセッションが期限の数時間前でも自動 refresh してライブ Keychain を書き換えることが判明したため、`last_live_hash` + profile API 確認による同一性検証を追加・**実装済み**

過去の実装ログ・設計決定の経緯は `docs/dev-log.md` を参照。
