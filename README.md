# cc-dashboard

PC の環境を Claude Code が最も効果的に動けるようにセットアップするデスクトップアプリ（Tauri v2 + React + TypeScript）。まずは Claude Code の活動状況をローカルデータから可視化するところから始まる。

## 機能

- **セッション**: プロジェクト（作業ディレクトリ）別のセッション履歴一覧。claude-mem の構造化サマリー（依頼 / 調査 / 学び / 完了 / 次の一手）を展開表示
- **会話ドリルダウン**: セッションの jsonl を遅延読みして会話全文を表示
- **横断検索**: 全プロジェクトのサマリーを FTS5 全文検索
- **Skills / Agents**: `~/.claude/skills` / `~/.claude/agents` のインベントリ一覧

## データソース

| データ | 場所 | アクセス方法 |
|---|---|---|
| セッション・サマリー | `~/.claude-mem/claude-mem.db` | SQLite 読み取り専用 |
| 会話全文 | `~/.claude/projects/*/<session_id>.jsonl` | ドリルダウン時に遅延読み |
| Skills / Agents | `~/.claude/skills` / `~/.claude/agents` | frontmatter パース |

claude-mem プラグインの導入が前提。DB への書き込みは一切行わない。

## 開発

```bash
npm install
npm run tauri dev    # 開発起動
npm run tauri build  # .app / .dmg ビルド
```

設計判断の経緯は `.claude/CLAUDE.md` を参照。
