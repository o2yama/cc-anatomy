//! 環境診断機能。
//! claude CLI をヘッドレス（-p / stream-json）で読み取り専用実行し、
//! ホームディレクトリ全体の改善ポイントをレポートする。
//! 修正の実行はアプリ内では行わず、Terminal.app で claude を対話起動して委ねる
//! （配布アプリ自身はユーザーのファイルに一切書き込まない）。

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};

/// 診断の1指摘。claude に出力させる JSON スキーマと一致させる
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Finding {
    pub id: String,
    pub severity: String, // "high" | "medium" | "low"
    pub category: String,
    pub title: String,
    pub detail: String,
    /// この指摘を修正するための自己完結した指示文（後で claude に渡す）
    pub fix_prompt: String,
    #[serde(default)]
    pub target_paths: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct DiagnosisReport {
    pub summary: String,
    pub findings: Vec<Finding>,
}

/// フロントに流す進捗イベント（diagnosis-progress）
#[derive(serde::Serialize, Clone)]
struct ProgressEvent {
    kind: String, // "tool" | "text" | "info"
    label: String,
}

/// 実行中の claude プロセスの PID。キャンセル用に保持する
static RUNNING_PID: Mutex<Option<u32>> = Mutex::new(None);

/// 暴走防止のタイムアウト（このバージョンの CLI に --max-turns が無いため Rust 側で守る）
const TIMEOUT_SECS: u64 = 15 * 60;

/// 読み取り専用を強制する許可ツール。書き込み系は明示的に禁止する
const ALLOWED_TOOLS: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "Bash(ls:*)",
    "Bash(du:*)",
    "Bash(df:*)",
    "Bash(find:*)",
    "Bash(stat:*)",
    "Bash(file:*)",
    "Bash(wc:*)",
    "Bash(head:*)",
    "Bash(cat:*)",
    // git は読み取り専用のサブコマンドだけ許可する（stash/branch/remote は
    // プレフィックスマッチで書き込み系（stash pop / branch -D 等）まで通るため入れない）
    "Bash(git status:*)",
    "Bash(git log:*)",
    "Bash(git diff:*)",
    "Bash(git stash list:*)",
    "Bash(sw_vers:*)",
    "Bash(uname:*)",
    "Bash(which:*)",
    // echo は許可しない。診断プロセスには選択中アカウントの長期トークンを環境変数で渡すため、
    // `echo $CLAUDE_CODE_OAUTH_TOKEN` が通ると transcript に平文で残る。
    // ホーム配下のファイルを読む以上プロンプトインジェクションの余地があり、診断用途でも不要
];

// Read(...) の拒否は doc_analysis.rs と同じ秘匿パス方針
// （https://code.claude.com/docs/en/permissions のパスルール構文）
const DISALLOWED_TOOLS: &[&str] = &[
    "Write",
    "Edit",
    "NotebookEdit",
    "Task",
    "WebFetch",
    "WebSearch",
    "Read(~/.ssh/**)",
    "Read(~/.aws/**)",
    "Read(~/.claude/.credentials.json)",
    "Read(**/.env)",
    "Read(**/.env.*)",
];

fn diagnosis_prompt(home: &str) -> String {
    format!(
        r#"あなたは macOS の開発環境を監査する専門家です。ホームディレクトリ（{home}）全体を読み取り専用でスキャンし、環境の改善ポイントをレポートしてください。

## 制約（厳守）
- ファイルの作成・変更・削除・移動は絶対に行わない。読み取り系コマンドのみ使用する
- スキャンは効率的に行う。ホーム直下は深さ2まで、サイズ調査は du -sh -d 2 程度に留める
- ~/Library と隠しディレクトリの深掘りはしない（~/.claude や dotfiles の存在確認・軽い読み取りは可）
- 個人情報・資格情報ファイルの中身は読まない（存在と置き場所の妥当性だけ評価する）
- Bash は許可リスト（ls / du / df / find / stat / file / wc / head / cat / git status など先頭一致）のみ実行できる。for / while ループや複合コマンドは拒否されるため使わず、1コマンドずつ実行する

## 監査観点
1. **ホーム直下の散らかり**: 用途不明の野良フォルダ・ファイル、一時作業の残骸
2. **Desktop / Downloads の堆積**: ファイル数・古さ・容量。放置されたインストーラ（.dmg/.pkg）や重複ダウンロード
3. **ディスクの大食い**: 巨大な node_modules / target / venv / キャッシュ、使われていなさそうな大容量ファイル
4. **Git 衛生**: 未コミット変更・未プッシュコミットを抱えたまま放置されたリポジトリ（find でホーム直下2〜3階層の .git を探し、更新が古い順に git status で確認。最大10リポジトリまで）
5. **開発環境の健全性**: 複数バージョンマネージャの残骸、明らかに古い開発ツールの残置など

## 出力形式（厳守）
最後のメッセージでは、前置き・後書き・コードフェンスを一切付けず、次の JSON オブジェクトだけを出力する:

{{
  "summary": "環境全体の健康状態を3〜4文で総評",
  "findings": [
    {{
      "id": "f1",
      "severity": "high",
      "category": "ディスク",
      "title": "50文字以内の見出し",
      "detail": "何が問題で、放置するとどうなるかを2〜3文で。数値（容量・件数・最終更新）を必ず含める",
      "fix_prompt": "この指摘を修正するための自己完結した指示文。絶対パスを含め、この文だけ読めば作業できるように書く。削除は直接せず、ゴミ箱やアーカイブフォルダへの移動を提案する。実行前提の確認事項があれば含める",
      "target_paths": ["/絶対/パス"]
    }}
  ]
}}

- severity は high（すぐ対処すべき）/ medium（近いうちに）/ low（好みの範囲）
- findings は重要度順に最大15件。問題がない観点は含めない
- fix_prompt は破壊的操作（rm）を避け、可逆な操作（移動・アーカイブ）を第一候補にする"#
    )
}

/// GUI アプリのイベントループを塞がないよう、呼び出し側で #[tauri::command(async)] にする
pub fn run_diagnosis(app: &tauri::AppHandle) -> Result<DiagnosisReport, String> {
    // accounts.rs 側のガードとの TOCTOU 対策: アカウント切替・ログイン処理が進行中は spawn しない
    if crate::accounts::ACCOUNT_OP_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("アカウント操作中のため実行できません".into());
    }
    let claude = crate::actions::resolve_claude_bin()?;
    let home = crate::db::home_dir();

    let mut cmd = Command::new(claude);
    cmd.current_dir(&home)
        .args([
            "-p",
            "--model",
            "sonnet",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "dontAsk",
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
        let mut guard = RUNNING_PID.lock().unwrap();
        if guard.is_some() {
            return Err("診断はすでに実行中です".into());
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
            let _ = Command::new("/bin/kill").args(["-9", &pid.to_string()]).status();
        });
    }

    // 後始末を確実にするため、本体をクロージャに包んで結果に関わらずフラグ類を戻す
    let result = (|| {
        {
            let stdin = child.stdin.as_mut().ok_or("stdin の取得に失敗")?;
            stdin
                .write_all(diagnosis_prompt(&home.display().to_string()).as_bytes())
                .map_err(|e| e.to_string())?;
        }
        drop(child.stdin.take());

        // stderr は別スレッドで排出する。stdout だけ読んでいると、claude が
        // stderr のパイプバッファを埋めた時点で相互ブロックのデッドロックになる
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        if let Some(se) = child.stderr.take() {
            let buf = stderr_buf.clone();
            std::thread::spawn(move || {
                use std::io::Read;
                let mut s = String::new();
                let _ = BufReader::new(se).read_to_string(&mut s);
                *buf.lock().unwrap() = s;
            });
        }

        let stdout = child.stdout.take().ok_or("stdout の取得に失敗")?;
        let mut result_text: Option<String> = None;
        let mut result_error: Option<String> = None;

        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|e| e.to_string())?;
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
                            format!("診断が異常終了しました（{}）", ev.get("subtype").and_then(|s| s.as_str()).unwrap_or("不明"))
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
                let err = stderr_buf.lock().unwrap().trim().to_string();
                return Err(format!("claude CLI がエラー終了: {err}"));
            }
            return Err("診断結果が返りませんでした（キャンセルされた可能性があります）".into());
        };

        parse_report(&text)
    })();

    done.store(true, Ordering::Relaxed);
    // キャンセル（cancel_diagnosis が take 済み）や後続 run の PID を潰さないよう条件付きで消す
    {
        let mut guard = RUNNING_PID.lock().unwrap();
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
            // 何をスキャンしているかが伝わる代表的な入力値を1つ拾う
            let input = block
                .get("input")
                .and_then(|i| {
                    i.get("command")
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
            if text.trim().is_empty() || text.trim_start().starts_with('{') {
                return; // 最終 JSON はログに流さない
            }
            ProgressEvent {
                kind: "text".into(),
                label: truncate(text.trim(), 140),
            }
        }
        _ => return,
    };
    let _ = app.emit("diagnosis-progress", ev);
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars).collect();
        format!("{t}…")
    }
}

/// 最終メッセージから JSON オブジェクトを取り出す。
/// 指示に反して前置きやコードフェンスが付いても救えるよう、最初の '{' 〜 最後の '}' を切り出す
fn parse_report(text: &str) -> Result<DiagnosisReport, String> {
    let start = text.find('{').ok_or("診断結果に JSON が含まれていません")?;
    let end = text.rfind('}').ok_or("診断結果の JSON が閉じていません")?;
    serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("診断結果の解析に失敗: {e}"))
}

/// 環境診断が実行中か。アカウント切り替え・追加は、診断が起動する claude -p 子プロセスも
/// ライブ資格情報を消費・書き戻しうるため、実行中はブロックする（accounts::ensure_no_running_sessions）
pub fn is_running() -> bool {
    RUNNING_PID.lock().unwrap().is_some()
}

pub fn cancel_diagnosis() -> Result<(), String> {
    let pid = RUNNING_PID.lock().unwrap().take();
    match pid {
        Some(pid) => {
            Command::new("/bin/kill")
                .args(["-9", &pid.to_string()])
                .status()
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        None => Err("実行中の診断はありません".into()),
    }
}

/// アプリ終了時、実行中の claude 子プロセスを孤児化させないための best-effort kill。
/// cancel_diagnosis と異なり「実行中でなければ何もしない」だけで、失敗もエラーにしない
pub fn kill_running() {
    if let Some(pid) = RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = Command::new("/bin/kill").args(["-9", &pid.to_string()]).status();
    }
}

/// 選択された指摘の fix_prompt を束ねて Terminal.app で claude を対話起動する。
/// acceptEdits + 安全なコマンドの事前許可に留め、削除などの危険操作は
/// ユーザーの目の前で通常の許可プロンプトにかける。
/// 既存の open_in_terminal は Ghostty だが、配布先の環境に依存しないよう
/// ここは AppleScript で制御できる標準の Terminal.app を使う（意図的な差分）
pub fn run_fixes_in_terminal(app: &tauri::AppHandle, prompts: Vec<String>) -> Result<(), String> {
    if prompts.is_empty() {
        return Err("修正項目が選択されていません".into());
    }
    let claude = crate::actions::resolve_claude_bin()?;
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("diagnosis");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let combined = format!(
        "PC環境診断の結果、以下の改善タスクの実行が承認されました。上から順に1つずつ実行してください。\n\
         各タスクの完了時に何をしたかを1行で報告し、すべて終わったら全体のまとめを出してください。\n\
         ファイルの削除（rm）は行わず、指示にある通り移動・アーカイブで対応してください。\n\n{}",
        prompts
            .iter()
            .enumerate()
            .map(|(i, p)| format!("## タスク{}\n{}", i + 1, p))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    let prompt_path = dir.join("fix-prompt.md");
    std::fs::write(&prompt_path, &combined).map_err(|e| e.to_string())?;

    // osascript の引用符地獄を避けるため、実行内容はシェルスクリプトに落としてパスだけ渡す
    let script_path = dir.join("run-fixes.sh");
    let script = format!(
        "#!/bin/zsh\ncd \"$HOME\"\nexec \"{}\" --permission-mode acceptEdits \\\n  --allowedTools \"Bash(mv:*)\" \"Bash(mkdir:*)\" \"Bash(ls:*)\" \"Bash(du:*)\" \"Bash(git:*)\" \\\n  \"$(cat '{}')\"\n",
        claude.display(),
        prompt_path.display()
    );
    std::fs::write(&script_path, script).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }

    let status = Command::new("osascript")
        .args([
            "-e",
            // パスにスペースが含まれてもシェルで分割されないよう単一引用符で包む
            &format!(
                "tell application \"Terminal\" to do script \"'{}'\"",
                script_path.display()
            ),
            "-e",
            "tell application \"Terminal\" to activate",
        ])
        .status()
        .map_err(|e| format!("Terminal の起動に失敗: {e}"))?;
    if !status.success() {
        return Err("Terminal の起動に失敗しました（オートメーション権限を確認してください）".into());
    }
    Ok(())
}
