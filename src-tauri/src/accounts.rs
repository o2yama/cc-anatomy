//! Claude サブスクアカウントの複数管理と切り替え。
//!
//! `claude setup-token` で発行した長期トークン（サブスク用・1年・ローテートなし）を
//! アカウントごとに Keychain に保管し、選択中のものを "CC Anatomy-active" に複製する。
//! シェルは起動時にそのエントリを CLAUDE_CODE_OAUTH_TOKEN として読み込む
//! （この環境変数はサブスク OAuth より優先されるため、ライブ資格情報より前に効く）。
//!
//! ライブの Keychain（"Claude Code-credentials"）は決して書き換えない。
//! 書き換え方式を実機検証したところ、①リフレッシュのたびに refreshToken が
//! ローテートされ旧トークンがサーバー側で即無効化される、②実行中の Claude Code
//! セッションが自分のトークンをライブへ上書きする、の2点により、切り替えた資格情報が
//! 踏み潰されて当該アカウントが再ログイン不能になる。長期トークン方式はこれを回避する。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const ACTIVE_SVC: &str = "CC Anatomy-active";
const TOKEN_SVC_PREFIX: &str = "CC Anatomy-token-";
const SHELL_BEGIN: &str = "# >>> CC Anatomy account switcher >>>";
const SHELL_END: &str = "# <<< CC Anatomy account switcher <<<";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    pub name: String,
    pub email: String,
    pub plan: String,
    /// このアプリが表示・診断に使う選択中アカウント
    pub active: bool,
    /// Claude Code が現在 /login しているアカウント（＝連携なしの起動中セッションが消費する先）
    pub is_live: bool,
    /// Keychain 側のトークンが失われている（手動削除・1年の期限切れ等）。切り替えできない
    pub missing_token: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountsState {
    pub accounts: Vec<Account>,
    /// .zshrc に読み込み行が入っているか。入っていないと切り替えても新しいシェルに効かない
    pub shell_integration: bool,
    /// 起動中の claude CLI セッション数。切り替えを反映するには再起動が要る
    pub running_sessions: usize,
}

#[derive(Serialize, Deserialize, Default)]
struct Meta {
    active: Option<String>,
    accounts: Vec<StoredAccount>,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredAccount {
    name: String,
    email: String,
    plan: String,
    /// 課金先の organization id。長期トークンではメールが取れないため、
    /// アカウントの同一性（重複登録の検出）はこの id で判定する
    #[serde(default)]
    org_id: String,
}

/// 現在 Claude Code が /login しているアカウントの organization id（~/.claude.json 由来）
fn live_org_id() -> Option<String> {
    let json = fs::read_to_string(crate::db::home_dir().join(".claude.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    v.pointer("/oauthAccount/organizationUuid")
        .and_then(|u| u.as_str())
        .map(String::from)
}

/// 長期トークンは profile API のスコープを持たずメールを取得できない。
/// ただし現在 Claude Code にログイン中のアカウントとは organization id で突き合わせられるので、
/// 一致すればそこからメールとプランを復元する
fn identity_from_live_login(org_id: &str) -> Option<(String, String)> {
    if live_org_id().as_deref() != Some(org_id) {
        return None;
    }
    let json = fs::read_to_string(crate::db::home_dir().join(".claude.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let account = v.get("oauthAccount")?;
    Some((
        account.get("emailAddress")?.as_str()?.to_string(),
        account
            .get("organizationType")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
    ))
}

/// 登録アカウントが現在ログイン中か（org_id がライブと一致するか）
fn is_live_account(org_id: &str, live: Option<&str>) -> bool {
    !org_id.is_empty() && live == Some(org_id)
}

fn base_dir() -> PathBuf {
    crate::db::home_dir().join(".claude/cc-anatomy")
}

fn load_meta() -> Meta {
    fs::read_to_string(base_dir().join("accounts.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 途中で落ちて JSON が壊れると、load_meta が既定値へフォールバックして
/// アカウントが全消失したように見える。一時ファイル経由の atomic rename で防ぐ
fn save_meta(meta: &Meta) -> Result<(), String> {
    fs::create_dir_all(base_dir()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    let tmp = base_dir().join("accounts.json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, base_dir().join("accounts.json")).map_err(|e| e.to_string())
}

fn token_svc(name: &str) -> String {
    format!("{TOKEN_SVC_PREFIX}{name}")
}

/// Keychain の値は秘密なので、戻り値をログ・エラーメッセージに載せない
fn keychain_read(service: &str) -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn keychain_write(service: &str, secret: &str) -> Result<(), String> {
    let user = std::env::var("USER").unwrap_or_else(|_| "claude".into());
    let status = Command::new("security")
        .args([
            "add-generic-password",
            "-a",
            &user,
            "-s",
            service,
            "-w",
            secret,
            "-U",
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Keychain への保存に失敗しました".into())
    }
}

/// エントリが元々無い場合も失敗するので、呼び出し側は「消えていること」だけを期待する
fn keychain_delete(service: &str) {
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", service])
        .output();
}

/// active の削除は「消えたはずのアカウントを消費し続ける」事故に直結するため、
/// 消えたことを読み直して確かめる
fn keychain_delete_verified(service: &str) -> Result<(), String> {
    keychain_delete(service);
    if keychain_read(service).is_some() {
        return Err("Keychain からトークンを削除できませんでした".into());
    }
    Ok(())
}

/// シェルの単一引用符に安全に埋め込む。パスにクオートが混ざっても実行内容が変わらないようにする
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// AppleScript の文字列リテラルに埋め込む（osascript -e に渡す1行の中で使う）
fn applescript_quote(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', "\\\"")
}

/// Keychain のサービス名とシェル引数に埋め込むため、名前は英数字・ハイフン・アンダースコアに限る
fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err("アカウント名は英数字・ハイフン・アンダースコア（32文字以内）で指定してください".into())
    }
}

/// 選択中アカウントの長期トークン。未登録なら None（＝従来どおりライブ資格情報を使う）
pub fn active_token() -> Option<String> {
    keychain_read(ACTIVE_SVC)
}

/// メニューバーのアカウント一覧・切り替え用。各アカウントの使用率を添えて返す。
/// トークンごとに推論 API を1回叩くため、呼び出しはワーカースレッドから行うこと
pub struct TrayAccount {
    pub name: String,
    pub is_live: bool,
    /// 5時間枠・7日枠の使用率（0〜100）。取得できなければ None
    pub usage: Option<(f64, f64)>,
}

pub fn accounts_with_usage() -> Vec<TrayAccount> {
    let meta = load_meta();
    let live = live_org_id();
    meta.accounts
        .iter()
        .map(|a| TrayAccount {
            name: a.name.clone(),
            is_live: is_live_account(&a.org_id, live.as_deref()),
            usage: keychain_read(&token_svc(&a.name))
                .and_then(|t| crate::actions::usage_summary(&t).ok()),
        })
        .collect()
}

/// アプリ内ポップオーバーのカルーセル用。全アカウントの素性＋詳細な使用量を返す
#[derive(Serialize)]
pub struct AccountUsageDetail {
    pub name: String,
    pub email: String,
    /// "claude_max" など。UI で "Max" に整形する
    pub plan: String,
    pub active: bool,
    pub is_live: bool,
    /// RateLimits 形（five_hour / seven_day / limits）。取得できなければ null
    pub usage: Option<serde_json::Value>,
    /// 取得に失敗した理由（null なら成功）
    pub error: Option<String>,
}

pub fn accounts_usage_detail() -> Vec<AccountUsageDetail> {
    let meta = load_meta();
    let active = meta.active.clone();
    let live = live_org_id();
    meta.accounts
        .iter()
        .map(|a| {
            let (usage, error) = match keychain_read(&token_svc(&a.name)) {
                Some(t) => match crate::actions::rate_limits_value(&t) {
                    Ok(v) => (Some(v), None),
                    Err(e) => (None, Some(e)),
                },
                None => (None, Some("トークンが見つかりません".to_string())),
            };
            AccountUsageDetail {
                name: a.name.clone(),
                email: a.email.clone(),
                plan: a.plan.clone(),
                active: active.as_deref() == Some(a.name.as_str()),
                is_live: is_live_account(&a.org_id, live.as_deref()),
                usage,
                error,
            }
        })
        .collect()
}

/// 選択中アカウントの表示用情報 (name, email, plan)。
/// claim / backfill 時に解決して accounts.json に保存済みのものを返す
pub fn active_display() -> Option<(String, String, String)> {
    let meta = load_meta();
    let active = meta.active?;
    meta.accounts
        .iter()
        .find(|a| a.name == active)
        .map(|a| (a.name.clone(), a.email.clone(), a.plan.clone()))
}

/// アプリ自身が claude CLI を起動するときに渡す環境変数。
/// これが無いと、UI で選択したアカウントと診断・タスク抽出の消費先がずれる
pub fn claude_env() -> Vec<(String, String)> {
    match active_token() {
        Some(token) => vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), token)],
        None => vec![],
    }
}

/// ユーザーが開いている Claude Code セッション数（＝再起動すれば切り替わるもの）。
///
/// claude CLI のプロセスをそのまま数えると、claude-mem のワーカーが起動した常駐 claude
/// （親が bun 等）まで混ざり、実測でセッション数を5割ほど過大に報告した。
/// 「開き直してください」と言える対象はシェルから起動されたものだけなので、親で絞る。
/// デスクトップアプリ（Claude.app）は別系統なので除く
fn running_sessions() -> usize {
    const SHELLS: [&str; 4] = ["zsh", "bash", "fish", "sh"];

    let Ok(out) = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()
    else {
        return 0;
    };
    let text = String::from_utf8_lossy(&out.stdout);

    let basename = |s: &str| -> Option<String> {
        PathBuf::from(s)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    };

    // 親コマンドを引けるよう、まず pid -> comm を作る
    let mut comm_by_pid: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    let mut claude_procs: Vec<(u32, u32)> = Vec::new();
    for line in text.lines() {
        let mut it = line.trim().splitn(3, char::is_whitespace);
        let (Some(pid), Some(ppid), Some(comm)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.trim().parse::<u32>()) else {
            continue;
        };
        let comm = comm.trim().to_string();
        let is_claude = !comm.contains("Claude.app")
            && basename(&comm).is_some_and(|n| n == "claude");
        if is_claude {
            claude_procs.push((pid, ppid));
        }
        comm_by_pid.insert(pid, comm);
    }

    claude_procs
        .iter()
        .filter(|(_, ppid)| {
            comm_by_pid
                .get(ppid)
                .and_then(|c| basename(c))
                .is_some_and(|n| SHELLS.contains(&n.as_str()))
        })
        .count()
}

fn zshrc_path() -> PathBuf {
    crate::db::home_dir().join(".zshrc")
}

fn shell_integration_installed() -> bool {
    fs::read_to_string(zshrc_path())
        .map(|s| s.contains(SHELL_BEGIN))
        .unwrap_or(false)
}

/// org_id を持たない古い登録（メールが引けず「(メール未取得)」になっていたもの）を、
/// トークンを1度だけ probe して埋め直す。以降は API を叩かない
fn backfill_identities(meta: &mut Meta) {
    let stale: Vec<String> = meta
        .accounts
        .iter()
        .filter(|a| a.org_id.is_empty())
        .map(|a| a.name.clone())
        .collect();
    if stale.is_empty() {
        return;
    }
    let mut changed = false;
    for name in stale {
        let Some(token) = keychain_read(&token_svc(&name)) else {
            continue;
        };
        let crate::actions::TokenCheck::Valid(org_id) = crate::actions::check_oauth_token(&token)
        else {
            continue;
        };
        let live = identity_from_live_login(&org_id);
        if let Some(acct) = meta.accounts.iter_mut().find(|a| a.name == name) {
            acct.org_id = org_id.clone();
            if acct.email.is_empty() || acct.email.starts_with('(') {
                acct.email = live
                    .as_ref()
                    .map(|(e, _)| e.clone())
                    .unwrap_or_else(|| format!("org {}", &org_id[..8.min(org_id.len())]));
            }
            if acct.plan.is_empty() {
                if let Some((_, plan)) = live {
                    acct.plan = plan;
                }
            }
            changed = true;
        }
    }
    if changed {
        let _ = save_meta(meta);
    }
}

pub fn get_accounts() -> Result<AccountsState, String> {
    let mut meta = load_meta();
    backfill_identities(&mut meta);
    let meta = meta;
    let live = live_org_id();
    let accounts = meta
        .accounts
        .iter()
        .map(|a| Account {
            name: a.name.clone(),
            email: a.email.clone(),
            plan: a.plan.clone(),
            active: meta.active.as_deref() == Some(a.name.as_str()),
            is_live: is_live_account(&a.org_id, live.as_deref()),
            missing_token: keychain_read(&token_svc(&a.name)).is_none(),
        })
        .collect();
    Ok(AccountsState {
        accounts,
        shell_integration: shell_integration_installed(),
        running_sessions: running_sessions(),
    })
}

/// `claude setup-token` はブラウザ認証を伴う対話フローなので、GUI から隠して実行できない。
/// Terminal.app で対話させ、取得したトークンはヘルパースクリプトが直接 Keychain に入れる
/// （トークンを GUI やファイルに materialize させないため）
pub fn add_account_in_terminal(app: &tauri::AppHandle, name: &str) -> Result<(), String> {
    use tauri::Manager;
    validate_name(name)?;
    if load_meta().accounts.iter().any(|a| a.name == name) {
        return Err(format!("アカウント「{name}」はすでに登録されています"));
    }
    // 中断した過去の追加が孤児トークンを残していることがある。消さずに始めると、
    // ポーリングがブラウザ認証の完了前にその古いトークンを掴んで登録してしまう
    keychain_delete(&token_svc(name));
    let claude = crate::actions::resolve_claude_bin()?;
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("accounts");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let script_path = dir.join("add-account.sh");
    // claude CLI は stdout がパイプだと bun ランタイムが TTY を要求して起動時に落ちる
    // （EINVAL: kqueue / process.stderr.fd）。出力を捕捉するには pty が要るので script(1) を挟む。
    //
    // 本文には awk のブロックや ${#token} など波括弧が多いので、format! ではなく
    // プレースホルダ置換で組み立てる（format! だとすべて {{ }} にエスケープする必要がある）
    const ADD_ACCOUNT_SCRIPT: &str = r#"#!/bin/zsh
# CC Anatomy: アカウント追加ヘルパー。トークンは Keychain にのみ保存する
set -uo pipefail
profile="$1"

# シェル連携済みの端末では既存アカウントのトークンが export されている。
# それを持ったまま setup-token を実行すると別アカウントでログインできない
unset CLAUDE_CODE_OAUTH_TOKEN

echo "==================================================="
echo " CC Anatomy: アカウント「$profile」を追加します"
echo " ブラウザが開いたら、登録したいアカウントでログインしてください。"
echo "==================================================="
echo

# 記録ファイルは自分だけが読める場所に置き、トークン抽出後すぐ上書き削除する
log="$(dirname "$0")/setup-token.log"
umask 077
: > "$log"

# setup-token は端末幅でトークンを折り返す。折り返すと1行では拾えず、行を継ぎ足す方式は
# 「Store this token securely」等の本文まで巻き込んだ（実機で155文字の壊れたトークンを生成）。
# pty を十分広くして折り返し自体を起こさせるのをやめる
script -q "$log" zsh -c "stty cols 400 >/dev/null 2>&1; exec __CLAUDE_BIN__ setup-token"
rc=$?

# Ink の再描画でトークンが途中まで描かれた行も記録に混ざるため、最長一致を採用する
token=$(sed $'s/\033\[[0-9;?]*[a-zA-Z]//g' "$log" 2>/dev/null | tr -d '\r' \
  | grep -oE 'sk-ant-oat[0-9]+-[A-Za-z0-9_-]+' \
  | awk '{ if (length($0) > length(best)) best = $0 } END { print best }')
rm -Pf "$log" 2>/dev/null || rm -f "$log"

if [ -z "$token" ]; then
  echo
  if [ $rc -ne 0 ]; then
    echo "setup-token がエラー終了しました（上の出力を確認してください）。"
  else
    echo "トークンを自動で取得できませんでした。"
  fi
  echo "上に表示されたトークン（sk-ant-oat… ）があれば貼り付けてください。"
  printf '入力は画面に表示されません（中止する場合は空のまま Enter）: '
  read -rs token
  echo
fi

case "$token" in
  sk-ant-oat*) ;;
  *) echo "トークンを取得できなかったため中止しました。"; exit 1 ;;
esac

# 折り返しの結合に失敗すると、先頭だけの切れたトークンが通ってしまう。
# 実物は 100 文字強なので、短すぎるものは壊れたとみなす
if [ ${#token} -lt 60 ]; then
  echo "取得したトークンが短すぎます（${#token} 文字）。保存せず中止しました。"
  exit 1
fi

security add-generic-password -a "$USER" -s "CC Anatomy-token-$profile" -w "$token" -U || exit 1
unset token
echo
echo "✅ 「$profile」を保存しました。CC Anatomy に戻ってください。"
"#;
    let script = ADD_ACCOUNT_SCRIPT.replace(
        "__CLAUDE_BIN__",
        &shell_quote(&claude.display().to_string()),
    );
    fs::write(&script_path, script).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }

    // シェル用（単一引用符）と AppleScript 用（"・\）の二段でエスケープする
    let command = format!(
        "{} {}",
        shell_quote(&script_path.display().to_string()),
        shell_quote(name)
    );
    let status = Command::new("osascript")
        .args([
            "-e",
            &format!(
                "tell application \"Terminal\" to do script \"{}\"",
                applescript_quote(&command)
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

/// Terminal 側のログイン完了をフロントがポーリングする。
/// Keychain にトークンが現れたら profile API で素性を確定し、メタデータに登録する
pub fn claim_pending_account(name: &str) -> Result<Option<Account>, String> {
    validate_name(name)?;
    let Some(token) = keychain_read(&token_svc(name)) else {
        return Ok(None);
    };

    // 壊れたトークンをここで弾かないと、切り替えても CLI が黙って別アカウントに
    // フォールバックするだけで、ユーザーは気づけない。
    // ただし失敗してもトークンは消さない。レート上限や通信断で消すと、有効な1年トークンを
    // 巻き添えで破壊する（実際に破壊した）。取り直しは追加操作の冒頭で行われる
    let org_id = match crate::actions::check_oauth_token(&token) {
        crate::actions::TokenCheck::Valid(org) => org,
        crate::actions::TokenCheck::Invalid => {
            return Err(
                "取得したトークンが無効でした。もう一度「アカウントを追加」からやり直してください。"
                    .into(),
            )
        }
        crate::actions::TokenCheck::Unavailable(e) => {
            // ポーリングが続くので、次の周回で再試行される
            return Err(format!("トークンを確認できませんでした（{e}）。再試行します。"));
        }
    };

    let mut meta = load_meta();
    if let Some(dup) = meta
        .accounts
        .iter()
        .find(|a| !a.org_id.is_empty() && a.org_id == org_id && a.name != name)
    {
        return Err(format!(
            "このトークンは既存の「{}」と同じアカウントです。別のアカウントでログインしてください。",
            dup.name
        ));
    }

    // 長期トークンではメールを引けないので、ログイン中アカウントと org_id が一致したときだけ
    // 素性を復元する。一致しなければ org の短縮表記で識別する
    let live = identity_from_live_login(&org_id);
    let email = live
        .as_ref()
        .map(|(e, _)| e.clone())
        .unwrap_or_else(|| format!("org {}", &org_id[..8.min(org_id.len())]));
    let plan = live.map(|(_, p)| p).unwrap_or_default();

    let is_live = is_live_account(&org_id, live_org_id().as_deref());
    meta.accounts.retain(|a| a.name != name);
    meta.accounts.push(StoredAccount {
        name: name.to_string(),
        email: email.clone(),
        plan: plan.clone(),
        org_id,
    });
    // 最初に登録したアカウントは、切り替え操作を待たずに有効化する
    let first = meta.active.is_none();
    if first {
        keychain_write(ACTIVE_SVC, &token)?;
        meta.active = Some(name.to_string());
    }
    // メタデータの保存に失敗したまま active だけ有効化すると、UI とトークン消費先がずれる
    if let Err(e) = save_meta(&meta) {
        if first {
            keychain_delete(ACTIVE_SVC);
        }
        return Err(e);
    }

    Ok(Some(Account {
        name: name.to_string(),
        email,
        plan,
        active: first,
        is_live,
        missing_token: false,
    }))
}

/// Keychain の active を書き換える前に登録済みか確かめる。
/// 逆順だと、エラーを返しつつトークン消費先だけ変わった状態になる
pub fn switch_account(name: &str) -> Result<(), String> {
    validate_name(name)?;
    let mut meta = load_meta();
    if !meta.accounts.iter().any(|a| a.name == name) {
        return Err(format!("アカウント「{name}」は登録されていません"));
    }
    let token = keychain_read(&token_svc(name)).ok_or_else(|| {
        format!("「{name}」のトークンが Keychain にありません。登録し直してください")
    })?;

    // 無効なトークンに切り替えると、CLI は警告なくログイン中アカウントへフォールバックし、
    // 「切り替えたつもりで別アカウントに課金」になる。切り替える前に生死を確かめる。
    // 枠を使い切ったアカウント（429）は切り替え先として正当なので通す
    if let crate::actions::TokenCheck::Invalid = crate::actions::check_oauth_token(&token) {
        return Err(format!(
            "「{name}」のトークンが無効です。削除して登録し直してください。"
        ));
    }

    let previous = meta.active.clone();
    keychain_write(ACTIVE_SVC, &token)?;
    meta.active = Some(name.to_string());
    if let Err(e) = save_meta(&meta) {
        // メタが古いままだと表示と実際の消費先がずれるので、Keychain を元に戻す
        match previous.and_then(|p| keychain_read(&token_svc(&p))) {
            Some(prev_token) => {
                let _ = keychain_write(ACTIVE_SVC, &prev_token);
            }
            None => keychain_delete(ACTIVE_SVC),
        }
        return Err(e);
    }
    Ok(())
}

/// active を先に確実に消す。消し損ねたままメタだけ更新すると、
/// UI から消えたアカウントを裏で消費し続ける（気づけない不整合になる）
pub fn remove_account(name: &str) -> Result<(), String> {
    validate_name(name)?;
    let mut meta = load_meta();
    let was_active = meta.active.as_deref() == Some(name);
    if was_active {
        keychain_delete_verified(ACTIVE_SVC)?;
        meta.active = None;
    }
    meta.accounts.retain(|a| a.name != name);
    save_meta(&meta)?;
    keychain_delete(&token_svc(name));
    Ok(())
}

/// 新しいシェルが選択中アカウントを拾えるようにする。
/// トークンは .zshrc に書かず、起動のたびに Keychain から読む。
///
/// ユーザーの shell 設定を壊すと復旧が重いので、
/// 読めない .zshrc は空とみなさず中断し、書き込みは一時ファイル経由の atomic rename にする
pub fn install_shell_integration() -> Result<(), String> {
    if shell_integration_installed() {
        return Ok(());
    }
    let path = zshrc_path();
    let mut content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(format!(
                ".zshrc を読めなかったため中断しました（上書きによる破壊を避けます）: {e}"
            ))
        }
    };

    if !content.is_empty() {
        let backup = crate::db::home_dir().join(".zshrc.cc-anatomy.bak");
        if !backup.exists() {
            fs::copy(&path, &backup)
                .map_err(|e| format!(".zshrc のバックアップに失敗したため中断しました: {e}"))?;
        }
        if !content.ends_with('\n') {
            content.push('\n');
        }
    }

    // 未登録のときに空文字を export すると認証が壊れるので、値があるときだけ export する
    content.push_str(&format!(
        "\n{SHELL_BEGIN}\n\
         __cc_anatomy_token=\"$(security find-generic-password -s 'CC Anatomy-active' -w 2>/dev/null)\"\n\
         [ -n \"$__cc_anatomy_token\" ] && export CLAUDE_CODE_OAUTH_TOKEN=\"$__cc_anatomy_token\"\n\
         unset __cc_anatomy_token\n\
         {SHELL_END}\n"
    ));

    let tmp = crate::db::home_dir().join(".zshrc.cc-anatomy.tmp");
    fs::write(&tmp, &content).map_err(|e| format!(".zshrc の更新に失敗: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!(".zshrc の更新に失敗: {e}"))
}
