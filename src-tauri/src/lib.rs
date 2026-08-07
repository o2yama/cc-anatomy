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
#[cfg(target_os = "macos")]
mod doc_analysis;
#[cfg(not(target_os = "macos"))]
#[path = "doc_analysis_stub.rs"]
mod doc_analysis;

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
fn write_doc(
    path: String,
    content: String,
    expected_modified_epoch: Option<i64>,
) -> Result<env::FileDoc, String> {
    env::write_doc_checked(&path, &content, expected_modified_epoch)
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

/// ドキュメントエディタの「AIに分析・改善してもらう」。診断機能とは独立した多重実行ガードで
/// claude -p を読み取り専用実行するので async
#[tauri::command(async)]
fn analyze_doc(
    app: tauri::AppHandle,
    path: String,
    content: String,
    project_dir: Option<String>,
) -> Result<String, String> {
    doc_analysis::analyze_doc(&app, path, content, project_dir)
}

#[tauri::command]
fn cancel_doc_analysis() -> Result<(), String> {
    doc_analysis::cancel_doc_analysis()
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
/// 事前 sync-back を含むので async。`force` は外部セッション確認済みの再実行フラグ。
/// `trust_unverified`（2026-08-08 issue #3）は `force` とは独立: 持ち主確認が
/// TokenExpired/NetworkError で失敗しても sync-back をスキップして続行する同意フラグ
/// （accounts::sync_back_live_login の SyncBack::SkippedUnverified 参照）。
/// 2026-07-26、統合フロー（1/2 ログイン→2/2 監視の承認）のため同じ Terminal で
/// 続けて setup-token も起動するようになり、スクリプトの書き出し先に AppHandle が要る
/// `target_name` を指定すると登録済みカードの「再ログイン」導線になり、ログイン結果の
/// org_id が対象と一致しない場合は取り込まず mismatch を返す（誤紐づけ防止）。
/// 未指定なら従来どおり「＋アカウントを追加」の汎用フロー
#[tauri::command(async)]
fn start_add_account_login(
    app: tauri::AppHandle,
    force: bool,
    trust_unverified: bool,
    target_name: Option<String>,
) -> Result<accounts::StartLoginOutcome, String> {
    accounts::start_add_account_login(&app, force, trust_unverified, target_name.as_deref())
}

/// フロントが2秒間隔で呼ぶ完了検知ポーリング（Flow B）
#[tauri::command(async)]
fn poll_add_account_login(baseline: String) -> Result<accounts::PollResult, String> {
    accounts::poll_add_account_login(&baseline)
}

/// 登録済みアカウントへ「常時監視を設定」（setup-token のみを Terminal で起動する単独フロー）。
/// 使用量の常時監視は切り替え機能とは完全に独立した任意機能
#[tauri::command(async)]
fn start_monitor_setup(app: tauri::AppHandle, name: String) -> Result<(), String> {
    accounts::start_monitor_setup(&app, &name)
}

/// フロントが2秒間隔で呼ぶ、監視トークンの紐づけ完了検知ポーリング
/// （「＋アカウントを追加」のステップ2、または既存アカウントの「常時監視を設定」の両方で使う）
#[tauri::command(async)]
fn poll_monitor_setup(name: String) -> Result<accounts::MonitorSetupPoll, String> {
    accounts::poll_monitor_setup(&name)
}

/// Keychain スワップによる切り替え（Flow C）。sync-back・読み戻し検証を伴うので async。
/// 切り替え成功時はトレイの使用率表示にも即反映する。`force` は外部セッション確認済みの再実行フラグ。
/// `trust_unverified`（2026-08-08 issue #3）は `force` とは独立した同意フラグ
/// （accounts::switch_account の doc 参照）
#[tauri::command(async)]
fn switch_account(
    app: tauri::AppHandle,
    name: String,
    force: bool,
    trust_unverified: bool,
) -> Result<accounts::SwitchOutcome, String> {
    let outcome = accounts::switch_account(&name, force, trust_unverified)?;
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

/// 登録済み全アカウントの使用率一括取得。Keychain 読み取り・ネットワーク呼び出しを
/// 伴うため async にし、一覧表示（get_accounts）をブロックしない別コマンドにする
#[tauri::command(async)]
fn get_accounts_usage() -> Result<Vec<accounts::AccountUsage>, String> {
    accounts::get_accounts_usage()
}

/// アプリ内右上の使用量ポップオーバー向け。トレイと同じ土台（`tray::fetch_raw_status`）から
/// 組み立てるため、表示内容・数値の優先順位はトレイと完全に一致する。
/// ライブ OAuth への HTTP を含むため async にする
#[tauri::command(async)]
fn get_usage_overview() -> tray::UsageOverview {
    tray::usage_overview()
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
            write_doc,
            list_sessions,
            search_summaries,
            get_transcript,
            list_skills,
            list_agents,
            open_in_finder,
            open_in_cmux,
            open_in_terminal,
            extract_tasks,
            run_diagnosis,
            cancel_diagnosis,
            run_fixes_in_terminal,
            analyze_doc,
            cancel_doc_analysis,
            get_accounts,
            import_live_account,
            start_add_account_login,
            poll_add_account_login,
            switch_account,
            remove_account,
            rename_account,
            reorder_accounts,
            get_accounts_usage,
            get_usage_overview,
            start_monitor_setup,
            poll_monitor_setup
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
