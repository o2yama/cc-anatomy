//! 非 macOS 向けの accounts スタブ。
//! アカウント切り替えは Keychain・`.zshrc` 連携・Terminal.app に依存する macOS 限定機能。
//! 監視のみの Windows/Linux 版では「アカウント未登録」相当として振る舞い（tray や
//! 使用量表示は既存のライブ資格情報フォールバックに自然に落ちる）、変更系は明示的に断る。
//! 型は accounts.rs と同一形を保ち、フロントとの API 契約を変えない。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    pub name: String,
    pub email: String,
    pub plan: String,
    pub active: bool,
    pub is_live: bool,
    pub missing_token: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountsState {
    pub accounts: Vec<Account>,
    pub shell_integration: bool,
    pub running_sessions: usize,
}

pub struct TrayAccount {
    pub name: String,
    pub is_live: bool,
    pub usage: Option<crate::actions::UsageSummary>,
}

#[derive(Serialize)]
pub struct AccountUsageDetail {
    pub name: String,
    pub email: String,
    pub plan: String,
    pub active: bool,
    pub is_live: bool,
    pub usage: Option<serde_json::Value>,
    pub error: Option<String>,
}

pub fn active_token() -> Option<String> {
    None
}

pub fn claude_env() -> Vec<(String, String)> {
    Vec::new()
}

pub fn active_display() -> Option<(String, String, String)> {
    None
}

pub fn accounts_with_usage() -> Vec<TrayAccount> {
    Vec::new()
}

pub fn accounts_usage_detail() -> Vec<AccountUsageDetail> {
    Vec::new()
}

pub fn get_accounts() -> Result<AccountsState, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn add_account_in_terminal(_app: &tauri::AppHandle, _name: &str) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn claim_pending_account(_name: &str) -> Result<Option<Account>, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn switch_account(_name: &str) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn remove_account(_name: &str) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn install_shell_integration() -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}
