//! Skills / Agents インベントリ。
//! ~/.claude/skills/*/SKILL.md と ~/.claude/agents/*.md の YAML frontmatter を読む。

use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::db::home_dir;

#[derive(Serialize)]
pub struct InventoryItem {
    pub name: String,
    pub description: String,
    pub path: String,
    pub modified_epoch: i64,
}

/// frontmatter（--- で囲まれた先頭ブロック）から key の値を取り出す。
/// 値が複数行（インデント継続 or ブロックスカラー）の場合は連結する。
fn frontmatter_value(content: &str, key: &str) -> Option<String> {
    let rest = content.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let fm = &rest[..end];

    let prefix = format!("{key}:");
    let mut lines = fm.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.starts_with(&prefix) {
            continue;
        }
        let first = line[prefix.len()..].trim();
        // ブロックスカラー（| や >）は値行を全部拾う
        let block = first == "|" || first == ">" || first == "|-" || first == ">-";
        let mut parts: Vec<String> = if block || first.is_empty() {
            vec![]
        } else {
            vec![first.trim_matches(|c| c == '"' || c == '\'').to_string()]
        };
        while let Some(next) = lines.peek() {
            if next.starts_with(' ') || next.starts_with('\t') {
                parts.push(next.trim().to_string());
                lines.next();
            } else {
                break;
            }
        }
        return Some(parts.join(" "));
    }
    None
}

pub fn read_item(md_path: &Path, fallback_name: &str) -> Option<InventoryItem> {
    let content = fs::read_to_string(md_path).ok()?;
    let name = frontmatter_value(&content, "name").unwrap_or_else(|| fallback_name.to_string());
    let description = frontmatter_value(&content, "description").unwrap_or_default();
    let modified_epoch = fs::metadata(md_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(InventoryItem {
        name,
        description,
        path: md_path.display().to_string(),
        modified_epoch,
    })
}

/// skills 形式（<dir>/<name>/SKILL.md）のディレクトリを走査
pub fn scan_skills_dir(dir: &Path) -> Vec<InventoryItem> {
    let mut items = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return items;
    };
    for entry in entries.flatten() {
        let skill_md = entry.path().join("SKILL.md");
        if skill_md.is_file() {
            let fallback = entry.file_name().to_string_lossy().to_string();
            if let Some(item) = read_item(&skill_md, &fallback) {
                items.push(item);
            }
        }
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items
}

/// フラットな *.md 群（agents / commands 形式）のディレクトリを走査
pub fn scan_md_dir(dir: &Path) -> Vec<InventoryItem> {
    let mut items = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return items;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            let fallback = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(item) = read_item(&path, &fallback) {
                items.push(item);
            }
        }
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items
}

pub fn list_skills() -> Result<Vec<InventoryItem>, String> {
    Ok(scan_skills_dir(&home_dir().join(".claude/skills")))
}

pub fn list_agents() -> Result<Vec<InventoryItem>, String> {
    Ok(scan_md_dir(&home_dir().join(".claude/agents")))
}
