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
- **監視用長期トークンの仕組みは全廃**（2026-07-25 ユーザー決定）: 切替が Keychain スワップで簡単になったため、複数アカウントの使用量を並べて見る機能自体を廃止した。`CC Anatomy-token-<name>` / `CC Anatomy-active`（Keychain）、setup-token 登録フロー（`add_account_in_terminal`/`claim_pending_account`）、`accounts_usage_detail`/`AccountUsageDetail`、UI の「使用量監視用トークン」セクションを削除。使用量は常に「現在ライブのアカウントのみ」を `/api/oauth/usage`・`/api/oauth/profile`（ライブ access token 直叩き）で表示する一本道にした（トレイ・ツールバーとも）。起動時マイグレーション `remove_legacy_monitor_tokens()` が旧 Keychain エントリを一度だけ削除する（冪等）。`accounts.json` は旧フィールドが残っていても読める後方互換を維持

### 未検証事項

- アカウント切替（Keychain スワップ方式）: 取り込み→切替→`claude auth status`確認、A→B→A往復、`.zshrc`撤去、`~/.claude.json`の`oauthAccount`以外不変、Keychain ACL 保持、同一アカウント再ログイン後の切替、「確認+force」フロー・profile 確認による同一性検証、いずれも実機の Keychain 書き換えを伴うため未検証（詳細は `docs/specs/2026-07-25-account-switch-keychain-swap.md` の検証項目）
- 監視用長期トークン全廃後のトレイ・ツールバー表示（ライブアカウントのみ表示、他アカウントは名前のみ列挙）の実機確認

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

### 2026-07-31 の決定

- **ドロワー内ドキュメント編集機能を実装・main マージ済み**: プロジェクト概要でファイル名クリック → 右ドロワー（`DetailDrawer`）内の `DocEditor.tsx`（CodeMirror 6）で CLAUDE.md 等を直接編集・Cmd+S 保存できる。保存は `write_doc`（`env.rs`）の楽観ロック（modified_epoch）で外部変更と競合検出し、conflict 時は専用 UI で再読込を促す。path が null / truncated のコンテンツは読み取り専用。日本語 IME はスパイク（SpikeEditor、削除済み）で CodeMirror 6 の問題なしを確認してから採用
- **未検証事項**: 実機 UI での編集→保存、IME 入力、conflict 検出 UI、未保存離脱ガードの動作確認（ビルド・型チェック・cargo check は通過済み）
- **ドキュメントAI分析機能を実装**（macOS 限定）: エディタ右上の ✨ ボタン（「AIに分析・改善してもらう」）→ `doc_analysis.rs` が `claude -p`（sonnet, stream-json, `--permission-mode dontAsk`, `--max-turns 40`, timeout 600s）を read-only 許可リスト `Read,Glob,Grep,WebFetch(domain:code.claude.com|docs.anthropic.com)` で起動し、ファイル種別に応じた Anthropic 公式ドキュメント（memory/skills/sub-agents/settings）を実行時に WebFetch して照合した改善提案をドロワー内パネルに表示。認証は claude CLI の優先順位そのまま（= ログイン中アカウント。サブスク/API 両対応。`--bare` は OAuth を読まないため不採用）。エディタの未保存バッファを正として分析（上限10万字）。`--setting-sources user --strict-mcp-config` で cwd プロジェクトの settings/hooks/MCP を遮断。stdin 書き込みは専用スレッド（20万字級でのパイプ相互デッドロック防止）。`doc_analysis::is_running()` をアカウント切替の `ensure_app_not_busy()` に登録済み（本アプリ自身のプロセスは常時ハードブロックの不変条件を維持）。グローバルスコープ文書（rules 等）は projectDir を渡さず文書単体+公式照合モードで分析。Opus レビュー1巡（ブロッカー1+要修正4+軽微10）対応済み
- **AI分析の未検証事項**: 実機での `claude -p` 実起動・WebFetch ドメイン許可の実効・進捗ストリーミング表示・キャンセル・分析中アカウント切替のブロック動作

### 2026-07-25 の決定

- **ゴール再定義**: 「Claude Code の活動データを1ダッシュボードで俯瞰する」から「PC の環境を Claude Code が最も効果的に動けるようにセットアップする」に最終ゴールを拡大（ユーザー決定）。可視化（ダッシュボード）は最終ゴールの手段・土台という位置づけに変わる。メニューバーから見える要素は現状維持
- **アカウント切替方式の変更**: 「setup-token 長期トークン + `CLAUDE_CODE_OAUTH_TOKEN` 注入」方式から「Keychain の `Claude Code-credentials` をアカウント別スナップショットでスワップし PC 全体のログインを書き換える」方式に変更・**実装済み**（詳細は上記「現在有効な設計決定」および `docs/specs/2026-07-25-account-switch-keychain-swap.md`）。実機での Keychain 書き換え検証は未実施
- **監視用長期トークンの全廃**: 切替が Keychain スワップで簡単になったため、複数アカウントの使用量を並べて見る機能自体を廃止。使用量は現在ライブのアカウントのみ表示する方式に変更・**実装済み**（詳細は上記「現在有効な設計決定」および `docs/specs/2026-07-25-account-switch-keychain-swap.md`）
- **セッションガードの緩和**: 実運用でシェルセッションが常時複数開いており「0件」を前提としたハードブロックでは切替・追加・再ログインが一切できなくなったため、「確認 + force」方式に緩和・**実装済み**
- **sync-back の同一性検証**: 実機観測でセッションが期限の数時間前でも自動 refresh してライブ Keychain を書き換えることが判明したため、`last_live_hash` + profile API 確認による同一性検証を追加・**実装済み**

過去の実装ログ・設計決定の経緯は `docs/dev-log.md` を参照。
