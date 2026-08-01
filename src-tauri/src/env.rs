//! プロジェクト単位の Claude Code 環境（CLAUDE.md / メモリ / MCP / agents / skills /
//! commands / hooks / rules)を一括収集する。このアプリの中核機能。

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::home_dir;
use crate::inventory::{scan_md_dir, scan_skills_dir, InventoryItem};

/// 設定ファイル編集中に切り詰めが起きにくいよう、閲覧上限は大きめに取る
const MAX_DOC_CHARS: usize = 200_000;
/// 保存できるコンテンツの文字数上限（誤操作での巨大書き込みを防ぐ安全弁）
const MAX_WRITE_CHARS: usize = 400_000;
/// フロントから編集・保存を許可する拡張子（read/write共通のホワイトリスト）
const ALLOWED_EXTENSIONS: [&str; 6] = ["md", "json", "txt", "yaml", "yml", "toml"];

#[derive(Serialize, Debug)]
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
    pub scope: String,  // "project" | "global"
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

/// read/write 共通のパスガード。ホーム配下・許可拡張子のみを通す。
/// テストではダミーの `home` を渡して実ホームディレクトリに触れずに検証する。
fn checked_path(path: &str, home: &Path) -> Result<PathBuf, String> {
    let canon = PathBuf::from(path)
        .canonicalize()
        .map_err(|e| format!("{path}: {e}"))?;
    if !canon.starts_with(home) {
        return Err("ホームディレクトリ外のファイルは扱えません".into());
    }
    let ext = canon
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("この拡張子は扱えません: .{ext}"));
    }
    Ok(canon)
}

/// フロントからの任意ファイル閲覧。ホームディレクトリ配下のテキスト系のみ許可。
pub fn read_doc_checked(path: &str) -> Result<FileDoc, String> {
    let home = home_dir().canonicalize().map_err(|e| e.to_string())?;
    let canon = checked_path(path, &home)?;
    read_doc(&canon).ok_or_else(|| format!("読み込めません: {path}"))
}

/// フロントからのドキュメント保存。read_doc_checked と同じガードに加え、
/// 楽観ロック（mtime不一致は上書きせず conflict エラー）とアトミック書き込みを行う。
pub fn write_doc_checked(
    path: &str,
    content: &str,
    expected_modified_epoch: Option<i64>,
) -> Result<FileDoc, String> {
    let home = home_dir().canonicalize().map_err(|e| e.to_string())?;
    write_doc_impl(path, content, expected_modified_epoch, &home)
}

fn write_doc_impl(
    path: &str,
    content: &str,
    expected_modified_epoch: Option<i64>,
    home: &Path,
) -> Result<FileDoc, String> {
    let canon = checked_path(path, home)?;

    if content.chars().count() > MAX_WRITE_CHARS {
        return Err(format!(
            "ファイルが大きすぎます（上限 {MAX_WRITE_CHARS} 文字）"
        ));
    }

    if let Some(expected) = expected_modified_epoch {
        let current = modified_epoch_of(&canon);
        if current != Some(expected) {
            return Err(format!(
                "conflict: {} は別の場所で変更されています",
                canon.display()
            ));
        }
    }

    // アトミック書き込み: 同一ディレクトリの隠し一時ファイルに書いて fsync → rename。
    // 同一ファイルシステム内の rename は原子的なため、書き込み中のクラッシュでも
    // 元ファイルが壊れた状態で残ることはない
    let dir = canon
        .parent()
        .ok_or_else(|| "親ディレクトリが取得できません".to_string())?;
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = canon
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("doc");
    let tmp_path = dir.join(format!(".{file_name}.tmp-{pid}-{nanos}"));

    // 元ファイルの mode（0600 等）を引き継ぐ。tmp→rename 方式は既定で 0644 になり
    // パーミッションを落としてしまうため。元ファイルが無い（新規作成扱い）場合はそのまま
    #[cfg(unix)]
    let original_permissions = fs::metadata(&canon).ok().map(|m| m.permissions());

    let write_result = (|| -> Result<(), String> {
        let mut f = fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
        f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(perms) = &original_permissions {
            fs::set_permissions(&tmp_path, perms.clone()).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp_path, &canon) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e.to_string());
    }

    read_doc(&canon).ok_or_else(|| format!("書き込み後の読み込みに失敗: {path}"))
}

/// ミリ秒精度の mtime。秒精度だと同一秒内の外部変更を楽観ロックが見逃すため、
/// 保存の衝突検知はこの精度で行う（表示側の relativeTime/formatEpoch は秒・ミリ秒どちらの
/// 桁でも自動判定するため、この単位変更に追従の必要はない）
fn modified_epoch_of(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

fn read_doc(path: &Path) -> Option<FileDoc> {
    let content = fs::read_to_string(path).ok()?;
    let truncated = content.chars().count() > MAX_DOC_CHARS;
    let content = if truncated {
        content.chars().take(MAX_DOC_CHARS).collect()
    } else {
        content
    };
    let modified_epoch = modified_epoch_of(path).unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// テスト専用の「ホーム」ディレクトリを実ホームに触れず用意する
    fn temp_home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cc-anatomy-env-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn write_doc_rejects_path_outside_home() {
        let home = temp_home();
        let outside_dir = std::env::temp_dir().join(format!(
            "cc-anatomy-env-test-outside-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&outside_dir).unwrap();
        let file = outside_dir.join("note.md");
        fs::write(&file, "hello").unwrap();

        let result = write_doc_impl(file.to_str().unwrap(), "new content", None, &home);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ホームディレクトリ外"));

        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&outside_dir);
    }

    #[test]
    fn write_doc_rejects_disallowed_extension() {
        let home = temp_home();
        let file = home.join("main.rs");
        fs::write(&file, "fn main() {}").unwrap();

        let result = write_doc_impl(file.to_str().unwrap(), "fn main() {}", None, &home);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("拡張子"));

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn write_doc_rejects_mtime_mismatch_as_conflict() {
        let home = temp_home();
        let file = home.join("CLAUDE.md");
        fs::write(&file, "original").unwrap();
        let actual_epoch = modified_epoch_of(&file).unwrap();

        // わざと1秒ずらした期待値を渡し、外部変更を検知させる（秒精度でも決定的に失敗させる）
        let result = write_doc_impl(
            file.to_str().unwrap(),
            "overwritten",
            Some(actual_epoch - 1),
            &home,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.starts_with("conflict:"), "got: {err}");

        // 上書きされず元の内容が残っていること
        assert_eq!(fs::read_to_string(&file).unwrap(), "original");

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn write_doc_atomic_success_path_updates_content_and_epoch() {
        let home = temp_home();
        let file = home.join("CLAUDE.md");
        fs::write(&file, "original").unwrap();
        let before_epoch = modified_epoch_of(&file).unwrap();

        let doc = write_doc_impl(file.to_str().unwrap(), "updated content", Some(before_epoch), &home)
            .expect("同じmtimeを渡した書き込みは成功するはず");

        assert_eq!(doc.content, "updated content");
        assert!(!doc.truncated);
        assert_eq!(fs::read_to_string(&file).unwrap(), "updated content");

        // 一時ファイルが残っていないこと（アトミックrenameの確認）
        let leftovers: Vec<_> = fs::read_dir(&home)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "一時ファイルが残存: {leftovers:?}");

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn write_doc_without_expected_epoch_skips_lock_check() {
        let home = temp_home();
        let file = home.join("notes.txt");
        fs::write(&file, "v1").unwrap();
        // ロック検証を働かせるため書き込み間で mtime を確実にずらす
        std::thread::sleep(Duration::from_millis(1100));

        let doc = write_doc_impl(file.to_str().unwrap(), "v2", None, &home)
            .expect("expected_modified_epoch が None のときはロック検証しない");
        assert_eq!(doc.content, "v2");

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn write_doc_rejects_oversized_content() {
        let home = temp_home();
        let file = home.join("big.md");
        fs::write(&file, "small").unwrap();
        let huge = "a".repeat(MAX_WRITE_CHARS + 1);

        let result = write_doc_impl(file.to_str().unwrap(), &huge, None, &home);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("大きすぎます"));

        let _ = fs::remove_dir_all(&home);
    }
}
