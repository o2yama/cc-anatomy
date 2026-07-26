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
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountsState {
    pub accounts: Vec<Account>,
    pub live_email: Option<String>,
    pub live_registered: bool,
    pub running_sessions: usize,
}

pub struct TrayAccount {
    pub display_name: String,
    pub is_live: bool,
}

#[derive(Serialize)]
#[serde(tag = "status")]
pub enum PollResult {
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "done")]
    Done { account: Account },
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

pub fn registered_accounts() -> Vec<TrayAccount> {
    Vec::new()
}

pub fn get_accounts() -> Result<AccountsState, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn remove_legacy_shell_integration() -> Result<bool, String> {
    Ok(false)
}

pub fn remove_legacy_monitor_tokens() {}

pub fn import_live_account() -> Result<Account, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn start_add_account_login(_force: bool) -> Result<StartLoginOutcome, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn poll_add_account_login(_baseline: &str) -> Result<PollResult, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn switch_account(_name: &str, _force: bool) -> Result<SwitchOutcome, String> {
    Err(crate::actions::MAC_ONLY.into())
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
