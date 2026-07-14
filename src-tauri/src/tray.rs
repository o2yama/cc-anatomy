//! メニューバー常駐アイコン。登録済みアカウントの使用状況を並列表示する（閲覧のみ）。
//! 切り替えはアプリ内のアカウント画面で行う。5分ごと自動更新。

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

/// 登録アカウントが無いときの表示。ライブ（ログイン中）資格情報の使用量が取れるなら
/// それを出す。アカウント登録機能の無い Windows/Linux ではこれが唯一の経路で、
/// ここでライブを引かないとトレイ監視自体が成立しない
fn live_only_status() -> StatusData {
    match crate::actions::live_usage_summary() {
        Ok(u) => {
            let f = u.five_pct.round() as i64;
            let s = u.seven_pct.round() as i64;
            StatusData {
                title: format!("{}%", u.five_pct.max(u.seven_pct).round() as i64),
                lines: vec![
                    "ログイン中アカウント".into(),
                    format!("5h   {} {f}%{}", gauge(f), reset_suffix(u.five_reset)),
                    format!("週次 {} {s}%{}", gauge(s), reset_suffix(u.seven_reset)),
                ],
            }
        }
        Err(_) => StatusData {
            title: "-".into(),
            #[cfg(target_os = "macos")]
            lines: vec!["アカウント未登録".into(), "CC Anatomy で追加してください".into()],
            #[cfg(not(target_os = "macos"))]
            lines: vec![
                "使用量を取得できません".into(),
                "Claude Code でログインしてください".into(),
            ],
        },
    }
}

fn fetch_status() -> StatusData {
    // 登録済みアカウントの使用状況を並べる（アプリを開かず見比べられるように）。
    // 切り替えはアプリ内のアカウント画面で行うため、ここは閲覧のみ
    let accounts = crate::accounts::accounts_with_usage();
    if accounts.is_empty() {
        return live_only_status();
    }

    let mut lines = Vec::new();
    let mut title = "-".to_string();
    for (i, a) in accounts.iter().enumerate() {
        // アカウントごとにセクションを区切る（先頭以外の前に区切り線）
        if i > 0 {
            lines.push("---".into());
        }
        // メニューバーは閲覧専用なので、意味のある「ログイン中」（起動中セッションの
        // 実際の消費先）だけを印として付ける。アプリ内の選択状態(active)はここには出さない
        let live = if a.is_live { "　⦿ ログイン中" } else { "" };
        lines.push(format!("{}{live}", a.name));
        match &a.usage {
            Some(u) => {
                // バッジはログイン中アカウントの使用率にする（起動中セッションの実際の消費先）
                if a.is_live {
                    title = format!("{}%", u.five_pct.max(u.seven_pct).round() as i64);
                }
                let f = u.five_pct.round() as i64;
                let s = u.seven_pct.round() as i64;
                lines.push(format!("5h   {} {f}%{}", gauge(f), reset_suffix(u.five_reset)));
                lines.push(format!("週次 {} {s}%{}", gauge(s), reset_suffix(u.seven_reset)));
            }
            None => lines.push("使用量取得不可".into()),
        }
    }

    // ログイン中アカウントが未登録なら、ライブ Keychain から直接バッジ用の使用率を取る
    if title == "-" {
        if let Ok(u) = crate::actions::live_usage_summary() {
            title = format!("{}%", u.five_pct.max(u.seven_pct).round() as i64);
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
