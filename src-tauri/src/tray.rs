//! メニューバー常駐アイコン。左クリックでカスタムパネル（tray-panel ウィンドウ）を
//! アイコン直下にトグル表示し、右クリックは最小限のネイティブメニュー
//! （アプリを開く/終了）だけを出す。5分ごとにアイコンのタイトル/ツールチップを自動更新する。
//!
//! 2026-07-26 ユーザー要望で、行を積んだネイティブメニュー（使用率・他アカウント一覧）は
//! 見づらいとして廃止し、CleanMyMac 風のカスタムパネルに置き換えた。パネル自体のデータ取得
//! （get_accounts / get_accounts_usage）とアカウント切り替え（switch_account）はパネルの
//! webview（src/TrayPanel.tsx）から直接 invoke するため、この tray.rs は
//! 「アイコン文字列の定期更新」と「パネルの表示位置決め・トグル・右クリックメニュー」だけを担う。
//! 監視用長期トークンは 2026-07-25 に全廃済みで、ここでは復活させていない。

use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};
use tauri_plugin_positioner::{Position, WindowExt};

const TRAY_ID: &str = "status-tray";
/// tauri.conf.json で定義したカスタムパネルのウィンドウラベル
const PANEL_LABEL: &str = "tray-panel";
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// アイコンのタイトル/ツールチップに出す短い使用率文字列（ライブアカウントのみ）。
/// 他アカウントの詳細はパネル側（TrayPanel.tsx が get_accounts_usage を直接 invoke）に任せる
fn status_text() -> String {
    match crate::actions::live_usage_summary() {
        Ok(u) => format!("{}%", u.five_pct.max(u.seven_pct).round() as i64),
        Err(_) => "-".to_string(),
    }
}

/// 取得（別スレッド）→ タイトル/ツールチップ反映（メインスレッド）。
/// アカウント切り替え直後にも外部から呼べるよう公開する（切り替え後の即時反映）
pub fn refresh<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let text = status_text();
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                #[cfg(target_os = "macos")]
                let _ = tray.set_title(Some(&text));
                #[cfg(not(target_os = "macos"))]
                let _ = tray.set_tooltip(Some(&format!("CC Anatomy 使用率 {text}")));
            }
        });
    });
}

/// 右クリックで出す最小限のネイティブメニュー。使用率・他アカウント一覧はパネルに移した
fn build_minimal_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let open_i = MenuItem::with_id(app, "open", "アプリを開く", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "アプリを終了", true, None::<&str>)?;
    Menu::with_items(app, &[&open_i, &PredefinedMenuItem::separator(app)?, &quit_i])
}

/// パネルをアイコン直下に位置決めしてから表示する。すでに表示中なら隠す（トグル）。
/// 位置決めは tauri-plugin-positioner（tray-icon feature）の TrayBottomCenter を使う
fn toggle_panel<R: Runtime>(app: &AppHandle<R>) {
    let Some(panel) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };
    if panel.is_visible().unwrap_or(false) {
        let _ = panel.hide();
        return;
    }
    let _ = panel.move_window(Position::TrayBottomCenter);
    let _ = panel.show();
    let _ = panel.set_focus();
}

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_minimal_menu(app)?;

    // アイコンはメニューバー向けの小サイズを明示的に埋め込む
    // （ウィンドウ用の大きな透過アイコンを渡すと不可視になることがある）。
    // タイトルも最初から設定し、アイコン描画に失敗しても文字は必ず見えるようにする
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(false)
        .menu(&menu)
        // 左クリックはパネルのトグルに使うため、メニューは右クリックのみに限定する
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            // positioner 側にトレイの現在位置を記録させる（TrayBottomCenter 等の計算に必要）
            tauri_plugin_positioner::on_tray_event(app, &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_panel(app);
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
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
