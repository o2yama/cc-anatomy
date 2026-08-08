//! フォルダ右クリックメニューのアクション群と Anthropic API 呼び出し。
//! 外部アプリ起動（Finder / cmux / Ghostty）とタスク抽出は macOS 限定機能。
//! 使用量取得は全プラットフォーム共通。
//!
//! ライブアカウント（現在ログイン中）の使用量は常にライブ資格情報の access token で
//! `/api/oauth/usage` を直接叩く（`live_usage_summary`。トレイ・アプリ内ポップオーバー
//! （`tray::usage_overview`）が共有する）。2026-07-26 に任意機能として復活した
//! 監視用長期トークン（`claude setup-token` 発行）は `oauth/usage` のスコープ外のため、
//! `usage_via_monitor_token` が `/v1/messages` へ最小リクエストを投げてレスポンスヘッダ
//! （`anthropic-ratelimit-unified-*`）から使用率を読む別経路を使う
//! （`accounts::get_accounts_usage` の優先順位2番目）。

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

/// token 自動復帰の claude CLI 裏起動（issue #5）が実行中か。子プロセスがライブ資格情報を
/// 更新しうる間はアカウント切り替え・追加をブロックする対象に含める
static TOKEN_NUDGE_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_agent_busy() -> bool {
    EXTRACT_TASKS_RUNNING.load(std::sync::atomic::Ordering::Relaxed)
        // nudge 側は ACCOUNT_OP_IN_PROGRESS との相互排他（二重チェック）を成立させるため SeqCst
        || TOKEN_NUDGE_RUNNING.load(std::sync::atomic::Ordering::SeqCst)
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

/// ライブ access token の取得状態。`expired` は Claude Code 側の自動 refresh 待ちの
/// 正常な状態であり、エラー扱いしない（2026-07-26 ユーザー報告: 期限切れ時の応答が
/// そのまま「再ログインが必要かもしれません」という誤誘導なエラー文言として毎回
/// 表示されていた）。`network`（接続失敗・タイムアウト等）は `error`（応答の構文エラー等、
/// それ以外の予期しない失敗）から分離している（2026-08-08 issue #4: トレイが「token 期限切れ」
/// と「通信できない」を区別して案内する必要が生じたため。live_usage_summary の
/// LiveUsageError で外部へ引き継ぐ）
#[derive(serde::Serialize, Clone)]
#[serde(tag = "status")]
pub enum UsageFetch {
    #[serde(rename = "ok")]
    Ok { body: String },
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "network")]
    Network,
    #[serde(rename = "error")]
    Error { message: String },
}

/// 期限切れ相当（401 or 応答本文の error フィールド）、通信不能（接続失敗・タイムアウト等）、
/// それ以外の予期しない失敗（応答の構文エラー等）を区別する。Network を Other から分離した
/// のは accounts::resolve_live_owner が「通信を確認して再試行」と「その他のエラー」を
/// 別文言で案内する必要があるため（2026-08-08、issue #1/#2 対応）。fetch_live_usage_status は
/// 従来どおり両方を同じ Error 表示にまとめるため、ここでの分離は表示側の挙動を変えない
pub(crate) enum FetchOutcome {
    Ok(String),
    Expired,
    Network,
    Other(String),
}

/// oauth_get_with_token と同じ理由でランタイムコンテキストの無い素の OS スレッドへ逃がす。
/// fetch_live_usage_status（使用量取得）と accounts::resolve_live_owner（持ち主確認）が共有する
pub(crate) fn oauth_get_checked(token: &str, url: &str) -> FetchOutcome {
    let token = token.to_string();
    let url = url.to_string();
    match std::thread::spawn(move || oauth_get_checked_blocking(&token, &url)).join() {
        Ok(outcome) => outcome,
        Err(_) => FetchOutcome::Other("API 呼び出し中に内部エラーが発生しました".to_string()),
    }
}

fn oauth_get_checked_blocking(token: &str, url: &str) -> FetchOutcome {
    let resp = match HTTP
        .get(url)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .timeout(std::time::Duration::from_secs(10))
        .send()
    {
        Ok(r) => r,
        Err(_) => return FetchOutcome::Network,
    };
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return FetchOutcome::Expired;
    }
    let body = match resp.text() {
        Ok(b) => b,
        Err(e) => return FetchOutcome::Other(e.to_string()),
    };
    let parsed: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return FetchOutcome::Other("API の応答が不正です".to_string()),
    };
    if parsed.get("error").is_some() {
        // 実測: 期限切れの access token は 401 ではなく 200 + error フィールドで
        // 返ってくることがある。「再ログインが必要」という誤誘導な文言は出さず、
        // 期限切れと同列（Expired）に扱う
        return FetchOutcome::Expired;
    }
    FetchOutcome::Ok(body)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// access token の有効期限切れ判定。fetch_live_usage_status と accounts::resolve_live_owner
/// の入口チェックが共有し、無駄な401リクエストを減らす（2026-08-08、issue #1:
/// resolve_live_owner がこのチェックを持っていなかった）。
/// なお accounts.rs の `token_is_still_valid`（get_accounts_usage 経由、登録済み他アカウントの
/// スナップショット判定）は判定の向きが逆（valid か）で用途も別関数のため、ここには統合していない
/// （今回のスコープ外）
pub(crate) fn is_token_expired(expires_at: Option<i64>) -> bool {
    expires_at.is_some_and(|exp| exp <= now_ms())
}

/// 事前に access token の有効期限を確認し、期限切れなら API を呼ばずに Expired を返す
/// （無駄な401リクエストを減らす）。期限内でも 401・error フィールド応答が返れば、
/// 事前判定をすり抜けたケースとして同じ Expired 扱いにする。refresh は一切行わない
/// （refresh してしまうと refresh token が消費され、アカウント切り替え機能の安全性の
/// 根幹である「refresh token は Claude Code 本体だけが触る」という前提が崩れるため）
fn fetch_live_usage_status(url: &str) -> UsageFetch {
    let (token, expires_at) = match crate::credentials::live_token_with_expiry() {
        Ok(v) => v,
        Err(e) => return UsageFetch::Error { message: e },
    };
    if is_token_expired(expires_at) {
        return UsageFetch::Expired;
    }
    match oauth_get_checked(&token, url) {
        FetchOutcome::Ok(body) => UsageFetch::Ok { body },
        FetchOutcome::Expired => UsageFetch::Expired,
        FetchOutcome::Network => UsageFetch::Network,
        FetchOutcome::Other(message) => UsageFetch::Error { message },
    }
}

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

/// 監視用長期トークン（`claude setup-token` 発行）で `/v1/messages` に最小リクエストを投げ、
/// レスポンスヘッダを返す。
///
/// **この経路でしか長期トークンの使用率は取れない**: `/api/oauth/usage` は長期トークンの
/// スコープ外で拒否される。ヘッダには `anthropic-ratelimit-unified-*`（そのトークンの
/// アカウントの使用率）が入っている。429（枠を使い切った状態）でもヘッダは返るので
/// ステータスでは弾かない。reqwest::blocking のランタイムパニック回避のため、
/// oauth_get_with_token と同様に素の std::thread へ逃がす
fn probe_headers(token: &str) -> Result<(u16, reqwest::header::HeaderMap), String> {
    let token = token.to_string();
    std::thread::spawn(move || probe_headers_blocking(&token))
        .join()
        .map_err(|_| "API 呼び出し中に内部エラーが発生しました".to_string())?
}

fn probe_headers_blocking(token: &str) -> Result<(u16, reqwest::header::HeaderMap), String> {
    let body = r#"{"model":"claude-haiku-4-5-20251001","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#;
    let resp = HTTP
        .post("https://api.anthropic.com/v1/messages")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .body(body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|_| "API に接続できませんでした".to_string())?;
    Ok((resp.status().as_u16(), resp.headers().clone()))
}

fn header_value(headers: &reqwest::header::HeaderMap, key: &str) -> Option<String> {
    headers.get(key).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string())
}

/// 監視用長期トークンの検証結果（旧実装 `claim_pending_account` の `check_oauth_token` を移植）。
/// レート上限（429）を「無効」と混同しないこと: 枠を使い切ったアカウントでも
/// `anthropic-organization-id` ヘッダは返るためトークン自体は有効。無効と確定できるのは
/// 401（認証拒否）のときだけ
pub enum TokenCheck {
    /// 使える。課金先の organization id を持つ（長期トークンではメールを引けないため、
    /// アカウントの同一性判定はこの id で行う）
    Valid(String),
    /// 認証そのものが拒否された。トークンが壊れているか失効している
    Invalid,
    /// 認証は問題ないが今は確認できない（レート上限・ネットワーク断・サーバ側エラー等）。
    /// 呼び出し側はここでトークンを破棄してはいけない（一時的な不調で有効なトークンを
    /// 誤って捨てないため）
    Unavailable(String),
}

/// トークンの生死と、紐づく organization id を確認する
/// （2026-07-26: setup-token 承認時にブラウザ側が別アカウントのままだった場合の
/// 誤紐づけ検知に使う。旧実装 `check_oauth_token` と同じ判定基準）
pub fn check_monitor_token(token: &str) -> TokenCheck {
    let (status, headers) = match probe_headers(token) {
        Ok(v) => v,
        Err(e) => return TokenCheck::Unavailable(e),
    };
    if status == 401 {
        return TokenCheck::Invalid;
    }
    match header_value(&headers, "anthropic-organization-id") {
        Some(org) => TokenCheck::Valid(org),
        None => TokenCheck::Unavailable(format!("アカウントを特定できませんでした（HTTP {status}）")),
    }
}

/// 監視用長期トークンでの使用率取得（`accounts::get_accounts_usage` の優先順位2番目）。
/// 401（トークン自体が無効・失効）のときだけ Err にする。refresh の概念が無い長期トークン
/// なので、失効したら「常時監視を設定」でのやり直しが必要になる（切り替え機能には影響しない）
pub fn usage_via_monitor_token(token: &str) -> Result<UsageSummary, String> {
    let (status, headers) = probe_headers(token)?;
    if status == 401 {
        return Err("監視トークンが無効です".into());
    }
    let pct = |prefix: &str| -> Option<f64> {
        header_value(&headers, &format!("anthropic-ratelimit-unified-{prefix}-utilization"))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v * 100.0)
    };
    let reset = |prefix: &str| -> Option<i64> {
        header_value(&headers, &format!("anthropic-ratelimit-unified-{prefix}-reset"))
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

/// live_usage_summary() の失敗理由。トレイ・使用量ポップオーバー（tray.rs）が原因つきの
/// 案内文を出し分けるための分類（2026-08-08、issue #4）。以前は「期限切れも通常の失敗も
/// 区別せず表示しない」としていたが、切り替え後に token 期限切れで使用量が固定表示のまま
/// 止まる原因が伝わらないという報告を受け、区別を外へ引き継ぐようにした。
/// Expired/Network の分類自体は fetch_live_usage_status（#1 で導入した is_token_expired・
/// FetchOutcome::Expired/Network）をそのまま使う
#[derive(Debug, Clone)]
pub enum LiveUsageError {
    Expired,
    Network,
    Other(String),
}

/// 現在ログイン中（ライブ）アカウントの使用率。`/api/oauth/usage` を直接叩く
/// （2026-07-25 ユーザー決定: 監視用長期トークンによる複数アカウント表示は全廃し、
/// ライブアカウントのみを表示する一本道にした）
pub fn live_usage_summary() -> Result<UsageSummary, LiveUsageError> {
    match fetch_live_usage_status(USAGE_URL) {
        UsageFetch::Ok { body } => parse_usage_body(&body).map_err(LiveUsageError::Other),
        UsageFetch::Expired => Err(LiveUsageError::Expired),
        UsageFetch::Network => Err(LiveUsageError::Network),
        UsageFetch::Error { message } => Err(LiveUsageError::Other(message)),
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

/// 直近に token 自動復帰を試みた時刻（デバウンス用）。refresh が失敗し続ける環境で
/// 60秒ポーリングのたびに claude CLI を起動し続けないよう、発火間隔を空ける。
/// Instant はシステムスリープ中に進まず、スリープ復帰直後（＝期限切れになりやすい
/// タイミング）の抑止が不当に延びるため SystemTime を使う
#[cfg(target_os = "macos")]
static LAST_TOKEN_NUDGE: std::sync::Mutex<Option<std::time::SystemTime>> = std::sync::Mutex::new(None);
#[cfg(target_os = "macos")]
const TOKEN_NUDGE_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(600);
/// 子プロセスの待機上限。無期限待ちにすると、ハング時に TOKEN_NUDGE_RUNNING が立ったまま
/// アカウント切り替えが長時間不能になるため必ず打ち切る。nudge の発火条件（期限切れ）は
/// ユーザーが切り替えたくなる状況と重なるため、ブロック窓は短めに取る
/// （第1段はローカル処理なのでさらに短い上限を使う）
#[cfg(target_os = "macos")]
const TOKEN_NUDGE_TIMEOUT_LOCAL: std::time::Duration = std::time::Duration::from_secs(15);
#[cfg(target_os = "macos")]
const TOKEN_NUDGE_TIMEOUT_API: std::time::Duration = std::time::Duration::from_secs(60);

/// nudge が起動した claude 子プロセスの PID。quit ハンドラからの best-effort kill 用
/// （doc_analysis / diagnostics の kill_running と同じ孤児化防止の方針）
#[cfg(target_os = "macos")]
static TOKEN_NUDGE_CHILD_PID: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
pub fn kill_token_nudge() {
    let pid = TOKEN_NUDGE_CHILD_PID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(pid) = pid {
        // 直後に app.exit するため、TERM を握り潰されないよう doc_analysis と同じ -9
        let _ = Command::new("/bin/kill").args(["-9", &pid.to_string()]).status();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn kill_token_nudge() {}

/// TOKEN_NUDGE_RUNNING を Drop で必ずクリアするガード（AccountOpGuard と同型）。
/// nudge スレッドが panic してもフラグが立ちっぱなしにならないようにする
#[cfg(target_os = "macos")]
struct TokenNudgeGuard;

#[cfg(target_os = "macos")]
impl Drop for TokenNudgeGuard {
    fn drop(&mut self) {
        TOKEN_NUDGE_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// ライブ access token が期限切れのとき、claude CLI を裏起動して Claude Code 本体に
/// 正規の refresh をさせ、usage 表示を自動復帰させる（issue #5）。
/// アプリ自身は refresh token に一切触れない（触れると one-time use の refresh token を
/// 消費してしまう）という設計制約を維持したまま、「refresh のきっかけ」だけを作る。
/// 失敗しても何もしない（既存の期限切れ案内表示のまま、次回デバウンス明けに再試行）。
#[cfg(target_os = "macos")]
pub fn spawn_token_refresh_nudge() {
    if debounced_token_nudge() {
        return;
    }
    // 多重起動の排除とフラグ設定を1操作で行う（先行 nudge が生きているうちに
    // 2本目が走って store(false) でガードを誤解除する事故の防止）。
    // spawn 前・呼び出しスレッド側で立てることで、フラグ可視化までの窓を最小にする
    if TOKEN_NUDGE_RUNNING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return;
    }
    let guard = TokenNudgeGuard;
    // フラグを立てた後にアカウント操作中でないことを再確認する（AccountOpGuard::acquire と
    // 同じ二重チェック。チェック→セットの間に切り替えが始まっていたら退く）
    if crate::accounts::ACCOUNT_OP_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
        return; // guard の Drop がフラグを戻す
    }
    // 両ガードを通過してから発火時刻を記録する（CAS 失敗やロールバックで
    // 10分窓を無駄に消費しないため）
    {
        let mut last = LAST_TOKEN_NUDGE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *last = Some(std::time::SystemTime::now());
    }
    std::thread::spawn(move || {
        let _guard = guard; // スレッド終了（panic 含む）で必ずフラグをクリア
        if let Err(e) = run_token_refresh_nudge() {
            eprintln!("token 自動復帰の試行に失敗（次回デバウンス明けに再試行）: {e}");
        }
    });
}

/// デバウンス窓の内側なら true（発火を抑止する）。時計巻き戻しで duration_since が
/// Err になったら「経過扱い」で再発火を許す
#[cfg(target_os = "macos")]
fn debounced_token_nudge() -> bool {
    let last = LAST_TOKEN_NUDGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match *last {
        Some(t) => std::time::SystemTime::now()
            .duration_since(t)
            .map(|d| d < TOKEN_NUDGE_MIN_INTERVAL)
            .unwrap_or(false),
        None => false,
    }
}

/// 子プロセスを PID 記録つきでタイムアウト待機する。超過したら kill して打ち切る
#[cfg(target_os = "macos")]
fn wait_child_with_timeout(
    mut child: std::process::Child,
    label: &str,
    timeout: std::time::Duration,
) -> Result<bool, String> {
    {
        let mut pid = TOKEN_NUDGE_CHILD_PID
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *pid = Some(child.id());
    }
    let started = std::time::Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(format!("{label} が {}秒以内に終了せず kill", timeout.as_secs()));
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) => break Err(format!("{label} の待機に失敗: {e}")),
        }
    };
    {
        let mut pid = TOKEN_NUDGE_CHILD_PID
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *pid = None;
    }
    result
}

/// 二段構えで refresh を誘発する。第1段（auth status）はローカル処理で無コストだが
/// refresh まで走るかは CLI の実装次第のため、効かなければ第2段（最小の headless 呼び出し）で
/// 実際の API アクセスを発生させて確実に refresh させる。どちらで復帰したかはログで判別できる
#[cfg(target_os = "macos")]
fn run_token_refresh_nudge() -> Result<(), String> {
    let claude = resolve_claude_bin()?;

    let child = Command::new(&claude)
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("claude CLI の起動に失敗: {e}"))?;
    let _ = wait_child_with_timeout(child, "claude auth status", TOKEN_NUDGE_TIMEOUT_LOCAL);
    // CLI が資格情報を書き戻すまでのラグを吸収してから期限を再確認する（経験則の余裕）
    std::thread::sleep(std::time::Duration::from_secs(3));
    if live_token_now_valid() {
        eprintln!("token 自動復帰: claude auth status で復帰");
        return Ok(());
    }

    // 無人・定期実行のため、ユーザー設定の hooks / MCP サーバを起動しない
    // （doc_analysis の headless 硬化方針を踏襲。cwd もプロジェクトに依存させない）
    let mut child = Command::new(&claude)
        .args([
            "-p",
            "--model",
            "haiku",
            "--max-turns",
            "1",
            "--strict-mcp-config",
            "--setting-sources",
            "user",
        ])
        .current_dir(crate::db::home_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("claude CLI の起動に失敗: {e}"))?;
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().ok_or("stdin の取得に失敗")?;
        stdin.write_all(b"ok").map_err(|e| e.to_string())?;
    }
    drop(child.stdin.take());
    let success = wait_child_with_timeout(child, "claude -p", TOKEN_NUDGE_TIMEOUT_API)?;
    if !success {
        return Err("claude CLI がエラー終了".into());
    }
    if live_token_now_valid() {
        eprintln!("token 自動復帰: headless 呼び出しで復帰");
        Ok(())
    } else {
        Err("claude CLI は成功したが token 期限が更新されていない".into())
    }
}

/// ライブ資格情報の expiresAt が現在有効か（nudge の成否判定用）
#[cfg(target_os = "macos")]
fn live_token_now_valid() -> bool {
    matches!(
        crate::credentials::live_token_with_expiry(),
        Ok((_, expires_at)) if !is_token_expired(expires_at)
    )
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
