//! 非 macOS 向けの diagnostics スタブ。
//! 環境診断は claude CLI のヘッドレス実行と Terminal.app 連携に依存する macOS 限定機能。
//! フロントの UI 出し分けが第一防衛で、ここは取りこぼし時の安全網としてエラーを返す。

#![allow(dead_code)]

#[derive(serde::Serialize)]
pub struct Finding {
    pub title: String,
    pub detail: String,
}

#[derive(serde::Serialize)]
pub struct DiagnosisReport {
    pub summary: String,
    pub findings: Vec<Finding>,
}

pub fn run_diagnosis(_app: &tauri::AppHandle) -> Result<DiagnosisReport, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn cancel_diagnosis() -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn run_fixes_in_terminal(_app: &tauri::AppHandle, _prompts: Vec<String>) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}
