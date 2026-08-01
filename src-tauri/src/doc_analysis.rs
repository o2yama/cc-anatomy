//! ドキュメントエディタ用の AI 分析機能。
//! claude CLI をヘッドレス（-p / stream-json）で読み取り専用実行し、
//! 編集中のドキュメント（未保存分含む）とプロジェクトの実態を突き合わせて改善提案を生成する。
//! 診断機能（diagnostics.rs）と実行パターンは同一だが、対象・プロンプト・多重実行ガードは独立させる。

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// フロントに流す進捗イベント（doc-analysis-progress）。diagnosis-progress と同型
#[derive(serde::Serialize, Clone)]
struct ProgressEvent {
    kind: String, // "tool" | "text" | "info"
    label: String,
}

/// 実行中の claude プロセスの PID。診断機能とは別の多重実行ガードにする
static RUNNING_PID: Mutex<Option<u32>> = Mutex::new(None);

const TIMEOUT_SECS: u64 = 600;

/// プロンプトに載せるドキュメント本文の上限文字数。これを超える巨大ファイルは
/// stdin 経由で送っても claude 側の実用的な分析にならないため事前に弾く
const MAX_CONTENT_CHARS: usize = 100_000;

/// accounts.rs の ensure_app_not_busy から多重実行チェックに使われる
pub fn is_running() -> bool {
    RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// 読み取り専用に限定する許可ツール。ドキュメント記載の公式URLだけ WebFetch を許可する
const ALLOWED_TOOLS: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "WebFetch(domain:code.claude.com)",
    "WebFetch(domain:docs.anthropic.com)",
];

/// 変更系・任意実行系は全面禁止する。加えて、許可された Read/Glob/Grep 経由でも
/// 秘匿情報のパスには到達できないよう明示的に拒否する
/// （https://code.claude.com/docs/en/permissions のパスルール構文）
const DISALLOWED_TOOLS: &[&str] = &[
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Bash",
    "Task",
    "WebSearch",
    "Read(~/.ssh/**)",
    "Read(~/.aws/**)",
    "Read(~/.claude/.credentials.json)",
    "Read(**/.env)",
    "Read(**/.env.*)",
];

/// path の種別から、提案前に必ず参照させる公式ドキュメント URL を決める
fn reference_doc_url(path: &str) -> &'static str {
    let basename = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if basename == "CLAUDE.md" {
        "https://code.claude.com/docs/en/memory"
    } else if basename == "SKILL.md" {
        "https://code.claude.com/docs/en/skills"
    } else if path.contains("/agents/") && basename.ends_with(".md") {
        "https://code.claude.com/docs/en/sub-agents"
    } else if basename == "settings.json" || basename == "settings.local.json" {
        "https://code.claude.com/docs/en/settings"
    } else {
        "https://code.claude.com/docs/en/memory"
    }
}

/// project_dir が None（＝グローバルスコープの文書。プロジェクトに紐づかない
/// rules/memory 等）の場合は、実態との乖離チェックの前提となる「プロジェクトの中身」が
/// 存在しないため、文書単体の分析＋公式ドキュメント照合だけを求めるプロンプトに出し分ける
fn analysis_prompt(path: &str, content: &str, project_dir: Option<&str>) -> String {
    let doc_url = reference_doc_url(path);

    let context_line = match project_dir {
        Some(dir) => format!("- プロジェクトルート: {dir}\n"),
        None => String::new(),
    };

    let procedure = match project_dir {
        Some(dir) => format!(
            r#"1. まず WebFetch で {doc_url} を取得し、公式仕様・ベストプラクティスを把握する
2. Read / Glob / Grep でプロジェクトディレクトリ（{dir}）の中身（コード構成・他の設定ファイル）を確認し、
   ドキュメントの記述内容が実態と乖離していないか（存在しないコマンド・古いファイル名・実装されていない方針など）を確認する
3. 上記を踏まえて改善提案をまとめる"#
        ),
        None => format!(
            r#"1. まず WebFetch で {doc_url} を取得し、公式仕様・ベストプラクティスを把握する
2. このドキュメントは特定のプロジェクトに紐づかないグローバル設定のため、実装ファイルとの
   突き合わせは行わず、文書単体の記述の一貫性・明確さ・公式仕様との整合のみを確認する
3. 上記を踏まえて改善提案をまとめる"#
        ),
    };

    let diff_section = match project_dir {
        Some(_) => format!(
            "（{doc_url} の内容と比較して、不足・矛盾している点があれば箇条書き。無ければ「特になし」）"
        ),
        None => format!(
            "（{doc_url} の内容と比較して、不足・矛盾している点があれば箇条書き。無ければ「特になし」。\
            プロジェクト実装との突き合わせは対象外）"
        ),
    };

    format!(
        r#"あなたは Claude Code の設定ドキュメントをレビューする専門家です。以下のドキュメントを分析し、改善提案をまとめてください。

## 対象ドキュメント
- パス: {path}
{context_line}
## 対象ドキュメントの現在の内容（エディタ上の未保存の編集を含む。ディスク上のファイルではなく、こちらを正として扱うこと）
```
{content}
```

## 手順（厳守）
{procedure}

## 制約
- ファイルの作成・変更・削除は一切行わない（読み取りと WebFetch のみ）
- APIキー・トークン等の秘匿値を提案文に含めない
- 前置き・後書きは書かない。最後のメッセージに以下の見出し構成の markdown だけを出力する

## 出力形式（厳守・markdown）
## 総評
（3〜4文でドキュメント全体の完成度・健全性を評価）

## 良い点
（箇条書き）

## 改善提案
（優先度が高い順に箇条書き。各項目に理由と具体的な修正例を含める）

## 公式ドキュメントとの差分
{diff_section}"#
    )
}

/// GUI アプリのイベントループを塞がないよう、呼び出し側で #[tauri::command(async)] にする
pub fn analyze_doc(
    app: &tauri::AppHandle,
    path: String,
    content: String,
    project_dir: Option<String>,
) -> Result<String, String> {
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Err("ドキュメントが大きすぎるため分析できません".into());
    }
    // accounts.rs 側のガードとの TOCTOU 対策: アカウント切替・ログイン処理が進行中は
    // spawn しない（アカウント操作側は ensure_app_not_busy でこちらの is_running を見ている）
    if crate::accounts::ACCOUNT_OP_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("アカウント操作中のため実行できません".into());
    }

    let claude = crate::actions::resolve_claude_bin()?;
    // has_project は「呼び出し側が明示的にプロジェクトルートを渡したか」をプロンプト側の
    // 出し分けに使う。グローバルスコープの文書（rules 等）では None のまま渡ってくる想定で、
    // cwd 自体はどちらにせよファイルの親ディレクトリにフォールバックさせて claude を起動する
    let has_project = project_dir.is_some();
    let cwd = project_dir.unwrap_or_else(|| {
        Path::new(&path)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| crate::db::home_dir().display().to_string())
    });
    let prompt = analysis_prompt(&path, &content, has_project.then(|| cwd.as_str()));

    let mut cmd = Command::new(claude);
    cmd.current_dir(&cwd)
        .args([
            "-p",
            "--model",
            "sonnet",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "dontAsk",
            "--max-turns",
            "40",
            // cwd プロジェクトの settings/hooks/MCP を読ませない（読み取り専用の分析実行に
            // 無関係な自動実行・追加ツールが混ざるのを防ぐ）
            "--setting-sources",
            "user",
            "--strict-mcp-config",
            // ツール面自体を絞る（--allowedTools は許可リストだが、こちらは
            // ビルトインツール群からそもそも利用可能な集合を制限する）
            "--tools",
            "Read,Glob,Grep,WebFetch",
        ])
        .arg("--allowedTools")
        .args(ALLOWED_TOOLS)
        .arg("--disallowedTools")
        .args(DISALLOWED_TOOLS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // 多重実行ガード。チェックと登録の間に別スレッドが割り込まないよう、
    // ロックを保持したまま spawn まで済ませる
    let (mut child, pid) = {
        let mut guard = RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Err("ALREADY_RUNNING:AI分析はすでに実行中です".into());
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("claude CLI の起動に失敗: {e}"))?;
        let pid = child.id();
        *guard = Some(pid);
        (child, pid)
    };

    // タイムアウト監視。完了フラグが立たないまま時間切れになったら kill する
    let done = Arc::new(AtomicBool::new(false));
    {
        let done = done.clone();
        std::thread::spawn(move || {
            for _ in 0..TIMEOUT_SECS {
                if done.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            // ループを抜けた直後にもう一度確認する。ここでチェックせず kill すると、
            // ちょうどタイムアウト直前に完了して PID が RUNNING_PID から外れ、
            // OS に再利用された無関係なプロセスを誤って kill する恐れがある
            if done.load(Ordering::Relaxed) {
                return;
            }
            let _ = Command::new("/bin/kill").args(["-9", &pid.to_string()]).status();
        });
    }

    // stdout/stderr の読み取り体制を先に整えてから stdin へ書き込む。
    // 逆順（書き込み→読み取り開始）だと、プロンプトが大きい場合に stdin バッファが
    // 埋まって claude 側の書き込みをブロック→親の read 開始前で相互デッドロックしうる。
    // stdin の書き込みは専用スレッドに逃がし、親スレッドの read 開始をブロックしないようにする
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    if let Some(se) = child.stderr.take() {
        let buf = stderr_buf.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut s = String::new();
            let _ = BufReader::new(se).read_to_string(&mut s);
            *buf.lock().unwrap_or_else(|e| e.into_inner()) = s;
        });
    }
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            // プロンプトは最大20万文字程度になりうる。write_all は書き切るまでブロックしうるが、
            // 専用スレッドなので親スレッドの stdout/stderr 読み取りを妨げない
            let _ = stdin.write_all(prompt.as_bytes());
            drop(stdin);
        });
    }

    let result = (|| {
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("stdout の取得に失敗".to_string());
            }
        };

        let mut result_text: Option<String> = None;
        let mut result_error: Option<String> = None;

        for line in BufReader::new(stdout).lines() {
            // 行の読み取り自体が失敗しても child.wait() には必ず到達させたいので、
            // ここで早期 return せずエラーを記録してループを抜けるだけにする
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    result_error = Some(e.to_string());
                    break;
                }
            };
            let Ok(ev) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match ev.get("type").and_then(|t| t.as_str()) {
                Some("assistant") => {
                    for block in ev
                        .pointer("/message/content")
                        .and_then(|c| c.as_array())
                        .into_iter()
                        .flatten()
                    {
                        emit_progress(app, block);
                    }
                }
                Some("result") => {
                    let ok = ev.get("subtype").and_then(|s| s.as_str()) == Some("success");
                    let text = ev
                        .get("result")
                        .and_then(|r| r.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if ok {
                        result_text = Some(text);
                    } else {
                        result_error = Some(if text.is_empty() {
                            format!(
                                "分析が異常終了しました（{}）",
                                ev.get("subtype").and_then(|s| s.as_str()).unwrap_or("不明")
                            )
                        } else {
                            text
                        });
                    }
                }
                _ => {}
            }
        }

        let status = child.wait().map_err(|e| e.to_string())?;
        if let Some(err) = result_error {
            return Err(err);
        }
        let Some(text) = result_text else {
            if !status.success() {
                let err = stderr_buf.lock().unwrap_or_else(|e| e.into_inner()).trim().to_string();
                return Err(format!("claude CLI がエラー終了: {err}"));
            }
            return Err("分析結果が返りませんでした（キャンセルされた可能性があります）".into());
        };

        Ok(text)
    })();

    done.store(true, Ordering::Relaxed);
    {
        let mut guard = RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner());
        if *guard == Some(pid) {
            *guard = None;
        }
    }
    result
}

/// assistant メッセージの content ブロックから進捗表示用のラベルを作って emit する
fn emit_progress(app: &tauri::AppHandle, block: &serde_json::Value) {
    let ev = match block.get("type").and_then(|t| t.as_str()) {
        Some("tool_use") => {
            let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let input = block
                .get("input")
                .and_then(|i| {
                    i.get("url")
                        .or_else(|| i.get("path"))
                        .or_else(|| i.get("file_path"))
                        .or_else(|| i.get("pattern"))
                })
                .and_then(|v| v.as_str())
                .unwrap_or("");
            ProgressEvent {
                kind: "tool".into(),
                label: format!("{name}: {}", truncate(input, 100)),
            }
        }
        Some("text") => {
            let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if text.trim().is_empty() {
                return;
            }
            ProgressEvent {
                kind: "text".into(),
                label: truncate(text.trim(), 140),
            }
        }
        _ => return,
    };
    let _ = app.emit("doc-analysis-progress", ev);
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars).collect();
        format!("{t}…")
    }
}

pub fn cancel_doc_analysis() -> Result<(), String> {
    let pid = RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner()).take();
    match pid {
        Some(pid) => {
            Command::new("/bin/kill")
                .args(["-9", &pid.to_string()])
                .status()
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        None => Err("実行中のAI分析はありません".into()),
    }
}

/// アプリ終了時、実行中の claude 子プロセスを孤児化させないための best-effort kill。
/// cancel_doc_analysis と異なり「実行中でなければ何もしない」だけで、失敗もエラーにしない
pub fn kill_running() {
    if let Some(pid) = RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = Command::new("/bin/kill").args(["-9", &pid.to_string()]).status();
    }
}
