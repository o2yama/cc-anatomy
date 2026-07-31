//! 非 macOS 向けの doc_analysis スタブ。
//! AI分析は claude CLI のヘッドレス実行（resolve_claude_bin が macOS 限定）に依存するため、
//! ここは取りこぼし時の安全網としてエラーを返す。

#![allow(dead_code)]
#![allow(unused_variables)]

pub fn analyze_doc(
    app: &tauri::AppHandle,
    path: String,
    content: String,
    project_dir: Option<String>,
) -> Result<String, String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn cancel_doc_analysis() -> Result<(), String> {
    Err(crate::actions::MAC_ONLY.into())
}

pub fn is_running() -> bool {
    false
}
