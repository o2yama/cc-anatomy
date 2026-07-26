//! メニューバー常駐アイコン。ライブ（現在ログイン中）アカウントの使用状況のみ表示する（閲覧のみ）。
//! 切り替えはアプリ内のアカウント画面で行う。5分ごと自動更新。
//!
//! 2026-07-25 ユーザー決定で、監視用長期トークンによる複数アカウント使用率の並列表示は
//! 全廃した。他の登録アカウントは名前だけを列挙し、使用率・リセット時刻は出さない。

use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Runtime,
};

const TRAY_ID: &str = "status-tray";
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// メニューに流し込む表示専用データ。ネットワーク取得はワーカースレッドで行い、
/// メニュー操作（NSMenu）はメインスレッド限定なので文字列だけを渡す
struct StatusData {
    /// メニューバーに常時出す短い文字列（選択中アカウントの使用率）
    title: String,
    /// 登録アカウントの使用状況を並べた行（閲覧のみ・クリック不可）
    lines: Vec<String>,
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
    // ライブ（現在ログイン中）アカウントの使用率だけを表示する。切り替えはアプリ内の
    // アカウント画面で行うため、ここは閲覧のみ
    let mut lines = Vec::new();
    let title = match crate::actions::live_usage_summary() {
        Ok(u) => {
            let f = u.five_pct.round() as i64;
            let s = u.seven_pct.round() as i64;
            lines.push("ログイン中アカウント".into());
            lines.push(format!("5h   {} {f}%{}", gauge(f), reset_suffix(u.five_reset)));
            lines.push(format!("週次 {} {s}%{}", gauge(s), reset_suffix(u.seven_reset)));
            format!("{}%", u.five_pct.max(u.seven_pct).round() as i64)
        }
        Err(_) => {
            lines.push("使用量を取得できません".into());
            lines.push("Claude Code でログインしてください".into());
            "-".to_string()
        }
    };

    // 他の登録アカウントは使用率を取得しない（監視用長期トークンを全廃したため）。
    // 名前だけを列挙し、切り替え先の見当をつけられるようにする
    #[cfg(target_os = "macos")]
    {
        let others: Vec<_> = crate::accounts::registered_accounts()
            .into_iter()
            .filter(|a| !a.is_live)
            .collect();
        if !others.is_empty() {
            lines.push("---".into());
            lines.push("登録済みの他アカウント".into());
            for a in others {
                lines.push(a.display_name);
            }
        }
    }

    StatusData { title, lines }
}

/// StatusData からメニューを組み立てる（メインスレッドで呼ぶこと）
fn build_menu<R: Runtime>(app: &AppHandle<R>, data: &StatusData) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for (i, line) in data.lines.iter().enumerate() {
        if line == "---" {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        } else {
            // 情報行。enabled=false だと macOS がグレー表示して読みづらいため、
            // 有効のままにして通常の文字色で出す（クリックしても何も起きない）。
            // id は連番で一意にする（重複 id は Windows の muda でイベント誤配の恐れ）
            menu.append(&MenuItem::with_id(
                app,
                format!("info-{i}"),
                line,
                true,
                None::<&str>,
            )?)?;
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
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            match event.id.as_ref() {
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
