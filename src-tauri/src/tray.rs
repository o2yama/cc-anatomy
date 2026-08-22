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

use std::sync::Mutex;
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
    /// 取得失敗時（live が None）の案内。原因（token 期限切れ／通信不能／その他）に応じて
    /// 文言が変わる2行を改行区切りの1文字列にまとめて渡す（2026-08-08 issue #4:
    /// 以前は原因を問わず固定の2行だった。`tray::usage_advisory` 参照）
    pub live_error: Option<String>,
    /// live が Some（前回取得値でゲージを埋められた）でも、その値が最新でない可能性がある
    /// ときの注記1行（例: token 期限切れでバッチキャッシュにフォールバックした）。
    /// live_error とは排他（片方が Some ならもう片方は必ず None）
    pub live_note: Option<String>,
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

/// 現在時刻の epoch 秒。usage_advisory が「表示中の値がどれだけ古いか」を判定するのに使う
/// （2026-08-22、第4ラウンド S-3）
fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// live_usage_summary() が失敗したときの第一フォールバック。get_accounts_usage() が
/// 既に取得済みのライブアカウント分を、追加の HTTP 無しで UsageSummary へ変換できるかを
/// 判定する（テスト容易性のため I/O から分離。2026-07-27 レビュー M-1）。
/// five_pct が無ければ（キャッシュ自体が存在しない等）使える値なしとして None を返す。
///
/// fetched_at はバッチ側（AccountUsage.fetched_at）をそのまま引き継ぐ（2026-08-22、S-3）。
/// ここで得る値は「今取得した」とは限らず（force_skips_freshness_check や
/// cache_is_fresh_enough によりキャッシュ返しのこともある）、そのキャッシュが実際に
/// いつ取得されたかを usage_advisory の古さ判定に渡す必要があるため
fn usage_summary_from_batch(batch_entry: Option<&crate::accounts::AccountUsage>) -> Option<crate::actions::UsageSummary> {
    let u = batch_entry?;
    Some(crate::actions::UsageSummary {
        five_pct: u.five_pct?,
        seven_pct: u.seven_pct.unwrap_or(0.0),
        five_reset: u.five_reset,
        seven_reset: u.seven_reset,
        fetched_at: u.fetched_at,
    })
}

/// R-1 の分岐述語（2026-08-22、T-3・追加テスト項目1）: `live_usage_summary()` への
/// フォールバックが必要かどうかを HTTP を打たずに判定する純粋関数。
/// 未登録（`live_internal_name` が None）、または名前はあっても `usage` バッチの中に
/// 対応エントリが無い（has_credentials=false・空バッチ等）場合に true を返す
fn should_use_live_fallback(live_internal_name: Option<&str>, usage: &[crate::accounts::AccountUsage]) -> bool {
    match live_internal_name {
        None => true,
        Some(name) => !usage.iter().any(|u| u.name == name),
    }
}

/// トレイ・アプリ内ポップオーバー（`get_usage_overview`）の両方が土台にする生データ。
/// 表示専用の整形（メニュー文字列・ゲージ描画）は呼び出し側でそれぞれ行う
struct RawStatus {
    live_name: Option<String>,
    other_accounts: Vec<OtherAccountEntry>,
    usage_result: Result<crate::actions::UsageSummary, String>,
    /// 「ライブ OAuth 直叩き」だけの失敗理由（2026-08-08 issue #4）。usage_result が Err の
    /// ときは「原因＋回復手段」の2行（Blocking）の根拠に、usage_result が Ok でもこれが
    /// Some（バッチ・監視トークンへのフォールバックで埋めた＝ライブは失敗していた）のときは
    /// ゲージ下の注記1行（Note）の根拠になる（2026-08-08 再レビュー: 以前は Err 時のみ
    /// 意味を持つとしていたが、Ok 側でも usage_advisory が参照するようになった）
    live_error: Option<crate::actions::LiveUsageError>,
}

/// live_error の値から「claude CLI を裏起動して自動復帰させるべきか」を判定する純粋関数
/// （issue #5 / R-8、2026-08-22）。Expired（token 期限切れ）のときだけ true。
/// RateLimited では絶対に発火させない（429 は Claude Code 側の自動 refresh とは無関係で、
/// claude -p を裏起動しても解決しない上、余計なリクエストを増やすだけのため）。
/// `#[cfg(target_os = "macos")]` のインライン判定のままだとテストできなかったため切り出した
///
/// T-6（2026-08-22、既知の副作用として意図的に許容）: 同一サイクル内で既に429を観測している
/// と live_error が RateLimited 固定になるため（`live_error_for_fresh_cache`）、その間に
/// token が本当に期限切れになっても、次のサイクル（5分後、S-1）で LiveOauth を実際に
/// 試すまでここが true にならず issue #5 の自動復帰がわずかに遅れる。429 で埋まっている
/// 最中に `claude -p` を裏起動して余計なリクエストを増やす方が有害と判断し、この遅延を
/// 許容する（第4ラウンドでグローバルバックオフ（最長60分）を撤去したため、この遅延も
/// 「次のサイクルまで」に短縮されている）
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn should_nudge_token_refresh(live_error: Option<&crate::actions::LiveUsageError>) -> bool {
    matches!(live_error, Some(crate::actions::LiveUsageError::Expired))
}

/// 登録アカウント一覧・使用率一括照会・ライブ使用率の取得元フォールバックをまとめて行う
/// （2026-07-31: `fetch_status`/`usage_overview` の共有ロジックとして抽出）。
///
/// `force` は `accounts::get_accounts_usage` へそのまま引き回す。トレイの60秒定期更新は
/// false（キャッシュ新鮮判定を効かせる）、「ステータス更新」メニューやフロントからの
/// 手動更新・アカウント画面表示は true（キャッシュ新鮮判定をスキップして必ず照会する）
/// にする（2026-08-22、B-2）。
///
/// 2026-08-22（B-1）: 以前はここで `accounts::get_accounts_usage()` と
/// `actions::live_usage_summary()` の両方を無条件に呼んでおり、ライブアカウントの
/// `/api/oauth/usage` を1サイクルに2回叩いていた。`get_accounts_usage` が返す
/// `UsageBatch::live_error`（ライブアカウントに対する LiveOauth 経路の試行結果）を
/// 使うことで、バッチにライブが居るときは `live_usage_summary()` を呼ばずに済ませる。
///
/// R-1（ブロッカー、2026-08-22）: ただし「ライブがバッチに存在しない」ケースが複数ある
/// （未取り込み初回起動・ライブ org_id が登録済みのどれとも一致しない・登録済みだが
/// has_credentials == false・Windows/Linux 全体（accounts_stub は常に空））。この場合に
/// `live_usage_summary()` を無条件に削除すると使用量表示が死んでしまうため、
/// 「バッチにライブが存在するか（live_usage_from_batch が Some か）」で分岐し、
/// 存在しないときだけ `live_usage_summary()` を直接呼ぶフォールバックを残す。
/// これは「キャッシュが無くて five_pct が None なだけ」（バッチは試行済み）とは区別する:
/// 後者は二重取得の禁止（B-1）を優先し、ここでは呼ばない
///
/// T-2（2026-08-22 導入、第4ラウンド S-2 で撤去）: このフォールバックには一時期
/// グローバルバックオフのゲートを効かせていたが、段階的バックオフそのものを撤去した
/// （撤去理由は actions.rs のバックオフ撤去コメント参照）。
///
/// U-1（2026-08-22、第5ラウンド）で訂正: 撤去後の一時期、この経路は
/// 「USAGE_MIN_REFETCH_SECS を5分に緩めたこと自体が主なスロットルであり、この
/// フォールバック単独の追加ゲートは不要」という想定で `live_usage_summary()` を
/// 無条件に呼んでいた。これは誤りだった。USAGE_MIN_REFETCH_SECS は
/// `accounts::get_accounts_usage` 内のキャッシュ新鮮判定にしか効いておらず、
/// **この経路（バッチにライブが存在しないケース。未取り込み初回起動・ライブ org_id が
/// 登録済みのどれとも一致しない・登録済みだが has_credentials == false、そして
/// Windows/Linux 全体）はその判定を一度も通らない**ため、実際には無条件で
/// 60秒ごとに `/api/oauth/usage` を叩いていた。特に Windows/Linux ではこの経路が
/// `/api/oauth/usage` の唯一の取得経路のため、影響が直接出る。
///
/// V-1（2026-08-22、第6ラウンド）でさらに訂正: U-1 で追加したゲートは
/// `actions::gate_usage_attempt` を `actions::live_usage_summary()`（資格情報の読み取り→
/// 期限チェック→HTTP の順で処理する）の**手前**にかけていた。この経路には
/// `accounts::get_accounts_usage` 側のような usage_cache が無いため、ゲートに塞がれると
/// そのまま数値が消えてエラー画面になる（トレイは60秒周期・自動経路の間隔は300秒のため、
/// 5〜6サイクルに1回しか数値が出ない）。加えて、ゲートが資格情報チェックより手前にあるため、
/// 未ログイン・期限切れという HTTP を伴わない失敗でもゲートを消費してしまい、一度も通信して
/// いないのに「取得が一時的に制限されています」と表示していた（事実と違う原因の表示）。
/// 現在は `actions::live_usage_summary_gated` が資格情報チェックを通過した後にだけゲートを
/// かけ（accounts.rs の LiveOauth 経路と同じ順序）、`actions::resolve_gated_live_usage` が
/// 塞がれたときはこの経路専用の保持値（`actions::store_tray_live_fallback_last_ok` /
/// `tray_live_fallback_last_ok`。成功時にのみ更新）へフォールバックすることで、数値を
/// 出し続けたまま RateLimited を誤って名乗らないようにしている（詳細は下記のフォールバック
/// 分岐内のコメント参照）
fn fetch_raw_status(force: bool) -> RawStatus {
    // 登録アカウント一覧からライブ（現在ログイン中）を判定し、見出しに実名を出す
    let registered = crate::accounts::registered_accounts();
    let live_name = registered.iter().find(|a| a.is_live).map(|a| a.display_name.clone());
    // 内部識別子（name）は get_accounts_usage の結果（AccountUsage.name）と突き合わせるための
    // キー。表示名（display_name）とは別に持つ
    let live_internal_name = registered.iter().find(|a| a.is_live).map(|a| a.name.clone());

    // 一括照会はここ（トレイの定期更新・手動更新）とアカウント画面を開いた時だけに絞る
    // （レート配慮。get_accounts_usage 自身も force=false のときは前回取得から
    // 一定秒数未満ならキャッシュ返しにする）。監視用長期トークンは復活させず、
    // 保存済みスナップショットの access token をそのまま使う
    let batch = crate::accounts::get_accounts_usage(force).unwrap_or(crate::accounts::UsageBatch {
        accounts: Vec::new(),
        live_error: None,
    });
    let usage = batch.accounts;

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

    let (usage_result, live_error): (Result<crate::actions::UsageSummary, String>, Option<crate::actions::LiveUsageError>) =
        if !should_use_live_fallback(live_internal_name.as_deref(), &usage) {
            // バッチにライブが居る: 表示用の原因分類は get_accounts_usage が内部で行った
            // 「ライブ OAuth 直叩き」の試行結果（UsageBatch::live_error）をそのまま使う
            // （issue #4 の方針を踏襲。バッチのフォールバックはあくまで数値を埋めるための
            // 代替経路で、その失敗理由は「現在ログイン中のアカウントの token 状態」を
            // 説明しないため使わない）。二重取得の禁止（B-1）により live_usage_summary() は呼ばない
            let entry = live_internal_name.as_ref().and_then(|name| usage.iter().find(|u| &u.name == name));
            let result = usage_summary_from_batch(entry)
                .ok_or_else(|| "バッチ結果に使える値なし".to_string())
                .or_else(|_| {
                    crate::accounts::live_account_monitor_token()
                        .ok_or_else(|| "監視トークンなし".to_string())
                        .and_then(|token| crate::actions::usage_via_monitor_token(&token))
                });
            (result, batch.live_error)
        } else {
            // R-1: バッチにライブが存在しないので、ここで唯一ライブ側の /api/oauth/usage を叩く。
            // S-2（2026-08-22、第4ラウンド）: ここに掛けていたバックオフのゲートは撤去した
            // （撤去理由は actions.rs のバックオフ撤去コメント参照）。この呼び出しは1サイクル
            // につき1回だけなので、429を観測しても同一サイクル内で叩き直すことはなく、
            // 「同一サイクル内で無駄打ちしない」というローカルフラグの出番自体が無い。
            //
            // V-1（2026-08-22、第6ラウンド。U-1 が作った退行の修正）: U-1 は、当時の
            // `actions::live_usage_summary()`（内部で「資格情報の読み取り→期限チェック→HTTP」
            // の順に処理していた。V-1 で `live_usage_summary_gated` に置き換えて削除済み）の
            // **手前**にゲートを置いていたため、(1) この経路には usage_cache のような永続
            // キャッシュが無く、ゲートで塞ぐと数値が消えてエラー画面になる、(2) 未ログイン・
            // 期限切れという HTTP を伴わない失敗でもゲートを消費してしまい、一度も通信して
            // いないのに「レート制限」と誤表示する、という2つの退行を生んでいた。
            // `live_usage_summary_gated` は資格情報チェックを通過した後にだけゲートをかけ
            // （accounts.rs の LiveOauth 経路と同じ順序）、ゲートに塞がれたことは
            // `LiveUsageError::RateLimited` とは別の `GatedLiveUsageOutcome::Gated` として返す。
            // `resolve_gated_live_usage` が、塞がれたときはこの経路専用の保持値
            // （`TRAY_LIVE_FALLBACK_LAST_OK`。成功時にのみ更新）へフォールバックする。
            //
            // W-1（2026-08-22、第7ラウンド）: 保持値（UsageSummary）が無い場合、以前は
            // ここで `Other`（＝「Claude Code でログインしてください」）を決め打ちしていたが、
            // `Gated` を返す時点で資格情報チェックは既に通過している＝ログイン済み・期限内が
            // 型レベルで確定しているのに矛盾した案内をしていた。`last_usage_error` で
            // `USAGE_LAST_ERROR` から「直近に実際に試行して分かった失敗理由」を取り出し、
            // `resolve_gated_live_usage` に渡す（記録・消去は `live_usage_summary_gated` 内で
            // 同じ key に対して行う）
            let min_interval = crate::actions::usage_attempt_min_interval(true, force);
            let outcome = crate::actions::live_usage_summary_gated(crate::actions::TRAY_LIVE_FALLBACK_KEY, min_interval);
            if let crate::actions::GatedLiveUsageOutcome::Ok(u) = &outcome {
                crate::actions::store_tray_live_fallback_last_ok(u.clone());
            }
            let held = crate::actions::tray_live_fallback_last_ok();
            let held_error = crate::actions::last_usage_error(crate::actions::TRAY_LIVE_FALLBACK_KEY);
            let live_result = crate::actions::resolve_gated_live_usage(outcome, held, held_error);
            let live_error = live_result.as_ref().err().cloned();
            let result = match live_result {
                Ok(u) => Ok(u),
                // 最後の手段: 監視トークン直接照会（従来どおり残す。/v1/messages 経由で
                // /api/oauth/usage を叩かないため、試行間隔ゲート中でも試してよい）
                Err(_) => crate::accounts::live_account_monitor_token()
                    .ok_or_else(|| "監視トークンなし".to_string())
                    .and_then(|token| crate::actions::usage_via_monitor_token(&token)),
            };
            (result, live_error)
        };

    match &live_error {
        Some(crate::actions::LiveUsageError::Other(msg)) => log_unexpected_usage_error_once(msg),
        // Other 以外（正常復帰、または Expired/RateLimited/Network という「原因が分かっている」
        // 失敗）に戻ったら dedup 状態をリセットする。しないと、一度ログした Other が回復を挟んで
        // 再発しても「直前と同じ」判定で再ログされなくなる（2026-08-08 再レビュー minor-3）
        _ => reset_logged_usage_error(),
    }
    // 期限切れなら claude CLI の裏起動で自動復帰を試みる（issue #5）。別スレッドに投げるだけで
    // トレイ更新はブロックしない。復帰すれば次の60秒ポーリングで表示が正常に戻る
    #[cfg(target_os = "macos")]
    if should_nudge_token_refresh(live_error.as_ref()) {
        crate::actions::spawn_token_refresh_nudge();
    }

    RawStatus { live_name, other_accounts, usage_result, live_error }
}

/// 直近に stderr へ出した LiveUsageError::Other のメッセージ。連続する同一メッセージの
/// 再出力を抑止するための状態（2026-08-08 issue #4 再レビュー minor-1）。
/// 未ログイン（Keychain / 資格情報ファイルが無い）は Other としてここへ流れ着く定常状態で、
/// 区別する新 variant を起こす代わりに「変化がなければ黙る」dedup で毎分のログ洪水を防ぐ。
/// fetch_raw_status（トレイの定期更新・ポップオーバー双方の共通経路）1箇所だけで呼ぶことで、
/// 従来 fetch_status だけがログし usage_overview はログしていなかった非対称も解消する
static LAST_LOGGED_OTHER_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// message が直前にログした内容と同じかどうかの純粋判定（テスト容易性のため IO から分離）
fn should_log_usage_error(last: Option<&str>, message: &str) -> bool {
    last != Some(message)
}

fn log_unexpected_usage_error_once(message: &str) {
    let mut last = LAST_LOGGED_OTHER_ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if should_log_usage_error(last.as_deref(), message) {
        eprintln!("live usage fetch failed with an unexpected error: {message}");
        *last = Some(message.to_string());
    }
}

fn reset_logged_usage_error() {
    let mut last = LAST_LOGGED_OTHER_ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *last = None;
}

/// 使用量表示に添える案内。usage_result が Err のとき（Blocking）は画面全体を
/// 置き換える2行（原因＋回復手段）、Ok のとき（Note）はゲージの下に添える注記
/// （表示している値が古いことと、その取得時刻）を返す。トレイ・アプリ内ポップオーバー
/// （usage_overview）共通で使う（2026-07-31 決定「表示を1箇所に集約する」の延長）。
///
/// Note の判定基準を「live_error があるか」から「表示に使った値の古さ（fetched_at）」に
/// 変えた（2026-08-22、第4ラウンド S-3）。経緯: 従来は live_error（ライブ OAuth 直叩きの
/// 失敗有無）を根拠にしていたが、ライブが失敗しても監視トークン等の別ソースが同じ周期内に
/// 新鮮な値を取れていることがあり、その場合「最新でない可能性」という注記が事実と
/// 食い違って出てしまっていた（表示中の値は実際には今取得したばかり）。
/// `now - fetched_at` が `USAGE_STALE_NOTE_SECS` を超えて古いときだけ、事実として古いと
/// 言える場合にだけ出す。
///
/// 注記の内容も原因（レート制限・通信不能等）は出さず取得時刻だけにする
/// （ユーザーに対処のしようがない原因を説明しても意味がないため）。例外は Expired
/// （token 期限切れ）だけ: これは「Claude Code を一度実行すれば直る」という対処可能な
/// 案内なので、復帰案内の行を追加する。
///
/// usage_is_ok=false（表示できる値がまったく無い）ときの2行案内（Blocking）は変更しない
#[derive(Debug)]
enum UsageAdvisory {
    Blocking(&'static str, &'static str),
    /// \n を含む場合は複数行（Expired のときだけ復帰案内の行が付く）
    Note(String),
}

fn usage_advisory(
    usage_is_ok: bool,
    live_error: Option<&crate::actions::LiveUsageError>,
    fetched_at: Option<i64>,
    now: i64,
) -> Option<UsageAdvisory> {
    if usage_is_ok {
        // fetched_at が無ければ古さを判定できないため注記を出さない（本来は usage_is_ok=true
        // なら常に Some のはずだが、防御的に安全側＝注記なしへ倒す）
        let fetched_at = fetched_at?;
        if now - fetched_at <= crate::actions::USAGE_STALE_NOTE_SECS {
            return None;
        }
        let time = reset_local(fetched_at).unwrap_or_else(|| "不明".to_string());
        let mut note = format!("{time} 時点");
        if matches!(live_error, Some(crate::actions::LiveUsageError::Expired)) {
            note.push('\n');
            note.push_str("Claude Code を一度実行すると復帰します");
        }
        return Some(UsageAdvisory::Note(note));
    }
    let (line1, line2) = match live_error {
        Some(crate::actions::LiveUsageError::Expired) => (
            "token 期限切れ",
            "Claude Code を一度実行すると復帰します",
        ),
        Some(crate::actions::LiveUsageError::RateLimited) => (
            "使用量を取得できません",
            "取得が一時的に制限されています。しばらくお待ちください",
        ),
        Some(crate::actions::LiveUsageError::Network) => (
            "使用量を取得できません",
            "接続できません。ネットワークを確認してください",
        ),
        Some(crate::actions::LiveUsageError::Other(_)) | None => (
            "使用量を取得できません",
            "Claude Code でログインしてください",
        ),
    };
    Some(UsageAdvisory::Blocking(line1, line2))
}

fn fetch_status(force: bool) -> StatusData {
    let raw = fetch_raw_status(force);
    let live_name = raw.live_name;
    let live_error = raw.live_error;
    let now = now_epoch();
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
            if let Some(UsageAdvisory::Note(note)) = usage_advisory(true, live_error.as_ref(), u.fetched_at, now) {
                // Note は Expired のときだけ \n で2行になる。Blocking と同じく1行1メニュー項目にする
                for line in note.split('\n') {
                    usage_lines.push(InfoLine::Plain(line.to_string()));
                }
            }
            let header = match &live_name {
                Some(name) => format!("ログイン中: {name}"),
                None => "ログイン中アカウント".to_string(),
            };
            (header, format!("{}%", u.five_pct.max(u.seven_pct).round() as i64))
        }
        Err(_) => {
            if let Some(UsageAdvisory::Blocking(line1, line2)) = usage_advisory(false, live_error.as_ref(), None, now) {
                usage_lines.push(InfoLine::Plain(line1.into()));
                usage_lines.push(InfoLine::Plain(line2.into()));
            }
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
/// `fetch_raw_status` を使うため、数値・フォールバック優先順位はトレイと完全に一致する。
/// フロントからの呼び出しはアカウント画面表示・手動更新に該当するため、呼び出し元
/// （lib.rs の `get_usage_overview` コマンド）は常に force=true で呼ぶ
pub fn usage_overview(force: bool) -> UsageOverview {
    let raw = fetch_raw_status(force);
    let live_error_detail = raw.live_error;
    let now = now_epoch();
    let (live, live_error, live_note) = match raw.usage_result {
        Ok(u) => {
            let note = match usage_advisory(true, live_error_detail.as_ref(), u.fetched_at, now) {
                Some(UsageAdvisory::Note(n)) => Some(n),
                _ => None,
            };
            (
                Some(LiveUsage {
                    five_pct: u.five_pct,
                    seven_pct: u.seven_pct,
                    five_reset: u.five_reset,
                    seven_reset: u.seven_reset,
                }),
                None,
                note,
            )
        }
        // トレイの2行分の案内文言（InfoLine::Plain）と同じ内容を改行区切りで渡す
        Err(_) => {
            let error = match usage_advisory(false, live_error_detail.as_ref(), None, now) {
                Some(UsageAdvisory::Blocking(line1, line2)) => Some(format!("{line1}\n{line2}")),
                _ => None,
            };
            (None, error, None)
        }
    };
    UsageOverview {
        live_name: raw.live_name,
        live,
        live_error,
        live_note,
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
/// アカウント切り替え直後にも外部から呼べるよう公開する（m7: 切り替え後の即時反映）。
/// `force` は `fetch_status`/`get_accounts_usage` へそのまま引き回す。60秒定期更新は false、
/// 「ステータス更新」メニュー・切り替え直後の即時反映は true にする（2026-08-22、B-2）
pub fn refresh<R: Runtime>(app: AppHandle<R>, force: bool) {
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

        let data = fetch_status(force);
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
/// actions::oauth_get_checked と同様に素の std::thread::spawn へ逃がす
/// （reqwest::blocking をランタイムコンテキスト内で呼ぶと過去に tokio パニックを踏んでいる）
/// trust_unverified は常に false（2026-08-08 issue #3, major-3）: 持ち主未確認のまま
/// 続行するかどうかは影響がユーザーに見える形（確認ダイアログ）で問うべきで、確認ダイアログを
/// 出せないトレイ導線でこれを黙って force するのは行わない。TokenExpired/NetworkError で
/// 失敗した場合は、Err 分岐から「アプリのアカウント画面から操作してください」に倒す
/// （アカウント画面には force で続行する導線がある）
fn switch_from_tray<R: Runtime>(app: AppHandle<R>, name: String) {
    std::thread::spawn(move || match crate::accounts::switch_account(&name, true, false) {
        Ok(crate::accounts::SwitchOutcome::Switched { warning }) => {
            refresh(app.clone(), true);
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
            // OwnerError（accounts.rs）由来なら `KIND:message` になっているため、
            // ダイアログに機械可読タグをそのまま出さないよう剥がす。
            // TS 側の api.ts::describeAccountError と対の処理（コマンド境界を越えない経路用）。
            // 持ち主未確認（TokenExpired/NetworkError）の force 続行はここでは行わない
            // （trust_unverified=false）ため、続行したい場合はアカウント画面へ誘導する
            let message = crate::accounts::strip_owner_error_tag(&e);
            info_dialog(
                &app,
                "CC Anatomy",
                &format!("切り替えに失敗しました: {message}\nアプリのアカウント画面から操作してください。"),
            );
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
                "refresh" => refresh(app.clone(), true),
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
                    crate::actions::kill_token_nudge();
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
        refresh(handle.clone(), false);
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
    fn usage_advisory_blocking_expired_names_the_cause_and_recovery() {
        // issue #4: 固定文言「Claude Code でログインしてください」のままだと、期限切れが
        // 原因であることが伝わらない。usage_is_ok=false（Blocking）は S-3 で変更していない
        match usage_advisory(false, Some(&crate::actions::LiveUsageError::Expired), None, 0) {
            Some(UsageAdvisory::Blocking(line1, line2)) => {
                assert_eq!(line1, "token 期限切れ");
                assert!(line2.contains("Claude Code を一度実行すると"));
            }
            _ => panic!("Blocking を期待した"),
        }
    }

    #[test]
    fn usage_advisory_blocking_network_is_distinguished_from_expired() {
        match usage_advisory(false, Some(&crate::actions::LiveUsageError::Network), None, 0) {
            Some(UsageAdvisory::Blocking(line1, line2)) => {
                assert_ne!(line1, "token 期限切れ");
                assert!(line2.contains("接続"));
            }
            _ => panic!("Blocking を期待した"),
        }
    }

    #[test]
    fn usage_advisory_blocking_other_and_none_fall_back_to_legacy_wording() {
        // Other・情報なし（None）は従来どおりの固定文言を維持する（既存挙動を変えない）
        let none_lines = match usage_advisory(false, None, None, 0) {
            Some(UsageAdvisory::Blocking(l1, l2)) => (l1, l2),
            _ => panic!("Blocking を期待した"),
        };
        let other_lines = match usage_advisory(false, Some(&crate::actions::LiveUsageError::Other("x".into())), None, 0) {
            Some(UsageAdvisory::Blocking(l1, l2)) => (l1, l2),
            _ => panic!("Blocking を期待した"),
        };
        assert_eq!(none_lines, other_lines);
        assert_eq!(none_lines, ("使用量を取得できません", "Claude Code でログインしてください"));
    }

    #[test]
    fn usage_advisory_blocking_rate_limited_is_distinguished_from_expired() {
        // usage_is_ok=false（表示できる値が無い）ときは、token 期限切れとは違う文言で
        // 「一時的な制限」であることを案内する（「再ログインが必要」と誤読させない）。
        // Blocking 側の文言は S-3 で変更していない
        match usage_advisory(false, Some(&crate::actions::LiveUsageError::RateLimited), None, 0) {
            Some(UsageAdvisory::Blocking(line1, line2)) => {
                assert_ne!(line1, "token 期限切れ");
                assert!(line2.contains("制限"));
            }
            _ => panic!("Blocking を期待した"),
        }
    }

    /// S-3（2026-08-22、第4ラウンド）: 表示に使った値が新鮮（USAGE_STALE_NOTE_SECS 以内）
    /// なら、live_error の種類にかかわらず注記は出ない
    #[test]
    fn usage_advisory_ok_within_freshness_has_no_note() {
        let now = 10_000;
        let fetched_at = now - crate::actions::USAGE_STALE_NOTE_SECS; // ちょうど境界（超えていない）
        assert!(usage_advisory(true, None, Some(fetched_at), now).is_none());
        assert!(usage_advisory(true, Some(&crate::actions::LiveUsageError::RateLimited), Some(fetched_at), now).is_none());
        assert!(usage_advisory(true, Some(&crate::actions::LiveUsageError::Expired), Some(fetched_at), now).is_none());
    }

    /// fetched_at が無い（＝古さを判定できない）ときは注記を出さない防御的な既定動作
    #[test]
    fn usage_advisory_ok_without_fetched_at_has_no_note() {
        assert!(usage_advisory(true, Some(&crate::actions::LiveUsageError::RateLimited), None, 10_000).is_none());
    }

    /// S-3: 表示に使った値が USAGE_STALE_NOTE_SECS を超えて古いときだけ、取得時刻の注記を出す。
    /// 原因（レート制限・通信不能等）は文言に出さない（対処のしようがないため）
    #[test]
    fn usage_advisory_ok_stale_shows_fetched_time_without_cause() {
        let now = 10_000;
        let fetched_at = now - crate::actions::USAGE_STALE_NOTE_SECS - 1; // 境界を1秒超える
        for live_error in [
            None,
            Some(crate::actions::LiveUsageError::Network),
            Some(crate::actions::LiveUsageError::RateLimited),
            Some(crate::actions::LiveUsageError::Other("x".into())),
        ] {
            match usage_advisory(true, live_error.as_ref(), Some(fetched_at), now) {
                Some(UsageAdvisory::Note(note)) => {
                    // U-2（2026-08-22、第5ラウンド）: 「時点」を含むだけの緩い検査だと、
                    // reset_local(fetched_at) を reset_local(now) に取り違えても全テストが
                    // 通ってしまう。fetched_at を実際に整形した文字列と厳密一致させることで、
                    // 注記の時刻が fetched_at 由来であることを固定する
                    // （期待値もタイムゾーン非依存になるよう reset_local を通して作る）
                    let expected_time = reset_local(fetched_at).expect("fetched_at の整形に失敗した");
                    let first_line = note.split('\n').next().unwrap_or_default();
                    assert_eq!(first_line, format!("{expected_time} 時点"), "取得時刻の行が fetched_at 由来であることを確認: {note}");
                    // 原因固有の語（レート制限「制限」・通信不能「接続」）は出さない
                    assert!(!note.contains("制限") && !note.contains("接続"), "原因は出さない: {note}");
                    // Expired 以外は復帰案内の行も付かない
                    assert!(!note.contains("復帰"), "Expired 以外に復帰案内は付かない: {note}");
                }
                other => panic!("Note を期待した（live_error={live_error:?}）: {other:?}"),
            }
        }
    }

    /// S-3: 古くて、かつ原因が Expired のときだけ「対処可能」な復帰案内の行を追加する
    #[test]
    fn usage_advisory_ok_stale_with_expired_adds_recovery_line() {
        let now = 10_000;
        let fetched_at = now - crate::actions::USAGE_STALE_NOTE_SECS - 1;
        match usage_advisory(true, Some(&crate::actions::LiveUsageError::Expired), Some(fetched_at), now) {
            Some(UsageAdvisory::Note(note)) => {
                // U-2: 1行目が fetched_at 由来の時刻であることを厳密一致で確認する
                let expected_time = reset_local(fetched_at).expect("fetched_at の整形に失敗した");
                let mut lines = note.split('\n');
                assert_eq!(lines.next(), Some(format!("{expected_time} 時点")).as_deref());
                assert_eq!(lines.next(), Some("Claude Code を一度実行すると復帰します"));
                assert_eq!(note.matches('\n').count(), 1, "取得時刻の行＋復帰案内の行の2行のはず: {note}");
            }
            other => panic!("Note を期待した: {other:?}"),
        }
    }

    /// R-8: 429起因の claude -p 裏起動を止めることが今回の目的の1つのため、
    /// Expired だけが true になることを単体テストで守る
    #[test]
    fn should_nudge_token_refresh_only_for_expired() {
        assert!(should_nudge_token_refresh(Some(&crate::actions::LiveUsageError::Expired)));
        assert!(!should_nudge_token_refresh(Some(&crate::actions::LiveUsageError::RateLimited)));
        assert!(!should_nudge_token_refresh(Some(&crate::actions::LiveUsageError::Network)));
        assert!(!should_nudge_token_refresh(Some(&crate::actions::LiveUsageError::Other("x".into()))));
        assert!(!should_nudge_token_refresh(None));
    }

    #[test]
    fn should_log_usage_error_suppresses_identical_repeats() {
        // 未ログイン状態（Keychain に資格情報が無い）は毎分同じ Other が上がってくる定常状態。
        // 直前と同じメッセージなら再ログしない
        assert!(should_log_usage_error(None, "資格情報が見つかりません"));
        assert!(!should_log_usage_error(Some("資格情報が見つかりません"), "資格情報が見つかりません"));
        assert!(should_log_usage_error(Some("資格情報が見つかりません"), "別のエラー"));
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
        account_usage_named("acct", five_pct)
    }

    fn account_usage_named(name: &str, five_pct: Option<f64>) -> crate::accounts::AccountUsage {
        crate::accounts::AccountUsage {
            name: name.into(),
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
        // S-3: fetched_at はバッチ側（AccountUsage.fetched_at）をそのまま引き継ぐ
        assert_eq!(u.fetched_at, Some(500));
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

    /// R-1 / T-3 追加テスト項目1: フォールバックの発火条件そのもの
    #[test]
    fn should_use_live_fallback_when_unregistered() {
        // 未登録（live_internal_name が None）: 未取り込み初回起動・ライブが登録のどれとも一致しない
        assert!(should_use_live_fallback(None, &[account_usage(Some(10.0))]));
    }

    #[test]
    fn should_use_live_fallback_when_missing_from_batch() {
        // 名前はあるが usage バッチに対応エントリが無い（has_credentials=false・空バッチ等）
        let usage = vec![account_usage_named("other", Some(10.0))];
        assert!(should_use_live_fallback(Some("live"), &usage));
        assert!(should_use_live_fallback(Some("live"), &[]));
    }

    #[test]
    fn should_use_live_fallback_false_when_present_in_batch() {
        // 名前があり usage バッチにも対応エントリがある: 二重取得を避けフォールバックしない
        // （five_pct が None のキャッシュ無しエントリでも「試行済み」なので false のまま）
        let usage = vec![account_usage_named("live", None)];
        assert!(!should_use_live_fallback(Some("live"), &usage));
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
