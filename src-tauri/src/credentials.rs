//! Claude Code のライブ（ログイン中）資格情報の取得。
//! 保存場所は OS で異なる（公式仕様）: macOS は Keychain の `Claude Code-credentials`、
//! Windows / Linux は `~/.claude/.credentials.json`（`CLAUDE_CONFIG_DIR` 設定時はその配下）。
//! トークンは呼び出し元の関数内で完結させ、戻り値以外のログ・エラー文言に含めないこと。

fn extract_access_token(creds: &serde_json::Value) -> Result<String, String> {
    creds
        .pointer("/claudeAiOauth/accessToken")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| "資格情報に accessToken がありません".to_string())
}

/// access token の有効期限（epoch ミリ秒）。旧形式の資格情報等で持たないこともあるため
/// Option で返す（呼び出し側は「不明」を「期限切れではない」として扱う＝素通しする）
fn extract_expires_at(creds: &serde_json::Value) -> Option<i64> {
    creds.pointer("/claudeAiOauth/expiresAt").and_then(|v| v.as_i64())
}

#[cfg(target_os = "macos")]
fn read_live_credentials() -> Result<serde_json::Value, String> {
    let keychain = std::process::Command::new("security")
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
    serde_json::from_slice(&keychain.stdout).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
fn read_live_credentials() -> Result<serde_json::Value, String> {
    let dir = std::env::var("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| crate::db::home_dir().join(".claude"));
    let path = dir.join(".credentials.json");
    let data = std::fs::read(&path).map_err(|_| {
        format!(
            "Claude Code の資格情報が見つかりません（{}）。Claude Code で一度ログインすると作成されます",
            path.display()
        )
    })?;
    serde_json::from_slice(&data).map_err(|_| "Claude Code の資格情報を読み取れませんでした".to_string())
}

/// access token と有効期限（epoch ミリ秒、取れなければ None）をまとめて返す。
/// 期限切れかどうかを API を呼ばずに事前判定するためのもの（refresh は一切行わない）
pub fn live_token_with_expiry() -> Result<(String, Option<i64>), String> {
    let creds = read_live_credentials()?;
    let token = extract_access_token(&creds)?;
    Ok((token, extract_expires_at(&creds)))
}
