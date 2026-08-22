//! 非 macOS 向けの accounts スタブ。
//! アカウント切り替えは Keychain・`~/.claude.json`・Terminal.app に依存する macOS 限定機能。
//! 監視のみの Windows/Linux 版では「アカウント未登録」相当として振る舞い（tray や
//! 使用量表示は既存のライブ資格情報フォールバックに自然に落ちる）、変更系は明示的に断る。
//! 型は accounts.rs と同一形を保ち、フロントとの API 契約を変えない。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    pub name: String,
    pub display_name: Option<String>,
    pub email: String,
    pub plan: String,
    pub is_live: bool,
    pub has_credentials: bool,
    pub has_monitor_token: bool,
    pub can_relogin: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountsState {
    pub accounts: Vec<Account>,
    pub live_email: Option<String>,
    pub live_registered: bool,
    pub running_sessions: usize,
    pub inconsistent: bool,
}

pub struct TrayAccount {
    pub name: String,
    pub display_name: String,
    pub is_live: bool,
    pub has_credentials: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountUsage {
    pub name: String,
    pub five_pct: Option<f64>,
    pub seven_pct: Option<f64>,
    pub five_reset: Option<i64>,
    pub seven_reset: Option<i64>,
    pub fetched_at: Option<i64>,
    pub stale: bool,
    pub five_probably_reset: bool,
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum PollResult {
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "done")]
    Done { account: Account },
    #[serde(rename = "mismatch")]
    Mismatch { expected_label: String, expected_email: String },
}

/// tray.rs の定期更新ループから呼ぶ自動取り込みの結果。macOS 限定機能のスタブなので
/// 実質何もしない（NoLiveLogin 固定）。accounts.rs と型を合わせるだけの存在
pub enum AutoSyncResult {
    Unchanged,
    Synced { warning: Option<String> },
    Unregistered,
    NoLiveLogin,
}

/// "accounts-updated" イベントのペイロード。accounts.rs と型を合わせるだけの存在
#[derive(Serialize, Clone)]
pub struct AccountsUpdatedEvent {
    pub warning: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum StartLoginOutcome {
    #[serde(rename = "started")]
    Started {
        baseline: String,
        warning: Option<String>,
    },
    #[serde(rename = "needs_import")]
    NeedsImport { live_email: Option<String> },
    #[serde(rename = "sessions_running")]
    SessionsRunning { count: usize },
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum SwitchOutcome {
    #[serde(rename = "switched")]
    Switched { warning: Option<String> },
    #[serde(rename = "needs_import")]
    NeedsImport { live_email: Option<String> },
    #[serde(rename = "sessions_running")]
    SessionsRunning { count: usize },
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum MonitorSetupPoll {
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "linked")]
    Linked,
    #[serde(rename = "mismatch")]
    Mismatch { expected_label: String, expected_email: String },
}

pub fn registered_accounts() -> Vec<TrayAccount> {
    Vec::new()
}

/// 非 macOS では監視トークンの紐づけ自体が成立しないため常に None（tray.rs の
/// ライブ使用量フォールバックが無条件で呼ぶため、型を合わせるだけの存在）
pub fn live_account_monitor_token() -> Option<String> {
    None
}

pub fn get_accounts() -> Result<AccountsState, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn remove_legacy_shell_integration() -> Result<bool, String> {
    Ok(false)
}

pub fn import_live_account() -> Result<Account, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn start_add_account_login(
    _app: &tauri::AppHandle,
    _force: bool,
    _trust_unverified: bool,
    _target_name: Option<&str>,
) -> Result<StartLoginOutcome, String> {
    Err(crate::actions::MAC_ONLY.into())
}

/// 非 macOS では自動取り込み自体が成立しないため、常に「ライブログインなし」扱いにする
/// （tray.rs の定期更新ループを毎分エラーで埋めないため。Err にすると eprintln が毎分出る）
pub fn auto_sync_live() -> Result<AutoSyncResult, String> {
    Ok(AutoSyncResult::NoLiveLogin)
}

pub fn start_monitor_setup(_app: &tauri::AppHandle, _name: &str) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn poll_monitor_setup(_name: &str) -> Result<MonitorSetupPoll, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn poll_add_account_login(_baseline: &str) -> Result<PollResult, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn switch_account(
    _name: &str,
    _force: bool,
    _trust_unverified: bool,
) -> Result<SwitchOutcome, String> {
    Err(crate::actions::MAC_ONLY.into())
}

/// 本実装（accounts.rs）の同名関数と対。非 macOS では OwnerError の wire format 自体が
/// 発生しないため、受け取った文字列をそのまま返す
pub fn strip_owner_error_tag(message: &str) -> &str {
    message
}

pub fn remove_account(_name: &str) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn rename_account(_name: &str, _display_name: &str) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn reorder_accounts(_names: &[String]) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

/// accounts.rs の同名構造体と型を合わせるだけの存在（#1〜#3 に続き、2026-08-22 の
/// B-1 でシグネチャ変更した get_accounts_usage(force: bool) に追従）
pub struct UsageBatch {
    pub accounts: Vec<AccountUsage>,
    pub live_error: Option<crate::actions::LiveUsageError>,
}

pub fn get_accounts_usage(_force: bool) -> Result<UsageBatch, String> {
    Err(crate::actions::MAC_ONLY.into())
}
