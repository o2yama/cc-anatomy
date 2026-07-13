//! フォルダ右クリックメニューのアクション群。
//! 外部アプリ起動（Finder / cmux / Ghostty）と、claude CLI ヘッドレス実行によるタスク抽出。

use std::path::Path;
use std::process::{Command, Stdio};

/// 存在するディレクトリだけを外部アプリに渡す（消えたパスで空ウィンドウを開かない）
fn ensure_dir(path: &str) -> Result<(), String> {
    if Path::new(path).is_dir() {
        Ok(())
    } else {
        Err(format!("ディレクトリが存在しません: {path}"))
    }
}

pub fn open_in_finder(path: &str) -> Result<(), String> {
    ensure_dir(path)?;
    Command::new("open")
        .arg(path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn open_in_cmux(path: &str) -> Result<(), String> {
    ensure_dir(path)?;
    // GUI アプリは zsh の PATH を継承しないため、cmux CLI は app bundle 内の実体を直接叩く
    const CMUX_BIN: &str = "/Applications/cmux.app/Contents/Resources/bin/cmux";
    let bin = if Path::new(CMUX_BIN).exists() {
        CMUX_BIN
    } else {
        "cmux"
    };
    Command::new(bin)
        .arg(path)
        .spawn()
        .map_err(|e| format!("cmux の起動に失敗: {e}"))?;
    Ok(())
}

pub fn open_in_terminal(path: &str) -> Result<(), String> {
    ensure_dir(path)?;
    Command::new("open")
        .args(["-na", "Ghostty", "--args", "--working-directory"])
        .arg(path)
        .spawn()
        .map_err(|e| format!("Ghostty の起動に失敗: {e}"))?;
    Ok(())
}

/// Claude Code と同じ Keychain 資格情報から OAuth アクセストークンを取り出す。
/// トークンは呼び出し元の関数内で完結させ、戻り値・ログに含めない
fn oauth_token() -> Result<String, String> {
    let keychain = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !keychain.status.success() {
        return Err("Keychain から Claude Code の資格情報を取得できません".into());
    }
    let creds: serde_json::Value =
        serde_json::from_slice(&keychain.stdout).map_err(|e| e.to_string())?;
    creds
        .pointer("/claudeAiOauth/accessToken")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or("資格情報に accessToken がありません".into())
}

fn oauth_get(url: &str) -> Result<String, String> {
    let token = oauth_token()?;
    let out = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "10",
            url,
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            "anthropic-beta: oauth-2025-04-20",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("API への接続に失敗しました".into());
    }
    let body = String::from_utf8_lossy(&out.stdout).to_string();
    // トークン失効時などは {"error": ...} が返る。JSONでなければそのままエラー扱い
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "API の応答が不正です".to_string())?;
    if parsed.get("error").is_some() {
        return Err("API がエラーを返しました（再ログインが必要かもしれません）".into());
    }
    Ok(body)
}

/// ログイン中アカウントのレートリミット（5時間枠・7日枠・モデル別・spend）を取得する
pub fn get_rate_limits() -> Result<String, String> {
    oauth_get("https://api.anthropic.com/api/oauth/usage")
}

/// アカウント・組織・プラン情報を取得する
pub fn get_account_profile() -> Result<String, String> {
    oauth_get("https://api.anthropic.com/api/oauth/profile")
}

/// GUI アプリは zsh の PATH（.zshrc）を継承しないため、claude CLI は既知の
/// インストール先から実体を探す
fn resolve_claude_bin() -> Result<std::path::PathBuf, String> {
    let home = crate::db::home_dir();
    let candidates = [
        std::path::PathBuf::from("/opt/homebrew/bin/claude"),
        home.join(".claude/local/claude"),
        home.join(".local/bin/claude"),
        std::path::PathBuf::from("/usr/local/bin/claude"),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| "claude CLI が見つかりません（/opt/homebrew/bin 等を確認）".into())
}

/// claude-mem のサマリー履歴を claude CLI（ヘッドレス）に渡して未完了タスクを抽出する。
/// 抽出は要約作業なので軽量モデル（haiku）で十分。
pub fn extract_tasks(project: &str) -> Result<String, String> {
    let digest = crate::db::task_digest(project)?;
    if digest.trim().is_empty() {
        return Err(format!(
            "「{project}」にはサマリー記録がなく、タスクを抽出できません"
        ));
    }

    let prompt = format!(
        "以下は Claude Code プロジェクト「{project}」の作業サマリー履歴（新しい順）です。\n\
         ここから現時点で未完了と思われるタスクを抽出してください。\n\
         ルール:\n\
         - Markdown のチェックリスト（- [ ]）で出力\n\
         - 後の日付で完了が確認できるものは除外\n\
         - 重複・同種のタスクは1つに統合\n\
         - 各項目の末尾に根拠となる記録の日付を (YYYY-MM-DD) で付ける\n\
         - 優先度が高そうな順に並べる\n\
         - 未完了タスクが見つからなければ「未完了タスクは見つかりませんでした」とだけ出力\n\
         - チェックリスト（または上記の一文）以外の前置き・後書きは書かない\n\n\
         ---\n{digest}"
    );

    let claude = resolve_claude_bin()?;
    let mut child = Command::new(claude)
        .args(["-p", "--model", "haiku"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("claude CLI の起動に失敗: {e}"))?;

    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().ok_or("stdin の取得に失敗")?;
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| e.to_string())?;
    }

    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("claude CLI がエラー終了: {err}"));
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err("claude CLI から出力がありませんでした".into());
    }
    Ok(text)
}
