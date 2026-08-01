//! 非 macOS 向けの diagnostics スタブ。
//! 環境診断は claude CLI のヘッドレス実行と Terminal.app 連携に依存する macOS 限定機能。
//! フロントの UI 出し分けが第一防衛で、ここは取りこぼし時の安全網としてエラーを返す。

#![allow(dead_code)]

// ⚠ diagnostics.rs の定義と常に同期させること。乖離するとフロントとの契約が黙って壊れる
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Finding {
    pub id: String,
    pub severity: String, // "high" | "medium" | "low"
    pub category: String,
    pub title: String,
    pub detail: String,
    pub fix_prompt: String,
    #[serde(default)]
    pub target_paths: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
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

pub fn is_running() -> bool {
    false
}

pub fn kill_running() {}

pub fn run_fixes_in_terminal(_app: &tauri::AppHandle, _prompts: Vec<String>) -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}
