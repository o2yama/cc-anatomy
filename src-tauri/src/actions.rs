//! フォルダ右クリックメニューのアクション群と Anthropic API 呼び出し。
//! 外部アプリ起動（Finder / cmux / Ghostty）とタスク抽出は macOS 限定機能。
//! 使用量取得（レート制限ヘッダ経由）は全プラットフォーム共通。

#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

/// macOS 限定コマンドを非対応 OS で呼ばれたときの応答。
/// フロントの UI 出し分けが第一防衛で、これは取りこぼし時の安全網
#[cfg(not(target_os = "macos"))]
pub const MAC_ONLY: &str = "この機能は macOS 版でのみ利用できます";

/// 存在するディレクトリだけを外部アプリに渡す（消えたパスで空ウィンドウを開かない）
#[cfg(target_os = "macos")]
fn ensure_dir(path: &str) -> Result<(), String> {
    if Path::new(path).is_dir() {
        Ok(())
    } else {
        Err(format!("ディレクトリが存在しません: {path}"))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn open_in_finder(_path: &str) -> Result<(), String> {
    Err(MAC_ONLY.into())
}

#[cfg(not(target_os = "macos"))]
pub fn open_in_cmux(_path: &str) -> Result<(), String> {
    Err(MAC_ONLY.into())
}

#[cfg(not(target_os = "macos"))]
pub fn open_in_terminal(_path: &str) -> Result<(), String> {
    Err(MAC_ONLY.into())
}

#[cfg(not(target_os = "macos"))]
pub fn extract_tasks(_project: &str) -> Result<String, String> {
    Err(MAC_ONLY.into())
}

#[cfg(target_os = "macos")]
pub fn open_in_finder(path: &str) -> Result<(), String> {
    ensure_dir(path)?;
    Command::new("open")
        .arg(path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn open_in_cmux(path: &str) -> Result<(), String> {
    ensure_dir(path)?;
    // GUI アプリは zsh の PATH を継承しないため、cmux CLI は app bundle 内の実体を直接叩く
    const CMUX_BIN: &str = "/Applications/cmux.app/Contents/Resources/bin/cmux";
    let bin = if Path::new(CMUX_BIN).exists() {
        CMUX_BIN
    } else {
        "cmux"
    };
    // cmux が interactive zsh を介さず claude を起動する場合、.zshrc のシェル連携が
    // 効かず選択アカウントが無視される。ここで直接渡して取りこぼしを防ぐ
    Command::new(bin)
        .envs(crate::accounts::claude_env())
        .arg(path)
        .spawn()
        .map_err(|e| format!("cmux の起動に失敗: {e}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn open_in_terminal(path: &str) -> Result<(), String> {
    ensure_dir(path)?;
    Command::new("open")
        .args(["-na", "Ghostty", "--args", "--working-directory"])
        .arg(path)
        .spawn()
        .map_err(|e| format!("Ghostty の起動に失敗: {e}"))?;
    Ok(())
}

/// Claude Code がログインに使っているライブ資格情報のアクセストークン（保存場所は OS 別）
fn live_keychain_token() -> Result<String, String> {
    crate::credentials::live_token()
}

/// 使用量ポーリングで繰り返し呼ぶため、接続を使い回す共有クライアント。
/// トークンは Authorization ヘッダで送る（argv に載らないので `ps` から見えない）
static HTTP: std::sync::LazyLock<reqwest::blocking::Client> = std::sync::LazyLock::new(|| {
    // rustls-no-provider 構成ではプロセスに crypto provider を明示登録する必要がある。
    // updater 等が先に登録済みならエラーになるだけなので無視してよい
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("HTTP クライアントの初期化に失敗")
});

/// 選択中アカウントのトークンで叩き、失敗したらライブ資格情報で再試行する。
///
/// setup-token 由来の長期トークンを usage/profile API が受け付けるかは非公開仕様のため
/// 保証がない。弾かれた場合にアカウント登録しただけで使用量表示が全部落ちるのを避ける
/// （表示対象がどのアカウントかはアカウント画面で確認できる）
fn oauth_get(url: &str) -> Result<String, String> {
    let active = crate::accounts::active_token();
    let token = match &active {
        Some(t) => t.clone(),
        None => live_keychain_token()?,
    };
    match oauth_get_with_token(&token, url) {
        Ok(body) => Ok(body),
        Err(e) if active.is_some() => {
            let fallback = live_keychain_token().map_err(|_| e)?;
            oauth_get_with_token(&fallback, url)
        }
        Err(e) => Err(e),
    }
}

pub fn oauth_get_with_token(token: &str, url: &str) -> Result<String, String> {
    let resp = HTTP
        .get(url)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|_| "API への接続に失敗しました".to_string())?;
    let body = resp.text().map_err(|e| e.to_string())?;
    // トークン失効時などは {"error": ...} が返る。JSONでなければそのままエラー扱い
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "API の応答が不正です".to_string())?;
    if parsed.get("error").is_some() {
        return Err("API がエラーを返しました（再ログインが必要かもしれません）".into());
    }
    Ok(body)
}

/// 推論 API に最小リクエストを投げ、レスポンスヘッダを返す。
///
/// **トークンの検証も使用量取得も、この経路でしかできない**:
/// - claude CLI は無効な `CLAUDE_CODE_OAUTH_TOKEN` を黙って握り潰し、Keychain の資格情報に
///   フォールバックして成功する（v2.1.207 実機確認）ため、CLI 経由では生死を判定できない
/// - `oauth/usage` `oauth/profile` は長期トークンのスコープ外で拒否される
///
/// ヘッダには `anthropic-organization-id`（どのアカウントに課金されたか）と
/// `anthropic-ratelimit-unified-*`（そのアカウントの使用率）が入っている
fn probe_headers(
    token: &str,
) -> Result<(u16, reqwest::header::HeaderMap, Option<String>), String> {
    let body = r#"{"model":"claude-haiku-4-5-20251001","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#;
    let resp = HTTP
        .post("https://api.anthropic.com/v1/messages")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .body(body)
        .send()
        .map_err(|_| "API に接続できませんでした".to_string())?;

    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp.text().unwrap_or_default();
    let error_type = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.pointer("/error/type").and_then(|t| t.as_str()).map(String::from));
    Ok((status, headers, error_type))
}

fn header_value(headers: &reqwest::header::HeaderMap, key: &str) -> Option<String> {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
}

pub enum TokenCheck {
    /// 使える。課金先の organization id を持つ（長期トークンではメールを引けないため、
    /// アカウントの同一性判定はこの id で行う）
    Valid(String),
    /// 認証そのものが拒否された。トークンが壊れているか失効している
    Invalid,
    /// 認証は問題ないが今は確認できない（レート上限・ネットワーク断・サーバ側エラー）
    Unavailable(String),
}

/// トークンの生死を確かめる。
///
/// **レート上限を「無効」と混同しないこと**。枠を使い切ったアカウントは 429 を返すが
/// トークン自体は有効で、これを無効扱いにして消すと利用者の1年トークンを破壊する
/// （実際に破壊した）。削除してよいのは 401 のときだけ
pub fn check_oauth_token(token: &str) -> TokenCheck {
    let (status, headers, error_type) = match probe_headers(token) {
        Ok(v) => v,
        Err(e) => return TokenCheck::Unavailable(e),
    };
    if status == 401 || error_type.as_deref() == Some("authentication_error") {
        return TokenCheck::Invalid;
    }
    match header_value(&headers, "anthropic-organization-id") {
        Some(org) => TokenCheck::Valid(org),
        None => TokenCheck::Unavailable(format!(
            "アカウントを特定できませんでした（HTTP {status}）"
        )),
    }
}

/// 推論 API のレスポンスヘッダから 5時間枠・7日枠の使用率を組み立てる。
///
/// 長期トークンは `oauth/usage` のスコープを持たない（同一アカウント・同時刻で live は受理、
/// 長期トークンは拒否されることを実測）。一方 `/v1/messages` の
/// `anthropic-ratelimit-unified-*` ヘッダは**そのトークンのアカウントの**使用率を返すので、
/// 選択中アカウントの使用量を正しく表示するにはこちらを使うしかない
fn rate_limits_from_headers(token: &str) -> Result<String, String> {
    // 429（枠を使い切った状態）でもレート制限ヘッダは返るので、ステータスでは弾かない
    let (_, headers, _) = probe_headers(token)?;

    // 使用率はヘッダでは 0〜1、usage API では 0〜100 で返る。UI は後者に合わせてある
    let window = |prefix: &str| -> serde_json::Value {
        let utilization = header_value(
            &headers,
            &format!("anthropic-ratelimit-unified-{prefix}-utilization"),
        )
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v * 100.0);
        let resets_at = header_value(
            &headers,
            &format!("anthropic-ratelimit-unified-{prefix}-reset"),
        )
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(epoch_to_iso);
        serde_json::json!({ "utilization": utilization, "resets_at": resets_at })
    };

    if header_value(&headers, "anthropic-ratelimit-unified-5h-utilization").is_none() {
        return Err("選択中アカウントの使用量を取得できませんでした".into());
    }

    // UI は usage.limits 配列を描画する（five_hour/seven_day は使っていない）ので、
    // ヘッダの 5h/7d 枠を limits エントリ（kind: session / weekly_all）に対応づける
    let entry = |prefix: &str, kind: &str| -> serde_json::Value {
        let percent = header_value(
            &headers,
            &format!("anthropic-ratelimit-unified-{prefix}-utilization"),
        )
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v * 100.0);
        let resets_at = header_value(
            &headers,
            &format!("anthropic-ratelimit-unified-{prefix}-reset"),
        )
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(epoch_to_iso);
        // status: allowed / allowed_warning / rejected。UI は "normal" を正常として隠すので合わせる
        let severity = header_value(
            &headers,
            &format!("anthropic-ratelimit-unified-{prefix}-status"),
        )
        .map(|s| match s.as_str() {
            "allowed" => "normal".to_string(),
            "allowed_warning" => "警告".to_string(),
            "rejected" => "上限到達".to_string(),
            other => other.to_string(),
        });
        serde_json::json!({
            "kind": kind,
            "group": null,
            "percent": percent,
            "severity": severity,
            "resets_at": resets_at,
            "scope": null,
        })
    };

    Ok(serde_json::json!({
        "five_hour": window("5h"),
        "seven_day": window("7d"),
        "limits": [entry("5h", "session"), entry("7d", "weekly_all")],
    })
    .to_string())
}

/// メニューバー向けの使用量サマリー。使用率は 0〜100、リセットは epoch 秒
pub struct UsageSummary {
    pub five_pct: f64,
    pub seven_pct: f64,
    pub five_reset: Option<i64>,
    pub seven_reset: Option<i64>,
}

/// 現在ログイン中（ライブ）アカウントの使用率。登録されていないアカウントで
/// ログインしている場合でも、ライブ Keychain のトークンから直接取れる
pub fn live_usage_summary() -> Result<UsageSummary, String> {
    let token = live_keychain_token()?;
    usage_summary(&token)
}

/// メニューバーのアカウント一覧向けに、5時間枠・7日枠の使用率とリセット時刻を返す。
/// 枠を使い切ったアカウント（429）でもヘッダは返るので値は取れる
pub fn usage_summary(token: &str) -> Result<UsageSummary, String> {
    let (_, headers, _) = probe_headers(token)?;
    let pct = |prefix: &str| -> Option<f64> {
        header_value(
            &headers,
            &format!("anthropic-ratelimit-unified-{prefix}-utilization"),
        )
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v * 100.0)
    };
    let reset = |prefix: &str| -> Option<i64> {
        header_value(
            &headers,
            &format!("anthropic-ratelimit-unified-{prefix}-reset"),
        )
        .and_then(|v| v.parse::<i64>().ok())
    };
    let five_pct = pct("5h").ok_or("使用量を取得できませんでした")?;
    Ok(UsageSummary {
        five_pct,
        seven_pct: pct("7d").unwrap_or(0.0),
        five_reset: reset("5h"),
        seven_reset: reset("7d"),
    })
}

fn epoch_to_iso(secs: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// 指定トークンのレートリミットを UI 用の JSON 値で返す（カルーセル表示の1アカウント分）
pub fn rate_limits_value(token: &str) -> Result<serde_json::Value, String> {
    let s = rate_limits_from_headers(token)?;
    serde_json::from_str(&s).map_err(|e| e.to_string())
}

/// 表示対象アカウントのレートリミットを取得する
pub fn get_rate_limits() -> Result<String, String> {
    match crate::accounts::active_token() {
        Some(token) => rate_limits_from_headers(&token),
        None => oauth_get("https://api.anthropic.com/api/oauth/usage"),
    }
}

/// アカウント・組織・プラン情報を取得する。
///
/// 長期トークンは profile API のスコープ（user:profile）を持たないため素性を引けない。
/// 代わりに、追加/バックフィル時に organization id 経由で解決して保存済みの
/// メール・プランを返す（ライブ資格情報にフォールバックすると選択中と違うアカウントの
/// メールを表示して「どちらを消費しているか」を誤認させる）
pub fn get_account_profile() -> Result<String, String> {
    match crate::accounts::active_display() {
        Some((name, email, plan)) => {
            let email = if email.is_empty() {
                "長期トークン認証のためメールは取得できません".to_string()
            } else {
                email
            };
            // planLabel は has_claude_max/pro からも "Max"/"Pro" を出せる。
            // organization_type も入れておき、表示側のどの経路でも拾えるようにする
            Ok(serde_json::json!({
                "account": {
                    "display_name": name,
                    "email": email,
                    "has_claude_max": plan == "claude_max",
                    "has_claude_pro": plan == "claude_pro",
                },
                "organization": {
                    "organization_type": if plan.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(plan) },
                    "subscription_status": "active",
                },
            })
            .to_string())
        }
        None => oauth_get("https://api.anthropic.com/api/oauth/profile"),
    }
}

#[cfg(test)]
mod tests {
    /// 実環境の資格情報とネットワークを使う e2e 検証。
    /// curl → reqwest 置き換え後も、ライブ資格情報 → /v1/messages のレート制限ヘッダ →
    /// 使用率組み立ての経路が実際に通ることを確かめる。
    /// CI では資格情報が無いため ignore とし、手元で `cargo test -- --ignored` で実行する
    #[test]
    #[ignore]
    fn live_usage_e2e() {
        let u = super::live_usage_summary().expect("ライブ使用量の取得に失敗");
        assert!((0.0..=100.0).contains(&u.five_pct), "5h 使用率が範囲外: {}", u.five_pct);
        assert!(u.five_reset.is_some(), "5h リセット時刻が取れていない");
    }
}

/// GUI アプリは zsh の PATH（.zshrc）を継承しないため、claude CLI は既知の
/// インストール先から実体を探す
#[cfg(target_os = "macos")]
pub fn resolve_claude_bin() -> Result<std::path::PathBuf, String> {
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
#[cfg(target_os = "macos")]
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
        .envs(crate::accounts::claude_env())
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
