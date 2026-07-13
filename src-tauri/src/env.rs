//! プロジェクト単位の Claude Code 環境（CLAUDE.md / メモリ / MCP / agents / skills /
//! commands / hooks / rules)を一括収集する。このアプリの中核機能。

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::home_dir;
use crate::inventory::{scan_md_dir, scan_skills_dir, InventoryItem};

const MAX_DOC_CHARS: usize = 20_000;

#[derive(Serialize)]
pub struct FileDoc {
    pub path: String,
    pub content: String,
    pub truncated: bool,
    pub modified_epoch: i64,
}

#[derive(Serialize)]
pub struct ScopedItem {
    pub name: String,
    pub description: String,
    pub path: String,
    pub scope: String, // "project" | "global"
}

#[derive(Serialize)]
pub struct McpServer {
    pub name: String,
    pub scope: String, // "project" | "global"
    pub source: String, // 設定ファイルの場所
    pub config: String, // 整形済みJSON（秘匿値はマスク）
}

#[derive(Serialize)]
pub struct HookInfo {
    pub event: String,
    pub matcher_count: usize,
    pub scope: String,
    pub config: String, // matcher定義の整形済みJSON
}

#[derive(Serialize)]
pub struct RuleFile {
    pub name: String,
    pub path: String,
}

/// APIキー等が入りうる env / headers の値を伏せる
fn mask_secrets(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(obj) = v.as_object_mut() {
        for key in ["env", "headers"] {
            if let Some(sec) = obj.get_mut(key).and_then(Value::as_object_mut) {
                for (_, val) in sec.iter_mut() {
                    *val = Value::String("•••".into());
                }
            }
        }
    }
    v
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

#[derive(Serialize)]
pub struct ObservationItem {
    pub id: i64,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    #[serde(rename = "type")]
    pub obs_type: String,
    pub narrative: Option<String>,
    pub facts: Vec<String>,
    pub files_modified: Vec<String>,
    pub created_at_epoch: i64,
}

#[derive(Serialize)]
pub struct ProjectEnv {
    pub path: Option<String>,
    /// claude-mem プラグインの有無。無い環境ではフロントがメモリカードを隠す
    pub has_claude_mem: bool,
    pub claude_mds: Vec<FileDoc>,
    pub memory_md: Option<FileDoc>,
    pub memory_files: Vec<RuleFile>,
    pub observations: Vec<ObservationItem>,
    pub next_steps: Option<String>,
    pub mcp_servers: Vec<McpServer>,
    pub agents: Vec<ScopedItem>,
    pub skills: Vec<ScopedItem>,
    pub commands: Vec<ScopedItem>,
    pub hooks: Vec<HookInfo>,
    pub rules: Vec<RuleFile>,
}

/// フロントからの任意ファイル閲覧。ホームディレクトリ配下のテキスト系のみ許可。
pub fn read_doc_checked(path: &str) -> Result<FileDoc, String> {
    let canon = PathBuf::from(path)
        .canonicalize()
        .map_err(|e| format!("{path}: {e}"))?;
    let home = home_dir()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !canon.starts_with(&home) {
        return Err("ホームディレクトリ外のファイルは表示できません".into());
    }
    let ext = canon
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !["md", "json", "txt", "yaml", "yml", "toml"].contains(&ext.as_str()) {
        return Err(format!("この拡張子は表示できません: .{ext}"));
    }
    read_doc(&canon).ok_or_else(|| format!("読み込めません: {path}"))
}

fn read_doc(path: &Path) -> Option<FileDoc> {
    let content = fs::read_to_string(path).ok()?;
    let truncated = content.chars().count() > MAX_DOC_CHARS;
    let content = if truncated {
        content.chars().take(MAX_DOC_CHARS).collect()
    } else {
        content
    };
    let modified_epoch = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(FileDoc {
        path: path.display().to_string(),
        content,
        truncated,
        modified_epoch,
    })
}

/// Claude Code が ~/.claude/projects/ で使うディレクトリ名エンコード
/// （英数字以外を '-' に置換）
fn encode_project_path(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn scoped(items: Vec<InventoryItem>, scope: &str) -> Vec<ScopedItem> {
    items
        .into_iter()
        .map(|i| ScopedItem {
            name: i.name,
            description: i.description,
            path: i.path,
            scope: scope.to_string(),
        })
        .collect()
}

fn collect_mcp_servers(project_path: Option<&str>) -> Vec<McpServer> {
    let mut servers = Vec::new();
    let mut push_all = |obj: &serde_json::Map<String, Value>, scope: &str, source: &str| {
        for (name, config) in obj {
            servers.push(McpServer {
                name: name.clone(),
                scope: scope.into(),
                source: source.into(),
                config: pretty(&mask_secrets(config)),
            });
        }
    };
    let claude_json = home_dir().join(".claude.json");
    if let Ok(text) = fs::read_to_string(&claude_json) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(obj) = v.get("mcpServers").and_then(Value::as_object) {
                push_all(obj, "global", "~/.claude.json");
            }
            if let Some(path) = project_path {
                if let Some(obj) = v
                    .pointer(&format!("/projects/{}/mcpServers", path.replace('/', "~1")))
                    .and_then(Value::as_object)
                {
                    push_all(obj, "project", "~/.claude.json (projects)");
                }
            }
        }
    }
    if let Some(path) = project_path {
        let mcp_json = Path::new(path).join(".mcp.json");
        if let Ok(text) = fs::read_to_string(&mcp_json) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(obj) = v.get("mcpServers").and_then(Value::as_object) {
                    push_all(obj, "project", ".mcp.json");
                }
            }
        }
    }
    servers
}

fn collect_hooks_from(settings_path: &Path, scope: &str, out: &mut Vec<HookInfo>) {
    let Ok(text) = fs::read_to_string(settings_path) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    if let Some(hooks) = v.get("hooks").and_then(Value::as_object) {
        for (event, matchers) in hooks {
            out.push(HookInfo {
                event: event.clone(),
                matcher_count: matchers.as_array().map(|a| a.len()).unwrap_or(0),
                scope: scope.to_string(),
                config: pretty(matchers),
            });
        }
    }
}

pub fn get_project_env(project: &str, path: Option<String>) -> Result<ProjectEnv, String> {
    let home = home_dir();

    // --- ファイル系（プロジェクトパスが解決できている場合のみ） ---
    let mut claude_mds = Vec::new();
    let mut memory_md = None;
    let mut memory_files = Vec::new();

    if let Some(p) = &path {
        let base = PathBuf::from(p);
        for candidate in [base.join("CLAUDE.md"), base.join(".claude/CLAUDE.md")] {
            if let Some(doc) = read_doc(&candidate) {
                claude_mds.push(doc);
            }
        }
        // auto-memory: ~/.claude/projects/<encoded>/memory/
        let memory_dir = home
            .join(".claude/projects")
            .join(encode_project_path(p))
            .join("memory");
        memory_md = read_doc(&memory_dir.join("MEMORY.md"));
        if let Ok(entries) = fs::read_dir(&memory_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name != "MEMORY.md" && name.ends_with(".md") {
                    memory_files.push(RuleFile {
                        path: entry.path().display().to_string(),
                        name,
                    });
                }
            }
            memory_files.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    // --- claude-mem 直近の記憶 ---
    let (observations, next_steps) = crate::db::recent_memory(project)?;

    // --- サブシステム ---
    let mcp_servers = collect_mcp_servers(path.as_deref());

    let mut agents = scoped(scan_md_dir(&home.join(".claude/agents")), "global");
    let mut skills = scoped(scan_skills_dir(&home.join(".claude/skills")), "global");
    let mut commands = scoped(scan_md_dir(&home.join(".claude/commands")), "global");
    if let Some(p) = &path {
        let base = PathBuf::from(p).join(".claude");
        let mut pa = scoped(scan_md_dir(&base.join("agents")), "project");
        let mut ps = scoped(scan_skills_dir(&base.join("skills")), "project");
        let mut pc = scoped(scan_md_dir(&base.join("commands")), "project");
        // プロジェクト固有を先頭に
        pa.append(&mut agents);
        ps.append(&mut skills);
        pc.append(&mut commands);
        agents = pa;
        skills = ps;
        commands = pc;
    }

    let mut hooks = Vec::new();
    collect_hooks_from(&home.join(".claude/settings.json"), "global", &mut hooks);
    collect_hooks_from(
        &home.join(".claude/settings.local.json"),
        "global",
        &mut hooks,
    );
    if let Some(p) = &path {
        let base = PathBuf::from(p).join(".claude");
        collect_hooks_from(&base.join("settings.json"), "project", &mut hooks);
        collect_hooks_from(&base.join("settings.local.json"), "project", &mut hooks);
    }

    let mut rules = Vec::new();
    if let Ok(entries) = fs::read_dir(home.join(".claude/rules")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                rules.push(RuleFile {
                    path: entry.path().display().to_string(),
                    name,
                });
            }
        }
        rules.sort_by(|a, b| a.name.cmp(&b.name));
    }

    Ok(ProjectEnv {
        path,
        has_claude_mem: crate::db::db_available(),
        claude_mds,
        memory_md,
        memory_files,
        observations,
        next_steps,
        mcp_servers,
        agents,
        skills,
        commands,
        hooks,
        rules,
    })
}
