//! セッション jsonl の遅延読み取り。
//! 事前インデックスは持たず、ドリルダウン時にセッションUUIDでファイルを探して1本だけ読む。

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::db::home_dir;

/// claude-mem observer 等、ユーザーセッションではないディレクトリ
fn is_noise_dir(name: &str) -> bool {
    name == "-" || name.contains("claude-mem-observer")
}

fn find_session_file(session_id: &str) -> Option<PathBuf> {
    let projects_dir = home_dir().join(".claude/projects");
    let filename = format!("{session_id}.jsonl");
    let entries = fs::read_dir(&projects_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_noise_dir(&name) {
            continue;
        }
        let candidate = entry.path().join(&filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // ノイズディレクトリしか残っていない場合の最終フォールバック
    let entries = fs::read_dir(&projects_dir).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(&filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// ~/.claude/projects/ の各ディレクトリから jsonl を1本読み、実際の cwd を回収する。
/// ディレクトリ名はフルパスのダッシュエンコードで復元不能（`_`と`-`が混在）なため、
/// jsonl 内の cwd フィールドを正とする。
pub fn scan_project_dir_cwds() -> Vec<String> {
    let projects_dir = home_dir().join(".claude/projects");
    let mut cwds = Vec::new();
    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return cwds;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_noise_dir(&name) {
            continue;
        }
        let Ok(files) = fs::read_dir(entry.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(cwd) = read_cwd_from_jsonl(&path) {
                cwds.push(cwd);
                break; // 1ディレクトリ1本で十分
            }
        }
    }
    cwds
}

/// claude-mem 無し環境向けのプロジェクト情報（transcript フォルダから復元）
pub struct FallbackProject {
    pub cwd: String,
    pub session_count: i64,
    /// 最新 jsonl の mtime（ミリ秒）
    pub last_activity_epoch: i64,
}

/// ~/.claude/projects/ をスキャンし、ディレクトリごとに
/// cwd（jsonl の中身から）・セッション数・最終更新を集める
pub fn scan_projects_fallback() -> Vec<FallbackProject> {
    let projects_dir = home_dir().join(".claude/projects");
    let mut result = Vec::new();
    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return result;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_noise_dir(&name) {
            continue;
        }
        let Ok(files) = fs::read_dir(entry.path()) else {
            continue;
        };
        let mut cwd = None;
        let mut count = 0i64;
        let mut latest_ms = 0i64;
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            count += 1;
            if let Ok(meta) = file.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                        latest_ms = latest_ms.max(d.as_millis() as i64);
                    }
                }
            }
            if cwd.is_none() {
                cwd = read_cwd_from_jsonl(&path);
            }
        }
        if let Some(cwd) = cwd {
            result.push(FallbackProject {
                cwd,
                session_count: count,
                last_activity_epoch: latest_ms,
            });
        }
    }
    result
}

fn read_cwd_from_jsonl(path: &PathBuf) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines().take(30).flatten() {
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if let Some(cwd) = v.get("cwd").and_then(Value::as_str) {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

#[derive(Serialize)]
pub struct TranscriptMessage {
    pub role: String,
    pub text: String,
    pub timestamp: Option<String>,
}

#[derive(Serialize)]
pub struct Transcript {
    pub session_id: String,
    pub cwd: Option<String>,
    pub messages: Vec<TranscriptMessage>,
    pub truncated: bool,
}

const MAX_MESSAGES: usize = 2000;

/// message.content は文字列 or ブロック配列（text/tool_use/tool_result等）の両形式がある
fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    b.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn get_transcript(session_id: &str) -> Result<Transcript, String> {
    // パストラバーサル防止: UUID形式以外は拒否
    if !session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("不正なセッションIDです".into());
    }
    let path = find_session_file(session_id)
        .ok_or_else(|| format!("セッション {session_id} の jsonl が見つかりません"))?;

    let file = fs::File::open(&path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(file);

    let mut cwd = None;
    let mut messages = Vec::new();
    let mut truncated = false;

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };

        if cwd.is_none() {
            if let Some(c) = v.get("cwd").and_then(Value::as_str) {
                cwd = Some(c.to_string());
            }
        }

        let msg_type = v.get("type").and_then(Value::as_str).unwrap_or("");
        if msg_type != "user" && msg_type != "assistant" {
            continue;
        }
        // hook結果などの attachment 行や、サブエージェントの sidechain は除外
        if v.get("attachment").is_some()
            || v.get("isSidechain").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(content) = v.pointer("/message/content") else { continue };
        let text = extract_text(content);
        if text.trim().is_empty() {
            continue;
        }

        if messages.len() >= MAX_MESSAGES {
            truncated = true;
            break;
        }
        messages.push(TranscriptMessage {
            role: msg_type.to_string(),
            text,
            timestamp: v
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    Ok(Transcript {
        session_id: session_id.to_string(),
        cwd,
        messages,
        truncated,
    })
}
