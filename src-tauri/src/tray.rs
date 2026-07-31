//! メニューバー常駐アイコン。ライブ（現在ログイン中）アカウントの使用状況を表示し、
//! 他の登録アカウントはクリックで Keychain スワップ切り替えができる。1分ごと自動更新。
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
//! （`tauri::menu::IconMenuItem`）に置き換えた。`render_dots_pixels` が新規クレート無しで
//! 純 Rust に RGBA バッファを直接描く（アンチエイリアス・角丸なし）。
//!
//! 2026-07-31、「前のデザインの方がスタイリッシュ」というユーザー評価を受け、塗りつぶし矩形
//! バーからモノクロの細いドットバー（等間隔の円の列）に変更した。色閾値（緑黄赤）は廃止し、
//! 使用率によらずライブは白・その他アカウントはグレーの明度差のみで塗り分ける
//! （詳細は仕様書の決定変更ログ参照）。

use std::time::Duration;
use tauri::{
    image::Image,
    menu::{IconMenuItemBuilder, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, Runtime,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

const TRAY_ID: &str = "status-tray";
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
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
/// usage 自体が無い（キャッシュも無い）ときは縦に長くなりすぎないよう「未取得」1行
/// （画像なし）にまとめる。
///
/// バーはグレー配色（`muted: true`）にして、ライブが主・その他アカウントが従という
/// 視覚的階層をつける（2026-07-26 追加要望）
fn usage_info_lines(usage: Option<&crate::accounts::AccountUsage>) -> Vec<InfoLine> {
    let Some(u) = usage else { return vec![InfoLine::Plain("未取得".to_string())] };
    let Some(five_pct) = u.five_pct else { return vec![InfoLine::Plain("未取得".to_string())] };
    // リセット時刻を過ぎている想定なら実質 0% とみなす
    let five_val = if u.five_probably_reset { 0 } else { five_pct.round() as i64 };
    let seven_val = u.seven_pct.unwrap_or(0.0).round() as i64;
    vec![
        InfoLine::Gauge {
            label: format!("5h {five_val}%{}", reset_suffix(u.five_reset)),
            pct: five_val,
            muted: true,
        },
        InfoLine::Gauge {
            label: format!("週次 {seven_val}%{}", reset_suffix(u.seven_reset)),
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

/// ライブアカウントのドット色。2026-07-31 に色閾値（緑黄赤）を廃止し、使用率によらず
/// 白固定にした（モノクロ化）
const LIVE_FILL_COLOR: (u8, u8, u8) = (0xff, 0xff, 0xff);

/// ドット数・直径・ピッチ（@2x ピクセル単位）。ピッチは BAR_WIDTH_PX / DOT_COUNT で、
/// 各ドットはピッチの中央に配置する
const DOT_COUNT: u32 = 20;
const DOT_DIAMETER_PX: f64 = 6.0;

/// pct% ぶんのドットを fill 色、残りを track 色で塗った RGBA バッファを作る
/// （テスト容易性のため `Image` への変換とは分離する）。ドットは水平等間隔・垂直中央揃えの
/// 円で、円の外は完全透明（alpha 0）にする。アンチエイリアスは付けない
fn render_dots_pixels(pct: i64, fill: (u8, u8, u8), track: (u8, u8, u8)) -> Vec<u8> {
    let p = pct.clamp(0, 100);
    let filled = ((p * i64::from(DOT_COUNT) + 50) / 100) as u32; // round(pct / 100 * DOT_COUNT)
    let pitch = f64::from(BAR_WIDTH_PX) / f64::from(DOT_COUNT);
    let radius = DOT_DIAMETER_PX / 2.0;
    let cy = f64::from(BAR_HEIGHT_PX) / 2.0;

    let mut buf = Vec::with_capacity((BAR_WIDTH_PX * BAR_HEIGHT_PX * 4) as usize);
    for y in 0..BAR_HEIGHT_PX {
        for x in 0..BAR_WIDTH_PX {
            let dot_idx = (f64::from(x) / pitch) as u32;
            let cx = (f64::from(dot_idx) + 0.5) * pitch;
            let dx = f64::from(x) + 0.5 - cx;
            let dy = f64::from(y) + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= radius {
                let (r, g, b) = if dot_idx < filled { fill } else { track };
                buf.extend_from_slice(&[r, g, b, 255]);
            } else {
                buf.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    buf
}

/// バー画像を組み立てる（IconMenuItem に渡す）
fn gauge_image(pct: i64, muted: bool) -> Image<'static> {
    let fill = if muted { OTHER_FILL_COLOR } else { LIVE_FILL_COLOR };
    let pixels = render_dots_pixels(pct, fill, TRACK_COLOR);
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

/// live_usage_summary() が失敗したときの第一フォールバック。get_accounts_usage() が
/// 既に取得済みのライブアカウント分を、追加の HTTP 無しで UsageSummary へ変換できるかを
/// 判定する（テスト容易性のため I/O から分離。2026-07-27 レビュー M-1）。
/// five_pct が無ければ（キャッシュ自体が存在しない等）使える値なしとして None を返す
fn usage_summary_from_batch(batch_entry: Option<&crate::accounts::AccountUsage>) -> Option<crate::actions::UsageSummary> {
    let u = batch_entry?;
    Some(crate::actions::UsageSummary {
        five_pct: u.five_pct?,
        seven_pct: u.seven_pct.unwrap_or(0.0),
        five_reset: u.five_reset,
        seven_reset: u.seven_reset,
    })
}

fn fetch_status() -> StatusData {
    // 登録アカウント一覧からライブ（現在ログイン中）を判定し、見出しに実名を出す
    let registered = crate::accounts::registered_accounts();
    let live_name = registered.iter().find(|a| a.is_live).map(|a| a.display_name.clone());
    // 内部識別子（name）は get_accounts_usage の結果（AccountUsage.name）と突き合わせるための
    // キー。表示名（display_name）とは別に持つ
    let live_internal_name = registered.iter().find(|a| a.is_live).map(|a| a.name.clone());

    // 一括照会はここ（トレイの定期更新・手動更新）とアカウント画面を開いた時だけに絞る
    // （レート配慮。get_accounts_usage 自身も前回取得から60秒未満はキャッシュ返しにする）。
    // 監視用長期トークンは復活させず、保存済みスナップショットの access token をそのまま使う
    let usage = crate::accounts::get_accounts_usage().unwrap_or_default();
    // ライブアカウント分を後段のフォールバックで再利用するため先に引いておく
    // （get_accounts_usage は is_live のアカウントにもライブOAuth→監視トークン→
    // スナップショットの順で既に試行済みなので、ここでもう一度 HTTP を打つ必要は無い）
    let live_usage_from_batch = live_internal_name.as_ref().and_then(|name| usage.iter().find(|u| &u.name == name));

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
    // ライブ OAuth を最優先で試す。失敗したら（典型的には切り替え直後で、スナップショット
    // 由来のライブトークンが期限切れ。リフレッシュは Claude Code 起動時にしか起きない）、
    // まず上の get_accounts_usage() がこのライブアカウント分について既に試行済みの結果
    // （ライブOAuth→監視トークン→スナップショットのフォールバック連鎖）を再利用する
    // （追加の HTTP なし）。それでも使える値が無いとき（例: 初回でキャッシュも無く
    // バッチ側の照会も失敗した）だけ、最後の手段として監視トークンへ直接照会する。
    // 2026-07-27 レビュー M-1: 従来はここで無条件にもう一度監視トークンへ POST しており、
    // 「ライブトークン期限切れ＋監視トークンあり」のケースで毎サイクル /v1/messages への
    // 実リクエストが2回（get_accounts_usage 内と、ここ）走っていた
    let usage_result = crate::actions::live_usage_summary()
        .or_else(|_| {
            usage_summary_from_batch(live_usage_from_batch)
                .ok_or_else(|| "バッチ結果に使える値なし".to_string())
        })
        .or_else(|_| {
            crate::accounts::live_account_monitor_token()
                .ok_or_else(|| "監視トークンなし".to_string())
                .and_then(|token| crate::actions::usage_via_monitor_token(&token))
        });
    let (live_header, title) = match usage_result {
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
        // 手動「セッション更新」ボタンの廃止に伴う自動化（2026-07-26）。定期更新のたびに
        // ライブセッションを確認し、登録済みアカウントかつスナップショットに変化があれば
        // 資格情報を最新化する。アカウント画面が開いていれば "accounts-updated" で気づかせる。
        // ユーザー操作を起点としない自動処理なので、エラーはダイアログを出さずログのみに留める。
        // auto_sync_live 側でハッシュ変化が無ければ Unchanged を返すため、ここでの emit は
        // 実際に状態が変わったときだけに絞られる（レビュー M-3: 未登録ライブが居座る間
        // 毎分 emit → 画面の reload が走り、D&D 並び替え中に順序が巻き戻る問題への対応）
        match crate::accounts::auto_sync_live() {
            Ok(crate::accounts::AutoSyncResult::Synced { warning }) => {
                let _ = app.emit("accounts-updated", crate::accounts::AccountsUpdatedEvent { warning });
            }
            Ok(crate::accounts::AutoSyncResult::Unregistered) => {
                let _ = app.emit(
                    "accounts-updated",
                    crate::accounts::AccountsUpdatedEvent { warning: None },
                );
            }
            Ok(_) => {}
            Err(e) => eprintln!("auto_sync_live failed (will retry next cycle): {e}"),
        }

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

    // 初回取得 + REFRESH_INTERVAL（1分）ごとの自動更新
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

    /// dot_idx 番目のドット中心にあたるピクセル座標（render_dots_pixels の計算と対応させる）
    fn dot_center(dot_idx: u32) -> (u32, u32) {
        let pitch = f64::from(BAR_WIDTH_PX) / f64::from(DOT_COUNT);
        let cx = ((f64::from(dot_idx) + 0.5) * pitch - 0.5).round() as u32;
        (cx, BAR_HEIGHT_PX / 2)
    }

    #[test]
    fn render_dots_pixels_has_expected_length() {
        let buf = render_dots_pixels(50, (1, 2, 3), (4, 5, 6));
        assert_eq!(buf.len(), (BAR_WIDTH_PX * BAR_HEIGHT_PX * 4) as usize);
    }

    #[test]
    fn render_dots_pixels_zero_percent_is_all_track_dots() {
        let track = (10, 20, 30);
        let buf = render_dots_pixels(0, (255, 0, 0), track);
        for i in 0..DOT_COUNT {
            let (x, y) = dot_center(i);
            assert_eq!(pixel_at(&buf, x, y), (track.0, track.1, track.2, 255));
        }
    }

    #[test]
    fn render_dots_pixels_hundred_percent_is_all_fill_dots() {
        let fill = (1, 2, 3);
        let buf = render_dots_pixels(100, fill, (9, 9, 9));
        for i in 0..DOT_COUNT {
            let (x, y) = dot_center(i);
            assert_eq!(pixel_at(&buf, x, y), (fill.0, fill.1, fill.2, 255));
        }
    }

    #[test]
    fn render_dots_pixels_half_fills_first_half_of_dots_only() {
        // 50% は 20 個のうち先頭 10 個だけ fill、残りは track のまま
        let fill = (1, 2, 3);
        let track = (9, 9, 9);
        let buf = render_dots_pixels(50, fill, track);
        for i in 0..10 {
            let (x, y) = dot_center(i);
            assert_eq!(pixel_at(&buf, x, y), (fill.0, fill.1, fill.2, 255));
        }
        for i in 10..DOT_COUNT {
            let (x, y) = dot_center(i);
            assert_eq!(pixel_at(&buf, x, y), (track.0, track.1, track.2, 255));
        }
    }

    #[test]
    fn render_dots_pixels_clamps_out_of_range_percent() {
        let fill = (1, 2, 3);
        let track = (9, 9, 9);
        let over = render_dots_pixels(150, fill, track);
        let (x, y) = dot_center(DOT_COUNT - 1);
        assert_eq!(pixel_at(&over, x, y), (fill.0, fill.1, fill.2, 255));
        let under = render_dots_pixels(-10, fill, track);
        let (x0, y0) = dot_center(0);
        assert_eq!(pixel_at(&under, x0, y0), (track.0, track.1, track.2, 255));
    }

    #[test]
    fn render_dots_pixels_gap_between_dots_is_transparent() {
        // ドット間の隙間（ピッチの境界付近）は完全透明になる
        let buf = render_dots_pixels(100, (255, 255, 255), (0, 0, 0));
        let (c0, _) = dot_center(0);
        let (c1, _) = dot_center(1);
        let gap_x = (c0 + c1) / 2;
        assert_eq!(pixel_at(&buf, gap_x, BAR_HEIGHT_PX / 2).3, 0);
    }

    fn account_usage(five_pct: Option<f64>) -> crate::accounts::AccountUsage {
        crate::accounts::AccountUsage {
            name: "acct".into(),
            five_pct,
            seven_pct: Some(12.0),
            five_reset: Some(1_000),
            seven_reset: Some(2_000),
            fetched_at: Some(500),
            stale: true,
            five_probably_reset: false,
        }
    }

    #[test]
    fn usage_summary_from_batch_converts_when_five_pct_present() {
        // stale（キャッシュ返し）でも値さえあれば使う。fresh かどうかは get_accounts_usage
        // 側の関心事で、ここ（トレイタイトルの表示可否）では問わない
        let u = usage_summary_from_batch(Some(&account_usage(Some(42.0)))).expect("値があるはず");
        assert_eq!(u.five_pct, 42.0);
        assert_eq!(u.seven_pct, 12.0);
        assert_eq!(u.five_reset, Some(1_000));
        assert_eq!(u.seven_reset, Some(2_000));
    }

    #[test]
    fn usage_summary_from_batch_none_when_no_batch_entry() {
        // ライブアカウントが registered に見つからない・get_accounts_usage の結果に
        // 対応するエントリが無い場合
        assert!(usage_summary_from_batch(None).is_none());
    }

    #[test]
    fn usage_summary_from_batch_none_when_five_pct_missing() {
        // キャッシュ自体が存在しない（five_pct が None）なら「使える値なし」として
        // 呼び出し側（fetch_status）を最後の手段（監視トークン直接照会）へ進ませる
        assert!(usage_summary_from_batch(Some(&account_usage(None))).is_none());
    }
}
