mod actions;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
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
            get_account_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
