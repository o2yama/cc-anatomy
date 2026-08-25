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

use crate::actions::{usage_attempt_min_interval, NON_LIVE_MIN_REFETCH_SECS, USAGE_MIN_REFETCH_SECS};
use serde::{Deserialize, Serialize};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// スナップショットの refresh token 期限接近の確認ダイアログを最後に出した時刻
    /// （epoch ミリ秒）。同じアカウントに毎サイクル聞かないための 24 時間スロットル用。
    /// 旧バージョンの accounts.json には無いフィールドなので default 必須
    #[serde(default)]
    refresh_prompted_at: Option<i64>,
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
/// トレイのタイトル表示（`live_usage_summary_gated` の失敗時）専用のフォールバック。
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

// 一括照会の連打防止。前回取得からこの秒数未満ならキャッシュをそのまま返す
// （モーダルを開き直す・トレイの手動更新連打で毎回 API を叩かないようにする）。
//
// 実体・「なぜ300秒か」の実測根拠はいずれも `crate::actions::USAGE_MIN_REFETCH_SECS`
// の doc コメントに一本化した（2026-08-22、第5ラウンド U-1 で定数の実体を移設、
// 第6ラウンド V-5 で根拠の説明もそちらへ移し重複を削った。以後の変更は actions.rs で行うこと）。
//
// NON_LIVE_MIN_REFETCH_SECS（非ライブアカウントの緩い閾値）も同じ第6ラウンド（V-2）で
// `crate::actions::NON_LIVE_MIN_REFETCH_SECS` へ移した。tray.rs のフォールバック経路が
// 使う `usage_attempt_min_interval` と同じ場所に置くことで、両ファイルが同じ規則を
// 別々に持つ構図（切り出したはずが本体に別ロジックが残る）を避けるため

// 表示中の使用量が「本当に古い」と言える閾値（tray.rs の注記表示が使う）は
// `crate::actions::USAGE_STALE_NOTE_SECS` へ移設した（2026-08-22、第5ラウンド U-3）。
// 元は accounts.rs と accounts_stub.rs の両方に同じ値をミラーしていたが、cfg が排他なので
// 値がズレてもコンパイル時・実行時のどちらでも検知できなかった。全プラットフォームで
// コンパイルされる actions.rs へ一本化し、ミラーは廃止した

// 429（レート制限）を受けた後に `/api/oauth/usage` への照会を控える段階的バックオフは
// 2026-08-22 の第4ラウンド（S-2）で撤去した。撤去の経緯・理由は actions.rs の
// バックオフ撤去コメントを参照。撤去後も「同一サイクル内で429を観測したら以降の
// ソースも同じエンドポイントを叩かない」というローカルフラグ（cycle_saw_rate_limited、
// get_accounts_usage 内）だけは残す

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

/// 直近の取得から閾値未満なら再照会せずキャッシュを返してよいか。
/// 閾値はライブアカウントかどうかで切り替える（is_live なら USAGE_MIN_REFETCH_SECS、
/// それ以外は NON_LIVE_MIN_REFETCH_SECS）
fn cache_is_fresh_enough(fetched_at: i64, now: i64, is_live: bool) -> bool {
    let threshold = if is_live { USAGE_MIN_REFETCH_SECS } else { NON_LIVE_MIN_REFETCH_SECS };
    now - fetched_at < threshold
}

/// force による「キャッシュ新鮮判定のスキップ」がこの対象に効くか（2026-08-22、R-6）。
/// force の効果はライブアカウントに限定する: 非ライブは force の値に関係なく常に
/// NON_LIVE_MIN_REFETCH_SECS の閾値が適用される（フロントがポップオーバーを開いたまま
/// accounts-updated を購読して再読込するため、常時 force=true だと開きっぱなしのたびに
/// 登録アカウント全件へ強制照会が走ってしまう）
fn force_skips_freshness_check(force: bool, is_live: bool) -> bool {
    force && is_live
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

/// `get_accounts_usage` の戻り値。従来はアカウント一覧だけを返していたが、ライブアカウントに
/// 対する LiveOauth 経路の試行結果（`live_error`）も一緒に返すようにした（2026-08-22、B-1）。
/// これにより `tray::fetch_raw_status` は `actions::live_usage_summary_gated()` を別途呼ばずに済み、
/// ライブアカウントの `/api/oauth/usage` への打鍵が1サイクルあたり2回→1回に減る。
///
/// `live_error` の規約（2026-08-22、第4ラウンド S-2 でグローバルバックオフを撤去したことに
/// 合わせて更新）。`targets`（＝ `has_credentials` の登録アカウントのうち今回照会対象になった
/// もの）にライブアカウントが含まれない場合のみ None。含まれる場合は次のいずれか:
/// - 新鮮キャッシュで continue した（今回は照会していない）→ このサイクルで既に429を
///   観測していなければ None、観測済みなら `RateLimited`（新鮮キャッシュ返しでも「今回は
///   取得を控えた」ことをそのまま伝えるため。`live_error_for_fresh_cache`）
/// - 今回 LiveOauth 経路を試行した → その結果そのまま（成功なら None、失敗ならその分類）
pub struct UsageBatch {
    pub accounts: Vec<AccountUsage>,
    pub live_error: Option<crate::actions::LiveUsageError>,
}

/// 新鮮キャッシュで continue する対象について live_error に何を入れるかの純粋関数
/// （2026-08-22、T-3・R-7。第4ラウンドでグローバルバックオフを撤去した後も、
/// 「このサイクルで既に429を観測したか」というローカルフラグに対して同じ規則を適用する）。
/// ライブでなければ常に None（この関数はライブのときしか呼ばれない想定だが、境界を
/// 明示するため is_live も引数に取る）。ライブかつ観測済みなら「今回は取得を控えた」ことを
/// None で隠さず RateLimited のまま伝える
fn live_error_for_fresh_cache(is_live: bool, cycle_saw_rate_limited: bool) -> Option<crate::actions::LiveUsageError> {
    if is_live && cycle_saw_rate_limited {
        Some(crate::actions::LiveUsageError::RateLimited)
    } else {
        None
    }
}

/// 429 を観測した後、同一サイクル内で次のソースへ `/api/oauth/usage` を試みてよいかの
/// 純粋関数（2026-08-22、第4ラウンド S-2）。段階的バックオフ（グローバル状態・指数的な
/// 待ち時間の延長）は撤去したが、「同一サイクル内で429を観測したら以降のソースも同じ
/// エンドポイントへ無駄打ちしない」というローカルな抑制だけは残す（次のサイクル＝5分後は
/// 必ず再試行する）。cycle_saw_rate_limited は false→true の一方向にしか変わらないため、
/// ループ内で都度この関数を呼んでも判定がぶれることはない
fn should_skip_usage_source(cycle_saw_rate_limited: bool) -> bool {
    cycle_saw_rate_limited
}

/// 登録済み全アカウント（has_credentials のもの）の使用率をまとめて取得する。
/// 取得元の優先順位は `resolve_usage_source_order` を参照。
///
/// - `force` は「ライブアカウントのキャッシュ新鮮判定だけをスキップする」という限定的な
///   意味を持つ（2026-08-22、R-6）。ライブは force=true で USAGE_MIN_REFETCH_SECS を無視して
///   必ず照会する。非ライブは force の値に関係なく常に NON_LIVE_MIN_REFETCH_SECS の閾値を
///   適用する（フロントがポップオーバーを開いたまま accounts-updated を購読して再読込するため、
///   常時 force=true だと開きっぱなしのたびに登録アカウント全件へ強制照会が走ってしまう）
///   （このサイクルで既に429を観測している場合はそれが優先され、force でも HTTP は打たない）
/// - U-1（2026-08-22、第5ラウンド）: キャッシュ新鮮判定を抜けた後も、実際に HTTP を打つ直前で
///   `usage_attempt_min_interval` / `gate_usage_attempt` による「最後に試行した時刻」ゲートを
///   通す。force=true でも無制限にはならず USAGE_FORCE_MIN_ATTEMPT_SECS（45秒）の下限は残る。
///   このゲートは**成否にかかわらず**試行時点で記録するため、429・期限切れ・通信不能等が
///   続く間もサイクルをまたいで間隔が保たれる（成功時のみ更新される usage_cache の
///   fetched_at では、失敗が続く間はスロットルが効かなくなってしまうため別の状態として持つ）
/// - 照会に成功したら usage_cache へ保存する。切り替え後もこれが「最終既知値」として残る
/// - refresh は一切行わない（access token 期限切れは正常な状態として静かにキャッシュへ委ねる）
pub fn get_accounts_usage(force: bool) -> Result<UsageBatch, String> {
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
    // ライブアカウント（登録済みなら targets に高々1件だけ含まれる）の LiveOauth 経路の
    // 試行結果。UsageBatch::live_error としてそのまま返す
    let mut live_error: Option<crate::actions::LiveUsageError> = None;
    // このサイクル（この関数の1回の呼び出し）内で429を一度でも観測したか。
    // 2026-08-22 第4ラウンド（S-2）でグローバルなバックオフ状態は撤去したが、
    // 「同一サイクル内で以降のソースへ無駄打ちしない」ためだけのローカルフラグとして残す
    // （actions.rs のバックオフ撤去コメント参照）
    let mut cycle_saw_rate_limited = false;
    for t in &targets {
        if !force_skips_freshness_check(force, t.is_live)
            && t.cache.as_ref().is_some_and(|c| cache_is_fresh_enough(c.fetched_at, now, t.is_live))
        {
            results.push(to_account_usage(&t.name, t.cache.as_ref(), true, now));
            // R-7: 新鮮キャッシュを返すだけの場合でも、このサイクルで既に429を観測して
            // いるなら「今回は取得を控えた」ことをそのまま伝える（None にすると Note が消え、
            // ユーザーから見て古い値と区別が付かなくなる）
            if t.is_live {
                live_error = live_error_for_fresh_cache(t.is_live, cycle_saw_rate_limited);
            }
            continue;
        }

        let snapshot_token = stored_access_token(&t.name).filter(|(_, exp)| token_is_still_valid(*exp, now));
        let source_order = resolve_usage_source_order(t.is_live, has_monitor_token(&t.name), snapshot_token.is_some());
        // U-1（2026-08-22、第5ラウンド）: 「最後に試行した時刻」ゲートの下限間隔。
        // LiveOauth・SnapshotOauth どちらも /api/oauth/usage を叩くため同じ間隔を使うが、
        // 記録するキーはソースごとに分ける（下記コメント参照）
        let min_interval = usage_attempt_min_interval(t.is_live, force);

        // 優先順位どおりに試し、最初に成功したものを採用する（失敗・スキップしたソースは
        // 次へ進む。2026-07-27: 従来は is_live のライブ OAuth 1本勝負で、期限切れのまま
        // 直接キャッシュへ落ちていた）
        let mut fetched = None;
        let mut live_oauth_error: Option<crate::actions::LiveUsageError> = None;
        for source in &source_order {
            // T-1（2026-08-22）: skip はソースごとに都度評価する。従来は target 単位で
            // 1回だけ確定していたため、同一 target 内で source_order の最初のソース（例:
            // LiveOauth）が429を観測して cycle_saw_rate_limited を立てても、その直後に試す
            // 次のソース（例: SnapshotOauth）には反映されず、「429観測後は同一サイクル内で
            // 以降のソースも打たない」が守られていなかった。false→true の一方向にしか
            // 変わらないため、都度評価してもループ内で判定がぶれることはない
            let skip = should_skip_usage_source(cycle_saw_rate_limited);
            fetched = match source {
                UsageSource::LiveOauth => {
                    // このサイクルで既に429を観測していたら打たない（force より優先）
                    if skip {
                        live_oauth_error = Some(crate::actions::LiveUsageError::RateLimited);
                        None
                    } else {
                        match crate::credentials::live_token_with_expiry() {
                            Err(e) => {
                                live_oauth_error = Some(crate::actions::LiveUsageError::Other(e));
                                None
                            }
                            // 期限切れなら照会せず Expired 扱い（無駄な401リクエストを避ける。
                            // expiresAt が取れない場合は「不明」として素通しし、実際の応答で判断する）
                            Ok((_, expires_at)) if expires_at.is_some_and(|exp| !token_is_still_valid(exp, now)) => {
                                live_oauth_error = Some(crate::actions::LiveUsageError::Expired);
                                None
                            }
                            Ok((token, _)) => {
                                // U-1: 試行間隔ゲート。同一サイクル内の cycle_saw_rate_limited
                                // （skip）とは別に、サイクルをまたいだ「最後に試行した時刻」を
                                // 見る。キーは LiveOauth 専用（`<name>:live`）にし、
                                // SnapshotOauth の試行間隔を巻き添えで消費しないようにする
                                // （同じキーを共有すると、429以外の理由でここが失敗した直後の
                                // SnapshotOauth フォールバックまで塞いでしまう）
                                let live_key = format!("{}:live", t.name);
                                if crate::actions::gate_usage_attempt(&live_key, min_interval) {
                                    let outcome = crate::actions::oauth_get_checked(&token, crate::actions::USAGE_URL);
                                    if matches!(outcome, crate::actions::FetchOutcome::RateLimited) {
                                        cycle_saw_rate_limited = true;
                                    }
                                    // FetchOutcome → LiveUsageError の3分岐（成功/429/期限切れ他）は
                                    // 純粋関数 live_oauth_outcome_to_result に集約済み（R-3 追加テスト
                                    // 項目1: HTTP を打たずにテストできるようにするための抽出）
                                    match crate::actions::live_oauth_outcome_to_result(outcome) {
                                        Ok(summary) => {
                                            // W-1（2026-08-22 第7ラウンド）: 成功したので
                                            // 直近の失敗理由を消す（次回このキーが
                                            // ゲートに塞がれたとき、古い失敗理由を
                                            // 誤って再利用しないため）
                                            crate::actions::clear_usage_error(&live_key);
                                            live_oauth_error = None;
                                            Some(summary)
                                        }
                                        Err(e) => {
                                            // W-1: 失敗理由を live_key に記録する。
                                            // 次回このキーがゲートに塞がれたとき、
                                            // `resolve_gated_error` がこれを「塞いだ」という
                                            // 事実の代わりに返す
                                            crate::actions::record_usage_error(&live_key, e.clone());
                                            live_oauth_error = Some(e);
                                            None
                                        }
                                    }
                                } else {
                                    // W-1（2026-08-22 第7ラウンド）: 従来はここで HTTP を
                                    // 一切打っていないのに `RateLimited`（レート制限）を
                                    // 決め打ちしていた。「ゲートに塞がれた」ことと
                                    // 「レート制限を受けた」ことは別物であり、
                                    // 前者を後者として表示すると事実と違う案内
                                    // （「取得が一時的に制限されています」）になる。
                                    // 直近にこのキーで実際に試行して分かった失敗理由
                                    // （USAGE_LAST_ERROR）を代わりに返す
                                    live_oauth_error = Some(crate::actions::resolve_gated_error(
                                        crate::actions::last_usage_error(&live_key),
                                    ));
                                    None
                                }
                            }
                        }
                    }
                }
                UsageSource::MonitorToken => keychain_read(&monitor_token_svc(&t.name))
                    .and_then(|token| crate::actions::usage_via_monitor_token(&token).ok()),
                UsageSource::SnapshotOauth => {
                    // このサイクルで既に429を観測していたら打たない
                    // （こちらも /api/oauth/usage を叩くため対象）
                    // U-1: さらに試行間隔ゲートも通す。キーは SnapshotOauth 専用
                    // （`<name>:snapshot`）にし、LiveOauth 側の記録とは独立させる
                    if skip || !crate::actions::gate_usage_attempt(&format!("{}:snapshot", t.name), min_interval) {
                        None
                    } else {
                        snapshot_token.as_ref().and_then(|(token, _)| {
                            match crate::actions::oauth_get_checked(token, crate::actions::USAGE_URL) {
                                crate::actions::FetchOutcome::Ok(body) => crate::actions::parse_usage_body(&body).ok(),
                                crate::actions::FetchOutcome::RateLimited => {
                                    cycle_saw_rate_limited = true;
                                    None
                                }
                                _ => None,
                            }
                        })
                    }
                }
            };
            if fetched.is_some() {
                break;
            }
        }

        if t.is_live {
            live_error = live_oauth_error;
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

    Ok(UsageBatch { accounts: results, live_error })
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
            "本アプリの環境診断/タスク抽出/AI分析/token 自動復帰の実行中は切り替え・追加ができません。完了してから実行してください。"
                .into(),
        );
    }
    Ok(())
}

/// アカウント操作（切替・ログイン系）が進行中かどうか。doc_analysis / diagnostics の
/// spawn 直前チェックに使う。ensure_app_not_busy 単体だと「チェック時点では非busy」を
/// 見るだけで、直後にアカウント操作が始まる TOCTOU が残るため、逆方向（アカウント操作側が
/// 分析の開始をブロックする）のガードとして用意する
pub static ACCOUNT_OP_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// switch_account / start_add_account_login の全区間で保持するガード。
/// 早期 return・`?` によるエラー経路でも Drop で必ずクリアされる
struct AccountOpGuard;

impl AccountOpGuard {
    fn acquire() -> Result<Self, String> {
        ensure_app_not_busy()?;
        // compare_exchange で「アカウント操作同士」の相互排他も保証する（2026-08-25 レビュー R3）。
        // 従来の store(true) は診断/分析の開始をブロックするだけで、switch_account と
        // refresh_snapshot_credentials（バックグラウンド起点）が並走でき、先に終わった側の
        // Drop がフラグを下ろしてガードが消えていた
        if ACCOUNT_OP_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("別のアカウント操作が進行中です。完了してから再試行してください。".into());
        }
        // セットした直後にもう一度確認する。ensure_app_not_busy の判定とこのセットの間に
        // 分析/診断が spawn されていたら、busy 状態のままアカウント操作を進めないよう戻す
        if crate::diagnostics::is_running()
            || crate::actions::is_agent_busy()
            || crate::doc_analysis::is_running()
        {
            ACCOUNT_OP_IN_PROGRESS.store(false, Ordering::SeqCst);
            return Err(
                "本アプリの環境診断/タスク抽出/AI分析/token 自動復帰の実行中は切り替え・追加ができません。完了してから実行してください。"
                    .into(),
            );
        }
        Ok(AccountOpGuard)
    }
}

impl Drop for AccountOpGuard {
    fn drop(&mut self) {
        ACCOUNT_OP_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
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
            refresh_prompted_at: None,
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
    /// 持ち主未確認（TokenExpired/NetworkError）のまま `trust_unverified=true` で続行した
    /// （2026-08-08 issue #3、レビュー案A）。**keychain_write も meta（org_id/email/
    /// oauth_account/last_live_hash）の更新も一切行わない**（sync-back を丸ごとスキップする）。
    /// 呼び出し側は切替処理自体は続行してよいが、警告を出すこと。
    ///
    /// なぜ「未確認のまま信じて書き込む」を選ばなかったか: 一度そのように実装したが
    /// レビューで却下された。last_live_hash（`Meta.last_live_hash` の doc 参照）が
    /// 「Keychain=旧アカウントの新トークン／oauthAccount=切り替え先」という過渡的な不整合の
    /// 状態で TokenExpired/NetworkError が起きると、誤った持ち主のまま登録済みアカウントの
    /// スナップショットを上書きし、かつ last_live_hash を「確認済み」として更新してしまう。
    /// 次サイクルの auto_sync_live（trust なし）はハッシュ一致で早期returnするため、
    /// 一度誤帰属すると二度と自己修復しない不可逆な破壊になる。書き込みを一切行わなければ
    /// last_live_hash は古いまま残り、次の sync-back（trust なしの手動再試行や
    /// auto_sync_live）が普通に再検証してくれる
    SkippedUnverified,
}

const PROFILE_UNCONFIRMED_MSG: &str = "ライブ資格情報の持ち主を確認できませんでした。少し待って再試行するか、全セッション終了後に再試行してください";
const LIVE_HIJACKED_WARNING: &str = "ライブのログインが実行中セッションにより巻き戻っていました。";
/// SyncBack::SkippedUnverified のとき、切替成功の warning としてそのまま notice に使われる
/// 文言（issue #3。フロント側は `outcome.warning` をそのまま表示するため、ここに成功文言
/// 込みのフルセンテンスを持つ）。UI の「続行する」直前に見せる確認文言（Accounts.tsx/App.tsx の
/// 説明テキスト）とは別物: あちらは「これから起きること」の予告、こちらは
/// 「実際に何が起きた／起きなかったか」の事後報告。「元のアカウントに戻す際、再ログインが
/// 必要になる場合があります」は、sync-back をスキップしたことで直前アカウントの最新資格情報
/// （snapshot 未更新）が失われるコストを明示する（2026-08-08 レビュー追記）
const UNVERIFIED_OWNER_SKIPPED_WARNING: &str = "切り替えました。ただし直前のアカウントの最新ログイン情報は同期されていません（持ち主未確認のため）。元のアカウントに戻す際、再ログインが必要になる場合があります。";

/// 持ち主確認（resolve_live_owner）で発生しうるエラーの分類（2026-08-08、issue #2 対応）。
/// 呼び出し元（sync_back_live_login / auto_sync_live）は既存どおり Result<_, String> で
/// UI（Tauri コマンド境界）まで運ぶため、型を消さずに `Display`／`From<OwnerError> for String`
/// でメッセージへ変換する。先頭の `KIND:` プレフィックス（wire format の契約は docs/dev-log.md
/// 参照）は2つの消費者が剥がして本文だけを表示する:
/// - TS 側（Tauri コマンド境界を越える経路）: api.ts の `describeAccountError`
/// - Rust 側（tray.rs のダイアログ表示など、コマンド境界を越えない経路）: `strip_owner_error_tag`
///
/// YAGNI: 「oauthAccount と実際の持ち主がズレていた」ケース（OwnerMismatch 相当）は
/// resolve_live_owner では発生させていない。LiveOwner.mismatched（Ok 側の結果。
/// apply_live_owner が登録済みアカウントと一致すれば警告付き Synced、一致しなければ
/// NeedsImport として扱う）で十分表現できており、専用の Err variant は不要と判断した
/// （2026-08-08 レビューで一度追加したが未使用のため撤去。必要になったら足す）
#[derive(Debug)]
enum OwnerError {
    /// access token の期限切れ（事前チェック、または 401・error フィールド応答での検出）。
    /// 期限切れなのは「現在ライブにログイン中のアカウント」の token。email が取れていれば
    /// 案内に埋め込む（identify() の結果を流用。取れなければ省略して汎用文言にする）
    TokenExpired(Option<String>),
    /// profile API に到達できなかった（接続失敗・タイムアウト等）
    NetworkError,
    /// 上記以外の予期しない失敗（応答の構文エラー・token 自体が読めない等）
    Other(String),
}

impl OwnerError {
    /// strip_owner_error_tag と同じ「既知の kind 一覧」を暗黙に共有する。
    /// kind を増減したら両方を確認すること
    fn kind(&self) -> &'static str {
        match self {
            OwnerError::TokenExpired(_) => "TOKEN_EXPIRED",
            OwnerError::NetworkError => "NETWORK_ERROR",
            OwnerError::Other(_) => "OTHER",
        }
    }

    fn message(&self) -> String {
        match self {
            // 「対象アカウントで」ではなく「現在ライブにログイン中のアカウントで」が正しい:
            // resolve_live_owner が見ているのは切り替え先ではなく、いま PC 全体のログインを
            // 握っているアカウントの access token（2026-08-08 レビュー指摘）
            OwnerError::TokenExpired(Some(email)) => format!(
                "現在 Claude Code にログイン中のアカウント（{email}）の token が期限切れです。\
                 Claude Code を一度実行すると token が更新されます。少し待ってから再試行してください"
            ),
            OwnerError::TokenExpired(None) =>
                "現在 Claude Code にログイン中のアカウントの token が期限切れです。\
                 Claude Code を一度実行すると token が更新されます。少し待ってから再試行してください"
                    .to_string(),
            // T-5（2026-08-22）: 429（レート制限）もここに寄せている（R-2）ため、通信障害専用の
            // 文言だと「ネットワークを確認してください」という誤案内になる。通信できない場合と
            // 一時的に制限されている場合のどちらにも当てはまる中立な文言にする
            OwnerError::NetworkError =>
                "現在 Claude Code にログイン中のアカウントの持ち主を確認できませんでした\
                 （通信できないか、一時的に制限されています）。時間をおいて再試行してください"
                    .to_string(),
            OwnerError::Other(msg) => msg.clone(),
        }
    }
}

impl std::fmt::Display for OwnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.kind(), self.message())
    }
}

impl From<OwnerError> for String {
    fn from(e: OwnerError) -> String {
        e.to_string()
    }
}

/// OwnerError の `KIND:message` からプレフィックスを剥がし本文だけを返す。
/// Tauri コマンド境界を越えない経路（tray.rs のダイアログ表示）向け。
/// TS 側の `describeAccountError`（api.ts）と対で、既知の kind 一覧は
/// `OwnerError::kind()` の実装と揃えること
pub(crate) fn strip_owner_error_tag(s: &str) -> &str {
    const KNOWN_KINDS: [&str; 3] = ["TOKEN_EXPIRED", "NETWORK_ERROR", "OTHER"];
    match s.split_once(':') {
        Some((kind, rest)) if KNOWN_KINDS.contains(&kind) => rest,
        _ => s,
    }
}

/// oauthAccount とライブ資格情報の実際の持ち主が一致するかを解決した結果
#[derive(Debug)]
struct LiveOwner {
    org_id: Option<String>,
    email: Option<String>,
    /// oauthAccount の記載と profile API の結果がズレていた（別アカウントのセッションが
    /// refresh でライブを巻き戻した等）。true のときは org_id を信用せず email だけで照合する
    mismatched: bool,
}

/// ライブの持ち主を解決する。ハッシュが前回記録と一致していれば「外部からの書き換えなし」と
/// みなし oauthAccount をそのまま信じる。不一致（または前回記録が無い）なら、まず
/// `expires_at` の事前チェックで期限切れを検出し（actions::is_token_expired と同じ
/// ロジックを共有。無駄な401リクエストを避ける。2026-08-08 issue #1 対応）、期限内なら
/// `fetch_profile`（実装は profile API 呼び出し。テストでは差し替える）で実際の持ち主を
/// 確認してから帰属を決める。確認できなければ Err で中断する（推測で書き込まない）。
///
/// force による「確認できなくても続行する」導線（issue #3）は、ここでは一切扱わない。
/// 一度は trust-fallback（oauthAccount を未確認のまま信じて書き込む）として実装したが
/// レビューで却下された: last_live_hash（`Meta.last_live_hash` の doc 参照）が
/// 「Keychain=旧アカウントの新トークン／oauthAccount=切り替え先」という過渡的な不整合の
/// 状態で TokenExpired/NetworkError が起きると、誤った持ち主のまま登録済みアカウントの
/// スナップショットを上書きし、かつ last_live_hash を更新してしまう。次サイクルの
/// auto_sync_live はハッシュ一致で「確認済み」と判断してしまうため、一度誤帰属すると
/// 二度と自己修復しない不可逆な破壊になる。安全な代替は sync_back_live_login 側で
/// 「書き込まずスキップする」（`SyncBack::SkippedUnverified`）
fn resolve_live_owner<F>(
    last_live_hash: Option<&str>,
    current_hash: &str,
    oauth_account: &serde_json::Value,
    access_token: Option<&str>,
    expires_at: Option<i64>,
    fetch_profile: F,
) -> Result<LiveOwner, OwnerError>
where
    F: FnOnce(&str) -> crate::actions::FetchOutcome,
{
    let (org_id, email) = identify(oauth_account);
    if last_live_hash == Some(current_hash) {
        return Ok(LiveOwner { org_id, email, mismatched: false });
    }

    let Some(token) = access_token else {
        return Err(OwnerError::Other(PROFILE_UNCONFIRMED_MSG.to_string()));
    };
    if crate::actions::is_token_expired(expires_at) {
        return Err(OwnerError::TokenExpired(email.clone()));
    }
    let body = match fetch_profile(token) {
        crate::actions::FetchOutcome::Ok(body) => body,
        crate::actions::FetchOutcome::Expired => return Err(OwnerError::TokenExpired(email.clone())),
        // 429（レート制限）は「認証は問題ないが今は確認できない・再試行すれば直る」という
        // 意味論が NetworkError と同じなので、そちらへ寄せる（2026-08-22、R-2）。
        // should_skip_unverified_sync_back と TS 側 canProceedUnverified が
        // TokenExpired/NetworkError のときしか「持ち主未確認でも続行」を許さないため、
        // Other のままだと 429 のたびに確認ダイアログすら出ずに切り替えが失敗する退行になる
        crate::actions::FetchOutcome::Network | crate::actions::FetchOutcome::RateLimited => {
            return Err(OwnerError::NetworkError)
        }
        crate::actions::FetchOutcome::Other(_) => {
            return Err(OwnerError::Other(PROFILE_UNCONFIRMED_MSG.to_string()))
        }
    };
    let profile: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| OwnerError::Other(PROFILE_UNCONFIRMED_MSG.to_string()))?;
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

/// resolve_live_owner の Err を受けて sync-back をスキップしてよいか（＝書き込まず切替続行を
/// 許可してよいか）の純粋判定（テスト容易性のため I/O から分離。2026-08-08 issue #3
/// レビュー案A）。trust_unverified かつ「今は確認できないだけ」（TokenExpired/NetworkError）の
/// ときだけ許可する。Other（応答の構文エラー等、真に予期しない失敗）・missing-token は
/// trust_unverified でも許可しない（不整合の疑いが残るため）
fn should_skip_unverified_sync_back(err: &OwnerError, trust_unverified: bool) -> bool {
    trust_unverified && matches!(err, OwnerError::TokenExpired(_) | OwnerError::NetworkError)
}

/// sync-back 本体。switch_account / start_add_account_login の「事前 sync-back」から呼ばれる
/// （呼び出し側がロックを保持したまま呼ぶ、ユーザー操作に伴う短時間の処理という位置づけ。
/// profile API 呼び出しを含めロック内で完結させる方針は今回変更しない。ロック外に出したのは
/// 60秒ごとに無人で走る auto_sync_live のみ。2026-07-26 レビュー M-B3 のスコープ）。
///
/// `trust_unverified` は `force`（外部セッション確認のスキップ）とは独立した引数
/// （2026-08-08 issue #3、major-2: レビュー指摘によりフラグを分離。セッション確認への同意が
/// 持ち主未確認への同意を兼ねてはいけない）。呼び出し元（switch_account/
/// start_add_account_login）の同名引数をそのまま渡す。true のとき、持ち主確認（profile API）が
/// TokenExpired/NetworkError で失敗しても `sync_back_live_login` 全体は中断せず、
/// `SyncBack::SkippedUnverified` を返して切替処理自体は続行させる（keychain_write も
/// meta の更新も一切行わない。レビュー案A。SyncBack::SkippedUnverified の doc 参照）。
/// missing-token・Other（真に予期しない失敗）は trust_unverified でも中断したままにする
/// （resolve_live_owner が返す OwnerError の種類で判定）。
/// NeedsImport（未登録ライブの取り込み確認）は trust_unverified の影響を受けない
/// （resolve_live_owner が Err を返す限り apply_live_owner 自体を呼ばないため、
/// find_match_idx によるガードを迂回しようがない）
fn sync_back_live_login(meta: &mut Meta, trust_unverified: bool) -> Result<SyncBack, String> {
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
            let expires_at = creds
                .pointer("/claudeAiOauth/expiresAt")
                .and_then(|v| v.as_i64());

            let owner = resolve_live_owner(
                meta.last_live_hash.as_deref(),
                &current_hash,
                &oauth_account,
                access_token,
                expires_at,
                |token| {
                    crate::actions::oauth_get_checked(
                        token,
                        "https://api.anthropic.com/api/oauth/profile",
                    )
                },
            );
            let owner = match owner {
                Ok(owner) => owner,
                Err(e) if should_skip_unverified_sync_back(&e, trust_unverified) => {
                    return Ok(SyncBack::SkippedUnverified);
                }
                Err(e) => return Err(e.into()),
            };

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
    let expires_at = creds.pointer("/claudeAiOauth/expiresAt").and_then(|v| v.as_i64());

    // フェーズ2（ロック外。最大で数秒〜10秒かかりうる profile API 呼び出しはここだけ）。
    // resolve_live_owner に trust-fallback は無い（issue #3 レビューで撤去）。無人で動く
    // 自動同期はユーザーの明示同意を得ようがないため、確認できなければそのまま Err で中断する
    let owner = resolve_live_owner(
        last_live_hash_snapshot.as_deref(),
        &current_hash,
        &oauth_account,
        access_token,
        expires_at,
        |token| crate::actions::oauth_get_checked(token, "https://api.anthropic.com/api/oauth/profile"),
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
        // apply_live_owner 自体は SkippedUnverified を作らない（sync_back_live_login の
        // trust_unverified 分岐が apply_live_owner を呼ぶ前に早期returnする形でしか
        // 発生しない。auto_sync_live はここで直接 apply_live_owner を呼んでおり、
        // その分岐を経由しないため現状は到達不能）。将来 apply_live_owner の呼び出し経路が
        // 変わってここに来ても、無人ループを panic で落とすのは避け、何もせず次サイクルに
        // 委ねる安全側に倒す（2026-08-08 レビュー: unreachable! は back ground loop で使わない）
        SyncBack::SkippedUnverified => Ok(AutoSyncResult::Unchanged),
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
    trust_unverified: bool,
    target_name: Option<&str>,
) -> Result<StartLoginOutcome, String> {
    // switch_account と同じ理由でガードを関数全体に保持する
    let _op_guard = AccountOpGuard::acquire()?;
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
        let sync_warning = match sync_back_live_login(&mut meta, trust_unverified)? {
            SyncBack::Unregistered(live_email) => return Ok(StartLoginOutcome::NeedsImport { live_email }),
            SyncBack::Synced { warning } => {
                save_meta(&meta)?;
                warning
            }
            SyncBack::NoLiveLogin => None,
            // SkippedUnverified は meta を一切変更していないので save_meta 不要
            SyncBack::SkippedUnverified => Some(UNVERIFIED_OWNER_SKIPPED_WARNING.to_string()),
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
/// 結果を踏み潰しうる。force=false の間は `SessionsRunning` を返して確認を挟む。
///
/// `trust_unverified`（2026-08-08 issue #3）は `force` とは独立した引数（major-2: レビュー指摘
/// によりフラグを分離。セッション確認への同意が持ち主未確認への同意を兼ねてはいけない）。
/// true のとき、持ち主確認が TokenExpired/NetworkError で失敗しても中断せず、sync-back を
/// スキップして切替自体は続行する（sync_back_live_login::SyncBack::SkippedUnverified 参照。
/// 書き込みは一切行わないため、誤帰属で登録済みアカウントのスナップショットを破壊する
/// リスクは無い）
pub fn switch_account(name: &str, force: bool, trust_unverified: bool) -> Result<SwitchOutcome, String> {
    // 関数全体（return地点すべて）で保持し、doc_analysis/diagnostics の spawn 直前チェックと
    // 突き合わせる。ensure_app_not_busy 単体のチェックだけでは通過直後に分析が
    // spawn される TOCTOU が残るため
    let _op_guard = AccountOpGuard::acquire()?;
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

    let sync_warning = match sync_back_live_login(&mut meta, trust_unverified)? {
        SyncBack::Unregistered(live_email) => return Ok(SwitchOutcome::NeedsImport { live_email }),
        SyncBack::Synced { warning } => warning,
        SyncBack::NoLiveLogin => None,
        // SkippedUnverified は meta を一切変更していない。ここでの save_meta（後続の
        // Keychain スワップ確定用）には影響しないので、そのまま先へ進んでよい
        SyncBack::SkippedUnverified => Some(UNVERIFIED_OWNER_SKIPPED_WARNING.to_string()),
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
    // （sync-back 済みなら、失われても sync-back 先の登録アカウントには最新分が残っている。
    // ただし SyncBack::SkippedUnverified のとき（trust_unverified=true で持ち主未確認のまま
    // sync-back をスキップしたケース）はこの前提が成立しない: どの登録アカウントにも
    // 書き戻していないため、この prior_live_cred がロールバック成功時の唯一の退避先になる。
    // 2026-08-08 issue #3 レビュー: この関数自体はスキップの有無を知らないが、
    // ここでの退避処理自体は両ケースで共通のロールバック機構としてそのまま機能する）
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

// ---------------------------------------------------------------------------
// スナップショットの refresh token 期限対策（2026-08-25 実装）
//
// Keychain 実物確認（2026-08-25）で `claudeAiOauth.refreshTokenExpiresAt` の存在を確認した。
// refresh token は発行から約30日で失効するため、非ライブのスナップショットを30日以上
// 放置すると切り替え後に再ログインが必要になる。期限が近づいたら OS ダイアログで確認し、
// 同意されたときだけアプリ自身が OAuth refresh を実行してスナップショットを更新する。
//
// **規約リスクをユーザーが受容済み（2026-08-25 決定）**: このエンドポイント・client_id の
// アプリからの直接利用は Anthropic の Consumer Terms 上「Claude Code 以外のツールからの
// OAuth 利用」にあたり、資格情報を失効させられるリスクがある（2026-01 のサーバー側
// ブロック強化・2026-02 の規約明文化）。調査記録は tmp/2026-08-25-token-refresh-research.md。
//
// ライブアカウントは絶対に対象外: Claude Code 本体の自動 refresh と one-time use の
// refresh token を取り合い、どちらかの世代が消費済みになって資格情報が壊れるため。
// ---------------------------------------------------------------------------

/// Claude Code の OAuth トークンエンドポイント（非公式利用。OSS 実装3件で確認済み）
const OAUTH_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
/// Claude Code の公開 OAuth client_id
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// 期限のこれだけ手前から確認ダイアログを出す（30日寿命のうち10日経過相当 = 残り20日）
const REFRESH_PROMPT_LEAD_MS: i64 = 20 * 24 * 60 * 60 * 1000;
/// 同一アカウントへの確認ダイアログの最小間隔（24時間）
const REFRESH_PROMPT_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;
/// refresh token の想定寿命のフォールバック。レスポンスに期限フィールドが無い場合に使う。
/// 実測（2026-08-25 のライブ資格情報）では発行から約30日だったが、ここで実際より長く
/// 見積もると期限判定が二度と発火しないまま失効しうる（レビュー R5）ため、保守側の
/// 25日に倒す。短く見積もる分には再確認が5日早まるだけで資格情報は失わない
const REFRESH_TOKEN_LIFETIME_MS: i64 = 25 * 24 * 60 * 60 * 1000;

/// 確認ダイアログを出すべきか（純粋関数）。
/// - 期限まで REFRESH_PROMPT_LEAD_MS を切っている
/// - まだ期限切れではない（切れていたら refresh 自体が失敗するので聞かない。
///   その場合の救済は既存の「再ログイン」導線）
/// - 前回の確認から REFRESH_PROMPT_INTERVAL_MS 以上経っている
fn refresh_prompt_due(refresh_expires_at_ms: i64, last_prompted_ms: Option<i64>, now_ms: i64) -> bool {
    let approaching =
        refresh_expires_at_ms > now_ms && refresh_expires_at_ms - now_ms <= REFRESH_PROMPT_LEAD_MS;
    let throttled = last_prompted_ms.is_some_and(|t| now_ms - t < REFRESH_PROMPT_INTERVAL_MS);
    approaching && !throttled
}

/// refresh 成功レスポンスをスナップショット JSON へ反映する（純粋関数）。
/// claudeAiOauth 配下の4フィールドだけ更新し、scopes 等の他フィールドは保持する。
/// レスポンスに refresh token の期限に相当するフィールドは確認できていないため、
/// refreshTokenExpiresAt は now + 30日 のフォールバック値を書く（発行時点からの
/// 固定寿命という実測に基づく近似。実際より短く見積もる分には再確認が早まるだけで害はない）
fn apply_refreshed_tokens(
    mut snapshot: serde_json::Value,
    access_token: &str,
    refresh_token: &str,
    expires_in_secs: i64,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    let oauth = snapshot
        .pointer_mut("/claudeAiOauth")
        .and_then(|v| v.as_object_mut())
        .ok_or("スナップショットの形式が想定外です")?;
    oauth.insert("accessToken".into(), serde_json::json!(access_token));
    oauth.insert("refreshToken".into(), serde_json::json!(refresh_token));
    oauth.insert("expiresAt".into(), serde_json::json!(now_ms + expires_in_secs * 1000));
    oauth.insert(
        "refreshTokenExpiresAt".into(),
        serde_json::json!(now_ms + REFRESH_TOKEN_LIFETIME_MS),
    );
    Ok(snapshot)
}

/// 期限接近の確認対象1件分（tray.rs のダイアログ表示用）
pub struct SnapshotRefreshCandidate {
    /// refresh_snapshot_credentials / mark_refresh_prompted に渡す内部識別子
    pub name: String,
    /// ダイアログ文言に使うメールアドレス（空なら表示名で代替済みの文字列）
    pub email: String,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// refresh 対象にしてよい「確実に非ライブ」の判定（純粋関数。レビュー R1/M8）。
/// `!is_live_account(..)` は org_id が空・ライブ org 不明のとき true（＝非ライブ扱い）に
/// 倒れてしまうが、ここは逆で「ライブでないと確定できなければ対象にしない」が正しい:
/// org_id が空の旧エントリが実はログイン中だったり、~/.claude.json の一時的な読み取り失敗で
/// 全アカウントが非ライブ扱いになると、Claude Code 本体と refresh を取り合って資格情報が壊れる
fn confirmed_non_live(org_id: &str, live_org: Option<&str>) -> bool {
    !org_id.is_empty() && live_org.is_some_and(|l| l != org_id)
}

/// accounts.json への書き込みが失敗した場合でも24時間スロットルを守るための
/// プロセス内の第二スロットル（レビュー R6。name → 最終確認時刻 epoch ms）
static PROMPTED_IN_MEMORY: Mutex<Option<std::collections::HashMap<String, i64>>> = Mutex::new(None);

fn prompted_in_memory_at(name: &str) -> Option<i64> {
    let guard = PROMPTED_IN_MEMORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref()?.get(name).copied()
}

/// refresh token の期限が近づいている非ライブアカウントを1件返す（60秒ループから毎回呼ぶ。
/// ダイアログは1サイクル1件しか出さないため、複数該当しても先頭で打ち切る）。
/// 判定に必要な refreshTokenExpiresAt はスナップショット（Keychain）にしか無いため、
/// 該当が見つかるまでのぶんだけ keychain_read を伴う。
/// 不整合状態・アカウント操作中・ライブ資格情報が読めない間は None（判断の保留。害がない）
pub fn snapshot_refresh_candidate() -> Option<SnapshotRefreshCandidate> {
    if ACCOUNT_OP_IN_PROGRESS.load(Ordering::SeqCst) {
        return None;
    }
    let _guard = lock_meta();
    let meta = load_meta();
    if meta.inconsistent {
        return None;
    }
    let live = live_org_id()?;
    // ライブの refresh token とも突き合わせる（org_id の照合だけでは旧エントリの
    // 空 org_id をすり抜ける。レビュー R1）。読めなければ判定不能として保留
    let live_rt = live_credentials_value()
        .ok()?
        .pointer("/claudeAiOauth/refreshToken")?
        .as_str()?
        .to_string();
    let now = now_ms();
    meta.accounts
        .iter()
        .filter(|a| a.has_credentials && confirmed_non_live(&a.org_id, Some(&live)))
        .find_map(|a| {
            let raw = keychain_read(&cred_svc(&a.name))?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            // スナップショットがライブと同じ refresh token を持つ＝実体はライブと同一。
            // ここで refresh すると Claude Code 本体と one-time use トークンを取り合う
            if v.pointer("/claudeAiOauth/refreshToken").and_then(|t| t.as_str()) == Some(live_rt.as_str()) {
                return None;
            }
            // 旧スナップショット（refreshTokenExpiresAt を持たない時期の取り込み）は
            // 期限を判定しようがないためスキップする。次の sync-back / 切り替えで
            // 最新形式に置き換われば対象に入る
            let expires = v.pointer("/claudeAiOauth/refreshTokenExpiresAt")?.as_i64()?;
            let last_prompted = match (a.refresh_prompted_at, prompted_in_memory_at(&a.name)) {
                (Some(p), Some(m)) => Some(p.max(m)),
                (p, m) => p.or(m),
            };
            if !refresh_prompt_due(expires, last_prompted, now) {
                return None;
            }
            let email = if a.email.is_empty() {
                resolve_display_name(&a.name, a.display_name.as_deref())
            } else {
                a.email.clone()
            };
            Some(SnapshotRefreshCandidate { name: a.name.clone(), email })
        })
}

/// 確認ダイアログを出した時刻を記録する（「はい」「いいえ」どちらでも呼ぶ。
/// 毎サイクル再表示しないための 24 時間スロットル）。accounts.json への永続化が
/// 失敗しても、プロセス内スロットル（PROMPTED_IN_MEMORY）が毎分の再表示を防ぐ
pub fn mark_refresh_prompted(name: &str) {
    {
        let mut guard = PROMPTED_IN_MEMORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(name.to_string(), now_ms());
    }
    let _guard = lock_meta();
    let mut meta = load_meta();
    if let Some(a) = meta.accounts.iter_mut().find(|a| a.name == name) {
        a.refresh_prompted_at = Some(now_ms());
        let _ = save_meta(&meta);
    }
}

/// スナップショットの OAuth refresh 本体（ユーザーが確認ダイアログで「はい」を選んだときだけ
/// 呼ばれる）。AccountOpGuard で切り替え・追加・診断系と相互排他にする。
///
/// refresh token は one-time use のため、POST が成功した瞬間に旧トークンは無効になり、
/// レスポンスの新トークンが唯一の有効な資格情報になる。したがって書き込み失敗は
/// 黙って握りつぶせない: 最大3回リトライし、全滅したら Err で呼び出し側（ダイアログ）に
/// 「再ログインが必要」と伝える
pub fn refresh_snapshot_credentials(name: &str) -> Result<(), String> {
    let _op = AccountOpGuard::acquire()?;

    // フェーズ1（ロック区間）: 対象がいまも「確実に非ライブ」であることを確認してから
    // refresh token を読む。ダイアログ表示中に切り替えが起きてライブ化していた場合、
    // ここで中断しないと Claude Code 本体の refresh と競合する。
    // 判定は org_id の照合（確定できなければ中断）に加えて、ライブの refresh token との
    // 一致も見る（org_id が空の旧エントリのすり抜け防止。レビュー R1）
    let refresh_token = {
        let _guard = lock_meta();
        let meta = load_meta();
        if meta.inconsistent {
            return Err("直前の切り替えが中途半端な状態のままです。先に解消してください".into());
        }
        let account = meta
            .accounts
            .iter()
            .find(|a| a.name == name)
            .ok_or("対象のアカウントが見つかりませんでした")?;
        let live = live_org_id()
            .ok_or("現在ログイン中のアカウントを確認できないため、安全のため更新を見送りました")?;
        if !confirmed_non_live(&account.org_id, Some(&live)) {
            return Err(
                "このアカウントがログイン中でないことを確認できませんでした。ログイン中のアカウントは Claude Code 本体が自動で更新します".into(),
            );
        }
        let live_rt = live_credentials_value()
            .ok()
            .and_then(|v| v.pointer("/claudeAiOauth/refreshToken").and_then(|t| t.as_str()).map(String::from))
            .ok_or("現在ログイン中の資格情報を確認できないため、安全のため更新を見送りました")?;
        let raw = keychain_read(&cred_svc(name))
            .ok_or("スナップショットを読み取れませんでした")?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| "スナップショットの形式が想定外です")?;
        let rt = v
            .pointer("/claudeAiOauth/refreshToken")
            .and_then(|t| t.as_str())
            .map(String::from)
            .ok_or("スナップショットに refresh token がありません")?;
        if rt == live_rt {
            return Err(
                "このスナップショットは現在ログイン中の資格情報と同一のため、Claude Code 本体の更新に任せます".into(),
            );
        }
        rt
    };

    // フェーズ2（ロック外・HTTP）。AccountOpGuard は保持したままなので、この間に
    // 切り替え等のアカウント操作が始まることはない
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": OAUTH_CLIENT_ID,
        "refresh_token": refresh_token,
    });
    let (status, resp_body) = crate::actions::post_json_checked(OAUTH_TOKEN_URL, body)?;
    if !(200..300).contains(&status) {
        // 本文にはトークンが含まれうるためエラーメッセージへ載せない
        return Err(match status {
            400 | 401 | 403 => format!(
                "refresh token が受け付けられませんでした（HTTP {status}）。このアカウントは再ログインが必要です"
            ),
            429 => "レート制限中です。時間をおいて再試行してください".to_string(),
            _ => format!("トークン更新に失敗しました（HTTP {status}）"),
        });
    }
    let resp: serde_json::Value = serde_json::from_str(&resp_body)
        .map_err(|_| "トークン更新の応答を解析できませんでした".to_string())?;
    let access = resp
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("応答に access_token がありません")?;
    let new_refresh = resp
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or("応答に refresh_token がありません")?;
    let expires_in = resp.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(8 * 60 * 60);
    // refresh token 側の期限がレスポンスにあれば優先する（フィールド名は未確認のため
    // 両候補を見る。無ければ apply_refreshed_tokens が保守側フォールバック25日を書く。
    // レビュー R5: 実際より長く見積もると期限判定が発火しないまま失効する）
    let refresh_expires_override = resp
        .get("refresh_token_expires_in")
        .or_else(|| resp.get("refresh_expires_in"))
        .and_then(|v| v.as_i64());

    // フェーズ3（ロック区間・書き込み）: この時点で旧 refresh token は消費済みで、
    // レスポンスの新トークンが唯一の有効な資格情報。まず TOCTOU 再検証を行う
    // （レビュー R2。auto_sync_live / poll_monitor_setup と同じ規律）:
    // HTTP 中に切り替え等でスナップショットが変わっていた（refresh token がフェーズ1と
    // 不一致）場合、その上に書くと切り替え後の新しい資格情報を巻き戻してしまうため中断する
    let _guard = lock_meta();
    let mut last_err = String::new();
    for attempt in 0..3 {
        // 読み取り・解析・整形も含めてリトライ対象にする（レビュー R4: security コマンドの
        // 一時失敗ひとつで取得済みの有効トークンを捨ててはいけない）
        let updated_str = match keychain_read(&cred_svc(name))
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(snapshot) => {
                let current_rt = snapshot
                    .pointer("/claudeAiOauth/refreshToken")
                    .and_then(|t| t.as_str());
                if attempt == 0 && current_rt != Some(refresh_token.as_str()) {
                    return Err(
                        "更新中にスナップショットが別の内容に置き換わったため、上書きを中止しました。アカウント画面から状態を確認してください".into(),
                    );
                }
                let now = now_ms();
                let mut v = apply_refreshed_tokens(snapshot, access, new_refresh, expires_in, now)?;
                if let (Some(secs), Some(o)) = (
                    refresh_expires_override,
                    v.pointer_mut("/claudeAiOauth").and_then(|p| p.as_object_mut()),
                ) {
                    o.insert("refreshTokenExpiresAt".into(), serde_json::json!(now + secs * 1000));
                }
                v.to_string()
            }
            // 既存スナップショットが読めない場合でも新トークンは失えないため、
            // 最小構成で新規に組み立てて書く（scopes 等は失うが資格情報を失うよりよい）
            None => {
                let now = now_ms();
                serde_json::json!({
                    "claudeAiOauth": {
                        "accessToken": access,
                        "refreshToken": new_refresh,
                        "expiresAt": now + expires_in * 1000,
                        "refreshTokenExpiresAt": now
                            + refresh_expires_override.map_or(REFRESH_TOKEN_LIFETIME_MS, |s| s * 1000),
                    }
                })
                .to_string()
            }
        };
        match keychain_write(&cred_svc(name), &updated_str) {
            Ok(()) => {
                let mut meta = load_meta();
                if let Some(a) = meta.accounts.iter_mut().find(|a| a.name == name) {
                    a.refresh_prompted_at = Some(now_ms());
                }
                let _ = save_meta(&meta);
                return Ok(());
            }
            Err(e) => last_err = e,
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    Err(format!(
        "新しいトークンの保存に失敗しました（{last_err}）。旧トークンは既に無効化されているため、このアカウントは再ログインが必要です"
    ))
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
            refresh_prompted_at: None,
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
        let owner = resolve_live_owner(Some("hash-a"), "hash-a", &account, Some("token"), None, |_| {
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
        let owner = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), None, |_| {
            crate::actions::FetchOutcome::Ok(
                serde_json::json!({ "account": { "email": "user@example.com" } }).to_string(),
            )
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
        let owner = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), None, |_| {
            crate::actions::FetchOutcome::Ok(
                serde_json::json!({ "account": { "email": "real@example.com" } }).to_string(),
            )
        })
        .expect("profile が確認できれば成功扱い（内容は mismatched で示す）");
        assert!(owner.mismatched);
        assert_eq!(owner.org_id, None, "ズレたら org_id は信用しない");
        assert_eq!(owner.email.as_deref(), Some("real@example.com"));
    }

    #[test]
    fn resolve_live_owner_aborts_when_profile_unconfirmed() {
        // hash 不一致で profile 確認も失敗（応答の構文エラー等）＝推測せず中断する。
        // メッセージ分類は Other になる
        let account = oauth("org-1", "user@example.com");
        let result = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), None, |_| {
            crate::actions::FetchOutcome::Other("応答が不正".to_string())
        });
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), OwnerError::Other(_)));
    }

    #[test]
    fn resolve_live_owner_classifies_expired_from_precheck() {
        // 事前チェック（expires_at）で期限切れと分かれば、profile API を呼ばずに
        // TokenExpired を返す（issue #1: 従来は expiresAt を見ず直接 fetch していた）
        let account = oauth("org-1", "user@example.com");
        let result = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), Some(1), |_| {
            panic!("期限切れが事前チェックで分かっているので profile を呼んではいけない")
        });
        assert!(matches!(result, Err(OwnerError::TokenExpired(Some(ref e))) if e == "user@example.com"));
    }

    #[test]
    fn resolve_live_owner_classifies_expired_from_api_response() {
        // 事前チェックをすり抜けても（expires_at 不明・またはギリギリ期限内）、
        // profile API 応答が Expired（401・error フィールド）なら同じ分類にする
        let account = oauth("org-1", "user@example.com");
        let result = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), None, |_| {
            crate::actions::FetchOutcome::Expired
        });
        assert!(matches!(result, Err(OwnerError::TokenExpired(_))));
    }

    #[test]
    fn resolve_live_owner_classifies_network_error() {
        // 接続失敗・タイムアウト等は NetworkError に分類する
        let account = oauth("org-1", "user@example.com");
        let result = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), None, |_| {
            crate::actions::FetchOutcome::Network
        });
        assert!(matches!(result, Err(OwnerError::NetworkError)));
    }

    #[test]
    fn resolve_live_owner_classifies_rate_limited_as_network_error() {
        // R-2: 429 は Other ではなく NetworkError に寄せる。should_skip_unverified_sync_back /
        // TS 側 canProceedUnverified が TokenExpired/NetworkError でしか続行を許さないため、
        // Other のままだと確認ダイアログすら出ずに切り替えが失敗する退行になる
        let account = oauth("org-1", "user@example.com");
        let result = resolve_live_owner(Some("hash-old"), "hash-new", &account, Some("token"), None, |_| {
            crate::actions::FetchOutcome::RateLimited
        });
        assert!(matches!(result, Err(OwnerError::NetworkError)));
    }

    #[test]
    fn should_skip_unverified_sync_back_allows_token_expired_and_network_when_trusted() {
        // issue #3 レビュー案A: trust_unverified=true のときだけ、TokenExpired/NetworkError で
        // スキップ（＝書き込まず切替続行）を許可する
        assert!(should_skip_unverified_sync_back(
            &OwnerError::TokenExpired(Some("user@example.com".to_string())),
            true
        ));
        assert!(should_skip_unverified_sync_back(&OwnerError::NetworkError, true));
    }

    #[test]
    fn should_skip_unverified_sync_back_rejects_without_trust_flag() {
        // trust_unverified=false（通常の切替・auto_sync_live）なら、理由を問わず中断のまま
        assert!(!should_skip_unverified_sync_back(&OwnerError::TokenExpired(None), false));
        assert!(!should_skip_unverified_sync_back(&OwnerError::NetworkError, false));
    }

    #[test]
    fn should_skip_unverified_sync_back_rejects_other_even_when_trusted() {
        // Other（応答の構文エラー等、真に予期しない失敗）は trust_unverified でもスキップ対象外
        // （「今は確認できないだけ」ではなく不整合の疑いが残るため）
        assert!(!should_skip_unverified_sync_back(&OwnerError::Other("応答が不正".to_string()), true));
    }

    #[test]
    fn resolve_live_owner_aborts_when_no_access_token_and_hash_differs() {
        // access token 自体が読めず、かつ hash も一致しない＝確認しようがないため中断する
        let account = oauth("org-1", "user@example.com");
        let result = resolve_live_owner(Some("hash-old"), "hash-new", &account, None, None, |_| {
            panic!("token が無いので呼ばれないはず")
        });
        assert!(result.is_err());
    }

    #[test]
    fn resolve_live_owner_no_baseline_requires_confirmation() {
        // last_live_hash が None（初回等）でも「未確認」と同様に profile 確認を要求する
        let account = oauth("org-1", "user@example.com");
        let owner = resolve_live_owner(None, "hash-new", &account, Some("token"), None, |_| {
            crate::actions::FetchOutcome::Ok(
                serde_json::json!({ "account": { "email": "user@example.com" } }).to_string(),
            )
        })
        .expect("profile が一致すれば成功するはず");
        assert!(!owner.mismatched);
    }

    #[test]
    fn owner_error_wire_format_has_kind_prefix() {
        // TS 側（api.ts の describeAccountError）・Rust 側（strip_owner_error_tag）の
        // 両方がこのプレフィックスでパースするため、形式が壊れていないことをここで固定する
        assert_eq!(OwnerError::TokenExpired(None).kind(), "TOKEN_EXPIRED");
        assert_eq!(OwnerError::NetworkError.kind(), "NETWORK_ERROR");
        assert!(OwnerError::TokenExpired(None).to_string().starts_with("TOKEN_EXPIRED:"));
        assert!(OwnerError::NetworkError.to_string().starts_with("NETWORK_ERROR:"));
    }

    #[test]
    fn strip_owner_error_tag_removes_known_kind_prefix() {
        assert_eq!(strip_owner_error_tag("TOKEN_EXPIRED:token 期限切れです"), "token 期限切れです");
        assert_eq!(strip_owner_error_tag("NETWORK_ERROR:接続できません"), "接続できません");
        assert_eq!(strip_owner_error_tag("OTHER:その他の理由"), "その他の理由");
    }

    #[test]
    fn strip_owner_error_tag_leaves_unrelated_messages_untouched() {
        // resolve_live_owner 由来ではないエラー（他コマンドの失敗等）はプレフィックスが
        // 無い、または既知の kind と一致しないため素通しする
        assert_eq!(
            strip_owner_error_tag("アカウント「foo」は登録されていません"),
            "アカウント「foo」は登録されていません"
        );
        assert_eq!(strip_owner_error_tag("UNKNOWN_KIND:本文"), "UNKNOWN_KIND:本文");
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
        // ライブアカウント: 300秒閾値（2026-08-22、第4ラウンド S-1）
        assert!(cache_is_fresh_enough(1_000, 1_299, true));
        assert!(!cache_is_fresh_enough(1_000, 1_300, true));
    }

    #[test]
    fn force_skips_freshness_check_only_for_live() {
        // R-6: force の効果はライブに限定する。非ライブは force=true でもキャッシュ新鮮判定を
        // 素通りしない（force_skips_freshness_check が false を返す＝スキップしない）
        assert!(force_skips_freshness_check(true, true));
        assert!(!force_skips_freshness_check(true, false));
        assert!(!force_skips_freshness_check(false, true));
        assert!(!force_skips_freshness_check(false, false));
    }

    #[test]
    fn cache_is_fresh_enough_non_live_uses_600s_threshold() {
        // 非ライブアカウント: 600秒閾値（2026-08-22 B-2: リクエスト削減のためライブより緩める）
        assert!(cache_is_fresh_enough(1_000, 1_599, false));
        assert!(!cache_is_fresh_enough(1_000, 1_600, false));
        // 60秒経過しただけではライブと違って再照会しない
        assert!(cache_is_fresh_enough(1_000, 1_060, false));
    }

    // 段階的バックオフ（usage_backoff_wait 等）は2026-08-22 第4ラウンド（S-2）で撤去した。
    // 撤去理由は actions.rs のコメント参照

    #[test]
    fn live_error_for_fresh_cache_matches_r7() {
        // T-3 追加テスト項目3: 新鮮キャッシュを返すだけの場合の live_error 代入規則
        // （第4ラウンドでグローバルバックオフを撤去した後も、「このサイクルで既に429を
        // 観測したか」という同じ意味の bool に対して同じ規則が成り立つことを確認する）
        assert!(matches!(
            live_error_for_fresh_cache(true, true),
            Some(crate::actions::LiveUsageError::RateLimited)
        ));
        assert!(live_error_for_fresh_cache(true, false).is_none());
        assert!(live_error_for_fresh_cache(false, true).is_none());
        assert!(live_error_for_fresh_cache(false, false).is_none());
    }

    #[test]
    fn should_skip_usage_source_gates_after_rate_limited_in_cycle() {
        // 2026-08-22 第4ラウンド（S-2）: サイクル内で429を観測する前は打ってよい（false）、
        // 観測した後は同一サイクル内で以降のソースを打たない（true）。get_accounts_usage の
        // ループはこの関数をソースごとに都度呼んで判定するため、これが「429観測後は
        // 同一サイクル内で以降のソースも `/api/oauth/usage` を叩かない」の実体になる
        assert!(!should_skip_usage_source(false));
        assert!(should_skip_usage_source(true));
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

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn refresh_prompt_due_within_lead_window() {
        // 期限まで残り19日（20日を切っている）・未確認 → 出す
        assert!(refresh_prompt_due(19 * DAY_MS, None, 0));
    }

    #[test]
    fn refresh_prompt_not_due_when_far_from_expiry() {
        // 期限まで残り25日 → まだ出さない
        assert!(!refresh_prompt_due(25 * DAY_MS, None, 0));
    }

    #[test]
    fn refresh_prompt_not_due_when_already_expired() {
        // 期限切れは refresh 自体が失敗するので聞かない（再ログイン導線に委ねる）
        assert!(!refresh_prompt_due(1_000, None, 2_000));
    }

    #[test]
    fn refresh_prompt_throttled_within_24h() {
        let now = 100 * DAY_MS;
        let expires = now + 5 * DAY_MS;
        // 12時間前に確認済み → 出さない。25時間前なら出す
        assert!(!refresh_prompt_due(expires, Some(now - DAY_MS / 2), now));
        assert!(refresh_prompt_due(expires, Some(now - DAY_MS - 3_600_000), now));
    }

    #[test]
    fn apply_refreshed_tokens_updates_only_token_fields() {
        let snapshot = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "old-at",
                "refreshToken": "old-rt",
                "expiresAt": 1,
                "refreshTokenExpiresAt": 2,
                "scopes": ["user:inference"],
                "subscriptionType": "max"
            }
        });
        let now = 1_000_000;
        let out = apply_refreshed_tokens(snapshot, "new-at", "new-rt", 28_800, now).unwrap();
        let o = out.pointer("/claudeAiOauth").unwrap();
        assert_eq!(o.get("accessToken").unwrap(), "new-at");
        assert_eq!(o.get("refreshToken").unwrap(), "new-rt");
        assert_eq!(o.get("expiresAt").unwrap().as_i64(), Some(now + 28_800 * 1000));
        assert_eq!(
            o.get("refreshTokenExpiresAt").unwrap().as_i64(),
            Some(now + REFRESH_TOKEN_LIFETIME_MS)
        );
        // トークン以外のフィールドは保持される
        assert_eq!(o.get("subscriptionType").unwrap(), "max");
        assert!(o.get("scopes").unwrap().is_array());
    }

    #[test]
    fn apply_refreshed_tokens_rejects_malformed_snapshot() {
        assert!(apply_refreshed_tokens(serde_json::json!({}), "a", "r", 1, 0).is_err());
    }

    /// レビュー R1: 「非ライブと確定できない」ケースはすべて対象外に倒す
    #[test]
    fn confirmed_non_live_requires_certainty() {
        // 通常の非ライブ → 対象
        assert!(confirmed_non_live("org-a", Some("org-b")));
        // ライブと一致 → 対象外
        assert!(!confirmed_non_live("org-a", Some("org-a")));
        // org_id が空の旧エントリ → 確定できないので対象外
        assert!(!confirmed_non_live("", Some("org-b")));
        // ライブ org 不明（~/.claude.json 読み取り失敗等）→ 確定できないので対象外
        assert!(!confirmed_non_live("org-a", None));
    }
}
