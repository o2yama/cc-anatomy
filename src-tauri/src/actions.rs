//! フォルダ右クリックメニューのアクション群と Anthropic API 呼び出し。
//! 外部アプリ起動（Finder / cmux / Ghostty）とタスク抽出は macOS 限定機能。
//! 使用量取得は全プラットフォーム共通。
//!
//! 2026-07-25 ユーザー決定で「監視用長期トークン」による複数アカウント使用率監視を全廃した。
//! 使用量・プロフィールは常にライブ資格情報（Claude Code がログイン中のアカウント）の
//! access token で `/api/oauth/usage` `/api/oauth/profile` を直接叩く一本道になっている。

#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

/// macOS 限定コマンドを非対応 OS で呼ばれたときの応答。
/// フロントの UI 出し分けが第一防衛で、これは取りこぼし時の安全網
#[cfg(not(target_os = "macos"))]
pub const MAC_ONLY: &str = "この機能は macOS 版でのみ利用できます";

/// 本アプリ自身が起動する claude -p 子プロセス（タスク抽出）が実行中か。
/// `ps` ベースの running_sessions() はシェルの子しか数えないため、これも別途持っておき、
/// アカウント切り替え・追加のブロック条件に含める（accounts::ensure_no_running_sessions）
static EXTRACT_TASKS_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_agent_busy() -> bool {
    EXTRACT_TASKS_RUNNING.load(std::sync::atomic::Ordering::Relaxed)
}

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
    // アカウント切り替えは Keychain スワップでライブ資格情報自体を書き換えるため、
    // cmux が起動する claude も素の呼び出しで正しいアカウントを拾える
    Command::new(bin)
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

/// ライブ資格情報の access token で叩く。監視用長期トークンは全廃したため経路はこれ一本
fn oauth_get(url: &str) -> Result<String, String> {
    let token = live_keychain_token()?;
    oauth_get_with_token(&token, url)
}

/// `reqwest::blocking` は内部に自前の tokio ランタイムを持つ。これを tokio ランタイム配下
/// （`#[tauri::command(async)]` が自動で使う `spawn_blocking` のスレッドを含む）から直接呼ぶと、
/// そのスレッドが元のランタイムの文脈（thread-local の Handle）を保持したままのため、
/// reqwest 側の内部ランタイムを drop する際に
/// 「Cannot drop a runtime in a context where blocking is not allowed」でパニックする
/// （実機で `switch_account` 押下時に発生・確認済み）。
/// `tokio::task::spawn_blocking` も同じブロッキングプール＝ランタイム配下なので効果が無く、
/// ランタイムの文脈を一切持たない素の OS スレッドで実行する必要がある
pub fn oauth_get_with_token(token: &str, url: &str) -> Result<String, String> {
    let token = token.to_string();
    let url = url.to_string();
    std::thread::spawn(move || oauth_get_with_token_blocking(&token, &url))
        .join()
        .map_err(|_| "API 呼び出し中に内部エラーが発生しました".to_string())?
}

fn oauth_get_with_token_blocking(token: &str, url: &str) -> Result<String, String> {
    let resp = HTTP
        .get(url)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|_| "API への接続に失敗しました".to_string())?;
    // access token の期限切れは正常な状態（Claude Code が次回利用時に自動 refresh する）。
    // 「再ログインが必要」という誤った不安を与えないよう専用の文言にする
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("取得できませんでした（Claude Code を一度使うと更新されます）".into());
    }
    let body = resp.text().map_err(|e| e.to_string())?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "API の応答が不正です".to_string())?;
    if parsed.get("error").is_some() {
        return Err("API がエラーを返しました（再ログインが必要かもしれません）".into());
    }
    Ok(body)
}

/// `/api/oauth/usage` のエンドポイント。ライブアカウントの使用量表示（actions.rs）と
/// 登録済み全アカウントの使用率一括取得（accounts::get_accounts_usage）の両方から使うため公開する
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// メニューバー向けの使用量サマリー。使用率は 0〜100、リセットは epoch 秒
pub struct UsageSummary {
    pub five_pct: f64,
    pub seven_pct: f64,
    pub five_reset: Option<i64>,
    pub seven_reset: Option<i64>,
}

fn iso_to_epoch(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
}

/// `/api/oauth/usage` の応答本文を UsageSummary へ変換する。ライブアカウント・登録済み他
/// アカウント問わず同じ形の応答なので、accounts::get_accounts_usage からも共有して使う
pub fn parse_usage_body(body: &str) -> Result<UsageSummary, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "使用量の応答が不正です".to_string())?;
    let five_pct = v
        .pointer("/five_hour/utilization")
        .and_then(|x| x.as_f64())
        .ok_or("使用量を取得できませんでした")?;
    let seven_pct = v
        .pointer("/seven_day/utilization")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0);
    let five_reset = v
        .pointer("/five_hour/resets_at")
        .and_then(|x| x.as_str())
        .and_then(iso_to_epoch);
    let seven_reset = v
        .pointer("/seven_day/resets_at")
        .and_then(|x| x.as_str())
        .and_then(iso_to_epoch);
    Ok(UsageSummary {
        five_pct,
        seven_pct,
        five_reset,
        seven_reset,
    })
}

/// 現在ログイン中（ライブ）アカウントの使用率。`/api/oauth/usage` を直接叩く
/// （2026-07-25 ユーザー決定: 監視用長期トークンによる複数アカウント表示は全廃し、
/// ライブアカウントのみを表示する一本道にした）
pub fn live_usage_summary() -> Result<UsageSummary, String> {
    let token = live_keychain_token()?;
    let body = oauth_get_with_token(&token, USAGE_URL)?;
    parse_usage_body(&body)
}

/// 表示対象アカウントのレートリミットを取得する（常にライブアカウント）
pub fn get_rate_limits() -> Result<String, String> {
    oauth_get(USAGE_URL)
}

/// アカウント・組織・プラン情報を取得する（常にライブアカウント）
pub fn get_account_profile() -> Result<String, String> {
    oauth_get("https://api.anthropic.com/api/oauth/profile")
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("claude CLI の起動に失敗: {e}"))?;

    // 子プロセスがライブ資格情報を消費・書き戻しうる間は、アカウント切り替え・追加を
    // ブロックする対象に含める。結果に関わらず確実にフラグを戻すためクロージャで包む
    EXTRACT_TASKS_RUNNING.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = (|| {
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
    })();
    EXTRACT_TASKS_RUNNING.store(false, std::sync::atomic::Ordering::Relaxed);
    result
}
