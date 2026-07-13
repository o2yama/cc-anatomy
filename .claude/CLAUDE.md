# cc-dashboard（アプリ名: CC Anatomy）

Claude Code の環境と活動状況を「解剖」して可視化するデスクトップアプリ（Tauri v2 + React + TypeScript）。
2026-07-12 に cc-dashboard → **CC Anatomy** に改名（identifier: com.o2yama.cc-anatomy）。フォルダ名は据え置き。

## ゴール

`~/.claude` / `~/.claude-mem` に散らばる Claude Code の活動データを1つのダッシュボードで俯瞰する：

1. ディレクトリ（プロジェクト）ごとのセッション履歴一覧と「何をやったか」のサマリー
2. Skills / Agents のインベントリ
3. （将来）hooks・plugins・rules などサブシステムの俯瞰

## 設計決定（2026-07-05）

| 決定 | 理由 |
|---|---|
| サマリーは **claude-mem の SQLite**（`~/.claude-mem/claude-mem.db`）から読む | jsonl（3.9GB・63,003ファイル）を直接インデックス化すると容量・管理で破綻する。claude-mem に構造化済みサマリー（request/investigated/learned/completed/next_steps）が4,112件あり、FTS5全文検索も構築済み |
| jsonl は**ドリルダウン時のみ遅延読み** | 会話全文を見たい瞬間に該当セッションの1ファイルだけ読む。事前インデックス不要 |
| DB は**読み取り専用で開く**（SQLITE_OPEN_READ_ONLY） | claude-mem worker が常時書き込むDBを壊さないため。書き込みは絶対禁止 |
| `project = 'unknown-project'` を除外 | 46,761件のノイズ（claude-mem observer セッション等）。実プロジェクトは約25件 |
| jsonl の探索は**セッションUUID（ファイル名）で検索** | claude-mem の `project` カラムは basename でフルパスと突合できないため。`~/.claude/projects/*/<session_id>.jsonl` を探す。ノイズディレクトリ（`-`、`*-claude-mem-observer-sessions`）はスキップ |

## データソースの構造メモ

- `sdk_sessions`: content_session_id（= jsonl ファイル名の UUID）, memory_session_id, project(basename), user_prompt, started_at_epoch(ms)
- `session_summaries`: memory_session_id で sdk_sessions と JOIN。**1セッションに複数行**（prompt_number ごと）
- 全セッション49,145件に対しサマリー4,112件。サマリー無しセッションは user_prompt で代替表示
- jsonl: 1行1イベント。`type:"user"/"assistant"` 行の `message.content` が会話本体。attachment / meta 行はスキップ
- Skills: `~/.claude/skills/*/SKILL.md`（YAML frontmatter の name/description）
- Agents: `~/.claude/agents/*.md`（同上）

## 開発コマンド

- `npm run tauri dev` — 開発起動
- `npm run tauri build` — 配布ビルド
- `scripts/release.sh <version> ["ノート"]` — リリース一式（バージョン反映→署名ビルド→latest.json→commit/tag/push→GitHub Release）

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

## 既知の落とし穴

- **Ice（メニューバー管理アプリ）が新規トレイアイコンを画面外（x=-8000台）に飛ばす**。隠しセクションにも出ないことがある。Ice 再起動 or レイアウト設定で表示側に割り当てて解決
- アプリ更新時は .app 差し替えだけでは反映されない。実行中プロセスの kill → open -a 再起動まで必要
- tauri build の .dmg 生成はバックグラウンドシェルからだと失敗する（Finder 操作が絡む）。フォアグラウンドで実行する
- 配布物は無署名・aarch64 のみ。初回起動は「プライバシーとセキュリティ → このまま開く」が必要
