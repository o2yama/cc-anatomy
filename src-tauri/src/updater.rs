//! 自動アップデート。GitHub Releases の latest.json を確認し、新版があれば
//! 確認ダイアログ → ダウンロード → 再起動する。起動時 + 12時間ごと + トレイから手動。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

/// 自動チェックと手動チェックが同時に走らないようにするガード
static CHECKING: AtomicBool = AtomicBool::new(false);

const STARTUP_DELAY: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

/// 起動時（少し遅らせて）+ 12時間ごとの自動チェックを開始する
pub fn setup_periodic_check<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio_sleep(STARTUP_DELAY).await;
        loop {
            check(handle.clone(), false);
            tokio_sleep(CHECK_INTERVAL).await;
        }
    });
}

/// アップデートを確認する。interactive=true（トレイからの手動実行）のときは
/// 「最新版です」「確認失敗」もダイアログで返す。自動チェック時は新版があるときだけ出す
pub fn check<R: Runtime>(app: AppHandle<R>, interactive: bool) {
    if CHECKING.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        run_check(&app, interactive).await;
        CHECKING.store(false, Ordering::SeqCst);
    });
}

async fn run_check<R: Runtime>(app: &AppHandle<R>, interactive: bool) {
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            if interactive {
                info_dialog(app, "アップデート", &format!("確認に失敗しました: {e}"));
            }
            return;
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            let current = app.package_info().version.to_string();
            let ok = app
                .dialog()
                .message(format!(
                    "新しいバージョン v{} が利用できます（現在 v{current}）。\n今すぐ更新しますか？",
                    update.version
                ))
                .title("CC Anatomy のアップデート")
                .kind(MessageDialogKind::Info)
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "更新する".into(),
                    "後で".into(),
                ))
                .blocking_show();
            if !ok {
                return;
            }
            match update.download_and_install(|_, _| {}, || {}).await {
                Ok(()) => {
                    info_dialog(
                        app,
                        "アップデート完了",
                        "新しいバージョンで再起動します。",
                    );
                    app.restart();
                }
                Err(e) => {
                    info_dialog(app, "アップデート失敗", &format!("更新に失敗しました: {e}"));
                }
            }
        }
        Ok(None) => {
            if interactive {
                info_dialog(app, "アップデート", "お使いのバージョンは最新です。");
            }
        }
        Err(e) => {
            // 自動チェックはオフライン等で普通に失敗しうるので黙って流す
            if interactive {
                info_dialog(app, "アップデート", &format!("確認に失敗しました: {e}"));
            }
        }
    }
}

fn info_dialog<R: Runtime>(app: &AppHandle<R>, title: &str, message: &str) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Info)
        .blocking_show();
}

/// tokio に依存名を増やさず tauri 再エクスポートの sleep を使うためのラッパ
async fn tokio_sleep(d: Duration) {
    tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d))
        .await
        .ok();
}
