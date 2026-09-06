//! フォルダ右クリックメニューのアクション群と Anthropic API 呼び出し。
//! 外部アプリ起動（Finder / cmux / Ghostty）とタスク抽出は macOS 限定機能。
//! 使用量取得は全プラットフォーム共通。
//!
//! ライブアカウント（現在ログイン中）の使用量は常にライブ資格情報の access token で
//! `/api/oauth/usage` を直接叩く。登録済みアカウント一括取得（accounts::get_accounts_usage）の
//! LiveOauth 経路が本線で、バッチにライブが存在しないとき（未登録ライブ・
//! has_credentials=false・非 macOS 全体）だけ `live_usage_summary_gated` が tray.rs の
//! フォールバック経路として同じ資格情報チェック→ゲート→HTTP の順で照会する（V-1、
//! 2026-08-22 第6ラウンド）。トレイ・アプリ内ポップオーバー（`tray::usage_overview`）は
//! `tray::fetch_raw_status` を共有することで、この2経路のどちらが返した値でも同じ表示ロジックに
//! 乗せている。2026-07-26 に任意機能として復活した
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

/// OAuth トークンエンドポイント（`post_json_checked`）だけが Cloudflare の 403 1010 で
/// 遮断される（2026-09-06 実測）。この値は通過確認済み。使用量ポーリング等の他の経路は
/// User-Agent 無しで従来どおり動いているため、共有クライアントには設定せず
/// `post_json_checked` のリクエストヘッダとして個別に付与する（2026-09-06 レビュー L-1）
const HTTP_USER_AGENT: &str = "claude-cli/2.1.261 (external, cli)";

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
/// ランタイムの文脈を一切持たない素の OS スレッドで実行する必要がある。
/// この理由から `oauth_get_checked`（後述）・`probe_headers` もすべて素の std::thread へ逃がす
///
/// `/api/oauth/usage` のエンドポイント。ライブアカウントの使用量表示（actions.rs）と
/// 登録済み全アカウントの使用率一括取得（accounts::get_accounts_usage）の両方から使うため公開する
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// 自動経路（force==false）でのライブアカウントに対する `/api/oauth/usage` 再試行間隔（秒）。
/// 元は accounts.rs 内に private const として持っていたが、U-1（2026-08-22、第5ラウンド）で
/// tray.rs のフォールバック経路（未登録ライブ・has_credentials=false・Windows/Linux 全体では
/// `/api/oauth/usage` の唯一の取得経路になる）にも同じ間隔を通す必要が生じ、全プラットフォームで
/// コンパイルされるここへ移した（accounts.rs は macOS 限定ファイルで、tray.rs からは
/// 参照できない）。
///
/// なぜ300秒か（実測に基づく根拠。V-5、2026-08-22 第6ラウンドで accounts.rs から
/// この doc へ一本化した。以前は定数の実体だけをここへ移し、根拠の説明は accounts.rs 側に
/// 残していたが、「本ファイルの doc に書いた」という自己参照が実際には別ファイルを指しており、
/// 実在しない参照になっていた）:
///
/// 2026-08-22（第4ラウンド、S-1）: 45秒から300秒（5分）へ大幅に緩めた。
///
/// 実測で分かったこと（同日、`/api/oauth/usage` に対して）:
/// - 429 は**リクエスト頻度**によるもので、そのアカウントの使用量の枠とは無関係。
///   5h 枠を 100% 使い切ったアカウント2件が 200 を返す一方、枠に余裕のある（16%）
///   アカウントが 429 を返した。違いは「そのアカウントがライブで、アプリがポーリング
///   していたかどうか」だった
/// - 枠は**アカウント単位**。非ライブのアカウントは、アプリが監視トークン経由で取得
///   していてこのエンドポイントを叩かないため、外部から3回連続で叩いても影響がない
/// - 枠は狭い。8秒間隔で3回叩けば 429 になる程度
///
/// つまり毎分1回（旧45秒設定＋60秒周期）は上限に張り付いた状態で、同一アカウントに
/// 対する消費者がもう1つあれば即あふれる。5分間隔にして実質的な余裕を作る方針にした
/// （ユーザー決定、2026-08-22）。
///
/// 未確定: 正確な閾値と回復速度。「claude セッションがこの枠を共有している」という仮説は
/// 立てたが、裏付けは取れていない（非ライブのアカウントが影響を受けていないことから、
/// むしろアプリ自身のポーリングが支配的だと考えられる）
///
/// 旧45秒の根拠（2026-08-22、R-5。当時はポーリング周期60秒に対して意図的に短くしていた）
/// は300秒化で意味を失った
pub(crate) const USAGE_MIN_REFETCH_SECS: i64 = 300;

/// 手動経路（force==true。トレイの「ステータス更新」・アカウント画面表示・切り替え直後）での
/// 試行間隔の下限（秒）。U-1（2026-08-22、第5ラウンド）新設。
/// ユーザー操作を自動経路と同じ5分間まったく効かなくするのは行き過ぎだが、
/// `accounts-updated` イベント経由でフロントが繰り返し再取得しうるため無制限にもしない。
///
/// 60 ではなく45（V-4、2026-08-22 第6ラウンド）: トレイの `REFRESH_INTERVAL`（60秒）と
/// 同値だと、到達時刻の揺れ（処理時間・スケジューラの遅延等）で「60秒経過している/いない」が
/// 実行のたびにぶれ、force 経路が通ったり弾かれたりする。R-5 で `USAGE_MIN_REFETCH_SECS` を
/// 45秒にしていた（後に S-1 で300秒化）のと同じ理由で、周期と同値を避ける
pub(crate) const USAGE_FORCE_MIN_ATTEMPT_SECS: i64 = 45;

/// 非ライブアカウント（切り替え前に眺めるだけの他アカウント）はライブほど鮮度が要らないため、
/// 別の緩い閾値を持つ（2026-08-22: リクエスト数削減。ライブと同じ60秒だと、登録アカウント数が
/// 増えるほど `/api/oauth/usage` への打鍵回数が線形に増えていた）。
///
/// 元は accounts.rs 内で完結していたが、V-2（2026-08-22 第6ラウンド）で
/// `usage_attempt_min_interval` をここへ一本化したのに伴い、この定数も一緒に移した
/// （accounts.rs は macOS 限定ファイルで、非ライブを扱わない tray.rs からは参照できないため）
pub(crate) const NON_LIVE_MIN_REFETCH_SECS: i64 = 600;

/// 「最後に試行した時刻」ゲート（`gate_usage_attempt`）に渡す下限間隔（U-1、2026-08-22
/// 第5ラウンドで導入。V-2、2026-08-22 第6ラウンドで accounts.rs から actions.rs へ移設）。
///
/// Why 移設: accounts.rs（macOS 限定）に private として置いていたが、tray.rs の
/// フォールバック経路も同じ規則（force かどうかで間隔を切り替える）を必要とし、
/// tray.rs 側は同じ規則をインラインの `if force {...} else {...}` として別々に持っていた。
/// 同じ規則を2箇所が別々に持つ構図（切り出したはずが本体側にも別ロジックが残る）は
/// 前ラウンドで一度指摘された構図の再発だったため、全プラットフォームでコンパイルされる
/// ここへ一本化し、accounts.rs と tray.rs の両方から呼ぶ形にした。
///
/// `force_skips_freshness_check` と同じ規約で、force の短縮効果はライブアカウントに
/// 限定する（非ライブは force の値に関係なく常に `NON_LIVE_MIN_REFETCH_SECS`）。
/// tray.rs のフォールバック経路は常にライブ単体を扱うため `is_live = true` 固定で呼ぶ。
///
/// これは「キャッシュがどれだけ新鮮なら試行自体をスキップするか」（accounts.rs の
/// `cache_is_fresh_enough`）とは別物: あちらは前回**成功**した時刻を基準にするが、
/// こちらは前回**試行した**時刻（成否を問わない）を基準にする。429 等が続いてキャッシュが
/// 一向に新鮮化しない場合でも、この間隔だけは守らせるためのゲート
pub(crate) fn usage_attempt_min_interval(is_live: bool, force: bool) -> std::time::Duration {
    let secs = if is_live && force {
        USAGE_FORCE_MIN_ATTEMPT_SECS
    } else if is_live {
        USAGE_MIN_REFETCH_SECS
    } else {
        NON_LIVE_MIN_REFETCH_SECS
    };
    std::time::Duration::from_secs(secs as u64)
}

/// 表示中の使用量が「本当に古い」と言える閾値（tray.rs の注記表示が使う。2026-08-22、
/// 第4ラウンド S-3 新設）。USAGE_MIN_REFETCH_SECS（照会間隔）とは別の目的の定数: こちらは
/// 「いつ取得したか」ではなく「表示に使っている値がどれだけ古いか」を見て注記を出すかどうかの
/// 基準。
///
/// 元は accounts.rs にあり、tray.rs（macOS 以外でもコンパイルされる）が
/// `accounts_stub.rs` 側に同じ値をミラーして参照していたが、cfg が排他なので値がズレても
/// コンパイル時・実行時のどちらでも検知できなかった（U-3、2026-08-22 第5ラウンドでここへ
/// 一本化。以後ミラーは持たない）
pub const USAGE_STALE_NOTE_SECS: i64 = 600;

/// 「最後に `/api/oauth/usage` への照会を試みた時刻」をプロセス内に保持する
/// （U-1、2026-08-22 第5ラウンド）。
///
/// Why: 従来のスロットルは「成功したら usage_cache に fetched_at を書き、次回それが
/// 閾値以内ならスキップ」だけだった。これは「成功時のキャッシュ」を鮮度判定に使っている
/// だけで、「試行したこと自体」はどこにも記録していなかった。そのため 429 が続く間は
/// fetched_at が一切更新されず、トレイの60秒サイクルごとに `/api/oauth/usage` を
/// 叩き続けてしまっていた（いちばん間隔を空けたい場面でスロットルが消えていた）。
///
/// **usage_cache 側の `fetched_at`（成功時のみ更新）を失敗時にも更新して代用してはいけない**。
/// S-3 の「表示に使った値の古さ」判定はその `fetched_at` に依存しており、失敗時にも
/// 更新してしまうと実際には古い値を「今取得した」と誤認させてしまい、注記が出なくなる
/// （S-3 の修正が丸ごと無意味になる）。そのため試行時刻は usage_cache とは別の状態として
/// ここに持つ。
///
/// キーは照会対象の識別子。`accounts::get_accounts_usage` は同一アカウントでも
/// LiveOauth と SnapshotOauth で別々のキー（`<name>:live` / `<name>:snapshot`）を使う
/// （同じキーを共有すると、429以外の理由でライブ経路が失敗した直後にスナップショット経路への
/// フォールバックまで塞いでしまい、2026-07-27 に導入したフォールバック連鎖が壊れるため）。
/// `tray.rs` のフォールバック経路は固定キー（`TRAY_LIVE_FALLBACK_KEY`）を使う
static USAGE_LAST_ATTEMPT: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::SystemTime>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// `tray.rs` のフォールバック経路（`live_usage_summary_gated` を直接呼ぶ側）専用の固定キー。
/// 登録アカウントの `name` ベースのキー（`<name>:live` 等）と衝突しない文字列にする
pub(crate) const TRAY_LIVE_FALLBACK_KEY: &str = "__live_fallback__";

/// 直近の試行時刻から `min_interval` が経過していれば true（＝試行してよい）。
/// HTTP を打たずに単体テストできるよう、状態・時計から分離した純粋関数（U-1）。
/// - `last` が `None`（まだ一度も試していない）→ true
/// - `now.duration_since(last)` が `Ok` で `min_interval` 以上経過している → true
/// - 時計が巻き戻っていて `duration_since` が `Err` → true（経過扱いにして再開を許す。
///   `debounced_token_nudge` と同じ方針）
/// - それ以外（`min_interval` 未満しか経過していない）→ false
pub(crate) fn usage_attempt_allowed(
    last: Option<std::time::SystemTime>,
    min_interval: std::time::Duration,
    now: std::time::SystemTime,
) -> bool {
    match last {
        None => true,
        Some(last) => now
            .duration_since(last)
            .map(|elapsed| elapsed >= min_interval)
            .unwrap_or(true),
    }
}

/// `key` について `usage_attempt_allowed` を判定し、許可されたときだけ「今試行した」ことを
/// 記録して true を返す。呼び出し側はこれが true のときだけ実際に HTTP を打つこと
/// （false ならキャッシュへフォールバックし、HTTP は一切打たない）。
///
/// 成否にかかわらず「試みた」時点で記録するのが要点（成功だけを記録する usage_cache の
/// `fetched_at` とは別物）。実際の成否は呼び出し側が別途 `oauth_get_checked` 等で判定する
pub(crate) fn gate_usage_attempt(key: &str, min_interval: std::time::Duration) -> bool {
    let now = std::time::SystemTime::now();
    let mut map = USAGE_LAST_ATTEMPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let last = map.get(key).copied();
    if usage_attempt_allowed(last, min_interval, now) {
        map.insert(key.to_string(), now);
        true
    } else {
        false
    }
}

/// 期限切れ相当（401 or 応答本文の authentication_error）、レート制限（429 or 応答本文の
/// rate_limit_error）、通信不能（接続失敗・タイムアウト等）、それ以外の予期しない失敗
/// （応答の構文エラー等）を区別する。Network を Other から分離したのは
/// accounts::resolve_live_owner が「通信を確認して再試行」と「その他のエラー」を
/// 別文言で案内する必要があるため（2026-08-08、issue #1/#2 対応）。
/// RateLimited は 2026-08-22 追加: 従来は HTTP ステータスを一切見ず「本文に error フィールドが
/// あれば全部期限切れ」と判定していたため、429（レート制限。実測: 本文
/// `{"error":{"type":"rate_limit_error",...}}`）が「token 期限切れ」と誤表示されていた
pub(crate) enum FetchOutcome {
    Ok(String),
    Expired,
    RateLimited,
    Network,
    Other(String),
}

/// HTTP ステータスと応答本文の文字列だけで FetchOutcome を判定する純粋関数。HTTP を打たずに
/// 単体テストできるよう、ネットワーク I/O（oauth_get_checked_blocking）から分離した
/// （2026-08-22）。判定順序:
/// 1. status 429 → RateLimited / status 401 → Expired（実測ベース、最優先）
/// 2. 本文に error フィールドがあれば error.type を見る
///    （authentication_error→Expired、rate_limit_error→RateLimited、それ以外→Other。
///    実測: 期限切れの access token は 401 ではなく 200 + error 本文で返ることがあるため、
///    ステータス判定だけでは拾いきれずこの経路を残す）
/// 3. それ以外の非 2xx → Other
pub(crate) fn classify_oauth_response(status: u16, body: &str) -> FetchOutcome {
    if status == 429 {
        return FetchOutcome::RateLimited;
    }
    if status == 401 {
        return FetchOutcome::Expired;
    }
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(error) = parsed.get("error") {
            let err_type = error.get("type").and_then(|t| t.as_str()).unwrap_or("");
            return match err_type {
                "authentication_error" => FetchOutcome::Expired,
                "rate_limit_error" => FetchOutcome::RateLimited,
                _ => {
                    let message = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("API がエラーを返しました（HTTP {status}）"));
                    FetchOutcome::Other(message)
                }
            };
        }
        if (200..300).contains(&status) {
            return FetchOutcome::Ok(body.to_string());
        }
    } else if (200..300).contains(&status) {
        return FetchOutcome::Other("API の応答が不正です".to_string());
    }
    FetchOutcome::Other(format!("API がエラーを返しました（HTTP {status}）"))
}

/// reqwest::blocking のランタイムパニック回避のため、上記と同じ理由で素の OS スレッドへ逃がす。
/// live_usage_summary_gated（使用量取得）と accounts::resolve_live_owner（持ち主確認）が共有する
pub(crate) fn oauth_get_checked(token: &str, url: &str) -> FetchOutcome {
    let token = token.to_string();
    let url = url.to_string();
    match std::thread::spawn(move || oauth_get_checked_blocking(&token, &url)).join() {
        Ok(outcome) => outcome,
        Err(_) => FetchOutcome::Other("API 呼び出し中に内部エラーが発生しました".to_string()),
    }
}

/// JSON ボディの POST（OAuth トークンエンドポイント用。accounts::refresh_snapshot_credentials
/// が使う）。素の OS スレッドへ逃がす理由は oauth_get_checked と同じ。
/// 戻り値の本文にはトークンが含まれうるため、呼び出し側はログ・エラーメッセージへ
/// 本文をそのまま載せないこと
pub(crate) fn post_json_checked(url: &str, body: serde_json::Value) -> Result<(u16, String), String> {
    let url = url.to_string();
    std::thread::spawn(move || {
        let resp = HTTP
            .post(&url)
            .header("Content-Type", "application/json")
            // token endpoint だけ Cloudflare 対策の User-Agent が要る（HTTP_USER_AGENT 参照）
            .header("User-Agent", HTTP_USER_AGENT)
            .body(body.to_string())
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .map_err(|_| "ネットワークエラーが発生しました".to_string())?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .map_err(|_| "API 応答の読み取りに失敗しました".to_string())?;
        Ok((status, text))
    })
    .join()
    .map_err(|_| "API 呼び出し中に内部エラーが発生しました".to_string())?
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
    let status = resp.status().as_u16();
    let body = match resp.text() {
        Ok(b) => b,
        Err(e) => return FetchOutcome::Other(e.to_string()),
    };
    classify_oauth_response(status, &body)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// UsageSummary::fetched_at のスタンプ用（S-3、2026-08-22）。now_ms と同じ SystemTime だが
/// 表示側（tray.rs::reset_local）が epoch 秒で扱うため秒精度で返す
fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// access token の有効期限切れ判定。live_usage_summary_gated と accounts::resolve_live_owner
/// の入口チェックが共有し、無駄な401リクエストを減らす（2026-08-08、issue #1:
/// resolve_live_owner がこのチェックを持っていなかった）。
/// なお accounts.rs の `token_is_still_valid`（get_accounts_usage 経由、登録済み他アカウントの
/// スナップショット判定）は判定の向きが逆（valid か）で用途も別関数のため、ここには統合していない
/// （今回のスコープ外）
pub(crate) fn is_token_expired(expires_at: Option<i64>) -> bool {
    expires_at.is_some_and(|exp| exp <= now_ms())
}

/// メニューバー向けの使用量サマリー。使用率は 0〜100、リセットは epoch 秒。
///
/// `fetched_at`（2026-08-22、S-3 追加）: この値が実際にいつ取得されたか（epoch 秒）。
/// tray.rs が「表示中の値がどれだけ古いか」を判定して注記を出すために使う
/// （live_error という「取得に失敗したか」ではなく「表示している値そのものの古さ」を
/// 見る方針に変えたため。live_error だけを見ると、失敗しても別ソースで新鮮な値が
/// 取れているケースで事実と違う注記が出てしまっていた）。
/// `parse_usage_body`・`usage_via_monitor_token` はいずれも HTTP 応答を受け取った直後にしか
/// 呼ばれないため、その時点の `now_epoch_secs()` をそのままスタンプしてよい
/// （キャッシュ経由で古い値を UsageSummary に詰め直す経路は tray.rs 側で別途 fetched_at を
/// 引き継ぐ。`usage_summary_from_batch` 参照）
///
/// `Clone` は V-1（2026-08-22、第6ラウンド）で追加: tray.rs のフォールバック経路専用の
/// 「直近に取得できた値」保持（`TRAY_LIVE_FALLBACK_LAST_OK`）に複製して格納するために必要。
/// `Debug` はテストの失敗時アサーションメッセージ（`{result:?}` 等）用
#[derive(Clone, Debug)]
pub struct UsageSummary {
    pub five_pct: f64,
    pub seven_pct: f64,
    pub five_reset: Option<i64>,
    pub seven_reset: Option<i64>,
    pub fetched_at: Option<i64>,
}

fn iso_to_epoch(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
}

/// `/api/oauth/usage` の応答本文を UsageSummary へ変換する。ライブアカウント・登録済み他
/// アカウント問わず同じ形の応答なので、accounts::get_accounts_usage からも共有して使う。
/// 呼ばれるのは常に HTTP 応答を受け取った直後なので、fetched_at は現在時刻でよい
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
        fetched_at: Some(now_epoch_secs()),
    })
}

/// 監視用長期トークン（`claude setup-token` 発行）で `/v1/messages` に最小リクエストを投げ、
/// レスポンスヘッダを返す。
///
/// **この経路でしか長期トークンの使用率は取れない**: `/api/oauth/usage` は長期トークンの
/// スコープ外で拒否される。ヘッダには `anthropic-ratelimit-unified-*`（そのトークンの
/// アカウントの使用率）が入っている。429（枠を使い切った状態）でもヘッダは返るので
/// ステータスでは弾かない。reqwest::blocking のランタイムパニック回避のため、
/// oauth_get_checked と同様に素の std::thread へ逃がす
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
        fetched_at: Some(now_epoch_secs()),
    })
}

/// ライブアカウントの使用率取得（`live_usage_summary_gated`。accounts.rs の LiveOauth 経路）の
/// 失敗理由。トレイ・使用量ポップオーバー（tray.rs）が原因つきの案内文を出し分けるための分類
/// （2026-08-08、issue #4）。以前は「期限切れも通常の失敗も区別せず表示しない」としていたが、
/// 切り替え後に token 期限切れで使用量が固定表示のまま止まる原因が伝わらないという報告を受け、
/// 区別を外へ引き継ぐようにした。Expired/Network の分類自体は #1 で導入した
/// is_token_expired・FetchOutcome::Expired/Network をそのまま使う
#[derive(Debug, Clone)]
pub enum LiveUsageError {
    Expired,
    /// 429（レート制限）。2026-08-22 追加: A. 429 の誤分類修正で Expired から分離した
    RateLimited,
    Network,
    Other(String),
}

/// FetchOutcome（HTTP から得た生の判定）を LiveUsageError 系の Result へ変換する純粋関数。
/// `accounts::get_accounts_usage` の LiveOauth 経路が使う（2026-08-22、R-3・追加テスト項目1:
/// 「成功→None、429→RateLimited、期限切れ→Expired」の3分岐を HTTP を打たずにテストできるよう、
/// get_accounts_usage 内にインラインしていた match をここへ抽出した）
pub(crate) fn live_oauth_outcome_to_result(outcome: FetchOutcome) -> Result<UsageSummary, LiveUsageError> {
    match outcome {
        FetchOutcome::Ok(body) => parse_usage_body(&body).map_err(LiveUsageError::Other),
        FetchOutcome::Expired => Err(LiveUsageError::Expired),
        FetchOutcome::RateLimited => Err(LiveUsageError::RateLimited),
        FetchOutcome::Network => Err(LiveUsageError::Network),
        FetchOutcome::Other(msg) => Err(LiveUsageError::Other(msg)),
    }
}

/// 「最後に `/api/oauth/usage` を試行した際の実際の失敗理由」を、`USAGE_LAST_ATTEMPT` と
/// 同じキー空間でプロセス内に保持する（W-1、2026-08-22 第7ラウンド）。
///
/// Why: 試行間隔ゲート（`USAGE_LAST_ATTEMPT` / `gate_usage_attempt`）は「いつ試行したか」だけを
/// 記録し、「その試行が何で失敗したか」は保持していなかった。そのためゲートに塞がれた
/// 呼び出し側は「塞がれた」という事実からしか結果を組み立てられず、実際には起きていない
/// 原因を名乗ってしまっていた: accounts.rs の LiveOauth 経路は通信ゼロのまま
/// `RateLimited`（レート制限）を決め打ちし、tray.rs の `resolve_gated_live_usage` は
/// 保持値（`UsageSummary`）が無いとき `Other`（＝「Claude Code でログインしてください」）に
/// 倒していた。後者は特に矛盾している: `Gated` を返す時点で `live_usage_summary_gated` は
/// 資格情報の読み取りと期限チェックを既に通過しており、ログイン済み・期限内であることは
/// 型レベルで確定しているのに「ログインしてください」と案内していた。
///
/// この状態を使い、ゲートで塞いだときは「塞いだ」という事実の代わりに「直近に実際に
/// 試行して分かった失敗理由」を返す（`resolve_gated_error` 参照）
static USAGE_LAST_ERROR: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, LiveUsageError>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// `key` の直近の失敗理由として `error` を記録する。実際に試行して失敗した箇所から呼ぶ
/// （ゲートに塞がれて試行しなかった場合は呼ばない）
pub(crate) fn record_usage_error(key: &str, error: LiveUsageError) {
    USAGE_LAST_ERROR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key.to_string(), error);
}

/// `key` の直近の失敗理由を消す。実際に試行して成功した箇所から呼ぶ
pub(crate) fn clear_usage_error(key: &str) {
    USAGE_LAST_ERROR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(key);
}

/// `key` が直近保持している失敗理由を取り出す（消費せず参照のみ）
pub(crate) fn last_usage_error(key: &str) -> Option<LiveUsageError> {
    USAGE_LAST_ERROR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(key)
        .cloned()
}

/// ゲートで塞いだときに何を返すべきかを決める純粋関数（W-1）。HTTP を打たずに単体テスト
/// できるよう、状態（`USAGE_LAST_ERROR`）から分離してある。
/// - `Some(e)` → 直近に実際に試行して分かった失敗理由をそのまま返す
/// - `None` → 塞がれたのに一度も試行していない状態。理論上は起きない（ゲートは
///   `gate_usage_attempt` が試行を許可した直後にしか消費されず、その消費と対になる形で
///   このキーへ必ず記録・消去のどちらかが行われるため）が、防御的に扱う。この場合も
///   `RateLimited`（実際は通信していない）や、既存の `Other` 決め打ち文言（実質
///   「ログインしてください」を意味していた）は名乗らせず、新しい中立な文言を持つ `Other` を返す
pub(crate) fn resolve_gated_error(held: Option<LiveUsageError>) -> LiveUsageError {
    held.unwrap_or_else(|| LiveUsageError::Other("使用量を取得できませんでした".to_string()))
}

/// `live_usage_summary_gated` の戻り値（V-1、2026-08-22 第6ラウンド）。
///
/// Why: `LiveUsageError` だけでは「試行間隔ゲートに塞がれて HTTP を一度も打っていない」ことと
/// 「実際に HTTP を打って（あるいは資格情報チェックの時点で）判明した失敗」を区別できない。
/// U-1（第5ラウンド）でこの経路にゲートを追加した際、ゲートを資格情報チェックより手前に
/// 置いてしまったため、(1) この経路には accounts.rs の usage_cache のような永続キャッシュが
/// 無く、ゲートで塞ぐとそのまま数値が消えてエラー画面になる、(2) 未ログイン・期限切れという
/// HTTP を伴わない失敗でもゲートを消費してしまい、一度も通信していないのに
/// 「レート制限」と誤表示する、という2つの退行を生んだ（accounts.rs の LiveOauth 経路は
/// もともと資格情報チェック→ゲート→HTTP の順で正しく、tray.rs 側だけが順序を誤っていた）。
/// この型で「塞がれた」ことを他の失敗と型レベルで分離し、呼び出し側が RateLimited を
/// 誤って名乗らないようにする
#[derive(Debug)]
pub(crate) enum GatedLiveUsageOutcome {
    Ok(UsageSummary),
    /// 資格情報の読み取り・期限チェック、または実際の HTTP 呼び出しで判明した失敗理由。
    /// ゲートは一切絡んでいない（RateLimited もここに含まれうるが、それは実際に 429 を
    /// 受け取った場合のみ）
    Failed(LiveUsageError),
    /// 資格情報チェックは通過したが、試行間隔ゲートに塞がれて HTTP を打たなかった。
    /// 「レート制限を受けた」という意味ではない
    Gated,
}

/// tray.rs のフォールバック経路（バッチにライブアカウントが存在しないとき＝未ログイン・
/// 未登録ライブ・`has_credentials=false`・非 macOS 全体）専用の取得関数（V-1(a)）。
/// 旧 `live_usage_summary()`（ゲート無し版。V-1 で本関数に統合し削除済み）と違い、
/// 「資格情報の読み取り → 期限チェック → ゲート → HTTP」の
/// 順で処理し、ゲートは資格情報チェックを通過した後にだけかける（accounts.rs の
/// LiveOauth 経路と同じ順序に揃えた）。これにより、資格情報が無い・期限切れという時点で
/// 判明する失敗はゲートを一切消費せず、実際の理由がそのまま呼び出し側へ渡る
pub(crate) fn live_usage_summary_gated(key: &str, min_interval: std::time::Duration) -> GatedLiveUsageOutcome {
    let (token, expires_at) = match crate::credentials::live_token_with_expiry() {
        Ok(v) => v,
        Err(e) => return GatedLiveUsageOutcome::Failed(LiveUsageError::Other(e)),
    };
    if is_token_expired(expires_at) {
        return GatedLiveUsageOutcome::Failed(LiveUsageError::Expired);
    }
    if !gate_usage_attempt(key, min_interval) {
        return GatedLiveUsageOutcome::Gated;
    }
    // W-1（2026-08-22 第7ラウンド）: ゲートを消費した実際の HTTP 結果を USAGE_LAST_ERROR に
    // 記録・消去する。次回この key がゲートに塞がれたとき、`resolve_gated_live_usage` が
    // ここで記録した値を「実際の失敗理由」として参照する
    match live_oauth_outcome_to_result(oauth_get_checked(&token, USAGE_URL)) {
        Ok(u) => {
            clear_usage_error(key);
            GatedLiveUsageOutcome::Ok(u)
        }
        Err(e) => {
            record_usage_error(key, e.clone());
            GatedLiveUsageOutcome::Failed(e)
        }
    }
}

/// tray.rs のフォールバック経路で最後に成功取得できた値をプロセス内に保持する（V-1(b)）。
///
/// Why: この経路には accounts.rs の usage_cache のような永続キャッシュが無い。ゲートに
/// 塞がれたときに何も返せないと数値がそのまま消えてエラー画面になってしまう
/// （トレイは60秒周期・自動経路の間隔は300秒のため、5〜6サイクルに1回しか通信せず、
/// 残りのサイクルではこの保持値だけが頼りになる）。**成功時にのみ更新すること**
/// （失敗・ゲート時にも更新すると `fetched_at`＝「実際にいつ取得したか」という意味が壊れ、
/// S-3 の古さ判定が丸ごと無意味になる）
static TRAY_LIVE_FALLBACK_LAST_OK: std::sync::Mutex<Option<UsageSummary>> = std::sync::Mutex::new(None);

pub(crate) fn tray_live_fallback_last_ok() -> Option<UsageSummary> {
    TRAY_LIVE_FALLBACK_LAST_OK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(crate) fn store_tray_live_fallback_last_ok(summary: UsageSummary) {
    *TRAY_LIVE_FALLBACK_LAST_OK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(summary);
}

/// `GatedLiveUsageOutcome` と保持値から、呼び出し側が使う最終結果を決める純粋関数
/// （V-1(b)(c)、W-1 で `held_error` 引数を追加）。HTTP を打たずに単体テストできるよう、
/// ネットワーク I/O（`live_usage_summary_gated`）から分離した。
///
/// - `Ok(u)` → そのまま `Ok(u)`。保持値の更新は呼び出し側の責務（この関数は参照のみ）
/// - `Failed(e)` → そのまま `Err(e)`。ゲートは絡んでいない実際の失敗理由なので、そのまま渡す
/// - `Gated` かつ保持値（`UsageSummary`）あり → 保持値を `Ok` として返す（数値が出続ける。
///   V-1(b)）。`fetched_at` は保持値が実際に取得された時刻のままなので、S-3 の古さ判定
///   （`USAGE_STALE_NOTE_SECS`）が引き続き機能する
/// - `Gated` かつ保持値なし → 従来はここで `RateLimited` を経ずに `Other`（＝「ログインして
///   ください」）を決め打ちしていたが、`Gated` を返す時点で資格情報チェックは既に通過して
///   いる（`live_usage_summary_gated` 参照）ため、ログイン済み・期限内の人に矛盾した案内を
///   していた（W-1）。塞いだという事実の代わりに `held_error`（`USAGE_LAST_ERROR` に記録
///   された、直近に実際に試行して分かった失敗理由）を `resolve_gated_error` で解決して返す
pub(crate) fn resolve_gated_live_usage(
    outcome: GatedLiveUsageOutcome,
    held: Option<UsageSummary>,
    held_error: Option<LiveUsageError>,
) -> Result<UsageSummary, LiveUsageError> {
    match outcome {
        GatedLiveUsageOutcome::Ok(u) => Ok(u),
        GatedLiveUsageOutcome::Failed(e) => Err(e),
        GatedLiveUsageOutcome::Gated => match held {
            Some(u) => Ok(u),
            None => Err(resolve_gated_error(held_error)),
        },
    }
}

// 429（レート制限）を受けた後に段階的に待ち時間を延ばすバックオフ（5→10→20→40→60分）は、
// 2026-08-22 の第3ラウンドまでで導入・完成させたが、同日の第4ラウンドで撤去した（S-2）。
//
// 導入した理由: 60秒サイクルで /api/oauth/usage を打ち続けると恒常的に429に張り付いていた
// ため、429を受けたら指数的に間隔を空けて自然に収まるのを待つ設計にした。
//
// 撤去した理由: 実運用で「ほぼ常時オン」になり、副作用の方が実害として大きかった。
// (1) バックオフ中は live_error が RateLimited 固定になるため、同じ周期内に他ソース
//     （監視トークン等）が新鮮な値を取れていても注記が出っぱなしになり、「最新でない
//     可能性」という文言が事実と食い違う（S-3 で注記の根拠を fetched_at に変えたのは
//     これが理由）。ユーザーからは「少し時間が経つと必ず出てくる。普通に使えてはいる」
//     という形で報告された
// (2) Expired の自動復帰判定も RateLimited 固定に埋もれて最大60分遅れる
//
// 実測で分かった 429 の性質は USAGE_MIN_REFETCH_SECS（本ファイル。2026-08-22 第5ラウンド
// U-1 で accounts.rs から移設した）の doc に書いた。
// 要点は「頻度によるものでアカウント単位、かつ枠が狭い」こと。段階的に延長するより、
// 照会間隔そのものを5分に空けて上限から距離を取るほうが素直に効く（ユーザー決定、2026-08-22）。
//
// 撤去後に残すもの: 「同一サイクル内で429を観測したら、以降のソースも同じエンドポイントを
// 叩かない」というローカルフラグ（cycle_saw_rate_limited、accounts.rs 参照）だけは残す。
// これは同一周期内の無駄打ちを避けるだけで、次の周期（5分後）は必ず再試行するため、
// 上記の副作用を持ち込まない。
//
// 注記（2026-08-22、第5ラウンド U-1）: 撤去した直後の時点では、この「次の周期（5分後）は
// 必ず再試行する」という記述は不正確だった。試行した時刻自体をどこにも記録していなかった
// ため、429が続く間は usage_cache の fetched_at（成功時のみ更新）が更新されず、
// キャッシュが古いままになって実際にはトレイの60秒サイクルごとに毎回再試行していた
// （5分に空けたはずの間隔が、いちばん空けたい場面で消えていた）。U-1 で
// 試行間隔ゲート（usage_attempt_allowed / gate_usage_attempt、成否にかかわらず
// 試行した時点を記録する）を導入したことで、この記述は文字通り正しくなっている。
//
// 将来429が再び問題になった場合は、グローバルな段階的バックオフを再導入する前に、
// まず照会間隔（USAGE_MIN_REFETCH_SECS）の見直しから検討すること。

#[cfg(test)]
mod tests {
    /// 実環境の資格情報とネットワークを使う e2e 検証。
    /// curl → reqwest 置き換え後も、ライブ資格情報 → /api/oauth/usage の
    /// 使用率組み立ての経路が実際に通ることを確かめる。V-1（2026-08-22 第6ラウンド）で
    /// tray.rs のフォールバック経路が `live_usage_summary_gated` に統一されたのに合わせ、
    /// このテストもそちらを直接呼ぶ。他テストのキーと衝突しない専用キー・ゲートを
    /// 一切効かせない `Duration::ZERO` を渡して、ゲート判定を経由せず実際の HTTP 経路だけを
    /// 検証する。CI では資格情報が無いため ignore とし、手元で `cargo test -- --ignored` で実行する
    #[test]
    #[ignore]
    fn live_usage_e2e() {
        let outcome = super::live_usage_summary_gated("test-e2e-live-usage", std::time::Duration::ZERO);
        let u = match outcome {
            super::GatedLiveUsageOutcome::Ok(u) => u,
            other => panic!("ライブ使用量の取得に失敗: {other:?}"),
        };
        assert!((0.0..=100.0).contains(&u.five_pct), "5h 使用率が範囲外: {}", u.five_pct);
        assert!(u.five_reset.is_some(), "5h リセット時刻が取れていない");
    }

    /// 実測: 429 のとき本文は `{"error":{"type":"rate_limit_error",...}}`、
    /// ヘッダ `retry-after: 0`（このテストでは本文とステータスだけを見る）で返る。
    /// 修正前は本文に error フィールドがあるだけで Expired に分類されていた
    /// （このバグを直すのが今回の主目的）
    #[test]
    fn classify_oauth_response_429_is_rate_limited() {
        let outcome = super::classify_oauth_response(
            429,
            r#"{"error":{"type":"rate_limit_error","message":"rate limited"}}"#,
        );
        assert!(matches!(outcome, super::FetchOutcome::RateLimited));
    }

    #[test]
    fn classify_oauth_response_401_is_expired() {
        let outcome = super::classify_oauth_response(401, "");
        assert!(matches!(outcome, super::FetchOutcome::Expired));
    }

    /// 実測: 期限切れの access token は 401 ではなく 200 + error 本文で返ることがある
    #[test]
    fn classify_oauth_response_200_with_authentication_error_is_expired() {
        let outcome = super::classify_oauth_response(
            200,
            r#"{"error":{"type":"authentication_error","message":"invalid token"}}"#,
        );
        assert!(matches!(outcome, super::FetchOutcome::Expired));
    }

    #[test]
    fn classify_oauth_response_200_with_other_error_is_other_with_message() {
        let outcome = super::classify_oauth_response(
            200,
            r#"{"error":{"type":"invalid_request_error","message":"boom"}}"#,
        );
        match outcome {
            super::FetchOutcome::Other(message) => assert_eq!(message, "boom"),
            _ => panic!("Other になるはず"),
        }
    }

    #[test]
    fn classify_oauth_response_2xx_without_error_field_is_ok() {
        let outcome = super::classify_oauth_response(200, r#"{"five_hour":{"utilization":1.0}}"#);
        assert!(matches!(outcome, super::FetchOutcome::Ok(_)));
    }

    #[test]
    fn classify_oauth_response_other_non_2xx_is_other() {
        let outcome = super::classify_oauth_response(500, "not json");
        assert!(matches!(outcome, super::FetchOutcome::Other(_)));
    }

    /// 追加テスト項目4: ステータスが 429/401 以外でも、本文の error.type が
    /// rate_limit_error なら RateLimited に分類する（実測: 500 + rate_limit_error 本文の
    /// 組み合わせも観測されている）
    #[test]
    fn classify_oauth_response_500_with_rate_limit_error_body_is_rate_limited() {
        let outcome = super::classify_oauth_response(
            500,
            r#"{"error":{"type":"rate_limit_error","message":"rate limited"}}"#,
        );
        assert!(matches!(outcome, super::FetchOutcome::RateLimited));
    }

    /// 追加テスト項目4: 200 + error オブジェクトはあるが type フィールドが無いケース。
    /// 旧実装は「本文に error フィールドがあれば全部 Expired」だったため、この経路は
    /// 現在は Other になる（Expired への誤判定に戻っていないことを明示的に固定する）
    #[test]
    fn classify_oauth_response_200_with_error_missing_type_is_other() {
        let outcome = super::classify_oauth_response(200, r#"{"error":{"message":"something"}}"#);
        assert!(matches!(outcome, super::FetchOutcome::Other(_)));
    }

    /// 追加テスト項目1: get_accounts_usage の LiveOauth 経路が使う変換の3分岐を、
    /// 抽出した純粋関数 `live_oauth_outcome_to_result` で直接検証する
    #[test]
    fn live_oauth_outcome_to_result_success_is_ok() {
        let result = super::live_oauth_outcome_to_result(super::FetchOutcome::Ok(
            r#"{"five_hour":{"utilization":0.5}}"#.to_string(),
        ));
        assert!(result.is_ok());
    }

    #[test]
    fn live_oauth_outcome_to_result_rate_limited_is_rate_limited_error() {
        let result = super::live_oauth_outcome_to_result(super::FetchOutcome::RateLimited);
        assert!(matches!(result, Err(super::LiveUsageError::RateLimited)));
    }

    #[test]
    fn live_oauth_outcome_to_result_expired_is_expired_error() {
        let result = super::live_oauth_outcome_to_result(super::FetchOutcome::Expired);
        assert!(matches!(result, Err(super::LiveUsageError::Expired)));
    }

    // U-1（2026-08-22、第5ラウンド）: 試行間隔ゲートの4分岐＋実際の状態管理（gate_usage_attempt）
    use std::time::{Duration, SystemTime};

    #[test]
    fn usage_attempt_allowed_when_never_attempted() {
        assert!(super::usage_attempt_allowed(None, Duration::from_secs(300), SystemTime::now()));
    }

    #[test]
    fn usage_attempt_allowed_when_interval_has_elapsed() {
        let now = SystemTime::now();
        let last = now - Duration::from_secs(301);
        assert!(super::usage_attempt_allowed(Some(last), Duration::from_secs(300), now));
    }

    #[test]
    fn usage_attempt_not_allowed_within_interval() {
        let now = SystemTime::now();
        let last = now - Duration::from_secs(100);
        assert!(!super::usage_attempt_allowed(Some(last), Duration::from_secs(300), now));
    }

    /// 時計が巻き戻って duration_since が Err になるケース。R-4 と同じ方針で
    /// 「経過扱い」にして再開を許す（無期限に止まったままにしない）
    #[test]
    fn usage_attempt_allowed_when_clock_went_backwards() {
        let now = SystemTime::now();
        let last = now + Duration::from_secs(50); // last が未来 = duration_since(last) が Err
        assert!(super::usage_attempt_allowed(Some(last), Duration::from_secs(300), now));
    }

    /// gate_usage_attempt は「成否にかかわらず、試行した時点で記録する」ことの確認。
    /// HTTP を一切打たずに、直後の再試行が間隔内でスキップされることだけを検証する
    /// （＝ 429 等で失敗しても記録は残り、間隔内の再試行を抑止できることの土台）
    #[test]
    fn gate_usage_attempt_throttles_subsequent_call_within_interval() {
        let key = "test-gate-basic-throttle";
        assert!(super::gate_usage_attempt(key, Duration::from_secs(300)), "初回は許可されるはず");
        assert!(!super::gate_usage_attempt(key, Duration::from_secs(300)), "直後の再試行は間隔内なのでスキップされるはず");
    }

    /// force 経路（USAGE_FORCE_MIN_ATTEMPT_SECS = 45秒。V-4 で60→45に変更）は
    /// 自動経路より短い下限でスロットルされることの確認
    #[test]
    fn gate_usage_attempt_force_interval_still_throttles_immediate_retry() {
        let key = "test-gate-force-interval";
        let force_interval = Duration::from_secs(super::USAGE_FORCE_MIN_ATTEMPT_SECS as u64);
        assert!(super::gate_usage_attempt(key, force_interval), "初回は許可されるはず");
        assert!(!super::gate_usage_attempt(key, force_interval), "下限秒未満の再試行はスキップされるはず");
    }

    /// 異なるキーは互いに独立してスロットルされる（アカウントごと・経路ごとに個別に
    /// 間隔を管理できることの確認）
    #[test]
    fn gate_usage_attempt_keys_are_independent() {
        assert!(super::gate_usage_attempt("test-gate-key-a", Duration::from_secs(300)));
        assert!(super::gate_usage_attempt("test-gate-key-b", Duration::from_secs(300)), "別キーは影響を受けないはず");
    }

    // V-3（2026-08-22 第6ラウンド）: usage_attempt_min_interval の4通り（ライブ×force）の
    // 戻り値を固定する。従来のテストは「60秒を引数で渡して塞がること」しか見ておらず、
    // 「force のときに USAGE_FORCE_MIN_ATTEMPT_SECS が選ばれること」自体は守っていなかった
    #[test]
    fn usage_attempt_min_interval_live_auto_uses_min_refetch_secs() {
        assert_eq!(
            super::usage_attempt_min_interval(true, false),
            Duration::from_secs(super::USAGE_MIN_REFETCH_SECS as u64),
        );
    }

    #[test]
    fn usage_attempt_min_interval_live_force_uses_force_min_attempt_secs() {
        assert_eq!(
            super::usage_attempt_min_interval(true, true),
            Duration::from_secs(super::USAGE_FORCE_MIN_ATTEMPT_SECS as u64),
        );
    }

    #[test]
    fn usage_attempt_min_interval_non_live_auto_uses_non_live_min_refetch_secs() {
        assert_eq!(
            super::usage_attempt_min_interval(false, false),
            Duration::from_secs(super::NON_LIVE_MIN_REFETCH_SECS as u64),
        );
    }

    /// 非ライブは force の効果を受けない（force_skips_freshness_check と同じ規約）。
    /// force=true でも NON_LIVE_MIN_REFETCH_SECS のままであることを固定する
    #[test]
    fn usage_attempt_min_interval_non_live_force_still_uses_non_live_min_refetch_secs() {
        assert_eq!(
            super::usage_attempt_min_interval(false, true),
            Duration::from_secs(super::NON_LIVE_MIN_REFETCH_SECS as u64),
        );
    }

    // V-1（2026-08-22 第6ラウンド）: ゲート後の資格情報チェックの順序と、保持値による
    // フォールバックを HTTP を打たずに検証する
    use super::{GatedLiveUsageOutcome, LiveUsageError, UsageSummary};

    fn dummy_summary(fetched_at: i64) -> UsageSummary {
        UsageSummary { five_pct: 12.0, seven_pct: 3.0, five_reset: None, seven_reset: None, fetched_at: Some(fetched_at) }
    }

    /// Ok は保持値の有無に関わらずそのまま Ok になる
    #[test]
    fn resolve_gated_live_usage_ok_passes_through_regardless_of_held() {
        let outcome = GatedLiveUsageOutcome::Ok(dummy_summary(100));
        let result = super::resolve_gated_live_usage(outcome, Some(dummy_summary(1)), None);
        assert!(matches!(result, Ok(u) if u.fetched_at == Some(100)));
    }

    /// Failed（資格情報チェックの失敗、または実際の HTTP 失敗）はゲートを経由していないため、
    /// 保持値があっても無視してそのままエラーを伝える（数値のすり替えをしない）
    #[test]
    fn resolve_gated_live_usage_failed_passes_through_even_with_held_value() {
        let outcome = GatedLiveUsageOutcome::Failed(LiveUsageError::Expired);
        let result = super::resolve_gated_live_usage(outcome, Some(dummy_summary(1)), None);
        assert!(matches!(result, Err(LiveUsageError::Expired)));
    }

    /// V-1(b): ゲートで塞がれても保持値（UsageSummary）があればそれを返す（数値が消えない）。
    /// held_error があっても held（数値）を優先する
    #[test]
    fn resolve_gated_live_usage_gated_with_held_value_returns_held_value() {
        let outcome = GatedLiveUsageOutcome::Gated;
        let result = super::resolve_gated_live_usage(
            outcome,
            Some(dummy_summary(42)),
            Some(LiveUsageError::Network),
        );
        assert!(matches!(result, Ok(u) if u.fetched_at == Some(42)));
    }

    /// W-1: ゲートで塞がれ、保持値（UsageSummary）は無いが直近の失敗理由（held_error）は
    /// あるとき、その実際の理由をそのまま返す（RateLimited でも Other でもなく、
    /// 記録されていた本来の理由になること）
    #[test]
    fn resolve_gated_live_usage_gated_without_held_value_uses_held_error() {
        let outcome = GatedLiveUsageOutcome::Gated;
        let result = super::resolve_gated_live_usage(outcome, None, Some(LiveUsageError::Network));
        assert!(matches!(result, Err(LiveUsageError::Network)));
    }

    /// V-1(c)/W-1: ゲートで塞がれ、保持値も held_error も無い（理論上は起きない防御的分岐）
    /// ときは RateLimited を名乗らない
    #[test]
    fn resolve_gated_live_usage_gated_without_held_value_or_error_is_not_rate_limited() {
        let outcome = GatedLiveUsageOutcome::Gated;
        let result = super::resolve_gated_live_usage(outcome, None, None);
        assert!(!matches!(result, Err(LiveUsageError::RateLimited)), "RateLimited を名乗ってはいけない: {result:?}");
    }

    // W-1（2026-08-22 第7ラウンド）: resolve_gated_error の2分岐と、USAGE_LAST_ERROR の
    // 記録・消去・参照（HTTP を打たない状態操作のみを対象にする）

    /// resolve_gated_error: 保持値があればそれをそのまま返す
    #[test]
    fn resolve_gated_error_with_held_returns_it() {
        let result = super::resolve_gated_error(Some(LiveUsageError::Expired));
        assert!(matches!(result, LiveUsageError::Expired));
    }

    /// resolve_gated_error: 保持値が無いとき（理論上は起きない防御的分岐）は RateLimited を
    /// 名乗らず、Other だが従来の「ログインしてください」に紐づいていた固定文言とも異なる
    /// 中立な文言になる
    #[test]
    fn resolve_gated_error_without_held_is_neutral_other_not_rate_limited() {
        let result = super::resolve_gated_error(None);
        match result {
            LiveUsageError::Other(msg) => assert_eq!(msg, "使用量を取得できませんでした"),
            other => panic!("Other になるはず: {other:?}"),
        }
    }

    /// USAGE_LAST_ERROR: 失敗を記録すると last_usage_error で取り出せる
    #[test]
    fn record_usage_error_then_last_usage_error_returns_it() {
        let key = "test-usage-error-record";
        super::record_usage_error(key, LiveUsageError::Network);
        assert!(matches!(super::last_usage_error(key), Some(LiveUsageError::Network)));
    }

    /// USAGE_LAST_ERROR: 成功で消去すると last_usage_error は None に戻る
    #[test]
    fn clear_usage_error_removes_recorded_entry() {
        let key = "test-usage-error-clear";
        super::record_usage_error(key, LiveUsageError::Expired);
        assert!(super::last_usage_error(key).is_some());
        super::clear_usage_error(key);
        assert!(super::last_usage_error(key).is_none());
    }

    /// USAGE_LAST_ERROR: 一度も記録していないキーは None
    #[test]
    fn last_usage_error_is_none_for_unrecorded_key() {
        assert!(super::last_usage_error("test-usage-error-never-recorded").is_none());
    }

    // 資格情報チェックがゲートより先に走ること（＝未ログイン・期限切れでゲートを消費しない）
    // の検証は、実 I/O 抜きでは書けないためテストにしない。
    //
    // 2026-08-22 第6ラウンドのレビューで、ここに `live_usage_summary_gated` を直接呼ぶ
    // テストを置いていたが、次の3つの理由で削除した:
    // 1. ログイン済みの開発機では Keychain を読んで `/api/oauth/usage` へ**実リクエストを送る**。
    //    まさに節約しようとしている枠を、テストを走らせるたびに消費していた
    // 2. その環境では分岐に入らず、アサーションが1つも実行されない空テストだった
    // 3. オフライン・プロキシ下では偽陽性で落ちた（実測で確認済み）
    //
    // 順序そのものは `live_usage_summary_gated` の実装（資格情報の読み取りと期限チェックを
    // `gate_usage_attempt` より手前に置く）で担保する。ゲート判定自体は
    // `usage_attempt_allowed` / `gate_usage_attempt` の単体テストでカバー済み。
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
