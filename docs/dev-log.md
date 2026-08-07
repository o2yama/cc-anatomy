.claude/CLAUDE.md から 2026-07-22 に移設した開発ログ・設計決定の記録

## 設計決定（2026-07-05）

| 決定 | 理由 |
|---|---|
| サマリーは **claude-mem の SQLite**（`~/.claude-mem/claude-mem.db`）から読む | jsonl（3.9GB・63,003ファイル）を直接インデックス化すると容量・管理で破綻する。claude-mem に構造化済みサマリー（request/investigated/learned/completed/next_steps）が4,112件あり、FTS5全文検索も構築済み |
| jsonl は**ドリルダウン時のみ遅延読み** | 会話全文を見たい瞬間に該当セッションの1ファイルだけ読む。事前インデックス不要 |
| DB は**読み取り専用で開く**（SQLITE_OPEN_READ_ONLY） | claude-mem worker が常時書き込むDBを壊さないため。書き込みは絶対禁止 |
| `project = 'unknown-project'` を除外 | 46,761件のノイズ（claude-mem observer セッション等）。実プロジェクトは約25件 |
| jsonl の探索は**セッションUUID（ファイル名）で検索** | claude-mem の `project` カラムは basename でフルパスと突合できないため。`~/.claude/projects/*/<session_id>.jsonl` を探す。ノイズディレクトリ（`-`、`*-claude-mem-observer-sessions`）はスキップ |

## クロスプラットフォーム対応（2026-07-14 実装）

macOS universal（Apple Silicon + Intel）と Windows（監視機能のみ）に対応。

| 決定 | 理由 |
|---|---|
| macOS は universal binary 1本で配布。latest.json は darwin-aarch64 / darwin-x86_64 の両キーに同一アセット | 配布物・cask・latest.json の管理を1系統に保つ。既存 aarch64 ユーザーは darwin-aarch64 キー経由でシームレスに universal へ更新される |
| リリースビルドは GitHub Actions（release.yml、tauri-action）。`max-parallel: 1` 必須 | Windows は Mac からクロスコンパイル不可のため CI 化が必須。tauri-action の latest.json 統合は read-modify-write で、並列実行するとプラットフォームエントリが消える |
| Windows 第1弾は監視のみ（使用量・トレイ・自動更新）。アカウント切替・環境診断・右クリックメニューは macOS 限定 | それらは Keychain・`.zshrc` 注入・Terminal.app・claude CLI パス解決に依存し、Windows では根本的に再設計が要る |
| 非 macOS は accounts / diagnostics を**同一シグネチャのスタブに `#[path]` 差し替え**。コマンド登録もフロント API 契約も変えない | 呼び出し側（lib.rs / tray.rs / actions.rs）を無修正に保つ。コンパイル時にコマンドを消すとフロントの invoke 漏れが即クラッシュになるため、スタブがエラー文字列を返す安全網にする |
| ライブ資格情報は `credentials.rs` に抽象化。macOS = Keychain、Windows/Linux = `~/.claude/.credentials.json`（`CLAUDE_CONFIG_DIR` 対応） | Claude Code の公式仕様。JSON 構造（`claudeAiOauth.accessToken`）は全 OS 共通 |
| API 呼び出しは curl CLI → **reqwest**（blocking + rustls-no-provider + rustls/ring） | Windows では GUI からのサブプロセス起動がコンソール窓を出す。TLS 構成は tauri-plugin-updater と同一にして依存を共有。**`rustls::crypto::ring::default_provider().install_default()` を呼ばないと実行時 panic**（actions.rs の HTTP クライアント初期化に組み込み済み） |
| `date` コマンド → chrono | Windows に互換 `date` が無い |
| トレイの使用率バッジ（`set_title`）は macOS のみ。他 OS はツールチップ + メニュー内ゲージ | tray-icon の title 描画は macOS 限定機能 |
| フロントの OS 出し分けは `get_platform` コマンド + `src/platform.ts` の `useIsMac()` | プラグイン（@tauri-apps/plugin-os）追加より軽い。取得完了までは macOS 挙動に倒す |
| Windows 版の検証は windows-build.yml（手動実行）で NSIS を作り、Windows 11 ARM VM の Prism x64 エミュレーションで行う | 手元に Windows 実機が無い。検証が取れるまで release.yml の matrix に windows-latest を**入れない**（未検証バイナリを配布しないため） |

- Windows 配布を開始するとき: release.yml の matrix に `- platform: windows-latest` / `args: '--bundles nsis'` を追加するだけ（tauri-action が windows-x86_64 キーを latest.json にマージする）
- 無署名 .exe は SmartScreen 警告が出る（「詳細情報」→「実行」で回避可能）。README に手順を書くこと
- CI Secrets: `TAURI_SIGNING_PRIVATE_KEY`（= ~/.tauri/cc-anatomy.key の中身）/ `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（空文字列）

## 自動アップデート（2026-07-13 実装・実機検証済み）

- リポジトリ: https://github.com/o2yama/cc-anatomy （public、o2yama アカウント）
- 仕組み: tauri-plugin-updater が GitHub Releases の `latest.json` を起動15秒後+12時間ごとに確認 → 確認ダイアログ → `.app.tar.gz` をダウンロード・差し替え → 再起動。トレイに「アップデートを確認」（手動）
- **署名鍵: `~/.tauri/cc-anatomy.key`（パスワードなし）。紛失すると以後の更新配信が不能になる。要バックアップ**。公開鍵は tauri.conf.json の `plugins.updater.pubkey`
- リリースは必ず `scripts/release.sh` で行う（latest.json の signature/url 生成を手作業にしない）
- 無署名（Apple Developer 署名なし）でも updater 経由の更新は quarantine が付かず Gatekeeper に阻まれないことを実機確認済み（v0.1.1→v0.1.2）
- v0.1.0（updater なし）を配布済みの相手には v0.1.1 以降の dmg での手動再インストールを一度だけ依頼する必要がある

## 追加の設計決定（2026-07-12〜13）

| 決定 | 理由 |
|---|---|
| レートリミット/アカウントは Keychain の Claude Code 資格情報で OAuth usage/profile API を叩く | ローカルにキャッシュが無い。非公開APIなので仕様変更時は表示が落ちる前提（フォールバック表示あり） |
| claude CLI / cmux は絶対パスで解決（/opt/homebrew/bin 等） | GUI アプリは zsh の PATH（.zshrc）を継承しない。zsh -lc でも .zshrc は読まれない |
| claude-mem 無し環境では transcript フォルダからプロジェクト一覧を復元 | 配布先に claude-mem が無くても最低限動かす。メモリ系 UI は has_claude_mem フラグで非表示 |
| ~/.claude 配下の cwd を持つプロジェクトはツリー非表示（DBは触らない） | スキル開発等の作業痕跡でありユーザーのプロジェクトではない。claude-mem の記憶は保全 |
| メニューバー常駐は NSStatusItem + 5分毎更新。メニュー操作は必ずメインスレッド | macOS の NSMenu 制約。取得スレッドからは文字列だけ渡して run_on_main_thread で反映 |
| ウィンドウ✕は hide のみ（終了はトレイの「終了」） | メニューバー常駐を維持するため |

## 環境診断機能（2026-07-13 実装）

ローカルの Claude Code アカウントで PC 環境全体（ホームディレクトリ）をスキャンし、改善ポイントをレポートする機能。トップバーの心拍アイコンから起動。実装: `src-tauri/src/diagnostics.rs` + `src/Diagnosis.tsx`。

| 決定 | 理由 |
|---|---|
| 診断は `claude -p` headless（stream-json）を read-only で実行、修正は Terminal.app で claude を対話起動 | 配布アプリ自身はユーザーファイルに一切書き込まない。危険操作はユーザーの目の前の通常許可フローにかける |
| read-only の担保は `--permission-mode dontAsk` + `--allowedTools`（読み取り系のみ）。git は `git status/log/diff/stash list` に限定 | プロンプト指示ではなく許可境界で保証する。`git stash:*` 等はプレフィックスマッチで書き込み系（stash pop）まで通るため不可 |
| `--bare` は使わない | Keychain/OAuth を一切読まない仕様（v2.1.207 実機確認）のため、サブスクログインと両立しない |
| 暴走防止は Rust 側 15 分タイムアウト + キャンセルボタン（PID kill） | このバージョンの CLI に `--max-turns` が無い |
| stderr は別スレッドで drain する | stdout だけ読むと stderr のパイプバッファ満杯で相互ブロックのデッドロック |
| 診断モデルは sonnet 固定、進捗は Tauri events（diagnosis-progress） | ユーザーのデフォルトモデル（opus 等）だとレートリミット消費が読めない |
| 修正実行は fix_prompt をファイル経由で `"$(cat '固定パス')"` 渡し + acceptEdits + mv/mkdir 等のみ事前許可 | モデル出力をシェルに直接埋め込まない（インジェクション遮断）。rm は許可せず通常プロンプトに落とす |
| 診断プロンプトに「for/while ループ禁止・1コマンドずつ」を明記 | 複合コマンドは許可リストに合致せず拒否される（e2e で確認）。`Bash(for:*)` を許可するとループ本体に任意コマンドが通るため許可側では対応しない |
| DiagnosisOverlay は常時マウントで `open` prop 切り替え | 実行中に閉じても診断を継続し、再度開いたとき結果を受け取れる |
| `Bash(echo:*)` は許可しない | アカウント機能で診断プロセスに長期トークンを環境変数で渡すため、`echo $CLAUDE_CODE_OAUTH_TOKEN` が通ると transcript に平文で残る |

## アカウント切り替え機能（2026-07-13 実装）

複数の Claude サブスクアカウントを登録し、トークン消費先を切り替える機能。トップバーの人物アイコンから起動。実装: `src-tauri/src/accounts.rs` + `src/Accounts.tsx`。

### 実機検証で判明した Claude Code の認証仕様（この設計の前提）

- ライブ資格情報は Keychain の `Claude Code-credentials`（JSON。`claudeAiOauth.accessToken` は約8時間、`refreshToken` は約3〜4週間）
- **リフレッシュのたびに refreshToken もローテートされ、古い refreshToken はサーバー側で即無効化される**（旧トークンを復元すると `OAuth session expired and could not be refreshed`）
- **実行中の Claude Code セッションは、自分のメモリ上のトークンでリフレッシュし、結果をライブ Keychain と `~/.claude.json` に上書きする**
- 上記2点により、**ライブ Keychain を差し替える方式は破綻する**（常駐セッションが差し替えた資格情報を踏み潰し、そのアカウントが再ログイン不能になる。検証中に実際に破壊した）
- `CLAUDE_CODE_OAUTH_TOKEN` はサブスク OAuth より優先され、**ライブ Keychain を一切汚染しない**（実機で認証成功・ハッシュ不変を確認）
- **⚠ 最重要: `CLAUDE_CODE_OAUTH_TOKEN` が無効でも、CLI は警告なくライブ Keychain の資格情報にフォールバックして成功する**（v2.1.207 実機確認。ゴミトークンでも `claude -p` が通る）。`claude auth status` は `authMethod: "oauth_token"` と表示するだけで正当性を検証しない。つまり**壊れたトークンを保存しても CLI 経由では一切気づけず、切り替えたつもりで別アカウントに課金され続ける**
- トークンの生死を確かめられるのは **API を直接叩く経路だけ**（`POST /v1/messages`、`max_tokens:1`）。無効なら 401 `Invalid bearer token` が返る。CC Anatomy は追加時と切り替え時にこれで検証する
- **OAuth の `profile` / `usage` API は長期トークンを拒否する**（実測）。`profile` は `OAuth token does not meet scope requirement any_of(user:profile, user:office)` を返す。`usage` は紛らわしく `Rate limited. Please try again later.` を返すが、**同一アカウント・同時刻で live トークンは受理されるのでレート制限ではなくスコープ拒否**。したがって長期トークンでは**メールアドレス・組織名を取得できない**（アカウントの識別はユーザーが付けた名前で行う）
- 選択中アカウントの使用量は **`/v1/messages` のレスポンスヘッダ `anthropic-ratelimit-unified-*`** から取れる（`5h-utilization` / `7d-utilization` は 0〜1、`*-reset` は epoch 秒）。`anthropic-organization-id` ヘッダでどのアカウントに課金されたかも判別できる。**この経路だけが「選択中アカウントの使用量」を正しく出せる**（usage API にフォールバックするとログイン中アカウントの数字を表示してしまい、消費先を誤認させる）
- macOS では `CLAUDE_CONFIG_DIR` を分けても Keychain は共有されるため、config 分離では認証を分けられない（公式ドキュメント記載）

### 設計決定

| 決定 | 理由 |
|---|---|
| `claude setup-token`（サブスク用・1年・ローテートなし）の長期トークンをアカウントごとに Keychain（`CC Anatomy-token-<name>`）へ保管し、選択中を `CC Anatomy-active` に複製 | 上記のとおりライブ Keychain 差し替え方式は常駐セッションに破壊される。長期トークンは env 経由で効くのでライブを触らずに済む |
| 切り替えの反映は `.zshrc` に「Keychain から読んで `CLAUDE_CODE_OAUTH_TOKEN` に export」する行を追加して行う。トークン本体は .zshrc に書かない | 新しいシェルが起動のたびに最新の選択を拾う。値が空のときは export しない（未登録時に認証を壊さないため） |
| Keychain への書き込みは `security` CLI 経由（argv にトークンが載る） | シェル起動時に `security` CLI から**無プロンプトで読める**ことが設計の必須要件。crate 経由で書くと ACL が変わりシェル読み取りのたびに承認ダイアログが出る。なお同一ユーザーの攻撃者は argv を覗くまでもなく Keychain を直接読めるため、argv 露出は新たな露出経路を増やさない |
| API 呼び出しの `curl` は `-K -`（設定を stdin 渡し）でトークンを argv に載せない | 使用量ポーリングのたびに繰り返し晒すことになるため（こちらは頻度が高く、対策の実益がある） |
| アカウント追加は Terminal.app でヘルパースクリプトを対話実行し、`script(1)` で pty を割り当てて setup-token を走らせる | **claude CLI は stdout がパイプ／stderr が `/dev/tty` 再オープンだと bun ランタイムが落ちる**（`EINVAL: kqueue` / `process.stderr.fd` undefined。実機で発生）。出力を捕捉しつつ正常起動させるには pty が要る。取得したトークンはスクリプトが直接 Keychain に入れ、GUI には渡さない。記録ファイルは 0600 で作り抽出後に `rm -P` で上書き削除する（setup-token 自身が端末にトークンを表示するのは仕様上避けられない） |
| アカウント選択中は、使用量を `/v1/messages` のレート制限ヘッダから組み立てる。profile はライブへフォールバックせず選択中の名前だけを返す | usage/profile API は長期トークンを拒否する（上記）。ライブ資格情報にフォールバックすると、選択中と違うアカウントの使用量・メールを表示して消費先を誤認させる |
| 追加時・切り替え時に `POST /v1/messages`（max_tokens:1）でトークンの生死を検証する | CLI は無効トークンを黙って握り潰してライブ資格情報にフォールバックするため、CLI 経由では検証できない。ここを通さないと壊れたトークンのまま「切り替えたつもり」になる |
| **検証失敗でトークンを削除しない**。認証失敗（401 / `authentication_error`）と、レート上限・通信断（`Unavailable`）を区別する | 検証失敗＝無効とみなして Keychain から消す実装にしたところ、**有効な1年トークンを実際に破壊した**。レート枠を使い切ったアカウントは 429 を返すがトークン自体は有効で、切り替え先としても正当。削除してよいのは 401 のときだけだが、取り直しは追加操作の冒頭で行うので削除自体が不要 |
| `active_token()` を `oauth_token` / 診断 / タスク抽出 / cmux 起動に注入 | 注入しないと、UI で選んだアカウントと実際の消費先・使用量表示がずれる |
| 「起動中セッション数」はシェル（zsh/bash/fish/sh）から起動された claude だけを数える | claude CLI プロセスをそのまま数えると claude-mem のワーカー（親が bun）まで混ざり、実測で9件中3件が誤カウントだった |

### 制約（回避不能）

- **起動中のセッションは切り替えの影響を受けない**。トークン消費先を変えるにはセッションを開き直す必要がある（どの方式でも同じ）
- 長期トークンは1年で失効するため、その時点で再登録が必要
- 公式記載では `CLAUDE_CODE_OAUTH_TOKEN` は Remote Control セッションと `--bare` に非対応

### 実機で踏んだ罠（ヘルパースクリプト）

- **zsh の `status` は読み取り専用**。`status=$?` はエラーになる（bash なら通る）。`rc` を使う
- **setup-token は端末幅でトークンを折り返して表示する**。実機で108文字のトークンが2行に割れ、1行だけ拾うと79文字の壊れたトークンになった。継続行を継ぎ足す方式は本文（"Store this token securely"）まで巻き込んで155文字の壊れたトークンを生成した。**`stty cols 400` で pty を広げて折り返し自体を起こさせない**のが正解
- Ink の再描画で記録に途中まで描かれたトークン行が混ざるため、抽出は**最長一致**を採る
- 壊れたトークンを保存しても CLI では気づけないので、**保存後に必ず API 検証する**（上記）

### 未検証（実機ログインが必要）

- 追加 → 切り替え → 新しいターミナルで消費先が実際に変わることの e2e 確認（別アカウントの使用量が増えることの確認）

## 手動「セッション更新」ボタンの廃止・自動化（2026-07-26 実装）

| 決定 | 理由 |
|---|---|
| ヘッダーの「セッション更新／取り込む」ボタンを廃止し、`sync_back_live_login` を60秒ごとの tray 定期更新ループから自動実行する（`auto_sync_live`）。UI へは "accounts-updated" イベントで通知する | 「登録済み＋ハッシュ変化あり」なら人手を介さず取り込めるため、ボタンでの手動操作自体が不要になった。アカウント画面を開いた時しか同期されない問題も解消する |
| 登録済みカードの「再ログイン」導線（`!has_credentials` のとき表示）は、既存の「＋アカウントを追加」フローに `target_name`（org_id 照合）を追加する形で流用する。汎用の「＋アカウントを追加」は対象なし（従来どおり誰でログインしても取り込む） | 対象アカウントを意識せず誰のログインでも取り込む既存フローをそのまま再ログインに使うと、別アカウントでログインした場合に誤って別の登録を更新・新規作成してしまう。org_id 不一致時は import せず mismatch を返すことで誤紐づけを防ぐ |

### 独立レビューを受けた追加修正（2026-07-26 同日中）

| 決定 | 理由 |
|---|---|
| `accounts.json` の read-modify-write を `static META_LOCK: Mutex<()>` で直列化する。ロックは最外殻のエントリポイント（Tauri コマンド・tray ループ）でのみ取り、内部関数（`import_live_account_locked` 等）はロック保持前提とする | `auto_sync_live`（60秒ごと）とユーザー操作（切り替え・削除・改名・並び替え等）が並行に load→save すると、後勝ちの save が他方の変更を巻き戻す（削除したアカウントが復活する等）。std::sync::Mutex は再入不可のため、`poll_add_account_login → import_live_account` のようなネスト呼び出しでは呼び出し先でロックを取らせず、内部実装（ロック無し版）を直接呼ぶ構成にして二重ロックを回避する |
| ロック区間は「meta の read-modify-write」だけに絞り、profile API・setup-token 検証・Terminal 起動（osascript）等の外部 I/O はロック外で行う。ロック外で判断材料を得た後、ロックを取り直して書き込み直前に前提（last_live_hash 等）が変わっていないか再検証してから書き込む（変わっていたら書き込まず次サイクルに委ねる） | 外部 I/O は数秒〜分単位でブロックしうる（profile API、オートメーション許可ダイアログ等）。ロック内に含めると「一覧表示は使用率取得にブロックされない」という既存設計に反し、無人で動く `auto_sync_live` がユーザー操作を長時間ブロックしてしまう。ロックを手放す代わりに、書き込み直前の再検証（TOCTOU 対策）で「ロックを手放していた間に前提が変わっていたら書き込まない」安全側に倒す |
| `last_checked_hash`（`last_live_hash` とは別フィールド）を導入する | `last_live_hash` は「登録済みとして書き戻し成功」したときしか更新されない。未登録ライブが居座るケースでは毎サイクル `resolve_live_owner`（＝ profile API 呼び出し）が走り続けてしまうため、「登録の有無に関わらず確認はした」という記録を別に持つ |
| 再ログイン導線で mismatch を検知したら、setup-token（常時監視・任意機能）の pending トークン（`PENDING_MONITOR_TOKEN_SVC`）を破棄する | ログインスクリプトはブラウザログイン成功後に続けて setup-token も実行するため、誤ったアカウントでログインした場合でも pending トークンが残る。放置すると、次回の正しいやり直しで `poll_monitor_setup` がこの残骸を拾って偽の mismatch を起こす |

## 使用量取得のライブ優先決め打ちをフォールバック連鎖に変更（2026-07-27）

| 決定 | 理由 |
|---|---|
| `resolve_usage_source`（単一ソースを返す）を `resolve_usage_source_order`（優先順位リストを返す）に再設計。ライブアカウントは ライブOAuth →（失敗なら）監視トークン →（無ければ/失敗なら）スナップショットOAuth の順に、成功するまで1つずつ試す。トレイタイトルの `live_usage_summary()` も同じ考え方で、失敗時だけライブアカウントの監視トークン（`live_account_monitor_token`）にフォールバックする | 従来はライブなら常にライブ OAuth 一本勝負で、失敗時は他ソースを試さず直接キャッシュ（トレイは「-」）へ落ちていた。ライブトークンはスナップショット由来で、久しぶりに切り替えたアカウントほど期限切れになりやすく、リフレッシュは Claude Code 起動時にしか起きないため、「切り替えた直後なのにメニューバーの使用量が見えない」空白期間が生じていた |
| 監視トークンでのフォールバックは、ライブ OAuth が失敗したときだけ叩く（`live_usage_summary().or_else(...)` で連鎖させ、成功時は監視トークンに一切触れない） | 監視トークン照会は実 API リクエスト（haiku 1トークン消費）で、毎分実行されるトレイの定期更新から無条件に叩くとコスト・レート消費が無視できない |

## resolve_live_owner のエラー分類と wire format 契約（2026-08-08）

`resolve_live_owner`（accounts.rs）は expiresAt 事前チェックを追加し、失敗を `OwnerError`
（`TokenExpired(Option<email>)` / `NetworkError` / `Other(String)`）に分類するようにした
（issue #1・#2 対応）。既存の Tauri コマンド境界（`Result<_, String>`）は変えず、
`OwnerError::Display` がメッセージ先頭に `KIND:` プレフィックスを埋め込んで運ぶ。

- **wire format**: `"<KIND>:<message>"`。`KIND` は `TOKEN_EXPIRED` / `NETWORK_ERROR` / `OTHER` のいずれか（`OwnerError::kind()` が唯一の発生源）
- **消費者**（プレフィックスを剥がして本文だけ表示する）:
  - TS（Tauri コマンド境界を越える経路）: `src/api.ts` の `describeAccountError`。`Accounts.tsx`（doSwitch/startAddLogin）・`App.tsx`（ヘッダーの切替）が使う
  - Rust（コマンド境界を越えない経路。tray のネイティブダイアログ等）: `src-tauri/src/accounts.rs` の `strip_owner_error_tag`。`tray.rs::switch_from_tray` が使う
- 2つの消費者は独立実装だが「既知の kind 一覧」を暗黙に共有している。`OwnerError::kind()` に kind を増減したら、`strip_owner_error_tag` の `KNOWN_KINDS` と `api.ts` の `OWNER_ERROR_PREFIXES` の両方を揃えること（自動同期の仕組みは無い）
- `OwnerMismatch` は一度 variant として追加したが、`resolve_live_owner` が実際には送出しない（mismatched は既存の `LiveOwner.mismatched` で表現済み・NeedsImport 導線に流れる）ため YAGNI で撤去した。必要になったら kind を1つ増やす形で足す
