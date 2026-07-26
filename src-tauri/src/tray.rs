//! メニューバー常駐アイコン。ライブ（現在ログイン中）アカウントの使用状況を表示し、
//! 他の登録アカウントはクリックで Keychain スワップ切り替えができる。5分ごと自動更新。
//!
//! 2026-07-25 ユーザー決定で、監視用長期トークン（`claude setup-token` による長期発行）は
//! 全廃した。2026-07-26、切り替え前にどのアカウントが空いているか見えるようにする要望を受け、
//! 保存済みスナップショットの access token（期限内のときだけ・refresh はしない）で他の
//! 登録アカウントの使用率も表示するようにした（`accounts::get_accounts_usage` 参照）。
//! 併せてトレイからのワンクリック切り替えにも対応した（確認ダイアログを出せないため常に force）。
//!
//! 2026-07-26（同日中）、CleanMyMac 風のカスタムパネルに一時置き換えたが、ユーザー評価が
//! 不評だったためこのネイティブメニュー実装へ回帰した（詳細は仕様書の決定変更ログ参照）。
//!
//! 使用率ゲージは当初ブロック文字（▓░ 等）で表現していたが、2026-07-26
//! （さらに同日中）ユーザー承認により動的生成した RGBA バー画像
//! （`tauri::menu::IconMenuItem`）に置き換えた。`render_bar_pixels` が新規クレート無しで
//! 純 Rust に RGBA バッファを直接描く（アンチエイリアス・角丸なし）。

use std::time::Duration;
use tauri::{
    image::Image,
    menu::{IconMenuItemBuilder, Menu, MenuItem, PredefinedMenuItem},
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
    usage_lines: Vec<InfoLine>,
    /// ライブ以外の登録アカウント。クリックでそのアカウントへ切り替える
    other_accounts: Vec<OtherAccountEntry>,
}

/// メニューの情報行1つ分。ゲージ行は画像バー付きの `IconMenuItem`、それ以外は
/// 通常のテキスト `MenuItem` として描画する（`build_menu` 側で分岐する）
enum InfoLine {
    Plain(String),
    Gauge {
        /// バー画像の左に出すラベル（例: "5h 67%（19:00 復活）"）。バー自体はもう
        /// テキストに含めない（画像アイコンで描く）
        label: String,
        pct: i64,
        /// true なら「その他のアカウント」用のグレー配色、false ならライブ用の使用率配色
        muted: bool,
    },
}

/// 「その他のアカウント」1件分。クリックで切り替えるメニュー項目に使用率も添える
/// （2026-07-26: 切り替え前にどのアカウントが空いているか見えるようにする要望）
struct OtherAccountEntry {
    name: String,
    display_name: String,
    has_credentials: bool,
    usage: Option<crate::accounts::AccountUsage>,
}

/// 「その他のアカウント」1件分の使用率行。ログイン中セクションと同じ2行構成
/// （`5h <バー> x%（リセット時刻）` / `週次 <バー> y%（リセット時刻）`）で揃える
/// （2026-07-26 ユーザー要望: サフィックス方式では見づらいため統一した）。
/// stale（キャッシュ返し）のときは % の直後に "*" を付ける。usage 自体が無い
/// （キャッシュも無い）ときは縦に長くなりすぎないよう「未取得」1行（画像なし）にまとめる。
///
/// バーはグレー配色（`muted: true`）にして、ライブが主・その他アカウントが従という
/// 視覚的階層をつける（2026-07-26 追加要望）
fn usage_info_lines(usage: Option<&crate::accounts::AccountUsage>) -> Vec<InfoLine> {
    let Some(u) = usage else { return vec![InfoLine::Plain("未取得".to_string())] };
    let Some(five_pct) = u.five_pct else { return vec![InfoLine::Plain("未取得".to_string())] };
    // バックエンドの stale フラグ（今回キャッシュ返しだったか）だけでなく、実際の経過時間
    // （5分以上）でも判定し直す。取得直後でも stale=true になりうるため、そのままだと
    // 「*」が常時ちらつく（2026-07-26 ユーザー指摘。Accounts.tsx の「0分前時点」と同じ問題）
    let stale_mark = if is_display_stale(u.fetched_at, now_epoch()) { "*" } else { "" };
    // リセット時刻を過ぎている想定なら実質 0% とみなす
    let five_val = if u.five_probably_reset { 0 } else { five_pct.round() as i64 };
    let seven_val = u.seven_pct.unwrap_or(0.0).round() as i64;
    vec![
        InfoLine::Gauge {
            label: format!("5h {five_val}%{stale_mark}{}", reset_suffix(u.five_reset)),
            pct: five_val,
            muted: true,
        },
        InfoLine::Gauge {
            label: format!("週次 {seven_val}%{stale_mark}{}", reset_suffix(u.seven_reset)),
            pct: seven_val,
            muted: true,
        },
    ]
}

/// バー画像のピクセルサイズ。論理サイズ 110×9px 相当を Retina（@2x）で描き、
/// メニューでも滲まず自然な大きさに見えるようにする
const BAR_WIDTH_PX: u32 = 220;
const BAR_HEIGHT_PX: u32 = 18;

/// トラック（背景）色は共通の暗いグレー
const TRACK_COLOR: (u8, u8, u8) = (0x3a, 0x3a, 0x3c);
/// 「その他のアカウント」の塗り色（ミディアムグレー。ライブとの階層を保つ）
const OTHER_FILL_COLOR: (u8, u8, u8) = (0x8e, 0x8e, 0x93);

/// ライブアカウントのバー色（<80% 緑・80〜95% 黄・>95% 赤）
fn live_fill_color(pct: i64) -> (u8, u8, u8) {
    if pct > 95 {
        (0xff, 0x3b, 0x30)
    } else if pct >= 80 {
        (0xff, 0xcc, 0x00)
    } else {
        (0x34, 0xc7, 0x59)
    }
}

/// pct% ぶんを fill 色、残りを track 色で塗った単純な RGBA バーのピクセルバッファを作る
/// （テスト容易性のため `Image` への変換とは分離する）。アンチエイリアス・角丸は付けない
fn render_bar_pixels(pct: i64, fill: (u8, u8, u8), track: (u8, u8, u8)) -> Vec<u8> {
    let p = pct.clamp(0, 100);
    let fill_w = (i64::from(BAR_WIDTH_PX) * p / 100) as u32;
    let mut buf = Vec::with_capacity((BAR_WIDTH_PX * BAR_HEIGHT_PX * 4) as usize);
    for _y in 0..BAR_HEIGHT_PX {
        for x in 0..BAR_WIDTH_PX {
            let (r, g, b) = if x < fill_w { fill } else { track };
            buf.extend_from_slice(&[r, g, b, 255]);
        }
    }
    buf
}

/// バー画像を組み立てる（IconMenuItem に渡す）
fn gauge_image(pct: i64, muted: bool) -> Image<'static> {
    let fill = if muted { OTHER_FILL_COLOR } else { live_fill_color(pct) };
    let pixels = render_bar_pixels(pct, fill, TRACK_COLOR);
    Image::new_owned(pixels, BAR_WIDTH_PX, BAR_HEIGHT_PX)
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

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 取得時刻から5分以上経っていれば「表示上も stale」とみなす。
/// Accounts.tsx（フロント）の閾値と揃えている
const STALE_DISPLAY_THRESHOLD_SECS: i64 = 5 * 60;

fn is_display_stale(fetched_at: Option<i64>, now: i64) -> bool {
    fetched_at.is_some_and(|t| now - t >= STALE_DISPLAY_THRESHOLD_SECS)
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
            usage_lines.push(InfoLine::Gauge {
                label: format!("5h {f}%{}", reset_suffix(u.five_reset)),
                pct: f,
                muted: false,
            });
            usage_lines.push(InfoLine::Gauge {
                label: format!("週次 {s}%{}", reset_suffix(u.seven_reset)),
                pct: s,
                muted: false,
            });
            let header = match &live_name {
                Some(name) => format!("ログイン中: {name}"),
                None => "ログイン中アカウント".to_string(),
            };
            (header, format!("{}%", u.five_pct.max(u.seven_pct).round() as i64))
        }
        Err(_) => {
            usage_lines.push(InfoLine::Plain("使用量を取得できません".into()));
            usage_lines.push(InfoLine::Plain("Claude Code でログインしてください".into()));
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

/// InfoLine 1件をメニューに追加する。ゲージ行はバー画像付きの `IconMenuItem`、
/// それ以外は通常の `MenuItem` にする
fn append_info_line<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Menu<R>,
    id: String,
    line: &InfoLine,
    enabled: bool,
) -> tauri::Result<()> {
    match line {
        InfoLine::Plain(text) => {
            menu.append(&MenuItem::with_id(app, id, text, enabled, None::<&str>)?)?;
        }
        InfoLine::Gauge { label, pct, muted } => {
            let item = IconMenuItemBuilder::with_id(id, label)
                .icon(gauge_image(*pct, *muted))
                .enabled(enabled)
                .build(app)?;
            menu.append(&item)?;
        }
    }
    Ok(())
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
        append_info_line(app, &menu, format!("info-usage-{i}"), line, true)?;
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
        for (idx, a) in data.other_accounts.iter().enumerate() {
            // 縦に長くなりすぎないバランスを取りつつ、アカウント間の境目が分かるよう
            // 2件目以降は直前のアカウントとの間に区切り線を入れる
            if idx > 0 {
                menu.append(&PredefinedMenuItem::separator(app)?)?;
            }
            if a.has_credentials {
                menu.append(&MenuItem::with_id(
                    app,
                    format!("{SWITCH_ID_PREFIX}{}", a.name),
                    format!("「{}」に切り替え", a.display_name),
                    true,
                    None::<&str>,
                )?)?;
                for (line_idx, line) in usage_info_lines(a.usage.as_ref()).into_iter().enumerate() {
                    // ここだけ enabled=false にして macOS の自動グレー描画を使う
                    // （グレー配色のバーと合わせて、ライブが主・その他アカウントが従という
                    // 視覚的階層をつける。ログイン中セクションの情報行は true のまま据え置き）
                    append_info_line(app, &menu, format!("info-other-{}-{line_idx}", a.name), &line, false)?;
                }
            } else {
                // 資格情報スナップショットが無いアカウントは切り替え不可（disabled）。
                // 使用率も取得しようがないのでゲージ行は出さない
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel_at(buf: &[u8], x: u32, y: u32) -> (u8, u8, u8, u8) {
        let idx = ((y * BAR_WIDTH_PX + x) * 4) as usize;
        (buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3])
    }

    #[test]
    fn render_bar_pixels_has_expected_length() {
        let buf = render_bar_pixels(50, (1, 2, 3), (4, 5, 6));
        assert_eq!(buf.len(), (BAR_WIDTH_PX * BAR_HEIGHT_PX * 4) as usize);
    }

    #[test]
    fn render_bar_pixels_zero_percent_is_all_track() {
        let track = (10, 20, 30);
        let buf = render_bar_pixels(0, (255, 0, 0), track);
        assert_eq!(pixel_at(&buf, 0, 0), (track.0, track.1, track.2, 255));
        assert_eq!(pixel_at(&buf, BAR_WIDTH_PX - 1, 0), (track.0, track.1, track.2, 255));
    }

    #[test]
    fn render_bar_pixels_hundred_percent_is_all_fill() {
        let fill = (1, 2, 3);
        let buf = render_bar_pixels(100, fill, (9, 9, 9));
        assert_eq!(pixel_at(&buf, 0, 0), (fill.0, fill.1, fill.2, 255));
        assert_eq!(pixel_at(&buf, BAR_WIDTH_PX - 1, 0), (fill.0, fill.1, fill.2, 255));
    }

    #[test]
    fn render_bar_pixels_half_fills_left_half_only() {
        // 50% は幅のちょうど半分だけ塗る（右端は track のまま残る）
        let fill = (1, 2, 3);
        let track = (9, 9, 9);
        let buf = render_bar_pixels(50, fill, track);
        assert_eq!(pixel_at(&buf, 0, 0), (fill.0, fill.1, fill.2, 255));
        assert_eq!(pixel_at(&buf, BAR_WIDTH_PX - 1, 0), (track.0, track.1, track.2, 255));
    }

    #[test]
    fn render_bar_pixels_clamps_out_of_range_percent() {
        let fill = (1, 2, 3);
        let track = (9, 9, 9);
        let over = render_bar_pixels(150, fill, track);
        assert_eq!(pixel_at(&over, BAR_WIDTH_PX - 1, 0), (fill.0, fill.1, fill.2, 255));
        let under = render_bar_pixels(-10, fill, track);
        assert_eq!(pixel_at(&under, 0, 0), (track.0, track.1, track.2, 255));
    }

    #[test]
    fn live_fill_color_thresholds() {
        // <80% 緑・80〜95% 黄・>95% 赤（境界値をそれぞれ確認する）
        assert_eq!(live_fill_color(0), (0x34, 0xc7, 0x59));
        assert_eq!(live_fill_color(79), (0x34, 0xc7, 0x59));
        assert_eq!(live_fill_color(80), (0xff, 0xcc, 0x00));
        assert_eq!(live_fill_color(95), (0xff, 0xcc, 0x00));
        assert_eq!(live_fill_color(96), (0xff, 0x3b, 0x30));
        assert_eq!(live_fill_color(100), (0xff, 0x3b, 0x30));
    }

    #[test]
    fn is_display_stale_uses_five_minute_threshold() {
        // 取得直後（0分経過）は stale 扱いにしない（「0分前」表記が出ていた不具合の回帰確認）
        assert!(!is_display_stale(Some(1_000), 1_000));
        // 5分未満は stale 扱いにしない
        assert!(!is_display_stale(Some(1_000), 1_000 + 299));
        // ちょうど5分・それ以上は stale 扱いにする
        assert!(is_display_stale(Some(1_000), 1_000 + 300));
        assert!(is_display_stale(Some(1_000), 1_000 + 3600));
    }

    #[test]
    fn is_display_stale_false_when_fetched_at_unknown() {
        assert!(!is_display_stale(None, 1_000));
    }
}
