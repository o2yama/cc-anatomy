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
//!
//! 2026-07-31（同日中、デザイン確定版）で以下を追加調整した:
//! - ドットの間隔を詰めるため DOT_COUNT を 20→32 に変更（ピッチ 220/32≒6.9px）
//! - ライブアカウントの行を「ラベル行＋バー行」の2段から `IconMenuItem` 1行
//!   （バー画像＋「5H 42%（…復活）」テキスト）に統合した
//! - その他アカウントのドットバーを廃止し、「5h: 12%（…復活） / 週次: 8%（…復活）」の
//!   数値・カッコ内で色を出し分けたテキスト1行に変更した。ネイティブメニューは部分色付け
//!   ができないため、この行は CoreText（`coretext_line` サブモジュール）でオフスクリーン
//!   描画した RGBA 画像を `IconMenuItem` の icon として渡す（macOS のみ。他 OS は色無しの
//!   プレーンテキストにフォールバック）

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

/// メニューの情報行1つ分。ゲージ行はバー画像をアイコンにした `IconMenuItem` 1行で描画し
/// （アイコンは項目の先頭に付くため、2行分のバーの左右端が自動的に揃う）、
/// それ以外は通常のテキスト `MenuItem` として描画する（`append_info_line` 側で分岐する）
enum InfoLine {
    Plain(String),
    Gauge {
        /// バー画像と同じ行に出すテキスト（例: "5H 42%（14:00 復活）"）。
        /// 2026-07-31（デザイン確定版）でラベル行とバー行の2段構成から1行に統合した
        label: String,
        pct: i64,
    },
}

/// 「その他のアカウント」1件分。クリックで切り替えるメニュー項目に使用率も添える
/// （2026-07-26: 切り替え前にどのアカウントが空いているか見えるようにする要望）。
/// アプリ内使用量ポップオーバー（`get_usage_overview`）とも共有するため Serialize を持つ
#[derive(serde::Serialize, Clone)]
pub struct OtherAccountEntry {
    pub name: String,
    pub display_name: String,
    pub has_credentials: bool,
    pub usage: Option<crate::accounts::AccountUsage>,
}

/// ライブアカウントの使用率（アプリ内ポップオーバー向け）。トレイの `InfoLine::Gauge` と
/// 同じ数値をそのまま渡す（フォーマットはフロント側で `reset_suffix` 相当を再現する）
#[derive(serde::Serialize, Clone)]
pub struct LiveUsage {
    pub five_pct: f64,
    pub seven_pct: f64,
    pub five_reset: Option<i64>,
    pub seven_reset: Option<i64>,
}

/// アプリ内使用量ポップオーバー（`get_usage_overview` コマンド）向けの表示専用データ。
/// トレイと同じ `fetch_raw_status` を土台にすることで、数値・優先順位（ライブOAuth→
/// バッチ結果→監視トークン）をトレイと完全に一致させる（2026-07-31）
#[derive(serde::Serialize, Clone)]
pub struct UsageOverview {
    /// ログイン中アカウントの表示名（取得できなければ None。フロントは
    /// 「ログイン中アカウント」にフォールバックする＝トレイの `live_header` と同じ規則）
    pub live_name: Option<String>,
    pub live: Option<LiveUsage>,
    /// 取得失敗時の案内。トレイの2行分（"使用量を取得できません" / "Claude Code で
    /// ログインしてください"）を改行区切りの1文字列にまとめて渡す
    pub live_error: Option<String>,
    pub others: Vec<OtherAccountEntry>,
}

/// 「その他のアカウント」1件分の使用率テキストを白/グレーの色付きセグメント列に分解する
/// （例: `5h: 12%`＝白 → `（14:00 復活）`＝グレー → ` / `＝グレー → `週次: 8%`＝白 →
/// `（8/2 14:00 復活）`＝グレー）。ネイティブメニューは部分色付けができないため、
/// 呼び出し側（macOS）がこれを CoreText でオフスクリーン画像にレンダリングして
/// `IconMenuItem` の icon に渡す。ドットバーはライブ専用にして主従の階層をつけ、
/// サブアカウントはテキスト1行に抑えてメニューが縦に長くなりすぎないようにする
/// （2026-07-31 デザイン確定）
fn compact_usage_segments(usage: Option<&crate::accounts::AccountUsage>, palette: &Palette) -> Vec<(String, (u8, u8, u8))> {
    let Some(u) = usage.filter(|u| u.five_pct.is_some()) else {
        return vec![("未取得".to_string(), palette.text_gray)];
    };
    // リセット時刻を過ぎている想定なら実質 0% とみなす
    let five_val = if u.five_probably_reset { 0 } else { u.five_pct.unwrap().round() as i64 };
    let seven_val = u.seven_pct.unwrap_or(0.0).round() as i64;
    vec![
        (format!("5h: {five_val}%"), palette.text_white),
        (reset_suffix(u.five_reset), palette.text_gray),
        (" / ".to_string(), palette.text_gray),
        (format!("週次: {seven_val}%"), palette.text_white),
        (reset_suffix(u.seven_reset), palette.text_gray),
    ]
}

/// CoreText が使えない OS 向けのフォールバック。色は付けず全セグメントを連結した1文字列にする
#[cfg(not(target_os = "macos"))]
fn compact_usage_plain_text(usage: Option<&crate::accounts::AccountUsage>) -> String {
    // 色を使わないプレーンテキストなので、どちらのパレットで分解しても結果は同じ
    compact_usage_segments(usage, &Palette::dark()).into_iter().map(|(text, _)| text).collect()
}

/// バー画像のピクセルサイズ。muda（tauri のメニュー実装）はメニュー画像を高さ18ptに
/// 正規化して表示するため、220×18px（@2x 相当）で描くと 220×18pt 表示になり、
/// メニュー本文の行高と釣り合う大きさに見える
const BAR_WIDTH_PX: u32 = 220;
const BAR_HEIGHT_PX: u32 = 18;

/// ダークモード配色（既存値のまま。2026-07-31 実機承認済みのため変更しない）。
/// トラック（背景）は暗いグレー、ライブのドット色は使用率によらず白固定（モノクロ化）
const TRACK_COLOR_DARK: (u8, u8, u8) = (0x3a, 0x3a, 0x3c);
const LIVE_FILL_COLOR_DARK: (u8, u8, u8) = (0xff, 0xff, 0xff);
const TEXT_COLOR_WHITE_DARK: (u8, u8, u8) = (0xff, 0xff, 0xff);
const TEXT_COLOR_GRAY_DARK: (u8, u8, u8) = (0x8e, 0x8e, 0x93);

/// ライトモード配色（2026-07-31 新設）。ダーク用の白固定塗りをそのままライトの
/// メニュー背景（ほぼ白）に出すと不可視化するため、明度を反転した組を別途持つ
const TRACK_COLOR_LIGHT: (u8, u8, u8) = (0xd1, 0xd1, 0xd6);
const LIVE_FILL_COLOR_LIGHT: (u8, u8, u8) = (0x33, 0x33, 0x36);
const TEXT_COLOR_WHITE_LIGHT: (u8, u8, u8) = (0x1d, 0x1d, 0x1f);
const TEXT_COLOR_GRAY_LIGHT: (u8, u8, u8) = (0x6e, 0x6e, 0x73);

/// 現在のメニュー外観に応じたゲージ・テキストの配色一式
struct Palette {
    track: (u8, u8, u8),
    live_fill: (u8, u8, u8),
    text_white: (u8, u8, u8),
    text_gray: (u8, u8, u8),
}

impl Palette {
    fn dark() -> Self {
        Self {
            track: TRACK_COLOR_DARK,
            live_fill: LIVE_FILL_COLOR_DARK,
            text_white: TEXT_COLOR_WHITE_DARK,
            text_gray: TEXT_COLOR_GRAY_DARK,
        }
    }

    fn light() -> Self {
        Self {
            track: TRACK_COLOR_LIGHT,
            live_fill: LIVE_FILL_COLOR_LIGHT,
            text_white: TEXT_COLOR_WHITE_LIGHT,
            text_gray: TEXT_COLOR_GRAY_LIGHT,
        }
    }

    /// メニューバーの現在の外観（ライト/ダーク）を判定して対応パレットを返す。
    /// macOS 以外はダークモード相当の既存配色（実質メニューが常にダーク基調）に固定する
    #[cfg(target_os = "macos")]
    fn current() -> Self {
        if appearance::is_dark_mode() {
            Self::dark()
        } else {
            Self::light()
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn current() -> Self {
        Self::dark()
    }
}

/// メニューバーの実効外観（ライト/ダーク）判定。build_menu ごとに1回だけ呼ぶ
#[cfg(target_os = "macos")]
mod appearance {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSAppearanceNameAqua, NSAppearanceNameDarkAqua};
    use objc2_foundation::NSArray;

    /// メインスレッドの `NSApplication.effectiveAppearance` を aqua/darkAqua と
    /// マッチングしてダーク判定する。`MainThreadMarker` が取れない場合
    /// （呼び出し元の前提が崩れている異常系）はダーク扱いにフォールバックする
    /// （2026-07-31 まで表示していた配色と同じになるため安全側）
    pub fn is_dark_mode() -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return true;
        };
        let app = NSApplication::sharedApplication(mtm);
        let appearance = app.effectiveAppearance();
        let candidates = NSArray::from_slice(&[
            unsafe { NSAppearanceNameAqua },
            unsafe { NSAppearanceNameDarkAqua },
        ]);
        let Some(best_match) = appearance.bestMatchFromAppearancesWithNames(&candidates) else {
            return true;
        };
        best_match.to_string() == unsafe { NSAppearanceNameDarkAqua }.to_string()
    }
}

/// ドット数・直径・ピッチ（@2x ピクセル単位）。ピッチは BAR_WIDTH_PX / DOT_COUNT で、
/// 各ドットはピッチの中央に配置する。2026-07-31（デザイン確定版）に 20→32 個へ増やし
/// 間隔を詰めた（ピッチ 220/32≒6.9px、直径6pxとの隙間は約0.9px@2x）
const DOT_COUNT: u32 = 32;
const DOT_DIAMETER_PX: f64 = 6.0;

/// pct% ぶんのドットを fill 色、残りを track 色で塗った RGBA バッファを作る
/// （テスト容易性のため `Image` への変換とは分離する）。ドットは水平等間隔・垂直中央揃えの
/// 円で、円の外は完全透明（alpha 0）にする。アンチエイリアスは付けない
fn render_dots_pixels(pct: i64, fill: (u8, u8, u8), track: (u8, u8, u8)) -> Vec<u8> {
    let p = pct.clamp(0, 100);
    let rounded = (p * i64::from(DOT_COUNT) + 50) / 100; // round(pct / 100 * DOT_COUNT)
    // 四捨五入だけだと 1% が0個・99%が満杯（=100%と見分けがつかない）表示になるため、
    // 0% と 100% 以外は必ず1個以上・DOT_COUNT-1個以下にクランプする（2026-07-31 レビュー）
    let filled = if p == 0 {
        0
    } else if p == 100 {
        DOT_COUNT as i64
    } else {
        rounded.clamp(1, i64::from(DOT_COUNT) - 1)
    } as u32;
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

/// バー画像を組み立てる（IconMenuItem に渡す）。バーはライブアカウント専用
fn gauge_image(pct: i64, palette: &Palette) -> Image<'static> {
    let pixels = render_dots_pixels(pct, palette.live_fill, palette.track);
    Image::new_owned(pixels, BAR_WIDTH_PX, BAR_HEIGHT_PX)
}

/// その他アカウント情報行の部分色付きテキストを CoreText でオフスクリーン描画し、
/// `IconMenuItem` に渡せる画像にする（macOS 限定。`coretext_line` 参照）
#[cfg(target_os = "macos")]
fn compact_usage_image(usage: Option<&crate::accounts::AccountUsage>, palette: &Palette) -> Image<'static> {
    let segments = compact_usage_segments(usage, palette);
    let refs: Vec<(&str, (u8, u8, u8))> = segments.iter().map(|(text, color)| (text.as_str(), *color)).collect();
    let (pixels, width, height) = coretext_line::render_colored_text_pixels(&refs);
    Image::new_owned(pixels, width, height)
}

/// 複数色のテキストランを1行の透過 RGBA 画像に描画する。ネイティブメニュー項目は
/// 部分的な文字色指定ができないため、その他アカウントの使用率行だけ画像化して回避する
/// （2026-07-31 デザイン確定）。日本語（週次・復活）を含むため、システム UI フォント +
/// CTLine 描画によるフォントフォールバックに任せる
#[cfg(target_os = "macos")]
mod coretext_line {
    use core_foundation::attributed_string::CFMutableAttributedString;
    use core_foundation::base::{CFRange, TCFType};
    use core_foundation::string::CFString;
    use core_graphics::base::kCGImageAlphaPremultipliedLast;
    use core_graphics::color::CGColor;
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_text::font::new_ui_font_for_language;
    use core_text::line::CTLine;
    use core_text::string_attributes::{kCTFontAttributeName, kCTForegroundColorAttributeName};

    /// Apple の `CTFontUIFontType` 定義における `kCTFontUIFontSystem`。
    /// core-text クレートはこの列挙値を公開していないため、CoreText.h の値をそのまま持つ
    const CT_FONT_UI_TYPE_SYSTEM: u32 = 2;

    /// メニュー本文と釣り合う実測フォントサイズ（論理13pt相当）。バー画像と同じ @2x で描く
    const TEXT_LOGICAL_PT: f64 = 13.0;
    const TEXT_SCALE: f64 = 2.0;

    /// 余白を付けない実測ぴったりのサイズだと縁のアンチエイリアスが欠けることがあるため、
    /// 四辺にわずかな余白（物理px）を足す
    const PADDING_PX: f64 = 2.0;

    /// `(テキスト, RGB色)` のセグメント列を1行に連結して CoreText で描画し、透明背景の
    /// RGBA バッファ（straight alpha, row-major 上から下）と幅・高さ（物理px）を返す
    pub fn render_colored_text_pixels(segments: &[(&str, (u8, u8, u8))]) -> (Vec<u8>, u32, u32) {
        let font = new_ui_font_for_language(CT_FONT_UI_TYPE_SYSTEM, TEXT_LOGICAL_PT * TEXT_SCALE, None);

        let mut attr = CFMutableAttributedString::new();
        for (text, (r, g, b)) in segments {
            let cf_text = CFString::new(text);
            let start = attr.char_len();
            attr.replace_str(&cf_text, CFRange::init(start, 0));
            let range = CFRange::init(start, cf_text.char_len());
            // SAFETY: CoreText の extern static を読むだけ（副作用なし）
            attr.set_attribute(range, unsafe { kCTFontAttributeName }, &font);
            let color = CGColor::rgb(f64::from(*r) / 255.0, f64::from(*g) / 255.0, f64::from(*b) / 255.0, 1.0);
            attr.set_attribute(range, unsafe { kCTForegroundColorAttributeName }, &color);
        }

        let line = CTLine::new_with_attributed_string(attr.as_concrete_TypeRef() as *const _);
        let bounds = line.get_typographic_bounds();

        let width = (bounds.width.ceil() + PADDING_PX * 2.0).max(1.0) as u32;
        let height = ((bounds.ascent + bounds.descent + bounds.leading).ceil() + PADDING_PX * 2.0).max(1.0) as u32;

        let colorspace = CGColorSpace::create_device_rgb();
        let mut ctx = CGContext::create_bitmap_context(
            None,
            width as usize,
            height as usize,
            8,
            (width * 4) as usize,
            &colorspace,
            kCGImageAlphaPremultipliedLast,
        );
        // 透過背景に載せるテキストなので、不透明背景を前提とするサブピクセルのフォント
        // スムージングは切る（縁に黒っぽい縁取りが出るのを防ぐ）。アンチエイリアスは残す
        ctx.set_should_smooth_fonts(false);
        ctx.set_should_antialias(true);
        ctx.set_text_position(PADDING_PX, bounds.descent + PADDING_PX);
        line.draw(&ctx);

        // CGBitmapContext は premultiplied alpha で埋まるため、Image::new_owned が期待する
        // straight alpha（render_dots_pixels と同じ規約）へ変換してから返す
        let premultiplied = ctx.data();
        let mut straight = Vec::with_capacity(premultiplied.len());
        for px in premultiplied.chunks_exact(4) {
            let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
            if a == 0 {
                straight.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                // 切り捨て除算だと半透明エッジの色が実際より暗く寄る（暗色バイアス）ため、
                // 四捨五入にして誤差を打ち消す（2026-07-31 レビュー）
                let unpremul = |c: u8| ((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8;
                straight.extend_from_slice(&[unpremul(r), unpremul(g), unpremul(b), a]);
            }
        }
        (straight, width, height)
    }
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

/// トレイ・アプリ内ポップオーバー（`get_usage_overview`）の両方が土台にする生データ。
/// 表示専用の整形（メニュー文字列・ゲージ描画）は呼び出し側でそれぞれ行う
struct RawStatus {
    live_name: Option<String>,
    other_accounts: Vec<OtherAccountEntry>,
    usage_result: Result<crate::actions::UsageSummary, String>,
}

/// 登録アカウント一覧・使用率一括照会・ライブ使用率の取得元フォールバックをまとめて行う
/// （2026-07-31: `fetch_status`/`usage_overview` の共有ロジックとして抽出。
/// 元は `fetch_status` に直接書かれていたトレイ専用ロジックで、ロジック自体は変更していない）
fn fetch_raw_status() -> RawStatus {
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

    RawStatus { live_name, other_accounts, usage_result }
}

fn fetch_status() -> StatusData {
    let raw = fetch_raw_status();
    let live_name = raw.live_name;
    let mut usage_lines = Vec::new();
    let (live_header, title) = match raw.usage_result {
        Ok(u) => {
            let f = u.five_pct.round() as i64;
            let s = u.seven_pct.round() as i64;
            usage_lines.push(InfoLine::Gauge {
                label: format!("5H {f}%{}", reset_suffix(u.five_reset)),
                pct: f,
            });
            usage_lines.push(InfoLine::Gauge {
                label: format!("週次 {s}%{}", reset_suffix(u.seven_reset)),
                pct: s,
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
        other_accounts: raw.other_accounts,
    }
}

/// アプリ内使用量ポップオーバー（`get_usage_overview` コマンド）向け。トレイと同じ
/// `fetch_raw_status` を使うため、数値・フォールバック優先順位はトレイと完全に一致する
pub fn usage_overview() -> UsageOverview {
    let raw = fetch_raw_status();
    let (live, live_error) = match raw.usage_result {
        Ok(u) => (
            Some(LiveUsage {
                five_pct: u.five_pct,
                seven_pct: u.seven_pct,
                five_reset: u.five_reset,
                seven_reset: u.seven_reset,
            }),
            None,
        ),
        // トレイの2行分の案内文言（InfoLine::Plain）と同じ内容を改行区切りで渡す
        Err(_) => (
            None,
            Some("使用量を取得できません\nClaude Code でログインしてください".to_string()),
        ),
    };
    UsageOverview {
        live_name: raw.live_name,
        live,
        live_error,
        others: raw.other_accounts,
    }
}

/// InfoLine 1件をメニューに追加する。ゲージはバー画像＋テキストを1行の `IconMenuItem`
/// に統合して出す（2026-07-31 デザイン確定）。それ以外は通常の `MenuItem` にする
fn append_info_line<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Menu<R>,
    id: String,
    line: &InfoLine,
    enabled: bool,
    palette: &Palette,
) -> tauri::Result<()> {
    match line {
        InfoLine::Plain(text) => {
            menu.append(&MenuItem::with_id(app, id, text, enabled, None::<&str>)?)?;
        }
        InfoLine::Gauge { label, pct } => {
            let bar_item = IconMenuItemBuilder::with_id(id, label)
                .icon(gauge_image(*pct, palette))
                .enabled(enabled)
                .build(app)?;
            menu.append(&bar_item)?;
        }
    }
    Ok(())
}

/// StatusData からメニューを組み立てる（メインスレッドで呼ぶこと）
fn build_menu<R: Runtime>(app: &AppHandle<R>, data: &StatusData) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    // ライト/ダーク判定はメニュー再構築ごとに1回だけ行う（NSApplication 呼び出しのコスト・
    // 判定タイミングのブレを避けるため。2026-07-31 ライトモード対応）
    let palette = Palette::current();

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
        append_info_line(app, &menu, format!("info-usage-{i}"), line, true, &palette)?;
    }

    // その他のアカウントは「名前 → コンパクト1行の使用率 → 切り替え」の3行構成。
    // ライブだけにバーを出して主従を分け、サブは縦の長さを抑える（2026-07-31 デザイン確定）
    for a in data.other_accounts.iter() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&MenuItem::with_id(
            app,
            format!("info-other-name-{}", a.name),
            &a.display_name,
            true,
            None::<&str>,
        )?)?;
        if a.has_credentials {
            // 使用率行は enabled=false にして、名前・切り替え行より一段落とした階層に見せる。
            // 部分色付け（白=数値、グレー=カッコ内）はネイティブメニューでは表現できないため、
            // macOS では CoreText でレンダリングした画像を icon として渡す（2026-07-31）
            #[cfg(target_os = "macos")]
            {
                let stats_item = IconMenuItemBuilder::with_id(format!("info-other-stats-{}", a.name), "")
                    .icon(compact_usage_image(a.usage.as_ref(), &palette))
                    .enabled(false)
                    .build(app)?;
                menu.append(&stats_item)?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                menu.append(&MenuItem::with_id(
                    app,
                    format!("info-other-stats-{}", a.name),
                    compact_usage_plain_text(a.usage.as_ref()),
                    false,
                    None::<&str>,
                )?)?;
            }
            menu.append(&MenuItem::with_id(
                app,
                format!("{SWITCH_ID_PREFIX}{}", a.name),
                "⇄ このアカウントへ切り替え",
                true,
                None::<&str>,
            )?)?;
        } else {
            // 資格情報スナップショットが無いアカウントは切り替え不可。
            // 使用率も取得しようがないので案内1行だけ出す
            menu.append(&MenuItem::with_id(
                app,
                format!("noop-{}", a.name),
                "未取り込み（切り替え不可）",
                false,
                None::<&str>,
            )?)?;
        }
    }

    // 下部の操作セクションは main の構成（ステータス更新・開く・バージョン確認・終了）を
    // 維持する（0be6342 でステータス更新・終了が仕様外に削除されていたため復元。
    // 「バージョンを確認する」は main の「バージョン確認」から改称済みのままにする）
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
        "CC Anatomy を開く",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "check-update",
        "バージョンを確認する",
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
                "quit" => {
                    // 実行中の doc_analysis / diagnostics 子プロセスを孤児化させないため、
                    // exit 前に best-effort で kill する
                    crate::doc_analysis::kill_running();
                    crate::diagnostics::kill_running();
                    app.exit(0);
                }
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
        // 50% は DOT_COUNT 個のうち先頭半分だけ fill、残りは track のまま
        let fill = (1, 2, 3);
        let track = (9, 9, 9);
        let buf = render_dots_pixels(50, fill, track);
        let half = DOT_COUNT / 2;
        for i in 0..half {
            let (x, y) = dot_center(i);
            assert_eq!(pixel_at(&buf, x, y), (fill.0, fill.1, fill.2, 255));
        }
        for i in half..DOT_COUNT {
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

    /// pct% で塗られるドット数を、実際に描いたバッファから数え上げる
    /// （render_dots_pixels の内部計算をそのまま再実装しないための検証手段）
    fn filled_dot_count(pct: i64) -> u32 {
        let fill = (1, 1, 1);
        let track = (0, 0, 0);
        let buf = render_dots_pixels(pct, fill, track);
        (0..DOT_COUNT)
            .filter(|&i| {
                let (x, y) = dot_center(i);
                pixel_at(&buf, x, y) == (fill.0, fill.1, fill.2, 255)
            })
            .count() as u32
    }

    #[test]
    fn render_dots_pixels_rounding_is_clamped_at_boundaries() {
        // 1%が0個・99%が満杯（=100%と見分けがつかない）になる四捨五入の問題を補正する
        // （2026-07-31 レビュー）。0%だけ0個・100%だけ満杯で、それ以外は1〜DOT_COUNT-1個
        assert_eq!(filled_dot_count(0), 0);
        assert_eq!(filled_dot_count(1), 1);
        assert_eq!(filled_dot_count(2), 1); // round(2%*32)=1 のまま（下限クランプの影響なし）
        assert_eq!(filled_dot_count(98), DOT_COUNT - 1);
        assert_eq!(filled_dot_count(99), DOT_COUNT - 1);
        assert_eq!(filled_dot_count(100), DOT_COUNT);
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

    #[test]
    fn compact_usage_segments_colors_numbers_white_and_parens_gray() {
        let u = account_usage(Some(12.0));
        let palette = Palette::dark();
        let segments = compact_usage_segments(Some(&u), &palette);
        assert_eq!(segments[0], ("5h: 12%".to_string(), palette.text_white));
        assert_eq!(segments[1].1, palette.text_gray);
        assert_eq!(segments[2], (" / ".to_string(), palette.text_gray));
        assert_eq!(segments[3], ("週次: 12%".to_string(), palette.text_white));
        assert_eq!(segments[4].1, palette.text_gray);
    }

    #[test]
    fn compact_usage_segments_falls_back_to_未取得_without_data() {
        let palette = Palette::dark();
        assert_eq!(
            compact_usage_segments(None, &palette),
            vec![("未取得".to_string(), palette.text_gray)]
        );
        assert_eq!(
            compact_usage_segments(Some(&account_usage(None)), &palette),
            vec![("未取得".to_string(), palette.text_gray)]
        );
    }

    // CoreText でのオフスクリーン描画は実機のフォント解決に依存するため、ここでは
    // 「バッファ長が幅×高さ×4 と一致し、サイズが正である」ことだけを確認する煙テストに留める
    #[cfg(target_os = "macos")]
    #[test]
    fn render_colored_text_pixels_returns_buffer_matching_its_own_dimensions() {
        let palette = Palette::dark();
        let segments = [("5h: 12%", palette.text_white), ("（14:00 復活）", palette.text_gray)];
        let (buf, width, height) = coretext_line::render_colored_text_pixels(&segments);
        assert!(width > 0 && height > 0);
        assert_eq!(buf.len(), (width * height * 4) as usize);
    }
}
