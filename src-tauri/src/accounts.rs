//! Claude サブスクアカウントの複数管理と切り替え。
//!
//! 切り替えは「PC 全体のログイン情報の書き換え」で行う: 選択先アカウントのスナップショット
//! （`CC Anatomy-cred-<name>`）をライブ Keychain（`Claude Code-credentials`）へそのまま書き込み、
//! `~/.claude.json` の `oauthAccount` も対応するものへ置換する。これにより新規ターミナル・
//! アプリ問わずどの経路で claude を起動してもスワップ後のアカウントが使われる。
//!
//! ## 旧結論を覆す理由（2026-07-25 reviewer 指摘への回答）
//!
//! 旧実装（1系）のヘッダには「ライブ Keychain は決して書き換えない」との記録があった。
//! 実機検証で否定された理由は次の2点:
//! 1. refresh token は one-time use でサーバー側にローテートされるため、素朴に上書きすると
//!    直前まで使っていたアカウントの refresh token が無効化され再ログイン不能になる。
//! 2. 実行中の Claude Code セッションが自分の（古い）トークンをライブへ書き戻し、
//!    切り替えた資格情報を踏み潰す。
//!
//! 本実装はこの2点を次で解消する:
//! 1. 切り替え・追加（`claude auth login`）の**直前に必ず sync-back**を行い、現在ログイン中の
//!    アカウントが登録済みならその最新資格情報をスナップショットへ書き戻してから上書きする。
//!    sync-back はベストエフォートにしない（読み取りに失敗したら中断する）。
//! 2. 外部セッションのガードは「確認 + force」方式（2026-07-25 ユーザー了解の上で緩和。
//!    経緯は下記）。本アプリ自身が起動する claude -p 子プロセス（環境診断・タスク抽出）は
//!    `ensure_app_not_busy` で常にハードブロックする（短時間で終わり、待てば済むため）。
//!
//! ### セッションガードの経緯（当初ハードブロック → 確認式に緩和）
//!
//! 当初は `running_sessions() > 0` の間、切り替え・追加そのものを一律ブロックしていた。
//! しかし実運用のユーザー環境ではシェルセッションが常時4件程度開いており、「0件のタイミング」
//! が実質存在せず、機能自体が使えなくなった（2026-07-25 ユーザー報告）。
//! ユーザー了解の上で「確認 + force」方式に緩和した: `switch_account` / `start_add_account_login`
//! に `force: bool` を追加し、`force=false` かつ外部セッションが1件以上あれば
//! `SessionsRunning { count }` を返して呼び出し側（UI）に確認を求める。ユーザーが
//! 「続行する」を選んだ場合のみ `force=true` で再実行する。**リスクは残ったまま**であり
//! （起動中セッションが古いトークンを書き戻して切り替えが巻き戻る、保存済みアカウントが
//! 後で再ログイン必要になる、等）、確認ダイアログの文言でこれを明示する。
//!
//! ## 既知の制約（対応不要と裁定済み）
//!
//! - `~/.claude.json` の read-modify-write はレースの余地があるが、書き戻しは
//!   tmp ファイル経由の atomic rename なので、競合しても「壊れた JSON」にはならない
//!   （最後に rename した側の内容で確定するだけ）。
//! - `security add-generic-password -w <secret>` は argv 経由で渡すため、実行中は
//!   `ps` から refresh token 等が見える（旧実装から継続の制約）。
//!   TODO: Security.framework のネイティブ API 呼び出しに置き換えれば解消できるが、
//!   本改修のスコープ外として見送る。
//! - `org_id` が空の旧エントリ同士は email でしか同一判定できず、email が変わっていると
//!   二重登録されうる。実データはすべて org_id を持つため今回は対応を見送る。
//! - TODO(perf): `get_accounts` は `~/.claude.json` を複数回パースしている
//!   （`live_org_id` / `sync_active_pointer` / `live_oauth_account` 等が独立に読む）。
//!   呼び出し頻度・ファイルサイズ的に現状は許容範囲だが、気になるなら1回読んで使い回す形に
//!   リファクタできる。
//!
//! ## 監視用長期トークン（2026-07-25 に全廃 → 2026-07-26 に「任意機能」として部分的に復活）
//!
//! 当初は `claude setup-token` で発行する長期トークン（`CC Anatomy-token-<name>`）を
//! メニューバーの複数アカウント使用率監視専用に維持していた（`CC Anatomy-active` は
//! その「選択中」ポインタ）。切り替えが Keychain スワップで簡単になったため一度は全廃したが、
//! 「全アカウントを常時監視したい」というユーザー要望を受け、位置づけを変えて復活させた:
//!
//! - **完全に任意**。「＋アカウントを追加」の最終ステップ（1/2 ログイン → 2/2 監視の承認）、
//!   または登録済みアカウント行の「常時監視を設定」からのみ発行される。ユーザーが setup-token
//!   をスキップ・キャンセルしても、アカウント追加・切り替え機能には一切影響しない
//! - **切り替え機能とは完全独立**。監視トークンの有無・失効は `switch_account` の判断に絡まない
//!   （Keychain スワップは `CC Anatomy-cred-<name>` スナップショットのみを見る）
//! - `CC Anatomy-active`（旧「選択中」ポインタ）は復活させない。選択中の記録は
//!   `meta.active`（Keychain の裏付けを持たない表示専用のブックキーピング）のままでよい
//! - 旧実装は起動時に `CC Anatomy-token-*` を一律削除するマイグレーションを持っていたが、
//!   復活に伴い削除している（これ以上消さない）
//!
//! 使用量取得の優先順位（`get_accounts_usage`）: (1) ライブアカウントはライブ OAuth
//! `/api/oauth/usage` → (2) 監視トークンがあれば `/v1/messages` の
//! `anthropic-ratelimit-unified-*` ヘッダ（`actions::usage_via_monitor_token`）→
//! (3) スナップショット access token が期限内ならそれで `/api/oauth/usage` →
//! (4) どれも取れなければ `usage_cache`（stale 表示）。取得結果はすべて usage_cache に保存する。
//!
//! `meta.active` フィールド自体は「ライブ追随の記録専用」として存置する
//! （Keychain の裏付けは持たない、表示・記録用のブックキーピングのみ）。

use serde::{Deserialize, Serialize};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use tauri::Manager;

/// accounts.json の read-modify-write 区間を直列化する（2026-07-26 レビュー指摘 M-5）。
/// auto_sync_live（60秒ごとの背景スレッド）と switch_account/remove_account/rename_account/
/// reorder_accounts/get_accounts_usage（ユーザー操作、フロントから async コマンド経由）が
/// 並行に load_meta → 変更 → save_meta を行うと、後勝ちの save_meta が他方の変更を
/// 巻き戻す（削除したはずのアカウントが復活する等）。ロック保持中に HTTP 呼び出しを含む
/// 区間もあるため厳密には理想的ではないが、まずは正しさ（レース排除）を優先する。
/// 万一パニックで poison しても機能停止しないよう、poison は無視して継続する
static META_LOCK: Mutex<()> = Mutex::new(());

fn lock_meta() -> std::sync::MutexGuard<'static, ()> {
    META_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 監視用長期トークン（`claude setup-token` 発行）のサービス名プレフィックス。
/// 発行直後は Terminal 側のスクリプトが `PENDING_MONITOR_TOKEN_SVC`（アカウント名が
/// 確定する前の一時置き場）にいったん書き、アプリ側が対象アカウントの名前で claim してから
/// この形（`CC Anatomy-token-<name>`）に移す
const TOKEN_SVC_PREFIX: &str = "CC Anatomy-token-";
/// setup-token 発行直後の一時置き場。「＋アカウントを追加」の統合フローでは
/// スクリプト起動時点でアカウント名がまだ確定していない（ログイン完了後に確定する）ため、
/// 固定のこのサービス名にいったん書かせ、`poll_monitor_setup` が対象アカウント名で
/// claim（コピー＋削除）する
const PENDING_MONITOR_TOKEN_SVC: &str = "CC Anatomy-token-pending";
const CRED_SVC_PREFIX: &str = "CC Anatomy-cred-";
const LIVE_CREDENTIALS_SVC: &str = "Claude Code-credentials";
/// 旧方式（.zshrc への CLAUDE_CODE_OAUTH_TOKEN 注入）のマーカー。撤去処理のみで使う
const LEGACY_SHELL_BEGIN: &str = "# >>> CC Anatomy account switcher >>>";
const LEGACY_SHELL_END: &str = "# <<< CC Anatomy account switcher <<<";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    /// 内部識別子。Keychain サービス名（`CC Anatomy-cred-<name>` 等）や照合キーに使うため不変。
    /// ユーザー向け表示は `display_name`（無ければこの `name`）を使うこと
    pub name: String,
    /// ユーザーが自由に付けられる表示名。None なら `name` をそのまま表示する
    pub display_name: Option<String>,
    pub email: String,
    pub plan: String,
    /// Claude Code が現在 /login しているアカウント（＝連携なしの起動中セッションが消費する先）
    pub is_live: bool,
    /// ライブ資格情報のスナップショット（CC Anatomy-cred-<name>）が登録済みか。
    /// これが無いアカウントには切り替えできない（旧登録は「再ログイン」での取り込みが必要）
    pub has_credentials: bool,
    /// 使用量の常時監視用に `claude setup-token` の長期トークンが紐づいているか（任意機能）。
    /// 切り替え機能とは完全に独立で、これが無くても切り替え・使用量取得（スナップショット AT
    /// 経由）自体は成立する
    pub has_monitor_token: bool,
    /// 「再ログイン」導線が使えるか（org_id か email のどちらかが登録されているか）。
    /// 両方とも空の旧登録は照合しようがなく再ログインを開始しても拒否されるため、
    /// フロント側で事前に案内・disabled を出し分けるために返す（2026-07-26 レビュー M-7）
    pub can_relogin: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountsState {
    pub accounts: Vec<Account>,
    /// 現在 PC にログイン中のアカウントの email（取得できなければ None）
    pub live_email: Option<String>,
    /// 現在のログインがすでにどれかのアカウントとして登録済みか
    pub live_registered: bool,
    /// 起動中の claude CLI セッション数。0 より大きい間は切り替え・追加をブロックする
    pub running_sessions: usize,
    /// 直前のスワップが中途半端な状態のまま残っている（meta.inconsistent）。true の間は
    /// sync-back が止まっており、「取り込む」（import_live_account）でしか解消できない。
    /// live_registered の状態に関わらず起こりうるため、専用フラグとして公開する
    /// （2026-07-26 レビュー High-1: 従来は live_registered=false のときしか
    /// 取り込み導線を出しておらず、登録済みのまま不整合になったケースが詰んでいた）
    pub inconsistent: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct Meta {
    active: Option<String>,
    accounts: Vec<StoredAccount>,
    /// スワップの部分適用（Keychain とoauthAccountが食い違う状態）からロールバックにも
    /// 失敗したときに立てるフラグ。true の間は sync-back を実行しない
    /// （どちらのアカウントの最新状態か確定できない資格情報を書き戻すと被害が広がるため）。
    /// 解消は「取り込む」（import_live_account）または「再ログイン」の成功でのみ行う
    #[serde(default)]
    inconsistent: bool,
    /// 最後にアプリが把握しているライブ資格情報（Claude Code-credentials）の SHA-256。
    /// switch_account のスワップ成功時・import_live_account・sync_back_live_login の
    /// 書き戻し成功時に更新する。sync-back のたびにこれと現在のハッシュを比較し、
    /// 一致しなければ「アプリの知らないところでライブが書き換わった」＝ Claude Code の
    /// 自動 refresh 等で外部から更新された可能性があるとみなし、oauthAccount の記述を
    /// 無条件には信じず profile API で実際の持ち主を確認する（2026-07-25 実機観測:
    /// セッションは access token 期限の数時間前でも refresh でライブ Keychain を
    /// 書き換える。oauthAccount 自体は refresh では変わらないため、切り替え後に旧セッションが
    /// 残っていると「Keychain=旧アカウントの新トークン / oauthAccount=切り替え先」という
    /// 不整合が時間の問題で発生する）
    #[serde(default)]
    last_live_hash: Option<String>,
    /// 「直近に確認済み」のライブハッシュ。last_live_hash は sync-back が実際に書き戻せた
    /// （＝登録済みと一致した）ときしか更新されないため、未登録ライブが居座るケースでは
    /// 毎サイクル resolve_live_owner（＝ profile API 呼び出し）が走ってしまう
    /// （2026-07-26 レビュー M-3）。ハッシュが変わらない限り「もう確認済み」として
    /// auto_sync_live を早期returnさせるためのフィールド
    #[serde(default)]
    last_checked_hash: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredAccount {
    name: String,
    /// ユーザーが任意に設定する表示名。`name`（Keychain サービス名・照合キー）は不変にしたまま
    /// 表示だけ変えられるようにするためのフィールド。空文字は保存前に None へ正規化する
    #[serde(default)]
    display_name: Option<String>,
    email: String,
    plan: String,
    /// 課金先の organization id。アカウントの同一性判定はこの id を第一キーにする
    #[serde(default)]
    org_id: String,
    /// ~/.claude.json の oauthAccount のコピー。切り替え時にそのまま書き戻す
    #[serde(default)]
    oauth_account: Option<serde_json::Value>,
    /// ライブ資格情報のスナップショット（CC Anatomy-cred-<name>）を登録済みか
    #[serde(default)]
    has_credentials: bool,
    /// 直近に取得できた使用率のキャッシュ。get_accounts_usage が保存・参照する
    /// （監視用長期トークンは復活させず、保存済みスナップショットの access token が
    /// 有効期限内のときだけ照会する。refresh は絶対に行わない）
    #[serde(default)]
    usage_cache: Option<UsageCache>,
}

/// 使用率キャッシュ。access token が期限切れ・照会失敗のときはこれをそのまま返し、
/// `fetched_at` で「いつ時点の値か」を示す
#[derive(Serialize, Deserialize, Clone, Debug)]
struct UsageCache {
    five_pct: f64,
    seven_pct: f64,
    five_reset: Option<i64>,
    seven_reset: Option<i64>,
    /// 取得時刻（epoch 秒）
    fetched_at: i64,
}

/// ~/.claude.json 全体を読む
fn read_claude_json() -> Result<serde_json::Value, String> {
    let path = crate::db::home_dir().join(".claude.json");
    let content = fs::read_to_string(&path).map_err(|_| {
        "~/.claude.json が見つかりません。Claude Code で一度ログインしてください".to_string()
    })?;
    serde_json::from_str(&content)
        .map_err(|e| format!("~/.claude.json の読み取りに失敗しました: {e}"))
}

/// 現在ログイン中の oauthAccount オブジェクト（無ければ None）
fn live_oauth_account() -> Option<serde_json::Value> {
    read_claude_json()
        .ok()?
        .get("oauthAccount")
        .cloned()
        .filter(|v| !v.is_null())
}

/// 現在 Claude Code が /login しているアカウントの organization id（~/.claude.json 由来）
fn live_org_id() -> Option<String> {
    live_oauth_account()?
        .get("organizationUuid")
        .and_then(|u| u.as_str())
        .map(String::from)
}

/// oauthAccount オブジェクトから同一性判定に使う organization id と email を取り出す
fn identify(oauth_account: &serde_json::Value) -> (Option<String>, Option<String>) {
    let org_id = oauth_account
        .get("organizationUuid")
        .and_then(|v| v.as_str())
        .map(String::from);
    let email = oauth_account
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .map(String::from);
    (org_id, email)
}

/// 登録済みアカウントの中から同一アカウントを探す。org_id を第一キー、email を第二キーとする。
/// org_id が取れているのに一致しない場合は、email が同じでも別アカウントと確定できる
/// （同一メールで複数 organization を持つケースを誤マージしないため、email へフォールバックしない）。
/// org_id が空の旧エントリ同士は email でしか照合できない既知の制約がある（対応見送り済み）
fn find_match_idx(accounts: &[StoredAccount], org_id: Option<&str>, email: Option<&str>) -> Option<usize> {
    if let Some(org) = org_id.filter(|o| !o.is_empty()) {
        return accounts.iter().position(|a| a.org_id == org);
    }
    let email = email.filter(|e| !e.is_empty())?;
    accounts.iter().position(|a| a.email == email)
}

/// email のローカル部から Keychain サービス名・表示名に使えるアカウント名を作る。
/// 取り込み経路（Flow A/B）はユーザーに名前を入力させないため、ここで自動採番する
fn derive_account_name(email: Option<&str>, existing: &[StoredAccount]) -> String {
    let base = email
        .and_then(|e| e.split('@').next())
        .map(|s| {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
                .collect::<String>()
                .trim_matches('-')
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "account".to_string());
    let base = if base.len() > 28 { base[..28].to_string() } else { base };

    if !existing.iter().any(|a| a.name == base) {
        return base;
    }
    for i in 2.. {
        let candidate = format!("{base}-{i}");
        if !existing.iter().any(|a| a.name == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// 登録アカウントが現在ログイン中か（org_id がライブと一致するか）
fn is_live_account(org_id: &str, live: Option<&str>) -> bool {
    !org_id.is_empty() && live == Some(org_id)
}

fn base_dir() -> PathBuf {
    crate::db::home_dir().join(".claude/cc-anatomy")
}

fn load_meta() -> Meta {
    fs::read_to_string(base_dir().join("accounts.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 途中で落ちて JSON が壊れると、load_meta が既定値へフォールバックして
/// アカウントが全消失したように見える。一時ファイル経由の atomic rename で防ぐ
fn save_meta(meta: &Meta) -> Result<(), String> {
    fs::create_dir_all(base_dir()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    let tmp = base_dir().join("accounts.json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, base_dir().join("accounts.json")).map_err(|e| e.to_string())
}

fn cred_svc(name: &str) -> String {
    format!("{CRED_SVC_PREFIX}{name}")
}

fn monitor_token_svc(name: &str) -> String {
    format!("{TOKEN_SVC_PREFIX}{name}")
}

/// 監視トークンが紐づいているか（秘密自体は読まず `acct` 属性の有無だけで判定する）
fn has_monitor_token(name: &str) -> bool {
    keychain_account_attr(&monitor_token_svc(name)).is_some()
}

/// `security find-generic-password -s <svc>` の出力から `"acct"<blob>="value"` 行の値を取り出す。
/// 値が無い属性は `"acct"<blob>=<NULL>`（引用符無し）で出るため、引用符で囲まれていない値は
/// 無効として弾く。これをやらないと、二重引用符探索だけの素朴な実装では `<NULL>` のケースで
/// ラベルの `"acct"` 自体を値と誤認してしまう（実際にあったバグ）
fn parse_acct_attr(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("\"acct\"")?;
        let value_part = rest.split('=').nth(1)?.trim();
        if !(value_part.len() >= 2 && value_part.starts_with('"') && value_part.ends_with('"')) {
            return None;
        }
        let value = &value_part[1..value_part.len() - 1];
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

/// 既存アイテムの `acct` 属性を読む（`-w` を付けずに秘密自体は取得しない）
fn keychain_account_attr(service: &str) -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", service])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_acct_attr(&String::from_utf8_lossy(&out.stdout))
}

/// Keychain の値は秘密なので、戻り値をログ・エラーメッセージに載せない
fn keychain_read(service: &str) -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

// TODO: -w にシークレットを渡すため実行中は `ps` から見える（旧実装から継続の制約）。
// Security.framework のネイティブ呼び出しに置き換えれば解消するが、本改修のスコープ外
fn write_generic_password(service: &str, user: &str, secret: &str) -> Result<(), String> {
    let status = Command::new("security")
        .args([
            "add-generic-password",
            "-a",
            user,
            "-s",
            service,
            "-w",
            secret,
            "-U",
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Keychain への保存に失敗しました".into())
    }
}

/// アカウント名（`-a`）は既存アイテムの実際の値を優先する。`$USER` 決め打ちだと、
/// 既存アイテムの acct と食い違い、同一サービス名で2つのアイテムが並立する事故になりうる。
/// 新規作成（＝このアプリ自身が作る監視トークン・スナップショット系サービス）のときだけ
/// $USER にフォールバックする
fn keychain_write(service: &str, secret: &str) -> Result<(), String> {
    let user = keychain_account_attr(service)
        .or_else(|| std::env::var("USER").ok())
        .ok_or("Keychain のアカウント名を特定できませんでした".to_string())?;
    write_generic_password(service, &user, secret)
}

/// ライブ資格情報（`Claude Code-credentials`）は Claude Code 本体が作った既存アイテムであり、
/// フォールバックで新規に $USER 名義のアイテムを作ってはいけない（仕様8）。
/// 既存の acct を読めない場合は中断する
fn keychain_write_live(secret: &str) -> Result<(), String> {
    let user = keychain_account_attr(LIVE_CREDENTIALS_SVC).ok_or_else(|| {
        "ライブ資格情報の Keychain アイテムのアカウント名を取得できませんでした。Claude Code でログイン済みか確認してください".to_string()
    })?;
    write_generic_password(LIVE_CREDENTIALS_SVC, &user, secret)
}

/// エントリが元々無い場合も失敗するので、呼び出し側は「消えていること」だけを期待する
fn keychain_delete(service: &str) {
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", service])
        .output();
}

/// シェルの単一引用符に安全に埋め込む。パスにクオートが混ざっても実行内容が変わらないようにする
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Keychain のサービス名とシェル引数に埋め込むため、名前は英数字・ハイフン・アンダースコアに限る
fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err("アカウント名は英数字・ハイフン・アンダースコア（32文字以内）で指定してください".into())
    }
}

/// 表示名のフォールバック規則: 表示名が設定されていればそれを、無ければ内部識別子(name)を使う。
/// Rust 側（tray 等）・フロント側どちらでも同じ規則を使うため、ロジックをここに集約する
fn resolve_display_name(name: &str, display_name: Option<&str>) -> String {
    display_name
        .filter(|d| !d.is_empty())
        .unwrap_or(name)
        .to_string()
}

/// メニューバーのアカウント一覧用。使用率は監視用長期トークンの全廃により持たない
/// （表示名とライブ状態だけ返す。使用率は accounts::get_accounts_usage で別途取得する）
pub struct TrayAccount {
    /// switch_account に渡す内部識別子（Keychain サービス名の一部）
    pub name: String,
    /// 表示名（display_name があればそちら、無ければ内部識別子 name）
    pub display_name: String,
    pub is_live: bool,
    /// false の場合は資格情報スナップショットが無く、切り替え不可（未取り込み）
    pub has_credentials: bool,
}

/// トレイ表示専用の読み取り（save_meta しない）。save_meta は tmp + atomic rename なので、
/// ロック無しで読んでも「壊れた JSON」や書きかけの内容を拾うことはない（見えるのは常に
/// 更新前・更新後のどちらか）。lost-update の対象になる RMW ではないため
/// META_LOCK の対象外とする（2026-07-26 レビュー M-5 の対応範囲を検討した上での除外）
pub fn registered_accounts() -> Vec<TrayAccount> {
    let meta = load_meta();
    let live = live_org_id();
    meta.accounts
        .iter()
        .map(|a| TrayAccount {
            name: a.name.clone(),
            display_name: resolve_display_name(&a.name, a.display_name.as_deref()),
            is_live: is_live_account(&a.org_id, live.as_deref()),
            has_credentials: a.has_credentials,
        })
        .collect()
}

/// ライブアカウントに対応する登録アカウントの監視トークンを読む（無ければ None）。
/// トレイのタイトル表示（`live_usage_summary` の失敗時）専用のフォールバック。
/// ライブトークンはスナップショット由来で、久しぶりに切り替えたアカウントほど期限切れに
/// なりやすく、リフレッシュは Claude Code 起動時にしか起きないため、切り替え直後は
/// メニューバーの使用量が「-」のまま見えなくなっていた（2026-07-27）。
/// HTTP は呼ばない（トークン文字列を返すだけ。実際の照会は呼び出し側で行う）。
/// registered_accounts と同じ理由で save_meta しない純粋な読み取りなので META_LOCK 対象外
pub fn live_account_monitor_token() -> Option<String> {
    let live = live_org_id()?;
    let meta = load_meta();
    // 空の org_id 同士を一致させない（is_live_account / sync_active_pointer / find_match_idx
    // と同じ同一性判定の規約。2026-07-27 レビュー M-2: live が空文字列のとき org_id 未設定の
    // 旧登録に誤マッチし、別アカウントの監視トークンで照会・表示してしまっていた）
    let name = meta.accounts.iter().find(|a| !a.org_id.is_empty() && a.org_id == live)?.name.clone();
    keychain_read(&monitor_token_svc(&name))
}

/// 一括照会の連打防止。前回取得からこの秒数未満ならキャッシュをそのまま返す
/// （モーダルを開き直す・トレイの手動更新連打で毎回 API を叩かないようにする）
const USAGE_MIN_REFETCH_SECS: i64 = 60;

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 登録済み全アカウントの使用率表示用。切り替え前に「どのアカウントが空いているか」を
/// 見られるようにするための情報で、監視用長期トークンは復活させない（2026-07-25 全廃済み）。
/// 保存済みスナップショットの access token をそのまま使い、期限切れでも refresh は一切しない
/// （refresh してしまうと当該アカウントの refresh token が消費され、切り替え機能の安全性の
/// 根幹である「refresh token は Claude Code 本体だけが触る」という前提が崩れるため）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountUsage {
    pub name: String,
    pub five_pct: Option<f64>,
    pub seven_pct: Option<f64>,
    pub five_reset: Option<i64>,
    pub seven_reset: Option<i64>,
    /// 取得時刻（epoch 秒）。cache が無ければ None
    pub fetched_at: Option<i64>,
    /// true ならキャッシュ返し（今回は新規取得できなかった）
    pub stale: bool,
    /// 5h 枠のリセット時刻を過ぎている想定（実質 0% とみなせる）
    pub five_probably_reset: bool,
}

/// access token の有効期限（epoch ミリ秒）が現在時刻（epoch 秒）より未来かどうか。
/// 期限切れなら照会せずキャッシュへフォールバックする（refresh は絶対にしない）
fn token_is_still_valid(expires_at_ms: i64, now_secs: i64) -> bool {
    expires_at_ms > now_secs * 1000
}

/// UsageCache から API 返却用の AccountUsage を組み立てる純粋関数（テスト容易性のため
/// ネットワーク・ファイル IO と分離する）。stale は呼び出し側が「今回は新規取得したか」を渡す
fn to_account_usage(name: &str, cache: Option<&UsageCache>, stale: bool, now: i64) -> AccountUsage {
    match cache {
        Some(c) => AccountUsage {
            name: name.to_string(),
            five_pct: Some(c.five_pct),
            seven_pct: Some(c.seven_pct),
            five_reset: c.five_reset,
            seven_reset: c.seven_reset,
            fetched_at: Some(c.fetched_at),
            stale,
            five_probably_reset: c.five_reset.is_some_and(|reset| reset <= now),
        },
        None => AccountUsage {
            name: name.to_string(),
            five_pct: None,
            seven_pct: None,
            five_reset: None,
            seven_reset: None,
            fetched_at: None,
            stale: true,
            five_probably_reset: false,
        },
    }
}

/// 直近の取得から USAGE_MIN_REFETCH_SECS 未満なら再照会せずキャッシュを返してよいか
fn cache_is_fresh_enough(fetched_at: i64, now: i64) -> bool {
    now - fetched_at < USAGE_MIN_REFETCH_SECS
}

/// 保存済みスナップショット（CC Anatomy-cred-<name>）から access token と有効期限
/// （epoch ミリ秒、claudeAiOauth.expiresAt）を取り出す
fn stored_access_token(name: &str) -> Option<(String, i64)> {
    let raw = keychain_read(&cred_svc(name))?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let token = v.pointer("/claudeAiOauth/accessToken")?.as_str()?.to_string();
    let expires_at = v.pointer("/claudeAiOauth/expiresAt")?.as_i64()?;
    Some((token, expires_at))
}

/// 使用量取得で実際に試行しうるソース。Cache はここには含めない
/// （「試すソース」ではなく「全滅した後の最終フォールバック」という別の位置づけのため）
#[derive(Debug, PartialEq, Eq)]
enum UsageSource {
    LiveOauth,
    MonitorToken,
    SnapshotOauth,
}

/// 使用量取得の優先順位（実際の I/O から分離した純粋な判定。テスト容易性のため）。
/// 上から順に試し、最初に成功したものを使う「真のフォールバック連鎖」を返す
/// （どれも成功しなければ呼び出し側が usage_cache へフォールバックする）。
///
/// ライブアカウント: (1) ライブ OAuth /api/oauth/usage →（期限切れ・失敗なら）
/// (2) 監視用長期トークンがあれば /v1/messages のヘッダ →（無ければ/失敗なら）
/// (3) スナップショット access token が期限内ならそれで /api/oauth/usage。
/// 非ライブアカウントはライブ OAuth を試さず (2)→(3) の順（ライブの資格情報は
/// 他アカウントが消費中のため使えない）。
///
/// 2026-07-27: 従来は is_live なら常にライブ OAuth 一本で、失敗時は他ソースを試さず
/// 直接キャッシュへ落ちていた。ライブトークンはスナップショット由来で、久しぶりに
/// 切り替えたアカウントほど期限切れになりやすく、リフレッシュは Claude Code 起動時にしか
/// 起きないため、「切り替え直後なのにメニューバーの使用量が見えない」空白期間が生じていた。
/// 単一ソースではなく優先順位リストにし、失敗したソースは飛ばして次を試すようにする
fn resolve_usage_source_order(is_live: bool, has_monitor_token: bool, has_valid_snapshot_token: bool) -> Vec<UsageSource> {
    let mut order = Vec::with_capacity(3);
    if is_live {
        order.push(UsageSource::LiveOauth);
    }
    if has_monitor_token {
        order.push(UsageSource::MonitorToken);
    }
    if has_valid_snapshot_token {
        order.push(UsageSource::SnapshotOauth);
    }
    order
}

/// 使用率照会の対象1件分。ロックを取り直さずに済むよう、フェーズ1で必要な値だけを
/// meta から取り出しておく（2026-07-26 レビュー M-B1）
struct UsageTarget {
    name: String,
    is_live: bool,
    cache: Option<UsageCache>,
}

/// 登録済み全アカウント（has_credentials のもの）の使用率をまとめて取得する。
/// 取得元の優先順位は `resolve_usage_source_order` を参照。
///
/// - 前回取得から USAGE_MIN_REFETCH_SECS 未満のアカウントも同様にキャッシュを返す（連打防止）
/// - 照会に成功したら usage_cache へ保存する。切り替え後もこれが「最終既知値」として残る
/// - refresh は一切行わない（access token 期限切れは正常な状態として静かにキャッシュへ委ねる）
pub fn get_accounts_usage() -> Result<Vec<AccountUsage>, String> {
    let live = live_org_id();
    let now = now_epoch();

    // フェーズ1（ロック区間）: 照会対象のスナップショットを取るだけ。HTTP はここでは呼ばない
    // （2026-07-26 レビュー M-B1: 「一覧表示は使用率取得にブロックされない」という設計を
    // 復元するため、ロック中に外部 I/O を含めない）
    let targets: Vec<UsageTarget> = {
        let _guard = lock_meta();
        let meta = load_meta();
        meta.accounts
            .iter()
            .filter(|a| a.has_credentials)
            .map(|a| UsageTarget {
                name: a.name.clone(),
                is_live: is_live_account(&a.org_id, live.as_deref()),
                cache: a.usage_cache.clone(),
            })
            .collect()
    };

    // フェーズ2（ロック外）: アカウントごとに HTTP 照会する。他の meta 操作をブロックしない
    let mut results = Vec::with_capacity(targets.len());
    let mut updates: Vec<(String, UsageCache)> = Vec::new();
    for t in &targets {
        if t.cache.as_ref().is_some_and(|c| cache_is_fresh_enough(c.fetched_at, now)) {
            results.push(to_account_usage(&t.name, t.cache.as_ref(), true, now));
            continue;
        }

        let snapshot_token = stored_access_token(&t.name).filter(|(_, exp)| token_is_still_valid(*exp, now));
        let source_order = resolve_usage_source_order(t.is_live, has_monitor_token(&t.name), snapshot_token.is_some());

        // 優先順位どおりに試し、最初に成功したものを採用する（失敗・スキップしたソースは
        // 次へ進む。2026-07-27: 従来は is_live のライブ OAuth 1本勝負で、期限切れのまま
        // 直接キャッシュへ落ちていた）
        let mut fetched = None;
        for source in &source_order {
            fetched = match source {
                UsageSource::LiveOauth => {
                    // 期限切れなら照会せず次のソースへ進む（無駄な401リクエストを避ける。
                    // expiresAt が取れない場合は「不明」として素通しし、実際の応答で判断する）
                    crate::credentials::live_token_with_expiry()
                        .ok()
                        .filter(|(_, expires_at)| expires_at.is_none_or(|exp| token_is_still_valid(exp, now)))
                        .and_then(|(token, _)| {
                            crate::actions::oauth_get_with_token(&token, crate::actions::USAGE_URL)
                                .ok()
                                .and_then(|body| crate::actions::parse_usage_body(&body).ok())
                        })
                }
                UsageSource::MonitorToken => keychain_read(&monitor_token_svc(&t.name))
                    .and_then(|token| crate::actions::usage_via_monitor_token(&token).ok()),
                UsageSource::SnapshotOauth => snapshot_token.as_ref().and_then(|(token, _)| {
                    crate::actions::oauth_get_with_token(token, crate::actions::USAGE_URL)
                        .ok()
                        .and_then(|body| crate::actions::parse_usage_body(&body).ok())
                }),
            };
            if fetched.is_some() {
                break;
            }
        }

        match fetched {
            Some(summary) => {
                let new_cache = UsageCache {
                    five_pct: summary.five_pct,
                    seven_pct: summary.seven_pct,
                    five_reset: summary.five_reset,
                    seven_reset: summary.seven_reset,
                    fetched_at: now,
                };
                results.push(to_account_usage(&t.name, Some(&new_cache), false, now));
                updates.push((t.name.clone(), new_cache));
            }
            None => results.push(to_account_usage(&t.name, t.cache.as_ref(), true, now)),
        }
    }

    // フェーズ3（ロック区間）: 取得できた分だけ meta を読み直して書き戻す。フェーズ1〜2の間に
    // 削除・改名された可能性があるため、対象は名前で引き直す（見つからなければ静かにスキップ。
    // 消えたアカウントの使用率を書き戻しても実害は無い実質的な no-op）
    if !updates.is_empty() {
        let _guard = lock_meta();
        let mut meta = load_meta();
        let mut changed = false;
        for (name, cache) in updates {
            if let Some(a) = meta.accounts.iter_mut().find(|a| a.name == name) {
                a.usage_cache = Some(cache);
                changed = true;
            }
        }
        if changed {
            save_meta(&meta)?;
        }
    }

    Ok(results)
}

/// ユーザーが開いている Claude Code セッション数（＝再起動すれば切り替わるもの）。
///
/// claude CLI のプロセスをそのまま数えると、claude-mem のワーカーが起動した常駐 claude
/// （親が bun 等）まで混ざり、実測でセッション数を5割ほど過大に報告した。
/// 「開き直してください」と言える対象はシェルから起動されたものだけなので、親で絞る。
/// デスクトップアプリ（Claude.app）は別系統なので除く
fn running_sessions() -> usize {
    const SHELLS: [&str; 4] = ["zsh", "bash", "fish", "sh"];

    let Ok(out) = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()
    else {
        return 0;
    };
    let text = String::from_utf8_lossy(&out.stdout);

    let basename = |s: &str| -> Option<String> {
        PathBuf::from(s)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    };

    // 親コマンドを引けるよう、まず pid -> comm を作る
    let mut comm_by_pid: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    let mut claude_procs: Vec<(u32, u32)> = Vec::new();
    for line in text.lines() {
        let mut it = line.trim().splitn(3, char::is_whitespace);
        let (Some(pid), Some(ppid), Some(comm)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.trim().parse::<u32>()) else {
            continue;
        };
        let comm = comm.trim().to_string();
        let is_claude = !comm.contains("Claude.app")
            && basename(&comm).is_some_and(|n| n == "claude");
        if is_claude {
            claude_procs.push((pid, ppid));
        }
        comm_by_pid.insert(pid, comm);
    }

    claude_procs
        .iter()
        .filter(|(_, ppid)| {
            comm_by_pid
                .get(ppid)
                .and_then(|c| basename(c))
                .is_some_and(|n| SHELLS.contains(&n.as_str()))
        })
        .count()
}

/// 本アプリ自身が起動する claude -p 子プロセス（環境診断・タスク抽出）の実行中は常にブロックする。
/// こちらは「完了を待てば済む」短時間の処理であり、force での迂回を認めない
/// （ユーザーのシェルセッションと違い、いつ終わるかアプリ自身が把握しているため待たせて問題ない）
fn ensure_app_not_busy() -> Result<(), String> {
    if crate::diagnostics::is_running()
        || crate::actions::is_agent_busy()
        || crate::doc_analysis::is_running()
    {
        return Err(
            "本アプリの環境診断/タスク抽出/AI分析実行中は切り替え・追加ができません。完了してから実行してください。"
                .into(),
        );
    }
    Ok(())
}

/// 起動中の Claude Code セッションが自分のトークンをライブへ書き戻すと切り替えを
/// 踏み潰しうる（旧結論を覆す理由の②）。
///
/// 当初はここをハードブロックにしていたが、実運用のユーザー環境ではシェルセッションが
/// 常時複数開いており「0件のタイミング」が実質存在せず、機能自体が使えなくなった
/// （2026-07-25 ユーザー報告）。そのため「確認 + force」方式に緩和する: force=false かつ
/// セッションが1件以上あれば `SessionsRunning` を返して呼び出し側に確認を求め、
/// ユーザーが続行を選んだ場合のみ force=true で再実行してもらう。
/// リスク自体は残る（踏み潰され得る）ため、確認ダイアログの文言で明示すること
fn count_running_sessions_unless_forced(force: bool) -> usize {
    if force {
        0
    } else {
        running_sessions()
    }
}

fn zshrc_path() -> PathBuf {
    crate::db::home_dir().join(".zshrc")
}

fn tmp_path_for(real_path: &Path) -> PathBuf {
    let file_name = real_path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("cc-anatomy");
    real_path.with_file_name(format!("{file_name}.cc-anatomy.tmp"))
}

/// symlink なら実体側へ書き、元ファイルのパーミッションを維持した atomic write（tmp + rename）。
/// `~/.zshrc` や `~/.claude.json` のようにユーザー設定ファイルを書き換える処理はすべてこれを使う。
/// tmp は最初から 0600 で作成する（rename までの短い間も他ユーザーに読めないように。
/// 直後に元ファイルの実際のパーミッションへ合わせ直す）
fn atomic_write_preserving(path: &Path, content: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let real_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = tmp_path_for(&real_path);
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("{} の書き込みに失敗しました: {e}", real_path.display()))?;
        f.write_all(content.as_bytes())
            .map_err(|e| format!("{} の書き込みに失敗しました: {e}", real_path.display()))?;
    }
    if let Ok(meta) = fs::metadata(&real_path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, &real_path)
        .map_err(|e| format!("{} の更新に失敗しました: {e}", real_path.display()))
}

/// マーカーで挟まれたブロックだけを取り除く純粋関数。複数ブロックが残っていても全部除去する。
/// 変更が無ければ None（呼び出し側はこれで書き込みをスキップできる）
fn strip_legacy_shell_blocks(content: &str) -> Option<String> {
    let mut content = content.to_string();
    let mut changed = false;
    while let Some(start) = content.find(LEGACY_SHELL_BEGIN) {
        let Some(end_rel) = content[start..].find(LEGACY_SHELL_END) else {
            break;
        };
        let end = start + end_rel + LEGACY_SHELL_END.len();

        let mut new_content = content[..start].to_string();
        let mut rest = &content[end..];
        // マーカー直前に自分が挿入した空行が残っていれば畳み、削除跡に空行を残さない
        while new_content.ends_with("\n\n") {
            new_content.pop();
        }
        if let Some(stripped) = rest.strip_prefix('\n') {
            rest = stripped;
        }
        new_content.push_str(rest);
        content = new_content;
        changed = true;
    }
    changed.then_some(content)
}

/// 旧方式（.zshrc への CLAUDE_CODE_OAUTH_TOKEN 注入）の撤去。アプリ起動時に一度呼ぶ。
/// マーカーが無ければ何もしない冪等な処理。ユーザーの他の記述を壊さないよう、
/// マーカーで挟まれたブロックだけを取り除く。
/// 戻り値は「実際にブロックを削除して書き換えたか」（呼び出し側のログで
/// 実行有無を確認できるようにする。過去に「除去できていない」報告があったため、
/// エラー時だけでなく実行結果そのものを追えるようにした）
pub fn remove_legacy_shell_integration() -> Result<bool, String> {
    let path = zshrc_path();
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!(".zshrc を読めなかったため中断しました: {e}")),
    };
    match strip_legacy_shell_blocks(&content) {
        Some(new_content) => atomic_write_preserving(&path, &new_content).map(|_| true),
        None => Ok(false),
    }
}

/// 「選択中 = ログイン中」を meta.active に記録する。Keychain の裏付けは持たない
/// 純粋なブックキーピングで、失敗する余地が無い（アプリの外で `claude login` された
/// 場合にもズレないよう、呼び出し側で毎回呼ぶ）
fn sync_active_pointer(meta: &mut Meta) {
    let Some(live) = live_org_id() else { return };
    let Some(name) = meta
        .accounts
        .iter()
        .find(|a| !a.org_id.is_empty() && a.org_id == live)
        .map(|a| a.name.clone())
    else {
        return;
    };
    if meta.active.as_deref() != Some(name.as_str()) {
        meta.active = Some(name);
    }
}

pub fn get_accounts() -> Result<AccountsState, String> {
    let meta = {
        // sync_active_pointer の条件付き save_meta だけが RMW。以降の読み取り専用処理
        // （アカウント一覧の整形・running_sessions 等）は atomic rename 前提でロック不要
        // （2026-07-26 レビュー M-B2: 「一覧表示は使用率取得にブロックされない」設計の維持）
        let _guard = lock_meta();
        let mut meta = load_meta();
        let active_before = meta.active.clone();
        sync_active_pointer(&mut meta);
        if meta.active != active_before {
            let _ = save_meta(&meta);
        }
        meta
    };

    let live = live_org_id();
    let accounts = meta
        .accounts
        .iter()
        .map(|a| Account {
            name: a.name.clone(),
            display_name: a.display_name.clone(),
            email: a.email.clone(),
            plan: a.plan.clone(),
            is_live: is_live_account(&a.org_id, live.as_deref()),
            has_credentials: a.has_credentials,
            has_monitor_token: has_monitor_token(&a.name),
            can_relogin: !a.org_id.is_empty() || !a.email.is_empty(),
        })
        .collect();

    let (live_email, live_registered) = match live_oauth_account() {
        Some(oauth) => {
            let (org_id, email) = identify(&oauth);
            let registered = find_match_idx(&meta.accounts, org_id.as_deref(), email.as_deref()).is_some();
            (email, registered)
        }
        None => (None, false),
    };

    Ok(AccountsState {
        accounts,
        live_email,
        live_registered,
        running_sessions: running_sessions(),
        inconsistent: meta.inconsistent,
    })
}

/// ライブ資格情報 JSON をそのまま読む（accessToken だけでなくスナップショット全体が要るため、
/// accessToken 等だけを返す credentials::live_token_with_expiry() とは別に用意する）
fn live_credentials_value() -> Result<serde_json::Value, String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", LIVE_CREDENTIALS_SVC, "-w"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("Claude Code のライブ資格情報が Keychain にありません".into());
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|_| "ライブ資格情報の読み取りに失敗しました".to_string())
}

/// 現在ログイン中アカウントを登録に取り込む（Flow A）。Tauri コマンドから直接呼ぶ公開版。
/// meta の read-modify-write をロックで直列化する（2026-07-26 レビュー M-5）
pub fn import_live_account() -> Result<Account, String> {
    let _guard = lock_meta();
    import_live_account_locked()
}

/// import_live_account の内部実装。**呼び出し側が既に META_LOCK を保持している前提**
/// （std::sync::Mutex は再入不可のため、ここで改めてロックを取ると自分自身の呼び出し元と
/// デッドロックする）。poll_add_account_login（再ログイン導線のポーリング）はロックを
/// 保持したままこちらを直接呼ぶ。
///
/// org_id 一致（無ければ email 一致）で既存登録を探し、あれば更新、無ければ新規追加する。
/// `has_credentials = true` の save_meta は Keychain へのスナップショット書き込みが
/// 成功した後に行う（書き込みが失敗したのに「スナップショットあり」と記録するのを防ぐ）。
/// 取り込みの成功は「取り込む/再ログインでの解消」条件の1つなので、混在状態フラグも解除する
fn import_live_account_locked() -> Result<Account, String> {
    let creds = live_credentials_value()?;
    let oauth_account = live_oauth_account().ok_or(
        "現在ログイン中のアカウント情報が見つかりません。Claude Code でログインしてください",
    )?;
    let (org_id, email) = identify(&oauth_account);
    let plan = oauth_account
        .get("organizationType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut meta = load_meta();
    let idx = find_match_idx(&meta.accounts, org_id.as_deref(), email.as_deref());
    let name = match idx {
        Some(i) => meta.accounts[i].name.clone(),
        None => derive_account_name(email.as_deref(), &meta.accounts),
    };

    match idx {
        Some(i) => {
            let a = &mut meta.accounts[i];
            if let Some(org) = &org_id {
                if !org.is_empty() {
                    a.org_id = org.clone();
                }
            }
            if let Some(e) = &email {
                a.email = e.clone();
            }
            if !plan.is_empty() {
                a.plan = plan.clone();
            }
            a.oauth_account = Some(oauth_account);
            a.has_credentials = true;
        }
        None => meta.accounts.push(StoredAccount {
            name: name.clone(),
            display_name: None,
            email: email.clone().unwrap_or_default(),
            plan: plan.clone(),
            org_id: org_id.clone().unwrap_or_default(),
            oauth_account: Some(oauth_account),
            has_credentials: true,
            usage_cache: None,
        }),
    }
    sync_active_pointer(&mut meta);

    let creds_str = creds.to_string();
    keychain_write(&cred_svc(&name), &creds_str)?;
    meta.inconsistent = false;
    // ユーザーが明示的に「現在のライブを取り込む」と判断した操作なので、この内容を
    // 以後の sync-back の「既知の良い状態」の基準にする
    meta.last_live_hash = Some(sha256_hex(&creds_str));
    save_meta(&meta)?;

    let live = live_org_id();
    let final_org_id = org_id.unwrap_or_default();
    let display_name = meta
        .accounts
        .iter()
        .find(|a| a.name == name)
        .and_then(|a| a.display_name.clone());
    let final_email = email.unwrap_or_default();
    Ok(Account {
        name: name.clone(),
        display_name,
        can_relogin: !final_org_id.is_empty() || !final_email.is_empty(),
        email: final_email,
        plan,
        is_live: is_live_account(&final_org_id, live.as_deref()),
        has_credentials: true,
        has_monitor_token: has_monitor_token(&name),
    })
}

/// アカウントの表示名を変更する。`name`（内部識別子・Keychain 照合キー）は不変のまま、
/// ユーザー向けの表示だけを変えられるようにする。トリム後に空文字なら表示名を解除し、
/// `name` をそのまま表示する状態に戻す
pub fn rename_account(name: &str, display_name: &str) -> Result<(), String> {
    validate_name(name)?;
    let _guard = lock_meta();
    let mut meta = load_meta();
    let idx = meta
        .accounts
        .iter()
        .position(|a| a.name == name)
        .ok_or_else(|| format!("アカウント「{name}」は登録されていません"))?;
    let trimmed = display_name.trim();
    meta.accounts[idx].display_name = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    save_meta(&meta)
}

/// `order` の並びに従って `accounts` を並び替える純粋関数（テスト容易性のため
/// ファイル IO と分離する）。`order` に含まれない既存アカウントは元の相対順序を保ったまま
/// 末尾へ温存し、`order` に含まれる未知の name（既に削除済み等）は無視する
fn reorder_stored_accounts(accounts: &mut Vec<StoredAccount>, order: &[String]) {
    let mut reordered = Vec::with_capacity(accounts.len());
    for name in order {
        if let Some(pos) = accounts.iter().position(|a| &a.name == name) {
            reordered.push(accounts.remove(pos));
        }
    }
    reordered.append(accounts);
    *accounts = reordered;
}

/// アカウント一覧の表示順（= accounts.json の accounts 配列の順序）を変更する。
/// ドラッグ&ドロップでの並び替え確定時にフロントから呼ばれる
pub fn reorder_accounts(names: &[String]) -> Result<(), String> {
    let _guard = lock_meta();
    let mut meta = load_meta();
    reorder_stored_accounts(&mut meta.accounts, names);
    save_meta(&meta)
}

fn hash_hex(s: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:x}", h.finish())
}

/// ライブ資格情報の内容ハッシュ。ログイン未完了の変化検知に使う（秘密自体はフロントへ渡さない）
fn live_credentials_hash() -> String {
    live_credentials_value()
        .map(|v| hash_hex(&v.to_string()))
        .unwrap_or_default()
}

/// ライブ資格情報の内容の SHA-256（暗号学的ハッシュ）。
/// sync-back 前の「アプリの知らないところでの外部書き換え」検知に使う（last_live_hash と比較）
fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Flow B の完了検知に使う基準値。ハッシュが変わった時点で完了とみなす。
/// 「同一アカウントの再ログイン」だと org/email が変わらないため、かつて
/// 「hash 変化 かつ identity 変化」を条件にしていたところ、同一アカウントの
/// 再ログインが永久に完了しない不具合になっていた。
/// hash だけを条件にし、完了後は import_live_account に委ねる: identity が変わっていれば
/// 新規アカウント取り込み、同一なら登録済みスナップショットの更新（sync-back 相当）として
/// 扱われる。自動 refresh によるハッシュ変化の誤検知が起きても、実質的には無害な
/// スナップショット更新が走るだけなので許容する
#[derive(Serialize, Deserialize)]
struct LoginBaseline {
    hash: String,
    /// 再ログイン（登録済みカードの「再ログイン」導線）の対象アカウント。
    /// None なら「＋アカウントを追加」の汎用フロー（誰でログインしても取り込む）。
    /// Some の場合、ポーリング側でログイン結果の org_id をこの対象と照合し、
    /// 一致しなければ取り込まずに Mismatch を返す（誤紐づけ防止。2026-07-26 要件）
    #[serde(default)]
    target_name: Option<String>,
}

fn hash_changed(baseline: &LoginBaseline, current_hash: &str) -> bool {
    baseline.hash != current_hash
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum StartLoginOutcome {
    #[serde(rename = "started")]
    Started {
        baseline: String,
        /// 事前 sync-back で oauthAccount と実際の持ち主のズレを検知したときだけ入る警告
        warning: Option<String>,
    },
    #[serde(rename = "needs_import")]
    NeedsImport { live_email: Option<String> },
    /// force=false で外部セッションが1件以上あった。呼び出し側は確認の上 force=true で再実行する
    #[serde(rename = "sessions_running")]
    SessionsRunning { count: usize },
}

/// sync-back の結果。Unregistered は「未登録アカウントがログイン中」で、
/// 呼び出し側は取り込みの確認を挟むまで先へ進んではいけない
enum SyncBack {
    /// warning は「oauthAccount と実際の持ち主がズレていた」ことを検知したときだけ入る
    Synced { warning: Option<String> },
    NoLiveLogin,
    Unregistered(Option<String>),
}

const PROFILE_UNCONFIRMED_MSG: &str = "ライブ資格情報の持ち主を確認できませんでした。少し待って再試行するか、全セッション終了後に再試行してください";
const LIVE_HIJACKED_WARNING: &str = "ライブのログインが実行中セッションにより巻き戻っていました。";

/// oauthAccount とライブ資格情報の実際の持ち主が一致するかを解決した結果
struct LiveOwner {
    org_id: Option<String>,
    email: Option<String>,
    /// oauthAccount の記載と profile API の結果がズレていた（別アカウントのセッションが
    /// refresh でライブを巻き戻した等）。true のときは org_id を信用せず email だけで照合する
    mismatched: bool,
}

/// ライブの持ち主を解決する。ハッシュが前回記録と一致していれば「外部からの書き換えなし」と
/// みなし oauthAccount をそのまま信じる。不一致（または前回記録が無い）なら、
/// `fetch_profile`（実装は profile API 呼び出し。テストでは差し替える）で実際の持ち主を
/// 確認してから帰属を決める。確認できなければ Err で中断する（推測で書き込まない）
fn resolve_live_owner<F>(
    last_live_hash: Option<&str>,
    current_hash: &str,
    oauth_account: &serde_json::Value,
    access_token: Option<&str>,
    fetch_profile: F,
) -> Result<LiveOwner, String>
where
    F: FnOnce(&str) -> Result<String, String>,
{
    let (org_id, email) = identify(oauth_account);
    if last_live_hash == Some(current_hash) {
        return Ok(LiveOwner { org_id, email, mismatched: false });
    }

    let Some(token) = access_token else {
        return Err(PROFILE_UNCONFIRMED_MSG.into());
    };
    let body = fetch_profile(token).map_err(|_| PROFILE_UNCONFIRMED_MSG.to_string())?;
    let profile: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| PROFILE_UNCONFIRMED_MSG.to_string())?;
    let profile_email = profile
        .pointer("/account/email")
        .and_then(|v| v.as_str())
        .map(String::from);

    let mismatched = matches!((&profile_email, &email), (Some(p), Some(o)) if p != o);
    if mismatched {
        // oauthAccount の記載は信用できないため、org_id も含めて破棄し email だけで照合する
        Ok(LiveOwner {
            org_id: None,
            email: profile_email,
            mismatched: true,
        })
    } else {
        Ok(LiveOwner {
            org_id,
            email: profile_email.or(email),
            mismatched: false,
        })
    }
}

/// sync-back: ライブ資格情報が登録済みアカウントに一致すれば書き戻す。
/// **ベストエフォートにしない**: 片方だけ読めた等の中途半端な状態では中断する
/// （黙ってスキップして上書きに進むと、直前のアカウントの最新資格情報を失う）。
/// 過去のスワップが中途半端な状態のまま残っている（`meta.inconsistent`）間は、
/// どちらのアカウントが「最新」か確定できないため実行しない。
///
/// ライブのハッシュが `meta.last_live_hash` と一致しない場合は、アプリの知らないところで
/// ライブが書き換わった（Claude Code の自動 refresh 等）とみなし、`resolve_live_owner` で
/// 実際の持ち主を確認してから進める（2026-07-25 実機観測: refresh は access token 期限の
/// 数時間前でも発生し、oauthAccount 自体は変わらないため、切り替え後に旧セッションが
/// 残っていると誤帰属が時間の問題で起きる）
/// resolve_live_owner の結果を meta.accounts へ適用する（Keychain 書き込み・
/// org_id/email/oauth_account の更新・last_live_hash 更新）。**呼び出し側が既に
/// META_LOCK を保持している前提**の内部ヘルパー（2026-07-26 レビュー M-B3 で
/// auto_sync_live 用に sync_back_live_login から切り出した。sync_back_live_login
/// 自身の呼び出し元 [switch_account/start_add_account_login] の挙動は変えない）
fn apply_live_owner(
    meta: &mut Meta,
    owner: &LiveOwner,
    oauth_account: serde_json::Value,
    creds_str: &str,
    current_hash: &str,
) -> Result<SyncBack, String> {
    match find_match_idx(&meta.accounts, owner.org_id.as_deref(), owner.email.as_deref()) {
        Some(idx) => {
            keychain_write(&cred_svc(&meta.accounts[idx].name), creds_str)?;
            let a = &mut meta.accounts[idx];
            if let Some(org) = &owner.org_id {
                if !org.is_empty() {
                    a.org_id = org.clone();
                }
            }
            if let Some(e) = &owner.email {
                a.email = e.clone();
            }
            // mismatched（ライブ乗っ取り検知）のときは oauthAccount の記載自体が
            // 信用できない（別セッションが書き戻した残留情報の可能性がある）ため、
            // profile で確認できた org_id/email 以外は上書きしない
            // （2026-07-26 レビュー High-2a: 無警告で汚染データが保存されていた）
            if !owner.mismatched {
                a.oauth_account = Some(oauth_account);
            }
            a.has_credentials = true;
            meta.last_live_hash = Some(current_hash.to_string());
            Ok(SyncBack::Synced {
                warning: owner.mismatched.then(|| LIVE_HIJACKED_WARNING.to_string()),
            })
        }
        None => Ok(SyncBack::Unregistered(owner.email.clone())),
    }
}

/// sync-back 本体。switch_account / start_add_account_login の「事前 sync-back」から呼ばれる
/// （呼び出し側がロックを保持したまま呼ぶ、ユーザー操作に伴う短時間の処理という位置づけ。
/// profile API 呼び出しを含めロック内で完結させる方針は今回変更しない。ロック外に出したのは
/// 60秒ごとに無人で走る auto_sync_live のみ。2026-07-26 レビュー M-B3 のスコープ）
fn sync_back_live_login(meta: &mut Meta) -> Result<SyncBack, String> {
    if meta.inconsistent {
        return Err(
            "直前の切り替えが中途半端な状態のままです。「取り込む」または「再ログイン」で解消してから実行してください"
                .into(),
        );
    }
    let creds = live_credentials_value();
    let oauth = live_oauth_account();
    match (creds, oauth) {
        (Err(_), None) => Ok(SyncBack::NoLiveLogin),
        (Ok(creds), Some(oauth_account)) => {
            let creds_str = creds.to_string();
            let current_hash = sha256_hex(&creds_str);
            let access_token = creds
                .pointer("/claudeAiOauth/accessToken")
                .and_then(|v| v.as_str());

            let owner = resolve_live_owner(
                meta.last_live_hash.as_deref(),
                &current_hash,
                &oauth_account,
                access_token,
                |token| {
                    crate::actions::oauth_get_with_token(
                        token,
                        "https://api.anthropic.com/api/oauth/profile",
                    )
                },
            )?;

            apply_live_owner(meta, &owner, oauth_account, &creds_str, &current_hash)
        }
        // 片方だけ読めた（Keychain と ~/.claude.json が矛盾した状態）は不整合。
        // 黙って進めると sync-back のつもりで実は何もできていない事態になるため中断する
        _ => Err(
            "現在ログイン中の資格情報を確認できませんでした。時間をおいて再試行してください"
                .into(),
        ),
    }
}

/// tray.rs の定期更新ループ（60秒ごと）からの自動取り込みの結果。
/// UI へ知らせる価値がある（＝画面の再描画が要る）のは Synced と Unregistered だけなので、
/// 呼び出し側はこの2つのときだけ "accounts-updated" を emit すればよい
pub enum AutoSyncResult {
    /// ハッシュが前回記録・前回確認済みから変わっていなかった。何もしなかった
    Unchanged,
    /// 登録済みアカウントに一致し、資格情報を最新化した（旧・手動「セッション更新」相当）。
    /// warning はライブ乗っ取り検知時の案内（旧・手動操作時に表示していたものと同じ。
    /// 2026-07-26 レビュー High-2b: 従来は自動化の過程でこの警告が握り潰されていた）
    Synced { warning: Option<String> },
    /// ライブセッションが未登録アカウントだった（取り込みはしない。UI に導線を出すだけ）
    Unregistered,
    NoLiveLogin,
}

/// "accounts-updated" イベントのペイロード（tray.rs の定期更新ループから emit）
#[derive(Serialize, Clone)]
pub struct AccountsUpdatedEvent {
    pub warning: Option<String>,
}

/// auto_sync_live の早期return条件（テスト容易性のため I/O から分離する。
/// 2026-07-26 レビュー L-10）。last_live_hash・last_checked_hash のどちらかが
/// 現在のハッシュと一致すれば「確認済み・変化なし」としてスキップしてよい
fn auto_sync_should_skip(last_live_hash: Option<&str>, last_checked_hash: Option<&str>, current_hash: &str) -> bool {
    last_live_hash == Some(current_hash) || last_checked_hash == Some(current_hash)
}

/// auto_sync_live のフェーズ3（ロックを取り直した後）の TOCTOU 再検証（テスト容易性のため
/// I/O から分離する。2026-07-26 レビュー M-B3）。フェーズ2（ロック外・profile API 呼び出し）
/// の間に不整合状態になった、または last_live_hash が動いていたら、その間に確認した owner の
/// 前提はもう崩れているため書き込まず bail すべき
fn auto_sync_should_bail(inconsistent: bool, current_last_live_hash: Option<&str>, snapshot: Option<&str>) -> bool {
    inconsistent || current_last_live_hash != snapshot
}

/// 手動の「セッション更新」ボタンを廃止し、その自動化として定期更新ループから呼ぶ（2026-07-26）。
/// 要件は「登録済みアカウントと一致し、かつ前回取り込み時（last_live_hash）からスナップショットが
/// 変わっていたら自動で取り込む」。sync_back_live_login はこれを内包するが、呼ぶたびに
/// Keychain へ書き込み・（内容次第で）profile API 呼び出しを伴うため、60秒ごとに無条件で
/// 呼ぶとコストが無視できない。ハッシュが「登録済みとして書き戻し済み」（last_live_hash）
/// または「前回すでに確認済み」（last_checked_hash。未登録ライブの居座り等、
/// last_live_hash が更新されないケースをカバーする。2026-07-26 レビュー M-3）のどちらかと
/// 一致する間は、sync_back_live_login 自体を呼ばずに早期returnする（auto_sync_should_skip）。
///
/// meta の read-modify-write はロックで直列化する（2026-07-26 レビュー M-5:
/// ユーザー操作 [switch/remove/rename/reorder] と競合すると save_meta の後勝ちで
/// 互いの変更を巻き戻しうる）。
///
/// ただし profile API 呼び出し（resolve_live_owner が内部で行う。最大で数秒〜10秒程度）は
/// ロックの外で行う（2026-07-26 レビュー M-B3）。60秒ごとに無人で走るこの関数がロックを
/// 持ったまま HTTP を待つと、その間ユーザー操作（切り替え・削除・改名等）がブロックされ、
/// 「一覧表示は使用率取得にブロックされない」という既存設計の趣旨に反する。
///
/// 3フェーズに分ける:
/// 1. （ロック区間・短時間）変化なし早期return の判定だけ行い、判定に使った
///    last_live_hash をスナップショットして手放す
/// 2. （ロック外）スナップショットを基準に resolve_live_owner を呼ぶ（HTTP はここでだけ）
/// 3. （ロック区間）meta を読み直し、フェーズ1のスナップショットが今も同じか再検証してから
///    書き込む（TOCTOU 対策）。フェーズ2の間に switch_account 等が last_live_hash を
///    進めていたら、フェーズ2の owner はもう古いので書き込まず Unchanged で bail し、
///    次サイクルに委ねる
pub fn auto_sync_live() -> Result<AutoSyncResult, String> {
    let creds = match live_credentials_value() {
        Ok(v) => v,
        Err(_) => return Ok(AutoSyncResult::NoLiveLogin),
    };
    let creds_str = creds.to_string();
    let current_hash = sha256_hex(&creds_str);

    // フェーズ1
    let last_live_hash_snapshot = {
        let _guard = lock_meta();
        let meta = load_meta();
        if meta.inconsistent {
            return Err(
                "直前の切り替えが中途半端な状態のままです。「取り込む」または「再ログイン」で解消してから実行してください"
                    .into(),
            );
        }
        if auto_sync_should_skip(meta.last_live_hash.as_deref(), meta.last_checked_hash.as_deref(), &current_hash) {
            return Ok(AutoSyncResult::Unchanged);
        }
        meta.last_live_hash.clone()
    };

    let Some(oauth_account) = live_oauth_account() else {
        // Keychain にはあるが ~/.claude.json に無い＝矛盾した状態。sync_back_live_login と
        // 同じ方針で中断する（黙って進めると「確認済み」を誤って記録しかねない）
        return Err("現在ログイン中の資格情報を確認できませんでした。時間をおいて再試行してください".into());
    };
    let access_token = creds.pointer("/claudeAiOauth/accessToken").and_then(|v| v.as_str());

    // フェーズ2（ロック外。最大で数秒〜10秒かかりうる profile API 呼び出しはここだけ）
    let owner = resolve_live_owner(
        last_live_hash_snapshot.as_deref(),
        &current_hash,
        &oauth_account,
        access_token,
        |token| crate::actions::oauth_get_with_token(token, "https://api.anthropic.com/api/oauth/profile"),
    )?;

    // フェーズ3
    let _guard = lock_meta();
    let mut meta = load_meta();
    if auto_sync_should_bail(meta.inconsistent, meta.last_live_hash.as_deref(), last_live_hash_snapshot.as_deref()) {
        // フェーズ2の間に状態が変わった（切り替え・別の取り込み等）。この owner 判定は
        // もう前提が崩れているため書き込まず、次の60秒サイクルに新しい前提で委ねる
        return Ok(AutoSyncResult::Unchanged);
    }

    let result = apply_live_owner(&mut meta, &owner, oauth_account, &creds_str, &current_hash)?;
    meta.last_checked_hash = Some(current_hash);
    match result {
        SyncBack::Synced { warning } => {
            save_meta(&meta)?;
            Ok(AutoSyncResult::Synced { warning })
        }
        SyncBack::Unregistered(_) => {
            save_meta(&meta)?;
            Ok(AutoSyncResult::Unregistered)
        }
        SyncBack::NoLiveLogin => Ok(AutoSyncResult::NoLiveLogin),
    }
}

/// setup-token 実行部分のシェルスクリプト断片。トークンは `PENDING_MONITOR_TOKEN_SVC`
/// （固定の一時置き場）へ書く。「＋アカウントを追加」の統合フローではスクリプト起動時点で
/// 対象アカウント名がまだ確定していない（ログイン完了後に確定する）ため、常に固定の
/// pending サービス名へいったん書かせ、`poll_monitor_setup` が対象アカウント名で claim する
/// （登録済みアカウントへの「常時監視を設定」でも同じ経路を使い、経路を1本にまとめる）。
///
/// setup-token の失敗・キャンセル・トークン抽出失敗は、いずれも `exit 0`
/// （スクリプト全体としては成功）で抜ける。監視トークンは完全に任意の後付け機能であり、
/// ここで失敗させてもアカウント追加・切り替え自体には一切影響しない設計なので、
/// 呼び出し側（Terminal を見ているユーザー）に「スキップされた」と分かるログだけ残せば十分
fn setup_token_script_body(claude_bin: &str) -> String {
    const BODY: &str = r#"
echo
echo "==================================================="
echo " CC Anatomy: 使用量の常時監視を設定します（任意・スキップ可）"
echo " ブラウザが開いたら、このアカウントで承認してください。"
echo " 不要なら Ctrl+C で中止しても、アカウントの追加・切り替えには影響しません。"
echo "==================================================="
echo

log="$(dirname "$0")/setup-token.log"
umask 077
: > "$log"

# setup-token は端末幅でトークンを折り返す。折り返すと1行では拾えず、行を継ぎ足す方式は
# 本文まで巻き込みうるため、pty を十分広くして折り返し自体を起こさせない
script -q "$log" zsh -c "stty cols 400 >/dev/null 2>&1; exec __CLAUDE_BIN__ setup-token"
rc=$?

# Ink の再描画でトークンが途中まで描かれた行も記録に混ざるため、最長一致を採用する
token=$(sed $'s/\033\[[0-9;?]*[a-zA-Z]//g' "$log" 2>/dev/null | tr -d '\r' \
  | grep -oE 'sk-ant-oat[0-9]+-[A-Za-z0-9_-]+' \
  | awk '{ if (length($0) > length(best)) best = $0 } END { print best }')
rm -Pf "$log" 2>/dev/null || rm -f "$log"

if [ -z "$token" ]; then
  echo
  if [ $rc -ne 0 ]; then
    echo "setup-token がエラー終了しました。監視の設定はスキップします。"
  else
    echo "トークンを取得できませんでした。監視の設定はスキップします。"
  fi
  echo "後からアプリの「常時監視を設定」からやり直せます。"
  exit 0
fi

case "$token" in
  sk-ant-oat*) ;;
  *) echo "トークンの形式が想定外だったため、監視の設定をスキップしました。"; exit 0 ;;
esac

# 折り返しの結合に失敗すると、先頭だけの切れたトークンが通ってしまう。
# 実物は 100 文字強なので、短すぎるものは壊れたとみなす
if [ ${#token} -lt 60 ]; then
  echo "取得したトークンが短すぎるため、監視の設定をスキップしました（${#token} 文字）。"
  exit 0
fi

security add-generic-password -a "$USER" -s "__PENDING_SVC__" -w "$token" -U || {
  echo "Keychain への保存に失敗したため、監視の設定をスキップしました。"
  exit 0
}
unset token
echo
echo "✅ 監視トークンを保存しました。CC Anatomy に戻ってください。"
"#;
    BODY.replace("__CLAUDE_BIN__", claude_bin)
        .replace("__PENDING_SVC__", PENDING_MONITOR_TOKEN_SVC)
}

/// osascript 経由で Terminal.app にスクリプトを実行させる（run_fixes_in_terminal と同じ流儀）
fn run_script_in_terminal(script_path: &Path) -> Result<(), String> {
    let status = Command::new("osascript")
        .args([
            "-e",
            &format!(
                "tell application \"Terminal\" to do script \"'{}'\"",
                script_path.display()
            ),
            "-e",
            "tell application \"Terminal\" to activate",
        ])
        .status()
        .map_err(|e| format!("Terminal の起動に失敗: {e}"))?;
    if !status.success() {
        return Err("Terminal の起動に失敗しました（オートメーション権限を確認してください）".into());
    }
    Ok(())
}

/// `claude auth login` はブラウザ承認を伴う対話フローなので、GUI から隠して実行できない。
/// 完了検知は Terminal の終了コードに頼らず、ライブ資格情報のハッシュ・org・email の
/// 変化ポーリングで行う（仕様上 exit code での完了判定は保証されていないため）。
///
/// `claude auth login` はライブ資格情報を上書きするため、Flow C の切り替えと同じ sync-back を
/// 事前に行う。未登録アカウントがログイン中なら（取り込まずに進むと失うため）ここで止める。
/// 実行中セッションがあると、そのセッションが自分のトークンをライブへ書き戻して
/// ログイン結果を踏み潰しうるため、force=false の間は `SessionsRunning` を返して確認を挟む
///
/// 2026-07-26、統合フロー（ユーザー承認）により、ログイン完了後に**同じ Terminal で続けて**
/// `claude setup-token` を実行するようにした（1/2 ブラウザでログイン → 2/2 使用量監視の承認）。
/// アカウント名はこの時点で確定していないため、トークンは `PENDING_MONITOR_TOKEN_SVC` へ
/// 書かせ、フロントが Flow B 完了（import_live_account）後に `poll_monitor_setup` で
/// 対象アカウント名に claim する。setup-token 側の失敗・キャンセルはアカウント追加の成否に
/// 一切影響しない
pub fn start_add_account_login(
    app: &tauri::AppHandle,
    force: bool,
    target_name: Option<&str>,
) -> Result<StartLoginOutcome, String> {
    ensure_app_not_busy()?;
    let sessions = count_running_sessions_unless_forced(force);
    if sessions > 0 {
        return Ok(StartLoginOutcome::SessionsRunning { count: sessions });
    }

    // ロックが必要なのは sync-back（meta の read-modify-write。2026-07-26 レビュー M-5）と
    // baseline 生成の区間だけ。この後の Terminal 起動（run_script_in_terminal は osascript
    // 経由でオートメーション許可ダイアログが出ることがあり、ユーザーの応答待ちで
    // 分単位ブロックしうる）はロック外で行う（2026-07-26 レビュー M-B5）
    let (sync_warning, baseline_json) = {
        let _guard = lock_meta();
        let mut meta = load_meta();
        // 再ログイン導線（target_name あり）は既存の登録カードを直すのが目的なので、
        // 対象がすでに存在しないなら（削除済み等）ここで止める。汎用の「＋アカウントを追加」
        // （target_name なし）は従来どおり対象を問わない
        if let Some(name) = target_name {
            let target = meta
                .accounts
                .iter()
                .find(|a| a.name == name)
                .ok_or_else(|| format!("アカウント「{name}」は登録されていません"))?;
            // org_id・email のどちらも無い旧登録は、ログイン結果を対象と照合しようがない
            // （2026-07-26 レビュー M-7）。誤って任意のログインを紐づけないよう、ここで拒否する
            if target.org_id.is_empty() && target.email.is_empty() {
                return Err(format!(
                    "「{name}」は照合に使える情報（組織ID・メールアドレス）が無いため再ログインでは紐づけできません。「＋アカウントを追加」から新規登録してください"
                ));
            }
        }
        let sync_warning = match sync_back_live_login(&mut meta)? {
            SyncBack::Unregistered(live_email) => return Ok(StartLoginOutcome::NeedsImport { live_email }),
            SyncBack::Synced { warning } => {
                save_meta(&meta)?;
                warning
            }
            SyncBack::NoLiveLogin => None,
        };

        let baseline = LoginBaseline {
            hash: live_credentials_hash(),
            target_name: target_name.map(String::from),
        };
        let baseline_json = serde_json::to_string(&baseline).map_err(|e| e.to_string())?;
        (sync_warning, baseline_json)
    };

    // 中断した過去の追加が孤児トークンを残していることがある。消さずに始めると、
    // poll_monitor_setup が古いトークンを今回のログインの成果と誤認して claim してしまう
    keychain_delete(PENDING_MONITOR_TOKEN_SVC);

    let claude = crate::actions::resolve_claude_bin()?;
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("accounts");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let script_path = dir.join("add-account.sh");
    let script = format!(
        "#!/bin/zsh\nset -uo pipefail\nunset CLAUDE_CODE_OAUTH_TOKEN\n\
         echo \"1/2 ブラウザでログインしてください\"\n\
         if {claude} auth login; then\n\
         echo\n\
         echo \"2/2 使用量の常時監視を設定します\"\n{setup_token}\n\
         else\n\
         echo \"ログインが完了しなかったため、監視の設定は行いません。\"\n\
         fi\n",
        claude = shell_quote(&claude.display().to_string()),
        setup_token = setup_token_script_body(&claude.display().to_string()),
    );
    fs::write(&script_path, script).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    run_script_in_terminal(&script_path)?;

    Ok(StartLoginOutcome::Started {
        baseline: baseline_json,
        warning: sync_warning,
    })
}

/// 登録済みアカウントへ「常時監視を設定」（setup-token のみを実行する単独フロー）。
/// `claude auth login` は行わない＝現在ログイン中のアカウントを変えない。
/// setup-token 自体は独自のブラウザ承認フローを持つため、対象アカウントが
/// ライブでなくても実行できる
pub fn start_monitor_setup(app: &tauri::AppHandle, name: &str) -> Result<(), String> {
    validate_name(name)?;
    // meta の読み取りをロックで直列化する（2026-07-26 レビュー M-5）。ロックが要るのは
    // 存在確認だけなので、osascript（Automation 許可ダイアログで分単位ブロックし得る）
    // まで保持しないようブロックで即時解放する
    {
        let _guard = lock_meta();
        if !load_meta().accounts.iter().any(|a| a.name == name) {
            return Err(format!("アカウント「{name}」は登録されていません"));
        }
    }
    // 追加フローと同じく、孤児の pending トークンを消してから始める
    keychain_delete(PENDING_MONITOR_TOKEN_SVC);

    let claude = crate::actions::resolve_claude_bin()?;
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("accounts");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let script_path = dir.join("setup-monitor.sh");
    let script = format!(
        "#!/bin/zsh\nset -uo pipefail\n{}\n",
        setup_token_script_body(&claude.display().to_string())
    );
    fs::write(&script_path, script).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    run_script_in_terminal(&script_path)
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum MonitorSetupPoll {
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "linked")]
    Linked,
    /// ブラウザ側が期待していたアカウントと別のアカウントで setup-token を承認していた
    /// （2026-07-26 ユーザー報告: share1/2/3 のような紛らわしい複数アカウント運用では、
    /// 黙って紐づけると別アカウントの使用率が以後ずっと誤表示され続ける事故になる）。
    /// トークンは紐づけず破棄済みなので、UI は再試行を促すこと
    #[serde(rename = "mismatch")]
    Mismatch { expected_label: String, expected_email: String },
}

/// 期待する org_id とトークンの org_id が一致するか（テスト容易性のため純粋関数に分離）。
/// 対象アカウントに org_id が無い（レアケース。org_id 導入前からの旧登録等）場合は
/// 照合しようがないため、照合をスキップして従来どおり紐づけを許可する
fn org_id_matches(expected_org_id: &str, token_org_id: &str) -> bool {
    expected_org_id.is_empty() || expected_org_id == token_org_id
}

/// フロントが2秒間隔で呼ぶ（「＋アカウントを追加」のステップ2、または「常時監視を設定」）。
/// pending 置き場にトークンが現れたら、対象アカウントの org_id と照合してから
/// 監視トークンとして claim（コピーして pending 側を消す）する。
/// 既存の監視トークンがあれば上書きする（「常時監視を設定」の再実行＝更新の意味にもなる）。
///
/// 照合不一致（ブラウザ側が別アカウントのまま承認された）ならトークンを破棄して
/// `Mismatch` を返す。トークン自体が無効（401）なら破棄してエラーにする。
/// レート上限・通信断等で確認できなかった場合は、有効なトークンを誤って捨てないよう
/// pending のトークンは消さず `Waiting` を返して次回ポーリングで再確認する
/// （旧実装 `claim_pending_account` と同じ方針）
pub fn poll_monitor_setup(name: &str) -> Result<MonitorSetupPoll, String> {
    validate_name(name)?;
    let Some(token) = keychain_read(PENDING_MONITOR_TOKEN_SVC) else {
        return Ok(MonitorSetupPoll::Waiting);
    };

    // フェーズ1（ロック区間・短時間）: 対象アカウントの org_id をスナップショットするだけ。
    // check_monitor_token（HTTP）はここでは呼ばない（2026-07-26 レビュー M-B4）
    let target_org_id = {
        let _guard = lock_meta();
        let meta = load_meta();
        let target = meta
            .accounts
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| format!("アカウント「{name}」は登録されていません"))?;
        target.org_id.clone()
    };

    if !target_org_id.is_empty() {
        // フェーズ2（ロック外）: HTTP 呼び出しはここでだけ
        match crate::actions::check_monitor_token(&token) {
            crate::actions::TokenCheck::Valid(token_org_id) => {
                // フェーズ3（ロック区間）: フェーズ2の間に対象の org_id が変わっていないか
                // 再検証してから確定する（TOCTOU 対策）。変わっていたら判定の前提が崩れて
                // いるため確定せず、pending トークンも消さず次回ポーリングへ委ねる
                let _guard = lock_meta();
                let meta = load_meta();
                let target = meta
                    .accounts
                    .iter()
                    .find(|a| a.name == name)
                    .ok_or_else(|| format!("アカウント「{name}」は登録されていません"))?;
                if target.org_id != target_org_id {
                    return Ok(MonitorSetupPoll::Waiting);
                }
                if !org_id_matches(&target.org_id, &token_org_id) {
                    keychain_delete(PENDING_MONITOR_TOKEN_SVC);
                    return Ok(MonitorSetupPoll::Mismatch {
                        expected_label: resolve_display_name(&target.name, target.display_name.as_deref()),
                        expected_email: target.email.clone(),
                    });
                }
            }
            crate::actions::TokenCheck::Invalid => {
                keychain_delete(PENDING_MONITOR_TOKEN_SVC);
                return Err(
                    "取得したトークンが無効でした。もう一度「常時監視を設定」からやり直してください。".into(),
                );
            }
            crate::actions::TokenCheck::Unavailable(reason) => {
                // 一時的な不調（レート上限・通信断等）。診断用にログへ残すだけで、
                // pending のトークンは消さず次回ポーリングで再確認する
                eprintln!("monitor token check unavailable (will retry): {reason}");
                return Ok(MonitorSetupPoll::Waiting);
            }
        }
    }

    keychain_write(&monitor_token_svc(name), &token)?;
    keychain_delete(PENDING_MONITOR_TOKEN_SVC);
    Ok(MonitorSetupPoll::Linked)
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum PollResult {
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "done")]
    Done { account: Account },
    /// 再ログイン導線（target_name あり）で、ログイン結果の org_id が対象アカウントと
    /// 一致しなかった。poll_monitor_setup の Mismatch と同じ形（誤紐づけ防止）
    #[serde(rename = "mismatch")]
    Mismatch { expected_label: String, expected_email: String },
}

/// 再ログイン対象アカウントとライブの持ち主の照合結果。判定不能（ライブの持ち主がまだ
/// 確認できない）と不一致を区別する。Keychain（完了検知）と ~/.claude.json（照合）は
/// 別々に書き込まれるため、その間隙で2秒ポーリングが入ると org_id/email が一時的に
/// 読めない・古いままのことがある。これを Mismatch にすると正しいログインでも誤って
/// 弾いてしまうため、Undetermined として区別する（2026-07-26 レビュー M-6a）
#[derive(Debug, PartialEq, Eq)]
enum ReloginMatch {
    Match,
    Mismatch,
    /// ライブの持ち主がまだ確認できない。不一致と決めつけず待ち続けるべき
    Undetermined,
}

/// 対象アカウントの org_id（第一キー）・email（org_id が空の旧登録向けフォールバック）を
/// ライブの持ち主と照合する純粋関数（org_id_matches の流儀を踏襲。2026-07-26 レビュー L-10）。
/// 対象が org_id・email のどちらも持たない場合は照合しようがないため Mismatch で拒否する
/// （2026-07-26 レビュー M-7。このケースは start_add_account_login 側の can_relogin
/// チェックで事前に弾く想定だが、念のためここでも安全側に倒す）
fn match_relogin_target(
    target_org_id: &str,
    target_email: &str,
    live_org_id: Option<&str>,
    live_email: Option<&str>,
) -> ReloginMatch {
    if !target_org_id.is_empty() {
        return match live_org_id {
            Some(o) if o == target_org_id => ReloginMatch::Match,
            Some(_) => ReloginMatch::Mismatch,
            None => ReloginMatch::Undetermined,
        };
    }
    if !target_email.is_empty() {
        return match live_email {
            Some(e) if e == target_email => ReloginMatch::Match,
            Some(_) => ReloginMatch::Mismatch,
            None => ReloginMatch::Undetermined,
        };
    }
    ReloginMatch::Mismatch
}

/// フロントが2秒間隔で呼ぶ。ハッシュが変われば完了とみなし、あとは import_live_account に
/// 判定を委ねる（同一アカウントの再ログインなら更新、別アカウントなら新規取り込みになる）。
///
/// target_name（再ログイン導線）がある場合だけ、import_live_account を呼ぶ前に
/// ログイン結果を対象アカウントと照合する（match_relogin_target）。Undetermined
/// （判定不能）なら Waiting を返して次のポーリングへ委ね、Mismatch のときだけ
/// **何も書き込まず** Mismatch を返す（誤って別アカウントの登録を上書き・新規作成しない）。
/// このときライブの Keychain / ~/.claude.json 自体はすでに書き換わってしまっているが、
/// ここで sync-back 等の追加ケアは行わない。60秒ごとの自動同期ループ（tray.rs →
/// auto_sync_live）が次サイクルで拾い、登録済みアカウントに一致すれば自動で取り込み、
/// 未登録ならアカウント画面に取り込み導線を出す（2026-07-26 コーディネーター了承）。
/// Mismatch 時は setup-token（常時監視・任意機能）の pending トークンが付随して残っている
/// ことがあるため破棄する（放置すると次回の正しいやり直しが偽 Mismatch で失敗する。
/// 2026-07-26 レビュー M-4）
pub fn poll_add_account_login(baseline: &str) -> Result<PollResult, String> {
    let baseline: LoginBaseline = serde_json::from_str(baseline)
        .map_err(|_| "内部状態が壊れています。もう一度「アカウントを追加」からやり直してください".to_string())?;

    if !hash_changed(&baseline, &live_credentials_hash()) {
        return Ok(PollResult::Waiting);
    }

    // meta の read-modify-write をロックで直列化する（2026-07-26 レビュー M-5）。
    // 対象アカウントの照合（読み取り）と、一致した場合の取り込み（書き込み）を
    // 同じロック区間で行う必要がある（そうしないと、照合直後に別操作で対象が
    // 削除・改名されて食い違う可能性がある）。ロックを保持したまま
    // import_live_account_locked を直接呼ぶ（公開版 import_live_account を呼ぶと
    // 二重ロックでデッドロックする）
    let _guard = lock_meta();

    if let Some(target_name) = &baseline.target_name {
        let meta = load_meta();
        if let Some(target) = meta.accounts.iter().find(|a| &a.name == target_name) {
            let live_oauth = live_oauth_account();
            let (live_org, live_email) = live_oauth.as_ref().map(identify).unwrap_or((None, None));
            match match_relogin_target(&target.org_id, &target.email, live_org.as_deref(), live_email.as_deref()) {
                ReloginMatch::Match => {}
                ReloginMatch::Undetermined => return Ok(PollResult::Waiting),
                ReloginMatch::Mismatch => {
                    keychain_delete(PENDING_MONITOR_TOKEN_SVC);
                    return Ok(PollResult::Mismatch {
                        expected_label: resolve_display_name(&target.name, target.display_name.as_deref()),
                        expected_email: target.email.clone(),
                    });
                }
            }
        }
        // 対象アカウントが見つからない（ポーリング中に削除された等）場合は照合できないため、
        // 保護対象が無いとみなして通常の取り込みへ進む
    }

    let account = import_live_account_locked()?;
    Ok(PollResult::Done { account })
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum SwitchOutcome {
    #[serde(rename = "switched")]
    Switched {
        /// active ポインタ更新など、切り替え自体の成否に関わらない付随処理が失敗したときだけ入る警告
        warning: Option<String>,
    },
    #[serde(rename = "needs_import")]
    NeedsImport {
        /// 確認ダイアログ表示用。取り込みを承諾したら import_live_account → switch_account を再実行する
        live_email: Option<String>,
    },
    /// force=false で外部セッションが1件以上あった。呼び出し側は確認の上 force=true で再実行する
    #[serde(rename = "sessions_running")]
    SessionsRunning { count: usize },
}

/// ~/.claude.json の oauthAccount 置換を準備する（読み込み・パース検証のみ行い、まだ書かない）。
/// 呼び出し順序の都合で、実際の書き込み（commit）は Keychain スワップの後に行う
fn build_oauth_replacement(oauth_account: &serde_json::Value) -> Result<(PathBuf, String), String> {
    let path = crate::db::home_dir().join(".claude.json");
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("~/.claude.json を読めなかったため中断しました: {e}"))?;
    let mut root: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        format!("~/.claude.json のパースに失敗したため中断しました（書き込みは行っていません）: {e}")
    })?;
    let obj = root
        .as_object_mut()
        .ok_or("~/.claude.json の形式が不正なため中断しました")?;
    obj.insert("oauthAccount".to_string(), oauth_account.clone());
    let json = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    Ok((path, json))
}

fn commit_oauth_replacement(prepared: &(PathBuf, String)) -> Result<(), String> {
    atomic_write_preserving(&prepared.0, &prepared.1)
}

/// スワップ後の読み戻し検証。書いたつもりで実は反映されていない（権限・同期の問題等）を検知する
fn verify_swap(expected_cred: &str, expected_oauth: &serde_json::Value) -> Result<(), String> {
    let actual_cred = keychain_read(LIVE_CREDENTIALS_SVC)
        .ok_or("切り替え後にライブ資格情報を確認できませんでした")?;
    if actual_cred != expected_cred {
        return Err("切り替え後のライブ資格情報が一致しませんでした".into());
    }
    let actual_oauth = live_oauth_account()
        .ok_or("切り替え後に ~/.claude.json の oauthAccount を確認できませんでした")?;
    if &actual_oauth != expected_oauth {
        return Err("切り替え後の ~/.claude.json が一致しませんでした".into());
    }
    Ok(())
}

/// Keychain スワップによる切り替え（Flow C）。
///
/// 書き込み順序: ~/.claude.json のパース検証 → sync-back 分の save_meta 確定 →
/// Keychain スワップ → oauthAccount 置換 → active 更新。
/// active 更新の失敗は警告に留め、切り替え全体を失敗扱いにしない
/// （Keychain と oauthAccount は既にスワップ済みで、切り替え自体は成功しているため）。
///
/// スワップ後は読み戻し検証を行い、失敗したらスワップ前の状態へロールバックする。
/// ロールバック自体にも失敗した場合は「混在状態」として `meta.inconsistent` に永続化し、
/// 「取り込む」または「再ログイン」で明示的に解消されるまで以後の sync-back を止める
/// （どちらのアカウントが最新か確定できない資格情報を書き戻すと被害が広がるため）。
///
/// 外部セッションが1件以上あると、そのセッションが自分のトークンをライブへ書き戻して
/// 結果を踏み潰しうる。force=false の間は `SessionsRunning` を返して確認を挟む
pub fn switch_account(name: &str, force: bool) -> Result<SwitchOutcome, String> {
    ensure_app_not_busy()?;
    let sessions = count_running_sessions_unless_forced(force);
    if sessions > 0 {
        return Ok(SwitchOutcome::SessionsRunning { count: sessions });
    }
    validate_name(name)?;
    let _guard = lock_meta();
    let mut meta = load_meta();
    let target_idx = meta
        .accounts
        .iter()
        .position(|a| a.name == name)
        .ok_or_else(|| format!("アカウント「{name}」は登録されていません"))?;
    if !meta.accounts[target_idx].has_credentials {
        return Err(format!(
            "「{name}」に資格情報スナップショットがありません。先に取り込みを行ってください"
        ));
    }

    let sync_warning = match sync_back_live_login(&mut meta)? {
        SyncBack::Unregistered(live_email) => return Ok(SwitchOutcome::NeedsImport { live_email }),
        SyncBack::Synced { warning } => warning,
        SyncBack::NoLiveLogin => None,
    };

    let target = meta.accounts[target_idx].clone();
    let target_cred = keychain_read(&cred_svc(&target.name))
        .ok_or_else(|| format!("「{name}」の資格情報スナップショットが Keychain にありません"))?;
    let target_oauth = target
        .oauth_account
        .clone()
        .ok_or_else(|| format!("「{name}」のログイン情報がありません"))?;
    serde_json::from_str::<serde_json::Value>(&target_cred)
        .map_err(|_| format!("「{name}」の資格情報スナップショットが壊れています"))?;

    // .claude.json のパース検証を先に済ませ、後段の Keychain スワップより前に失敗を検出する
    let prepared_oauth = build_oauth_replacement(&target_oauth)?;
    // sync-back 分の変更（現在ログイン中アカウントのスナップショット更新）をここで確定する
    save_meta(&meta)?;

    // ロールバック用に、スワップ直前のライブ状態を退避しておく
    // （sync-back 済みなので、失われても sync-back 先の登録アカウントには最新分が残っている）
    let prior_live_cred = live_credentials_value().ok().map(|v| v.to_string());
    let prior_live_oauth = live_oauth_account();

    keychain_write_live(&target_cred)?;
    let commit_result =
        commit_oauth_replacement(&prepared_oauth).and_then(|_| verify_swap(&target_cred, &target_oauth));
    if let Err(e) = commit_result {
        let cred_restored = match &prior_live_cred {
            Some(prev) => keychain_write_live(prev).is_ok(),
            None => true, // 元々ライブが空だった＝復元すべき対象が無い
        };
        let oauth_restored = match &prior_live_oauth {
            Some(prev_oauth) => build_oauth_replacement(prev_oauth)
                .and_then(|p| commit_oauth_replacement(&p))
                .is_ok(),
            None => true,
        };
        if cred_restored && oauth_restored {
            return Err(format!(
                "切り替えに失敗したため、元のログイン状態へ復元しました: {e}"
            ));
        }
        meta.inconsistent = true;
        let _ = save_meta(&meta);
        return Err(format!(
            "切り替えが中途半端な状態で失敗し、復元にも失敗しました。混在状態としてマークしたので、「取り込む」または「再ログイン」で解消してください: {e}"
        ));
    }

    // スワップ後のライブは target_cred で確定した。次回 sync-back の「外部書き換え検知」の
    // 基準として記録しておく（これを忘れると、次の sync-back が「前回記録なし」を装って
    // 毎回 profile API を叩くことになる）
    meta.last_live_hash = Some(sha256_hex(&target_cred));
    meta.active = Some(name.to_string());
    if let Err(e) = save_meta(&meta) {
        // Keychain と oauthAccount は既にスワップ済み＝切り替え自体は成功している。
        // 内部ポインタの保存失敗だけで「切り替え失敗」と報告すると誤解を招く
        return Ok(SwitchOutcome::Switched {
            warning: Some(format!(
                "切り替えは完了しましたが、内部状態の保存に失敗しました: {e}"
            )),
        });
    }

    Ok(SwitchOutcome::Switched {
        warning: sync_warning,
    })
}

pub fn remove_account(name: &str) -> Result<(), String> {
    validate_name(name)?;
    let _guard = lock_meta();
    let mut meta = load_meta();
    if meta.active.as_deref() == Some(name) {
        meta.active = None;
    }
    meta.accounts.retain(|a| a.name != name);
    save_meta(&meta)?;
    keychain_delete(&cred_svc(name));
    // 監視トークンは切り替え機能とは独立した任意機能だが、アカウント自体を削除するなら
    // 紐づく監視トークンも道連れで消す（孤児のまま Keychain に残さない）
    keychain_delete(&monitor_token_svc(name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub(name: &str, org_id: &str, email: &str) -> StoredAccount {
        StoredAccount {
            name: name.to_string(),
            display_name: None,
            email: email.to_string(),
            plan: String::new(),
            org_id: org_id.to_string(),
            oauth_account: None,
            has_credentials: false,
            usage_cache: None,
        }
    }

    #[test]
    fn find_match_idx_prefers_org_id() {
        let accounts = vec![
            stub("work", "org-1", "work@example.com"),
            stub("personal", "org-2", "personal@example.com"),
        ];
        assert_eq!(find_match_idx(&accounts, Some("org-2"), None), Some(1));
        // email が同じでも org_id が一致しなければ別アカウント扱い
        assert_eq!(
            find_match_idx(&accounts, Some("org-3"), Some("work@example.com")),
            None
        );
    }

    #[test]
    fn find_match_idx_falls_back_to_email_only_when_org_id_missing() {
        let accounts = vec![stub("work", "", "work@example.com")];
        assert_eq!(find_match_idx(&accounts, None, Some("work@example.com")), Some(0));
        assert_eq!(find_match_idx(&accounts, None, Some("other@example.com")), None);
    }

    #[test]
    fn find_match_idx_empty_org_id_does_not_match_empty_org_id() {
        // org_id が空同士でも「org_id 一致」扱いにしない（未識別の複数アカウントを
        // 誤って同一視すると、二重登録の検出漏れになる）
        let accounts = vec![
            stub("legacy-a", "", "a@example.com"),
            stub("legacy-b", "", "b@example.com"),
        ];
        assert_eq!(find_match_idx(&accounts, Some(""), None), None);
        assert_eq!(find_match_idx(&accounts, None, None), None);
    }

    #[test]
    fn identify_extracts_org_and_email() {
        let v = serde_json::json!({
            "organizationUuid": "org-abc",
            "emailAddress": "user@example.com",
            "organizationType": "claude_max",
        });
        assert_eq!(
            identify(&v),
            (Some("org-abc".to_string()), Some("user@example.com".to_string()))
        );
    }

    #[test]
    fn identify_missing_fields_returns_none() {
        let v = serde_json::json!({ "organizationType": "claude_pro" });
        assert_eq!(identify(&v), (None, None));
    }

    #[test]
    fn derive_account_name_dedupes() {
        let existing = vec![stub("alice", "org-1", "alice@example.com")];
        assert_eq!(derive_account_name(Some("alice@example.com"), &existing), "alice-2");
        assert_eq!(derive_account_name(Some("new@example.com"), &existing), "new");
        assert_eq!(derive_account_name(None, &existing), "account");
    }

    #[test]
    fn is_live_account_requires_nonempty_org_id() {
        assert!(!is_live_account("", Some("")));
        assert!(!is_live_account("org-1", None));
        assert!(is_live_account("org-1", Some("org-1")));
    }

    #[test]
    fn resolve_display_name_falls_back_to_name() {
        assert_eq!(resolve_display_name("share3", None), "share3");
    }

    #[test]
    fn resolve_display_name_prefers_display_name_when_set() {
        assert_eq!(resolve_display_name("share3", Some("仕事用")), "仕事用");
    }

    #[test]
    fn resolve_display_name_falls_back_when_display_name_is_empty() {
        // rename_account がトリム後に空文字を None へ正規化するが、
        // 何らかの経路で空文字が入っても表示が壊れないよう表示側でも防御する
        assert_eq!(resolve_display_name("share3", Some("")), "share3");
    }

    fn names(accounts: &[StoredAccount]) -> Vec<&str> {
        accounts.iter().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn reorder_stored_accounts_full_order() {
        let mut accounts = vec![stub("a", "", ""), stub("b", "", ""), stub("c", "", "")];
        let order = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        reorder_stored_accounts(&mut accounts, &order);
        assert_eq!(names(&accounts), vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_stored_accounts_partial_order_keeps_rest_at_end() {
        // b は order に含まれない。元の相対順序を保ったまま末尾に残る
        let mut accounts = vec![stub("a", "", ""), stub("b", "", ""), stub("c", "", "")];
        let order = vec!["c".to_string(), "a".to_string()];
        reorder_stored_accounts(&mut accounts, &order);
        assert_eq!(names(&accounts), vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_stored_accounts_ignores_unknown_names() {
        // "ghost" は存在しないアカウント名（削除済み等）。無視される
        let mut accounts = vec![stub("a", "", ""), stub("b", "", ""), stub("c", "", "")];
        let order = vec!["c".to_string(), "ghost".to_string(), "a".to_string()];
        reorder_stored_accounts(&mut accounts, &order);
        assert_eq!(names(&accounts), vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_stored_accounts_empty_order_keeps_original() {
        let mut accounts = vec![stub("a", "", ""), stub("b", "", "")];
        reorder_stored_accounts(&mut accounts, &[]);
        assert_eq!(names(&accounts), vec!["a", "b"]);
    }

    #[test]
    fn parse_acct_attr_reads_quoted_value() {
        let text = "    \"acct\"<blob>=\"taisei\"\n    \"svce\"<blob>=\"Claude Code-credentials\"\n";
        assert_eq!(parse_acct_attr(text), Some("taisei".to_string()));
    }

    #[test]
    fn parse_acct_attr_rejects_null_value() {
        // <NULL>（引用符無し）を素朴に二重引用符探索すると "acct" というラベル自体を
        // 値と誤認するバグがあった。引用符で囲まれていない値は無効として弾く
        let text = "    \"acct\"<blob>=<NULL>\n";
        assert_eq!(parse_acct_attr(text), None);
    }

    #[test]
    fn parse_acct_attr_rejects_empty_value() {
        let text = "    \"acct\"<blob>=\"\"\n";
        assert_eq!(parse_acct_attr(text), None);
    }

    #[test]
    fn parse_acct_attr_ignores_other_lines() {
        let text = "    \"svce\"<blob>=\"Claude Code-credentials\"\n";
        assert_eq!(parse_acct_attr(text), None);
    }

    #[test]
    fn strip_legacy_shell_blocks_removes_single_block() {
        let content = format!(
            "export FOO=1\n\n{LEGACY_SHELL_BEGIN}\nexport CLAUDE_CODE_OAUTH_TOKEN=\"x\"\n{LEGACY_SHELL_END}\nexport BAR=2\n"
        );
        let result = strip_legacy_shell_blocks(&content).expect("ブロックがあれば Some");
        assert!(!result.contains(LEGACY_SHELL_BEGIN));
        assert!(result.contains("export FOO=1"));
        assert!(result.contains("export BAR=2"));
    }

    #[test]
    fn strip_legacy_shell_blocks_removes_multiple_blocks() {
        let block = format!("{LEGACY_SHELL_BEGIN}\nexport X=1\n{LEGACY_SHELL_END}\n");
        let content = format!("export A=1\n{block}export B=2\n{block}export C=3\n");
        let result = strip_legacy_shell_blocks(&content).expect("複数ブロックがあれば Some");
        assert!(!result.contains(LEGACY_SHELL_BEGIN));
        assert!(result.contains("export A=1"));
        assert!(result.contains("export B=2"));
        assert!(result.contains("export C=3"));
    }

    #[test]
    fn strip_legacy_shell_blocks_returns_none_when_absent() {
        assert_eq!(strip_legacy_shell_blocks("export FOO=1\n"), None);
    }

    /// 実機で報告された ~/.zshrc の現物（`security find-generic-password` 経由で
    /// CC Anatomy-active を読む2世代目の形式）をそのまま貼った回帰テスト。
    /// direct-export 形式（1世代目、上のテストで既にカバー）とは本文が異なるため、
    /// 別の世代のブロックとして独立に確認する
    #[test]
    fn strip_legacy_shell_blocks_removes_real_world_active_lookup_format() {
        let content = "\n# sentry\nfpath=(\"/Users/taisei_o2yama/.local/share/zsh/site-functions\" $fpath)\n\n# >>> CC Anatomy account switcher >>>\n__cc_anatomy_token=\"$(security find-generic-password -s 'CC Anatomy-active' -w 2>/dev/null)\"\n[ -n \"$__cc_anatomy_token\" ] && export CLAUDE_CODE_OAUTH_TOKEN=\"$__cc_anatomy_token\"\nunset __cc_anatomy_token\n# <<< CC Anatomy account switcher <<<\n";
        let result = strip_legacy_shell_blocks(content).expect("ブロックがあれば Some");
        assert!(!result.contains(LEGACY_SHELL_BEGIN));
        assert!(!result.contains("CC Anatomy-active"));
        assert!(result.contains("# sentry"));
        assert!(result.contains("fpath="));
    }

    #[test]
    fn hash_changed_detects_difference() {
        let baseline = LoginBaseline { hash: "abc".to_string(), target_name: None };
        assert!(!hash_changed(&baseline, "abc"));
        assert!(hash_changed(&baseline, "xyz"));
    }

    #[test]
    fn sha256_hex_is_deterministic_and_sensitive_to_content() {
        let a = sha256_hex("hello");
        let b = sha256_hex("hello");
        let c = sha256_hex("world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64); // 32 bytes -> 64 hex chars
    }

    fn oauth(org_id: &str, email: &str) -> serde_json::Value {
        serde_json::json!({ "organizationUuid": org_id, "emailAddress": email })
    }

    #[test]
    fn resolve_live_owner_trusts_oauth_account_when_hash_matches() {
        // hash が前回記録と一致＝外部からの書き換えなし。profile を呼ばずに oauthAccount を信じる
        let account = oauth("org-1", "user@example.com");
        let owner = resolve_live_owner(Some("hash-a"), "hash-a", &account, Some("token"), |_| {
            panic!("hash が一致しているのに profile を呼んではいけない")
        })
        .expect("一致時は成功するはず");
        assert_eq!(owner.org_id.as_deref(), Some("org-1"));
        assert_eq!(owner.email.as_deref(), Some("user@example.com"));
        assert!(!owner.mismatched);
    }

    #[test]
    fn resolve_live_owner_confirms_via_profile_when_hash_differs_but_agrees() {
        // hash 不一致（＝自動 refresh 等）でも、profile の email が oauthAccount と一致すれば
        // ズレなしとして org_id を信頼したまま進める
        let account = oauth("org-1", "user@example.com");
        let owner = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), |_| {
            Ok(serde_json::json!({ "account": { "email": "user@example.com" } }).to_string())
        })
        .expect("profile が一致すれば成功するはず");
        assert_eq!(owner.org_id.as_deref(), Some("org-1"));
        assert_eq!(owner.email.as_deref(), Some("user@example.com"));
        assert!(!owner.mismatched);
    }

    #[test]
    fn resolve_live_owner_detects_hijack_when_profile_disagrees() {
        // hash 不一致で、profile の実際の持ち主が oauthAccount の記載と違う
        // ＝別アカウントのセッションが refresh でライブを巻き戻した状態
        let account = oauth("org-1", "stale@example.com");
        let owner = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), |_| {
            Ok(serde_json::json!({ "account": { "email": "real@example.com" } }).to_string())
        })
        .expect("profile が確認できれば成功扱い（内容は mismatched で示す）");
        assert!(owner.mismatched);
        assert_eq!(owner.org_id, None, "ズレたら org_id は信用しない");
        assert_eq!(owner.email.as_deref(), Some("real@example.com"));
    }

    #[test]
    fn resolve_live_owner_aborts_when_profile_unconfirmed() {
        // hash 不一致で profile 確認も失敗（401・ネットワークエラー等）＝推測せず中断する
        let account = oauth("org-1", "user@example.com");
        let result = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), |_| {
            Err("401".to_string())
        });
        assert!(result.is_err());
    }

    #[test]
    fn resolve_live_owner_aborts_when_no_access_token_and_hash_differs() {
        // access token 自体が読めず、かつ hash も一致しない＝確認しようがないため中断する
        let account = oauth("org-1", "user@example.com");
        let result = resolve_live_owner(Some("hash-old"), "hash-new", &account, None, |_| {
            panic!("token が無いので呼ばれないはず")
        });
        assert!(result.is_err());
    }

    #[test]
    fn resolve_live_owner_no_baseline_requires_confirmation() {
        // last_live_hash が None（初回等）でも「未確認」と同様に profile 確認を要求する
        let account = oauth("org-1", "user@example.com");
        let owner = resolve_live_owner(None, "hash-new", &account, Some("token"), |_| {
            Ok(serde_json::json!({ "account": { "email": "user@example.com" } }).to_string())
        })
        .expect("profile が一致すれば成功するはず");
        assert!(!owner.mismatched);
    }

    fn cache(five_pct: f64, seven_pct: f64, five_reset: Option<i64>, fetched_at: i64) -> UsageCache {
        UsageCache {
            five_pct,
            seven_pct,
            five_reset,
            seven_reset: None,
            fetched_at,
        }
    }

    #[test]
    fn token_is_still_valid_compares_ms_to_secs() {
        // expiresAt は epoch ミリ秒、now は epoch 秒。単位を揃え忘れると
        // 「期限切れなのに有効」「有効なのに期限切れ」の両方の誤判定になりうる
        assert!(token_is_still_valid(1_000_000, 900)); // 1_000_000ms = 1000s > 900s
        assert!(!token_is_still_valid(1_000_000, 1_100)); // 1000s <= 1100s
    }

    #[test]
    fn resolve_usage_source_order_live_leads_when_available() {
        // ライブなら先頭はライブ OAuth。ただし単一ソースではなく、失敗時に備えて
        // 監視トークン・スナップショットも後続候補として並ぶ（2026-07-27: フォールバック連鎖化）
        assert_eq!(
            resolve_usage_source_order(true, true, true),
            vec![UsageSource::LiveOauth, UsageSource::MonitorToken, UsageSource::SnapshotOauth]
        );
        assert_eq!(resolve_usage_source_order(true, false, false), vec![UsageSource::LiveOauth]);
    }

    #[test]
    fn resolve_usage_source_order_live_falls_back_to_monitor_then_snapshot() {
        // ライブ OAuth が失敗（期限切れ等）したときに実際に試される後続候補の並び。
        // get_accounts_usage 側はこの順で1つずつ試し、最初に成功したものを採用する
        let order = resolve_usage_source_order(true, true, true);
        assert_eq!(order, vec![UsageSource::LiveOauth, UsageSource::MonitorToken, UsageSource::SnapshotOauth]);
    }

    #[test]
    fn resolve_usage_source_order_non_live_excludes_live_oauth() {
        // 非ライブはライブの資格情報を使えない（他アカウントが消費中のため）ので
        // ライブ OAuth はそもそも候補に入らない
        assert_eq!(
            resolve_usage_source_order(false, true, true),
            vec![UsageSource::MonitorToken, UsageSource::SnapshotOauth]
        );
        assert_eq!(resolve_usage_source_order(false, true, false), vec![UsageSource::MonitorToken]);
    }

    #[test]
    fn resolve_usage_source_order_non_live_falls_back_to_snapshot() {
        assert_eq!(resolve_usage_source_order(false, false, true), vec![UsageSource::SnapshotOauth]);
    }

    #[test]
    fn resolve_usage_source_order_empty_when_nothing_available() {
        // 呼び出し側（get_accounts_usage）はここでキャッシュへフォールバックする
        assert_eq!(resolve_usage_source_order(false, false, false), Vec::<UsageSource>::new());
    }

    #[test]
    fn org_id_matches_requires_exact_match_when_expected_is_present() {
        assert!(org_id_matches("org-1", "org-1"));
        assert!(!org_id_matches("org-1", "org-2"));
    }

    #[test]
    fn org_id_matches_skips_check_when_expected_is_empty() {
        // org_id 導入前からの旧登録等、対象アカウントに org_id が無いレアケースは
        // 照合しようがないため、従来どおり紐づけを許可する（トークン側の org_id は問わない）
        assert!(org_id_matches("", "org-anything"));
        assert!(org_id_matches("", ""));
    }

    #[test]
    fn match_relogin_target_matches_by_org_id() {
        assert_eq!(
            match_relogin_target("org-1", "a@example.com", Some("org-1"), Some("b@example.com")),
            ReloginMatch::Match
        );
    }

    #[test]
    fn match_relogin_target_mismatches_by_org_id() {
        assert_eq!(
            match_relogin_target("org-1", "a@example.com", Some("org-2"), Some("a@example.com")),
            ReloginMatch::Mismatch
        );
    }

    #[test]
    fn match_relogin_target_undetermined_when_live_org_unknown() {
        // Keychain 完了検知と ~/.claude.json 照合の書き込み順の隙間で、まだ live 側の
        // org_id が読めないことがある。不一致と決めつけず待つ
        assert_eq!(
            match_relogin_target("org-1", "a@example.com", None, None),
            ReloginMatch::Undetermined
        );
    }

    #[test]
    fn match_relogin_target_falls_back_to_email_when_org_id_empty() {
        assert_eq!(
            match_relogin_target("", "a@example.com", Some("org-anything"), Some("a@example.com")),
            ReloginMatch::Match
        );
        assert_eq!(
            match_relogin_target("", "a@example.com", Some("org-anything"), Some("b@example.com")),
            ReloginMatch::Mismatch
        );
        assert_eq!(
            match_relogin_target("", "a@example.com", Some("org-anything"), None),
            ReloginMatch::Undetermined
        );
    }

    #[test]
    fn match_relogin_target_rejects_when_both_org_id_and_email_are_empty() {
        // start_add_account_login 側の can_relogin チェックで通常は事前に弾かれるが、
        // ここでも安全側に倒して拒否する
        assert_eq!(
            match_relogin_target("", "", Some("org-anything"), Some("a@example.com")),
            ReloginMatch::Mismatch
        );
    }

    #[test]
    fn auto_sync_should_skip_when_hash_matches_last_live_hash() {
        assert!(auto_sync_should_skip(Some("abc"), None, "abc"));
    }

    #[test]
    fn auto_sync_should_skip_when_hash_matches_last_checked_hash() {
        // last_live_hash は「登録済みとして書き戻し済み」の記録なので、未登録ライブの
        // 居座り中は更新されない。last_checked_hash 側の一致でもスキップできる必要がある
        assert!(auto_sync_should_skip(None, Some("xyz"), "xyz"));
    }

    #[test]
    fn auto_sync_should_not_skip_when_hash_matches_neither() {
        assert!(!auto_sync_should_skip(Some("abc"), Some("def"), "xyz"));
        assert!(!auto_sync_should_skip(None, None, "xyz"));
    }

    #[test]
    fn auto_sync_should_bail_when_became_inconsistent() {
        // フェーズ2（ロック外の profile API 呼び出し）の間に switch_account のロールバック
        // 失敗等で不整合状態になっていたら、last_live_hash が同じでも書き込んではいけない
        assert!(auto_sync_should_bail(true, Some("abc"), Some("abc")));
    }

    #[test]
    fn auto_sync_should_bail_when_last_live_hash_moved() {
        // フェーズ2の間に switch_account / import_live_account 等が last_live_hash を
        // 進めていたら、フェーズ2で確認した owner はもう前提が崩れている
        assert!(auto_sync_should_bail(false, Some("new-hash"), Some("old-hash")));
        assert!(auto_sync_should_bail(false, None, Some("old-hash")));
        assert!(auto_sync_should_bail(false, Some("new-hash"), None));
    }

    #[test]
    fn auto_sync_should_not_bail_when_nothing_changed() {
        assert!(!auto_sync_should_bail(false, Some("abc"), Some("abc")));
        assert!(!auto_sync_should_bail(false, None, None));
    }

    #[test]
    fn cache_is_fresh_enough_within_window() {
        assert!(cache_is_fresh_enough(1_000, 1_059));
        assert!(!cache_is_fresh_enough(1_000, 1_060));
    }

    #[test]
    fn to_account_usage_no_cache_reports_stale_without_values() {
        let usage = to_account_usage("acct", None, false, 1_000);
        assert_eq!(usage.five_pct, None);
        assert!(usage.stale, "キャッシュが無ければ stale 扱いにする");
        assert!(!usage.five_probably_reset);
    }

    #[test]
    fn to_account_usage_fresh_fetch_is_not_stale() {
        let c = cache(9.0, 52.0, Some(2_000), 1_000);
        let usage = to_account_usage("acct", Some(&c), false, 1_000);
        assert_eq!(usage.five_pct, Some(9.0));
        assert_eq!(usage.seven_pct, Some(52.0));
        assert_eq!(usage.fetched_at, Some(1_000));
        assert!(!usage.stale);
    }

    #[test]
    fn to_account_usage_cached_fetch_is_stale() {
        let c = cache(9.0, 52.0, Some(2_000), 1_000);
        let usage = to_account_usage("acct", Some(&c), true, 5_000);
        assert!(usage.stale);
    }

    #[test]
    fn to_account_usage_flags_five_hour_probably_reset() {
        // 現在時刻(5_000) がリセット時刻(2_000) を過ぎている＝実質 0% とみなせる
        let c = cache(80.0, 52.0, Some(2_000), 1_000);
        let usage = to_account_usage("acct", Some(&c), true, 5_000);
        assert!(usage.five_probably_reset);
    }

    #[test]
    fn to_account_usage_not_yet_reset_before_reset_time() {
        let c = cache(80.0, 52.0, Some(9_000), 1_000);
        let usage = to_account_usage("acct", Some(&c), true, 5_000);
        assert!(!usage.five_probably_reset);
    }

    #[test]
    fn to_account_usage_no_reset_time_never_flags_reset() {
        let c = cache(80.0, 52.0, None, 1_000);
        let usage = to_account_usage("acct", Some(&c), true, 5_000);
        assert!(!usage.five_probably_reset);
    }
}
