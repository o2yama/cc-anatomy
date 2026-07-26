// アカウント切り替えと環境診断は Keychain・.zshrc・Terminal.app に依存する macOS 限定機能。
// 非 macOS では同一シグネチャのスタブに差し替え、コマンド登録とフロント API 契約は維持する
#[cfg(target_os = "macos")]
mod accounts;
#[cfg(not(target_os = "macos"))]
#[path = "accounts_stub.rs"]
mod accounts;
#[cfg(target_os = "macos")]
mod diagnostics;
#[cfg(not(target_os = "macos"))]
#[path = "diagnostics_stub.rs"]
mod diagnostics;

mod actions;
mod credentials;
mod db;
mod env;
mod inventory;
mod transcript;
mod tray;
mod updater;

#[tauri::command]
fn list_projects() -> Result<Vec<db::ProjectInfo>, String> {
    db::list_projects()
}

#[tauri::command]
fn get_home_dir() -> String {
    db::home_dir().display().to_string()
}

/// フロントが macOS 限定 UI（アカウント切替・環境診断・Finder 等）を出し分けるための OS 名
#[tauri::command]
fn get_platform() -> String {
    std::env::consts::OS.to_string()
}

#[tauri::command]
fn get_project_env(project: String, path: Option<String>) -> Result<env::ProjectEnv, String> {
    env::get_project_env(&project, path)
}

#[tauri::command]
fn read_doc(path: String) -> Result<env::FileDoc, String> {
    env::read_doc_checked(&path)
}

#[tauri::command]
fn list_sessions(project: String) -> Result<Vec<db::SessionInfo>, String> {
    db::list_sessions(&project)
}

#[tauri::command]
fn search_summaries(query: String, project: Option<String>) -> Result<Vec<db::SearchHit>, String> {
    db::search_summaries(&query, project.as_deref())
}

#[tauri::command]
fn get_transcript(session_id: String) -> Result<transcript::Transcript, String> {
    transcript::get_transcript(&session_id)
}

#[tauri::command]
fn list_skills() -> Result<Vec<inventory::InventoryItem>, String> {
    inventory::list_skills()
}

#[tauri::command]
fn list_agents() -> Result<Vec<inventory::InventoryItem>, String> {
    inventory::list_agents()
}

#[tauri::command]
fn open_in_finder(path: String) -> Result<(), String> {
    actions::open_in_finder(&path)
}

#[tauri::command]
fn open_in_cmux(path: String) -> Result<(), String> {
    actions::open_in_cmux(&path)
}

#[tauri::command]
fn open_in_terminal(path: String) -> Result<(), String> {
    actions::open_in_terminal(&path)
}

/// claude CLI を同期実行するので async にして UI スレッドを塞がない
#[tauri::command(async)]
fn extract_tasks(project: String) -> Result<String, String> {
    actions::extract_tasks(&project)
}

/// ネットワーク呼び出しなので async
#[tauri::command(async)]
fn get_rate_limits() -> Result<String, String> {
    actions::get_rate_limits()
}

#[tauri::command(async)]
fn get_account_profile() -> Result<String, String> {
    actions::get_account_profile()
}

/// 環境スキャンは数分かかるので async で UI スレッドを塞がない
#[tauri::command(async)]
fn run_diagnosis(app: tauri::AppHandle) -> Result<diagnostics::DiagnosisReport, String> {
    diagnostics::run_diagnosis(&app)
}

#[tauri::command]
fn cancel_diagnosis() -> Result<(), String> {
    diagnostics::cancel_diagnosis()
}

#[tauri::command(async)]
fn run_fixes_in_terminal(app: tauri::AppHandle, prompts: Vec<String>) -> Result<(), String> {
    diagnostics::run_fixes_in_terminal(&app, prompts)
}

/// ps とファイル読みを伴うので async
#[tauri::command(async)]
fn get_accounts() -> Result<accounts::AccountsState, String> {
    accounts::get_accounts()
}

/// 現在ログイン中のアカウントを登録に取り込む（Flow A）。Keychain・~/.claude.json を読むので async
#[tauri::command(async)]
fn import_live_account() -> Result<accounts::Account, String> {
    accounts::import_live_account()
}

/// `claude auth login` を Terminal で起動し、完了検知の基準値を返す（Flow B）。
/// 事前 sync-back を含むので async。`force` は外部セッション確認済みの再実行フラグ
#[tauri::command(async)]
fn start_add_account_login(force: bool) -> Result<accounts::StartLoginOutcome, String> {
    accounts::start_add_account_login(force)
}

/// フロントが2秒間隔で呼ぶ完了検知ポーリング（Flow B）
#[tauri::command(async)]
fn poll_add_account_login(baseline: String) -> Result<accounts::PollResult, String> {
    accounts::poll_add_account_login(&baseline)
}

/// Keychain スワップによる切り替え（Flow C）。sync-back・読み戻し検証を伴うので async。
/// 切り替え成功時はトレイの使用率表示にも即反映する。`force` は外部セッション確認済みの再実行フラグ
#[tauri::command(async)]
fn switch_account(
    app: tauri::AppHandle,
    name: String,
    force: bool,
) -> Result<accounts::SwitchOutcome, String> {
    let outcome = accounts::switch_account(&name, force)?;
    if matches!(outcome, accounts::SwitchOutcome::Switched { .. }) {
        tray::refresh(app);
    }
    Ok(outcome)
}

#[tauri::command(async)]
fn remove_account(name: String) -> Result<(), String> {
    accounts::remove_account(&name)
}

/// 表示名の変更。内部識別子（name）は変えない
#[tauri::command(async)]
fn rename_account(name: String, display_name: String) -> Result<(), String> {
    accounts::rename_account(&name, &display_name)
}

/// アカウント一覧の表示順（ドラッグ&ドロップでの並び替え）を保存する
#[tauri::command(async)]
fn reorder_accounts(names: Vec<String>) -> Result<(), String> {
    accounts::reorder_accounts(&names)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 旧方式（.zshrc への CLAUDE_CODE_OAUTH_TOKEN 注入）の撤去。冪等なので毎起動で呼んでよい。
            // 「除去できていない」という報告が過去にあったため、エラーだけでなく
            // 実行結果（削除した/対象が無かった）も毎回ログに残す
            match accounts::remove_legacy_shell_integration() {
                Ok(true) => println!("legacy shell integration: removed a stale block from ~/.zshrc"),
                Ok(false) => println!("legacy shell integration: no stale block found (already clean)"),
                Err(e) => eprintln!("legacy shell integration cleanup failed: {e}"),
            }
            // 旧「監視用長期トークン」方式の撤去（2026-07-25 ユーザー決定）。冪等
            accounts::remove_legacy_monitor_tokens();
            tray::setup(app.handle())?;
            updater::setup_periodic_check(app.handle());
            Ok(())
        })
        // ウィンドウを閉じてもメニューバー常駐を続ける（終了はトレイの「終了」から）
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            get_home_dir,
            get_platform,
            get_project_env,
            read_doc,
            list_sessions,
            search_summaries,
            get_transcript,
            list_skills,
            list_agents,
            open_in_finder,
            open_in_cmux,
            open_in_terminal,
            extract_tasks,
            get_rate_limits,
            get_account_profile,
            run_diagnosis,
            cancel_diagnosis,
            run_fixes_in_terminal,
            get_accounts,
            import_live_account,
            start_add_account_login,
            poll_add_account_login,
            switch_account,
            remove_account,
            rename_account,
            reorder_accounts
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
