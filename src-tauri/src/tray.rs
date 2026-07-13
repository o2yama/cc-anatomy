//! メニューバー常駐アイコン。クリックで /status 相当の情報（アカウント・プラン・
//! レートリミット各枠・今日の活動）をメニュー表示する。5分ごと自動更新。

use serde_json::Value;
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Runtime,
};

const TRAY_ID: &str = "status-tray";
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// メニューに流し込む表示専用データ。ネットワーク・DB 取得はワーカースレッドで行い、
/// メニュー操作（NSMenu）はメインスレッド限定なので文字列だけを渡す
struct StatusData {
    /// メニューバーに常時出す短い文字列（セッション枠の使用率）
    title: String,
    /// 情報行（クリック不可の行として並べる）
    lines: Vec<String>,
}

fn gauge(pct: i64) -> String {
    let filled = (pct.clamp(0, 100) / 10) as usize;
    format!("{}{} {pct}%", "▓".repeat(filled), "░".repeat(10 - filled))
}

/// UTC ISO8601 をローカル "M/D H:MM" に変換。
/// 依存クレートを増やさないため date コマンドで epoch を経由する
fn reset_local(iso: &str) -> String {
    let base = iso.split('.').next().unwrap_or(iso);
    let utc = format!("{}+0000", base.trim_end_matches('Z'));
    let epoch = std::process::Command::new("date")
        .args(["-j", "-f", "%Y-%m-%dT%H:%M:%S%z", &utc, "+%s"])
        .output();
    if let Ok(o) = epoch {
        if o.status.success() {
            let secs = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if let Ok(o2) = std::process::Command::new("date")
                .args(["-r", &secs, "+%-m/%-d %H:%M"])
                .output()
            {
                if o2.status.success() {
                    return String::from_utf8_lossy(&o2.stdout).trim().to_string();
                }
            }
        }
    }
    // 変換失敗時は日付部分だけそのまま出す
    iso.split('T').next().unwrap_or(iso).to_string()
}

fn limit_line(l: &Value) -> Option<String> {
    let kind = l.get("kind")?.as_str()?;
    let pct = l.get("percent")?.as_i64()?;
    let label = match kind {
        "session" => "5時間枠".to_string(),
        "weekly_all" => "週間・全体".to_string(),
        "weekly_scoped" => {
            let model = l
                .pointer("/scope/model/display_name")
                .and_then(Value::as_str)
                .unwrap_or("モデル別");
            format!("週間・{model}")
        }
        other => other.to_string(),
    };
    let reset = l
        .get("resets_at")
        .and_then(Value::as_str)
        .map(reset_local)
        .unwrap_or_default();
    Some(format!("{label}: {}（{reset} 復活）", gauge(pct)))
}

fn fetch_status() -> StatusData {
    let mut lines = Vec::new();
    let mut title = "-".to_string();

    match crate::actions::get_account_profile()
        .and_then(|p| serde_json::from_str::<Value>(&p).map_err(|e| e.to_string()))
    {
        Ok(p) => {
            let name = p
                .pointer("/account/display_name")
                .and_then(Value::as_str)
                .unwrap_or("(不明)");
            let email = p
                .pointer("/account/email")
                .and_then(Value::as_str)
                .unwrap_or("");
            let tier = p
                .pointer("/organization/rate_limit_tier")
                .and_then(Value::as_str)
                .unwrap_or("");
            lines.push(format!("{name} · {email}"));
            lines.push(format!("プラン: {}", plan_label(tier)));
        }
        Err(e) => lines.push(format!("アカウント取得失敗: {e}")),
    }

    match crate::actions::get_rate_limits()
        .and_then(|u| serde_json::from_str::<Value>(&u).map_err(|e| e.to_string()))
    {
        Ok(u) => {
            lines.push("---".into());
            if let Some(limits) = u.get("limits").and_then(Value::as_array) {
                for l in limits {
                    if let Some(line) = limit_line(l) {
                        lines.push(line);
                    }
                }
            }
            if let Some(pct) = u.pointer("/five_hour/utilization").and_then(Value::as_f64) {
                title = format!("{}%", pct.round() as i64);
            }
            let extra = u
                .pointer("/extra_usage/is_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            lines.push(format!(
                "追加クレジット: {}",
                if extra { "有効" } else { "無効" }
            ));
        }
        Err(e) => lines.push(format!("使用状況取得失敗: {e}")),
    }

    if let Ok((count, last)) = crate::db::today_activity() {
        lines.push("---".into());
        lines.push(format!("今日のセッション: {count}件"));
        if let Some(p) = last {
            lines.push(format!("直近プロジェクト: {p}"));
        }
    }

    StatusData { title, lines }
}

fn plan_label(tier: &str) -> String {
    // "default_claude_max_20x" → "Max 20x"
    if let Some(rest) = tier.strip_prefix("default_claude_") {
        let mut parts = rest.splitn(2, '_');
        let plan = parts.next().unwrap_or(rest);
        let mult = parts.next().unwrap_or("");
        if plan.is_empty() {
            return tier.to_string();
        }
        let mut label = format!("{}{}", plan[..1].to_uppercase(), &plan[1..]);
        if !mult.is_empty() {
            label.push(' ');
            label.push_str(mult);
        }
        return label;
    }
    tier.to_string()
}

/// StatusData からメニューを組み立てる（メインスレッドで呼ぶこと）
fn build_menu<R: Runtime>(app: &AppHandle<R>, data: &StatusData) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for line in &data.lines {
        if line == "---" {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        } else {
            // 情報行。enabled=false だと macOS がグレー表示して読みづらいため、
            // 有効のままにして通常の文字色で出す（クリックしても何も起きない）
            menu.append(&MenuItem::with_id(app, "", line, true, None::<&str>)?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "refresh",
        "今すぐ更新",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "CC Anatomy を開く",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "終了", true, None::<&str>)?)?;
    Ok(menu)
}

/// 取得（別スレッド）→ メニュー反映（メインスレッド）
fn refresh<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let data = fetch_status();
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                if let Ok(menu) = build_menu(&handle, &data) {
                    let _ = tray.set_menu(Some(menu));
                }
                let _ = tray.set_title(Some(&data.title));
            }
        });
    });
}

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let placeholder = StatusData {
        title: "…".into(),
        lines: vec!["取得中…".into()],
    };
    let menu = build_menu(app, &placeholder)?;

    // アイコンはメニューバー向けの小サイズを明示的に埋め込む
    // （ウィンドウ用の大きな透過アイコンを渡すと不可視になることがある）。
    // タイトルも最初から設定し、アイコン描画に失敗しても文字は必ず見えるようにする
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(false)
        .title("…")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "refresh" => refresh(app.clone()),
            "open" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    // 初回取得 + 5分ごとの自動更新
    let handle = app.clone();
    std::thread::spawn(move || loop {
        refresh(handle.clone());
        std::thread::sleep(REFRESH_INTERVAL);
    });
    Ok(())
}
