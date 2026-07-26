//! メニューバー常駐アイコン。ライブ（現在ログイン中）アカウントの使用状況を表示し、
//! 他の登録アカウントはクリックで Keychain スワップ切り替えができる。5分ごと自動更新。
//!
//! 2026-07-25 ユーザー決定で、監視用長期トークン（`claude setup-token` による長期発行）は
//! 全廃した。2026-07-26、切り替え前にどのアカウントが空いているか見えるようにする要望を受け、
//! 保存済みスナップショットの access token（期限内のときだけ・refresh はしない）で他の
//! 登録アカウントの使用率も表示するようにした（`accounts::get_accounts_usage` 参照）。
//! 併せてトレイからのワンクリック切り替えにも対応した（確認ダイアログを出せないため常に force）。

use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Runtime,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

const TRAY_ID: &str = "status-tray";
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// クリックで切り替えるメニュー項目 id のプレフィックス（id は "switch-<name>"）。
/// name は validate_name により英数字・ハイフン・アンダースコアのみと保証されるため、
/// このプレフィックスを剥がすだけで元の name を復元できる
const SWITCH_ID_PREFIX: &str = "switch-";

/// メニューに流し込む表示専用データ。ネットワーク取得はワーカースレッドで行い、
/// メニュー操作（NSMenu）はメインスレッド限定なので文字列だけを渡す
struct StatusData {
    /// メニューバーに常時出す短い文字列（選択中アカウントの使用率）
    title: String,
    /// 「ログイン中: <名前>」の見出し行（取得できない時は代替文言）
    live_header: String,
    /// 使用率ゲージ行（5h/週次）。ログイン中アカウントが分かる時だけ入る
    usage_lines: Vec<String>,
    /// ライブ以外の登録アカウント。クリックでそのアカウントへ切り替える
    other_accounts: Vec<OtherAccountEntry>,
}

/// 「その他のアカウント」1件分。クリックで切り替えるメニュー項目に使用率も添える
/// （2026-07-26: 切り替え前にどのアカウントが空いているか見えるようにする要望）
struct OtherAccountEntry {
    name: String,
    display_name: String,
    has_credentials: bool,
    usage: Option<crate::accounts::AccountUsage>,
}

/// メニュー項目の末尾に添える使用率サフィックス（例: " — 5h 9% / 週 52%*"）。
/// キャッシュ返し（stale）のときは末尾に "*" を付けるだけに留め、トレイの限られた幅で
/// 「いつ時点か」の詳細までは出さない（詳細はアカウント画面側で見せる）
fn usage_suffix(usage: Option<&crate::accounts::AccountUsage>) -> String {
    let Some(u) = usage else { return String::new() };
    let Some(five) = u.five_pct else { return String::new() };
    let five_text = if u.five_probably_reset {
        "5hリセット済み".to_string()
    } else {
        format!("5h {}%", five.round() as i64)
    };
    let seven_text = u
        .seven_pct
        .map(|p| format!(" / 週{}%", p.round() as i64))
        .unwrap_or_default();
    let stale_mark = if u.stale { "*" } else { "" };
    format!(" — {five_text}{seven_text}{stale_mark}")
}

/// テキストのステータスバー（NSMenu は実バーを描けないので block 文字で表現する）
fn gauge(pct: i64) -> String {
    let p = pct.clamp(0, 100);
    // 10 分割で四捨五入（75% → ▓8つ）
    let filled = ((p + 5) / 10) as usize;
    format!("{}{}", "▓".repeat(filled), "░".repeat(10 - filled))
}

/// epoch 秒をローカル時刻の表示に変換する。今日中なら時刻だけ、それ以外は日付つき
fn reset_local(secs: i64) -> Option<String> {
    use chrono::{Datelike, Local, TimeZone};
    let dt = Local.timestamp_opt(secs, 0).single()?;
    let time = dt.format("%H:%M");
    if dt.date_naive() == Local::now().date_naive() {
        Some(time.to_string())
    } else {
        Some(format!("{}/{} {time}", dt.month(), dt.day()))
    }
}

fn reset_suffix(epoch: Option<i64>) -> String {
    epoch
        .and_then(reset_local)
        .map(|t| format!("（{t} 復活）"))
        .unwrap_or_default()
}

fn fetch_status() -> StatusData {
    // 登録アカウント一覧からライブ（現在ログイン中）を判定し、見出しに実名を出す
    let registered = crate::accounts::registered_accounts();
    let live_name = registered.iter().find(|a| a.is_live).map(|a| a.display_name.clone());

    // 一括照会はここ（トレイの定期更新・手動更新）とアカウント画面を開いた時だけに絞る
    // （レート配慮。get_accounts_usage 自身も前回取得から60秒未満はキャッシュ返しにする）。
    // 監視用長期トークンは復活させず、保存済みスナップショットの access token をそのまま使う
    let usage = crate::accounts::get_accounts_usage().unwrap_or_default();
    let other_accounts: Vec<_> = registered
        .into_iter()
        .filter(|a| !a.is_live)
        .map(|a| {
            let u = usage.iter().find(|u| u.name == a.name).cloned();
            OtherAccountEntry {
                name: a.name,
                display_name: a.display_name,
                has_credentials: a.has_credentials,
                usage: u,
            }
        })
        .collect();

    let mut usage_lines = Vec::new();
    let (live_header, title) = match crate::actions::live_usage_summary() {
        Ok(u) => {
            let f = u.five_pct.round() as i64;
            let s = u.seven_pct.round() as i64;
            usage_lines.push(format!("5h   {} {f}%{}", gauge(f), reset_suffix(u.five_reset)));
            usage_lines.push(format!("週次 {} {s}%{}", gauge(s), reset_suffix(u.seven_reset)));
            let header = match &live_name {
                Some(name) => format!("ログイン中: {name}"),
                None => "ログイン中アカウント".to_string(),
            };
            (header, format!("{}%", u.five_pct.max(u.seven_pct).round() as i64))
        }
        Err(_) => {
            usage_lines.push("使用量を取得できません".into());
            usage_lines.push("Claude Code でログインしてください".into());
            ("ログイン中アカウント".to_string(), "-".to_string())
        }
    };

    StatusData {
        title,
        live_header,
        usage_lines,
        other_accounts,
    }
}

/// StatusData からメニューを組み立てる（メインスレッドで呼ぶこと）
fn build_menu<R: Runtime>(app: &AppHandle<R>, data: &StatusData) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;

    // 情報行。enabled=false だと macOS がグレー表示して読みづらいため、有効のままにして
    // 通常の文字色で出す（クリックしても何も起きない）
    menu.append(&MenuItem::with_id(
        app,
        "info-live-header",
        &data.live_header,
        true,
        None::<&str>,
    )?)?;
    for (i, line) in data.usage_lines.iter().enumerate() {
        menu.append(&MenuItem::with_id(
            app,
            format!("info-usage-{i}"),
            line,
            true,
            None::<&str>,
        )?)?;
    }

    if !data.other_accounts.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&MenuItem::with_id(
            app,
            "info-others-header",
            "その他のアカウント",
            true,
            None::<&str>,
        )?)?;
        for a in &data.other_accounts {
            if a.has_credentials {
                menu.append(&MenuItem::with_id(
                    app,
                    format!("{SWITCH_ID_PREFIX}{}", a.name),
                    format!("「{}」に切り替え{}", a.display_name, usage_suffix(a.usage.as_ref())),
                    true,
                    None::<&str>,
                )?)?;
            } else {
                // 資格情報スナップショットが無いアカウントは切り替え不可（disabled）
                menu.append(&MenuItem::with_id(
                    app,
                    format!("noop-{}", a.name),
                    format!("「{}」（未取り込み）", a.display_name),
                    false,
                    None::<&str>,
                )?)?;
            }
        }
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "refresh",
        "ステータス更新 ♻️",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "アプリを開く",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "check-update",
        "バージョン確認",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "quit",
        "アプリを終了",
        true,
        None::<&str>,
    )?)?;
    Ok(menu)
}

/// 取得（別スレッド）→ メニュー反映（メインスレッド）。
/// アカウント切り替え直後にも外部から呼べるよう公開する（m7: 切り替え後の即時反映）
pub fn refresh<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let data = fetch_status();
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                if let Ok(menu) = build_menu(&handle, &data) {
                    let _ = tray.set_menu(Some(menu));
                }
                // トレイ横の文字列表示（タイトル）は macOS のみ対応。
                // 他 OS はアイコンだけになるため、ホバーのツールチップで使用率を出す
                #[cfg(target_os = "macos")]
                let _ = tray.set_title(Some(&data.title));
                #[cfg(not(target_os = "macos"))]
                let _ = tray.set_tooltip(Some(&format!("CC Anatomy 使用率 {}", data.title)));
            }
        });
    });
}

fn info_dialog<R: Runtime>(app: &AppHandle<R>, title: &str, message: &str) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Info)
        .blocking_show();
}

/// トレイのメニュー項目クリックによるアカウント切り替え。
/// 確認ダイアログを出せない導線のため常に force=true で実行する
/// （実行中ジョブに対する ensure_app_not_busy のハードブロックは維持）。
///
/// switch_account は Keychain 読み書き・profile API 呼び出し等のブロッキング処理を含むため、
/// tokio ランタイムのコンテキストを持つスレッド（NSMenu イベントハンドラ）から直接呼ばず、
/// oauth_get_with_token と同様に素の std::thread::spawn へ逃がす
/// （reqwest::blocking をランタイムコンテキスト内で呼ぶと過去に tokio パニックを踏んでいる）
fn switch_from_tray<R: Runtime>(app: AppHandle<R>, name: String) {
    std::thread::spawn(move || match crate::accounts::switch_account(&name, true) {
        Ok(crate::accounts::SwitchOutcome::Switched { warning }) => {
            refresh(app.clone());
            if let Some(w) = warning {
                info_dialog(&app, "CC Anatomy", &w);
            }
        }
        Ok(crate::accounts::SwitchOutcome::NeedsImport { .. }) => {
            info_dialog(
                &app,
                "CC Anatomy",
                "現在ログイン中のアカウントが未登録のため、トレイからは切り替えられません。\
                 アプリのアカウント画面から操作してください。",
            );
        }
        Ok(crate::accounts::SwitchOutcome::SessionsRunning { .. }) => {
            // force=true では発生しないはずだが、念のため同じ案内で受ける
            info_dialog(
                &app,
                "CC Anatomy",
                "アプリのアカウント画面から操作してください。",
            );
        }
        Err(e) => {
            info_dialog(&app, "CC Anatomy", &format!("切り替えに失敗しました: {e}"));
        }
    });
}

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let placeholder = StatusData {
        title: "…".into(),
        live_header: "取得中…".into(),
        usage_lines: Vec::new(),
        other_accounts: Vec::new(),
    };
    let menu = build_menu(app, &placeholder)?;

    // アイコンはメニューバー向けの小サイズを明示的に埋め込む
    // （ウィンドウ用の大きな透過アイコンを渡すと不可視になることがある）。
    // タイトルも最初から設定し、アイコン描画に失敗しても文字は必ず見えるようにする
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            if let Some(name) = id.strip_prefix(SWITCH_ID_PREFIX) {
                switch_from_tray(app.clone(), name.to_string());
                return;
            }
            match id {
                "refresh" => refresh(app.clone()),
                "check-update" => crate::updater::check(app.clone(), true),
                "open" => {
                    if let Some(win) = app.get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            }
        });
    // メニューバー横の文字列（使用率バッジ）は macOS のみ描画できる
    #[cfg(target_os = "macos")]
    let builder = builder.title("…");
    builder.build(app)?;

    // 初回取得 + 5分ごとの自動更新
    let handle = app.clone();
    std::thread::spawn(move || loop {
        refresh(handle.clone());
        std::thread::sleep(REFRESH_INTERVAL);
    });
    Ok(())
}
