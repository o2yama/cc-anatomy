# 使用量取得まわりの仕様（v0.5.3 時点 / 2026-08-22 午前）

> **注意: この文書は v0.5.3 の記述です。v0.5.4 で以下が変わっており、まだ反映していません。**
> - 段階的バックオフ（5→10→20→40→60分）は**撤去**した
> - ライブの照会間隔を 45秒 → **300秒**にした
> - 「最後に試行した時刻」のゲートを追加し、**失敗が続く間も間隔が保たれる**ようにした
> - 注記の判定を `live_error` から「表示中の値の古さ」に変えた。
>   「取得が一時的に制限されています（最新でない可能性）」の注記は**廃止**
> - `/v1/messages`（監視トークン経由）が使用量を消費する点は**変わっていない**
>
> 全面改訂は別途行う。それまでは `docs/dev-log.md` の 2026-08-22 の記述を優先すること。

判断材料として、コードから裏を取った事実だけを書く。
各項目に検証状況を付ける。**［確認］**＝コードまたは実測で確認、**［未確認］**＝確かめていない。

---

## 1. 定期的に自動で走るもの

| 何が | 間隔 | 定義場所 | ネットワーク |
|---|---|---|---|
| トレイの使用量更新（`tray::refresh`） | **60秒** | `tray.rs:44` `REFRESH_INTERVAL` | あり（下記2） |
| 自動アップデート確認 | 起動15秒後 → 以降**12時間**ごと | `updater.rs:13,14` | あり（GitHub） |
| token 自動復帰の裏起動（期限切れ検知時のみ） | 発火は最短**10分**間隔 | `actions.rs:735` `TOKEN_NUDGE_MIN_INTERVAL` | `claude` CLI 経由 |

**［確認］** これ以外に定期実行はない。フロントエンド（`src/`）に使用量のポーリングは無く、`setInterval` は2箇所ともアカウント追加フローの一時的なポーリングのみ（`Accounts.tsx:540,654`）。

60秒サイクルの中身は順に:
1. `auto_sync_live()` — ライブ Keychain を読み、ハッシュが前回と違えば持ち主確認（下記2-3）
2. `get_accounts_usage(force=false)` — 登録アカウントの使用量照会
3. トレイメニューの再構築

---

## 2. ネットワークアクセスの全経路

### 2-1. `GET https://api.anthropic.com/api/oauth/usage`

**［確認］** `actions.rs:132` に URL 定義。呼び出しは3箇所。

| 呼び出し | 使うトークン | 頻度 |
|---|---|---|
| `accounts.rs:820` ライブ枠 | ライブ Keychain（`Claude Code-credentials`）の access token | 60秒ごと（キャッシュが45秒より古いとき） |
| `accounts.rs:856` スナップショット枠 | `CC Anatomy-cred-<name>` の access token | 非ライブは600秒ごと |
| `tray.rs:546` フォールバック | ライブ Keychain の access token | ライブが登録アカウントに居ないときだけ |

### 2-2. `POST https://api.anthropic.com/v1/messages` ← **要注意**

**［確認］** `actions.rs:343`。`{"model":"claude-haiku-4-5-20251001","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}` を投げ、レスポンスヘッダ `anthropic-ratelimit-unified-*` から使用率を読む。

**これは実際の推論リクエストで、使用量を消費する。**

使うトークンは監視用長期トークン `CC Anatomy-token-<name>`（`sk-ant-oat01...`、108文字）。

**［確認・重要］** `.claude/CLAUDE.md` には「監視用長期トークンの仕組みは全廃（2026-07-25）」「起動時マイグレーション `remove_legacy_monitor_tokens()` が旧 Keychain エントリを一度だけ削除する」と書いてあるが、**これは事実と異なる**。

- `remove_legacy_monitor_tokens` という関数はコードに存在しない
- Keychain に `CC Anatomy-token-share1` / `-share2` / `-share3` が**3本とも実在する**
- `has_monitor_token()`（`accounts.rs:341`）は3アカウントとも **true** を返す（`acct` 属性が `"taisei_o2yama"` で非 NULL）
- `actions.rs:7` のコメントに「2026-07-26 に任意機能として復活した監視用長期トークン」とある

つまり **2026-07-25 に全廃 → 2026-07-26 に復活** しており、CLAUDE.md の記述が古い。

**取得元の優先順位**（`accounts.rs:652` `resolve_usage_source_order`）:

- ライブアカウント: `/api/oauth/usage` → **失敗したら `/v1/messages`** → スナップショットで `/api/oauth/usage`
- **非ライブアカウント: `/v1/messages` が最優先** → スナップショットで `/api/oauth/usage`

したがって:

- 非ライブ2アカウントは、**600秒ごとに1回ずつ推論リクエストを投げている**
- ライブは `/api/oauth/usage` が 429 になるたびに推論リクエストへフォールバックする
- **429 バックオフ中でも `/v1/messages` はゲートされていない**（`accounts.rs:848`）。バックオフ中のライブは毎分1回、推論リクエストを投げ続ける

**［未確認］** 1リクエストあたりの使用量への寄与は測っていない。`max_tokens:1` なので極小のはずだが、ゼロではない。

### 2-3. `GET https://api.anthropic.com/api/oauth/profile`

**［確認］** `accounts.rs:1730, 1856`。ライブ Keychain の access token を使う。

呼ばれるのは、ライブ資格情報の SHA-256 ハッシュが前回記録（`last_live_hash` / `last_checked_hash`）と**違うとき**だけ（`accounts.rs:1573`, `1777`）。ハッシュが一致していれば呼ばない。

つまり **Claude Code が token を refresh するたびに1回**。定期的ではない。

### 2-4. GitHub（自動アップデート）

**［確認］** `updater.rs`。tauri-plugin-updater が latest.json を取得。12時間ごと。Anthropic の使用量とは無関係。

---

## 3. 1分あたりの実際の通信量

**［確認］** 登録3アカウント（ライブ1・非ライブ2、全員が監視トークンあり）の場合。

| 状況 | `/api/oauth/usage` | `/v1/messages`（推論） |
|---|---|---|
| 正常時 | 1.0/分 + 非ライブ分（10分に1回、ただし監視トークンが先なので通常0） | **0.2/分**（非ライブ2件 ÷ 10分） |
| ライブが 429 | 1/分 → バックオフ後は0/分 | **1.2/分**（ライブ毎分 + 非ライブ） |
| バックオフ中 | **0/分** | **1.2/分**（ゲートされない） |

**［実測 11:23〜11:27］** ライブ（share3）は 11:23:05 / 11:24:05 / 11:25:05 / 11:26:05 / 11:27:05 と正確に60秒ごと。非ライブ（share1/share2）は 11:18:05 のまま据え置き。

---

## 4. トークンの種類と、認証切れが起きうる箇所

| トークン | 保管先 | 有効期限 | 誰が更新するか |
|---|---|---|---|
| ライブ access token | Keychain `Claude Code-credentials` | **約8時間** | **Claude Code 本体のみ** |
| ライブ refresh token | 同上（同じ JSON 内） | 長期 | **Claude Code 本体のみ**。one-time use |
| スナップショット access token | Keychain `CC Anatomy-cred-<name>` | 取得時点から**約8時間** | **誰も更新しない** |
| 監視用長期トークン | Keychain `CC Anatomy-token-<name>` | ［未確認］ | 更新の概念なし。失効したら再発行 |

**［確認］アプリが refresh token を使って refresh する箇所は無い。** `accounts.rs` / `actions.rs` を通しで確認したが、`/oauth/token` を叩く実装は存在しない。

### 認証切れが起きうる箇所

**A. スナップショットの期限切れ（日常的に起きる・仕様）**
最後にライブだった時刻から約8時間で切れる。切れると `/api/oauth/usage` は打てず（`accounts.rs:785` の `token_is_still_valid` で事前に弾く）、監視トークン経由に落ちる。監視トークンがあれば使用量は取れ続ける。切り替えれば Claude Code が refresh して復活する。

**B. アカウント切り替えによる refresh token の巻き戻し（設計上の最大リスク）**
切り替えは Keychain の `Claude Code-credentials` を**スナップショット JSON でまるごと上書き**する（`accounts.rs:2467`）。この JSON には refresh token が含まれる。

refresh token は one-time use なので、スナップショットを取ったあとに Claude Code がその refresh token を使っていた場合、古いスナップショットを書き戻すと**そのアカウントは refresh できなくなり、再ログインが必要になる**。

これを防ぐのが sync-back（切り替え直前に、今ライブに居るアカウントの最新資格情報をスナップショットへ書き戻す）。ただし:

- 持ち主を確認できない（token 期限切れ・通信不能・**429**）と `SkippedUnverified` になり、sync-back を飛ばして切り替えが進む（`accounts.rs:1739` 付近）
- 「持ち主未確認でも続行しますか」の確認ダイアログで続行を選ぶと、この状態で切り替わる
- 部分適用に失敗すると `meta.inconsistent = true` が立ち、以後の sync-back が止まる

**［確認］** 429 を `NetworkError` に分類する変更（今回入れた）により、429 のときも「続行できる」側に倒れる。**続行すれば B のリスクを踏む可能性がある。**

**C. 外部セッションによる巻き戻し**
シェルで開いている claude セッションが自動 refresh してライブ Keychain を書き換える。切り替え直後に旧セッションが refresh すると、切り替え先が巻き戻る。`last_live_hash` とプロフィール確認で検知はするが、防止はしない。

---

## 5. Keychain とファイルの読み書き

**［確認］**

| 対象 | 読み | 書き | タイミング |
|---|---|---|---|
| Keychain `Claude Code-credentials` | あり | **あり** | 読み: 毎サイクル。書き: 切り替え時のみ |
| Keychain `CC Anatomy-cred-<name>` | あり | あり | 読み: 使用量照会・切り替え。書き: sync-back |
| Keychain `CC Anatomy-token-<name>` | あり | あり | 読み: 使用量照会。書き: 監視トークン登録時 |
| `~/.claude.json` | あり | あり | 書きは切り替え時、`oauthAccount` キーのみ差し替え |
| `~/.claude/cc-anatomy/accounts.json` | 毎サイクル | 使用量が更新できたサイクルのみ | 通常は毎分 |

Keychain 書き込みは `security add-generic-password -w <token>` で行うため、**argv 経由で `ps` から secret が見える**（`accounts.rs` の既知の制約）。

---

## 6. レート制限バックオフ

**［確認］** 状態は `actions.rs:479-482`（プロセス内グローバル1本）。

- 入る条件: `/api/oauth/usage` が HTTP 429 または本文 `error.type = "rate_limit_error"`
- 待ち時間: `min(5分 × 2^連続回数, 60分)` → 5 / 10 / 20 / 40 / 60分
- ゲートされるもの: `/api/oauth/usage` の全3経路（`accounts.rs:804, 852` / `tray.rs:543`）
- **ゲートされないもの: `/v1/messages`（監視トークン）と `/api/oauth/profile`**
- 解除: 照会が成功した時点、または待ち時間の経過
- 集計は `get_accounts_usage` 1回の呼び出し単位（`accounts.rs:899`）

**［実測］** 429 を意図的に起こすと、ちょうど5分間 `/api/oauth/usage` を打たなくなり、明けたら自動復帰する（2回再現）。

---

## 7. キャッシュと表示

**［確認］** キャッシュは `accounts.json` の `accounts[].usage_cache`（`five_pct` / `seven_pct` / `five_reset` / `seven_reset` / `fetched_at`）。

再照会の閾値:
- ライブ **45秒**（`accounts.rs:529` `USAGE_MIN_REFETCH_SECS`）
- 非ライブ **600秒**（`accounts.rs:533` `NON_LIVE_MIN_REFETCH_SECS`）
- `force=true` はライブの閾値だけをスキップする

### 表示側が鮮度情報を使っていない

**［確認］** バックエンドは `stale` と `fetched_at` を返しているが、**トレイもフロントも一切参照していない**。`grep` で確認済み（唯一のヒットはテストコード `tray.rs:1280`）。

したがって取得に失敗してキャッシュを表示している間も、見た目は最新値と区別がつかない。

### リセット時刻を過ぎたときの補正

| 表示 | 5h | 週次 | 「〇〇復活」の日時 |
|---|---|---|---|
| ライブ | 補正なし | 補正なし | 補正なし |
| 非ライブ | **0% とみなす**（`tray.rs:129`, `App.tsx:178`） | 補正なし | 補正なし（過去の日時が出続ける） |

---

## 8. アカウント切り替えの処理順

**［確認］** `accounts.rs::switch_account`

1. 本アプリ自身の子プロセス（AI分析・環境診断）実行中ならハードブロック
2. 外部の claude セッション数を `ps` で数え、1件以上なら確認ダイアログ（force で続行可）
3. **sync-back**: ライブ Keychain を読み、ハッシュが前回と違えば `/api/oauth/profile` で持ち主確認 → 一致すればスナップショットへ書き戻す
4. 切り替え先のスナップショットを Keychain から読む
5. **Keychain `Claude Code-credentials` を上書き**
6. `~/.claude.json` の `oauthAccount` を差し替え（一時ファイル経由の atomic rename）
7. 検証。失敗したらロールバック。ロールバックも失敗したら `meta.inconsistent = true`
8. `last_live_hash` を記録して保存

ネットワークを使うのは 3 の profile API だけ。

---

## 9. CLAUDE.md との食い違い（要修正）

1. **「監視用長期トークンの仕組みは全廃（2026-07-25）」は誤り。** 2026-07-26 に任意機能として復活しており、Keychain に3本実在し、コードも生きている。使用量取得の主経路の一つになっている（非ライブでは最優先）
2. **「起動時マイグレーション `remove_legacy_monitor_tokens()` が旧 Keychain エントリを一度だけ削除する」も誤り。** その関数は存在しない
3. 「使用量は常に『現在ライブのアカウントのみ』を表示する一本道にした」も誤り。非ライブの使用量も取得・表示している

---

## 10. まだ分かっていないこと

- **1リクエスト/分でも `/api/oauth/usage` が 429 になる理由。** 実測でこのエンドポイントはバースト5リクエスト程度で 429 に落ちる。アプリ以外に叩いているもの（Claude Code 本体、私の検証用リクエスト）の寄与を切り分けられていない
- 監視用長期トークンの有効期限
- `/v1/messages` の最小リクエストが使用量表示に与える寄与の実測値
- 429 時のトレイ文言の実表示（目視未確認）
