//! claude-mem の SQLite（~/.claude-mem/claude-mem.db）読み取り層。
//! worker が常時書き込むDBのため、読み取り専用で開くこと（書き込み禁止）。

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    // Windows に HOME 環境変数は無いため std に任せる（近年の Rust で挙動修正・非推奨解除済み）
    std::env::home_dir().expect("home directory not found")
}

/// claude-mem プラグインが導入されているか（DB ファイルの有無で判定）。
/// 無い環境ではメモリ系機能を隠し、transcript ベースのフォールバックで動く
pub fn db_available() -> bool {
    home_dir().join(".claude-mem/claude-mem.db").exists()
}

fn open_db() -> Result<Connection, String> {
    let path = home_dir().join(".claude-mem/claude-mem.db");
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("claude-mem DB を開けません ({}): {}", path.display(), e))?;
    conn.busy_timeout(std::time::Duration::from_millis(3000))
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// observer セッション等のノイズを除外する共通条件（column には別名付きカラムを渡す）
fn project_filter(column: &str) -> String {
    format!("{column} != 'unknown-project' AND {column} != ''")
}

#[derive(Serialize)]
pub struct ProjectInfo {
    pub project: String,
    pub path: Option<String>,
    pub session_count: i64,
    pub summary_count: i64,
    pub last_activity_epoch: i64,
    pub last_request: Option<String>,
}

/// worktree 内で起動したセッションの cwd は親プロジェクトに正規化する
fn normalize_cwd(cwd: &str) -> String {
    for marker in ["/.worktrees/", "/.claude/worktrees/"] {
        if let Some(pos) = cwd.find(marker) {
            return cwd[..pos].to_string();
        }
    }
    cwd.to_string()
}

/// project(basename) → フルパスの解決。
/// 1. claude-mem の pending_messages に残る cwd（直近セッション分のみ）
/// 2. ~/.claude/projects/*/ の jsonl 先頭から拾った cwd（basename 一致で補完）
fn resolve_project_paths(conn: &Connection) -> std::collections::HashMap<String, String> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT s.project, p.cwd
         FROM sdk_sessions s
         JOIN pending_messages p ON p.content_session_id = s.content_session_id
         WHERE p.cwd IS NOT NULL AND s.project != 'unknown-project'",
    ) {
        if let Ok(rows) =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            for (project, cwd) in rows.flatten() {
                let cwd = normalize_cwd(&cwd);
                // サブディレクトリ起動の cwd より短い（=浅い）パスを採用する
                map.entry(project)
                    .and_modify(|cur| {
                        if cwd.len() < cur.len() {
                            *cur = cwd.clone();
                        }
                    })
                    .or_insert(cwd);
            }
        }
    }

    let by_basename = crate::transcript::scan_project_dir_cwds();
    let mut basename_to_path: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for cwd in by_basename {
        let cwd = normalize_cwd(&cwd);
        if let Some(base) = std::path::Path::new(&cwd).file_name() {
            basename_to_path
                .entry(base.to_string_lossy().to_string())
                .or_insert(cwd);
        }
    }
    for (base, path) in basename_to_path {
        map.entry(base).or_insert(path);
    }
    map
}

/// 記録が消えた古いプロジェクトの最終フォールバック。
/// 解決済みパスの親ディレクトリ群に「プロジェクト名のディレクトリ」が実在すればそれを採用する。
fn guess_paths_from_siblings(
    map: &mut std::collections::HashMap<String, String>,
    all_projects: &[String],
) {
    let mut parents: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    for path in map.values() {
        if let Some(parent) = std::path::Path::new(path).parent() {
            parents.insert(parent.to_path_buf());
        }
    }
    for project in all_projects {
        if map.contains_key(project) {
            continue;
        }
        for parent in &parents {
            let candidate = parent.join(project);
            if candidate.is_dir() {
                map.insert(project.clone(), candidate.display().to_string());
                break;
            }
        }
    }
}

pub fn list_projects() -> Result<Vec<ProjectInfo>, String> {
    // claude-mem 無し環境: transcript フォルダから最低限のプロジェクト一覧を作る。
    // worktree の cwd は親プロジェクトに正規化し、同一 cwd はマージする
    if !db_available() {
        let mut by_cwd: std::collections::HashMap<String, ProjectInfo> =
            std::collections::HashMap::new();
        for p in crate::transcript::scan_projects_fallback() {
            let cwd = normalize_cwd(&p.cwd);
            let entry = by_cwd.entry(cwd.clone()).or_insert_with(|| ProjectInfo {
                project: std::path::Path::new(&cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| cwd.clone()),
                path: Some(cwd),
                session_count: 0,
                summary_count: 0,
                last_activity_epoch: 0,
                last_request: None,
            });
            entry.session_count += p.session_count;
            entry.last_activity_epoch = entry.last_activity_epoch.max(p.last_activity_epoch);
        }
        let mut projects: Vec<ProjectInfo> = by_cwd.into_values().collect();
        projects.sort_by_key(|p| -p.last_activity_epoch);
        return Ok(projects);
    }
    let conn = open_db()?;
    let sql = format!(
        "SELECT s.project,
                COUNT(DISTINCT s.id),
                COUNT(DISTINCT ss.id),
                MAX(s.started_at_epoch),
                (SELECT request FROM session_summaries ss2
                  WHERE ss2.project = s.project ORDER BY ss2.created_at_epoch DESC LIMIT 1)
         FROM sdk_sessions s
         LEFT JOIN session_summaries ss ON ss.memory_session_id = s.memory_session_id
         WHERE {}
         GROUP BY s.project
         ORDER BY MAX(s.started_at_epoch) DESC",
        project_filter("s.project")
    );
    let paths = resolve_project_paths(&conn);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ProjectInfo {
                project: r.get(0)?,
                path: None,
                session_count: r.get(1)?,
                summary_count: r.get(2)?,
                last_activity_epoch: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                last_request: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut projects = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let names: Vec<String> = projects.iter().map(|p| p.project.clone()).collect();
    let mut paths = paths;
    guess_paths_from_siblings(&mut paths, &names);
    for p in &mut projects {
        p.path = paths.get(&p.project).cloned();
    }
    Ok(projects)
}

#[derive(Serialize)]
pub struct SummaryEntry {
    pub request: Option<String>,
    pub investigated: Option<String>,
    pub learned: Option<String>,
    pub completed: Option<String>,
    pub next_steps: Option<String>,
    pub files_edited: Option<String>,
    pub created_at_epoch: i64,
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub content_session_id: String,
    pub user_prompt: Option<String>,
    pub started_at_epoch: i64,
    pub status: String,
    pub summaries: Vec<SummaryEntry>,
}

pub fn list_sessions(project: &str) -> Result<Vec<SessionInfo>, String> {
    if !db_available() {
        return Ok(vec![]);
    }
    let conn = open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT content_session_id, memory_session_id, user_prompt, started_at_epoch, status
             FROM sdk_sessions WHERE project = ?1
             ORDER BY started_at_epoch DESC LIMIT 500",
        )
        .map_err(|e| e.to_string())?;

    struct Row {
        info: SessionInfo,
        memory_session_id: Option<String>,
    }
    let sessions: Vec<Row> = stmt
        .query_map([project], |r| {
            Ok(Row {
                info: SessionInfo {
                    content_session_id: r.get(0)?,
                    user_prompt: r.get(2)?,
                    started_at_epoch: r.get(3)?,
                    status: r.get(4)?,
                    summaries: vec![],
                },
                memory_session_id: r.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut sum_stmt = conn
        .prepare(
            "SELECT request, investigated, learned, completed, next_steps, files_edited, created_at_epoch
             FROM session_summaries WHERE memory_session_id = ?1 ORDER BY created_at_epoch ASC",
        )
        .map_err(|e| e.to_string())?;

    let mut result = Vec::with_capacity(sessions.len());
    for mut row in sessions {
        if let Some(mid) = &row.memory_session_id {
            let sums = sum_stmt
                .query_map([mid], |r| {
                    Ok(SummaryEntry {
                        request: r.get(0)?,
                        investigated: r.get(1)?,
                        learned: r.get(2)?,
                        completed: r.get(3)?,
                        next_steps: r.get(4)?,
                        files_edited: r.get(5)?,
                        created_at_epoch: r.get(6)?,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            row.info.summaries = sums;
        }
        result.push(row.info);
    }
    Ok(result)
}

/// タスク抽出（claude CLI）に渡すサマリー履歴のダイジェストを組み立てる。
/// 「何を頼まれ・何を終え・何が残っているか」だけに絞り、直近50件・新しい順。
pub fn task_digest(project: &str) -> Result<String, String> {
    if !db_available() {
        return Err("タスク抽出には claude-mem プラグインが必要です".into());
    }
    let conn = open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT request, completed, next_steps,
                    date(created_at_epoch / 1000, 'unixepoch', 'localtime')
             FROM session_summaries
             WHERE project = ?1
             ORDER BY created_at_epoch DESC LIMIT 50",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([project], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut digest = String::new();
    for row in rows.flatten() {
        let (request, completed, next_steps, date) = row;
        let mut block = String::new();
        if let Some(v) = request.filter(|v| !v.trim().is_empty()) {
            block.push_str(&format!("依頼: {v}\n"));
        }
        if let Some(v) = completed.filter(|v| !v.trim().is_empty()) {
            block.push_str(&format!("完了: {v}\n"));
        }
        if let Some(v) = next_steps.filter(|v| !v.trim().is_empty()) {
            block.push_str(&format!("次の一手: {v}\n"));
        }
        if !block.is_empty() {
            digest.push_str(&format!("[{date}]\n{block}\n"));
        }
    }
    Ok(digest)
}

/// DBにJSON文字列として入っている配列カラム（facts等）を Vec<String> に落とす
fn json_str_array(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// プロジェクトの「直近の記憶」: observations 最新30件と最新の next_steps
pub fn recent_memory(
    project: &str,
) -> Result<(Vec<crate::env::ObservationItem>, Option<String>), String> {
    if !db_available() {
        return Ok((vec![], None));
    }
    use rusqlite::OptionalExtension;
    let conn = open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, title, subtitle, type, narrative, facts, files_modified, created_at_epoch
             FROM observations
             WHERE project = ?1 ORDER BY created_at_epoch DESC LIMIT 30",
        )
        .map_err(|e| e.to_string())?;
    let observations = stmt
        .query_map([project], |r| {
            Ok(crate::env::ObservationItem {
                id: r.get(0)?,
                title: r.get(1)?,
                subtitle: r.get(2)?,
                obs_type: r.get(3)?,
                narrative: r.get(4)?,
                facts: json_str_array(r.get(5)?),
                files_modified: json_str_array(r.get(6)?),
                created_at_epoch: r.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let next_steps: Option<String> = conn
        .query_row(
            "SELECT next_steps FROM session_summaries
             WHERE project = ?1 AND next_steps IS NOT NULL AND next_steps != ''
             ORDER BY created_at_epoch DESC LIMIT 1",
            [project],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    Ok((observations, next_steps))
}

#[derive(Serialize)]
pub struct SearchHit {
    pub project: String,
    pub content_session_id: Option<String>,
    pub request: Option<String>,
    pub completed: Option<String>,
    pub created_at_epoch: i64,
}

/// session_summaries の FTS5 インデックスを使った全文検索。
/// project を渡すとそのプロジェクト内に絞り込む。
pub fn search_summaries(query: &str, project: Option<&str>) -> Result<Vec<SearchHit>, String> {
    if !db_available() {
        return Err("横断検索には claude-mem プラグインが必要です".into());
    }
    let conn = open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT ss.project, s.content_session_id, ss.request, ss.completed, ss.created_at_epoch
             FROM session_summaries_fts f
             JOIN session_summaries ss ON ss.id = f.rowid
             LEFT JOIN sdk_sessions s ON s.memory_session_id = ss.memory_session_id
             WHERE session_summaries_fts MATCH ?1
               AND (?2 IS NULL OR ss.project = ?2)
             ORDER BY ss.created_at_epoch DESC LIMIT 100",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![query, project], |r| {
            Ok(SearchHit {
                project: r.get(0)?,
                content_session_id: r.get(1)?,
                request: r.get(2)?,
                completed: r.get(3)?,
                created_at_epoch: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}
