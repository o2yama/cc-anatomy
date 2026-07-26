import { useCallback, useEffect, useState } from "react";
import {
  api,
  accountLabel,
  relativeTime,
  AccountProfile,
  AccountsState,
  AccountUsage,
  RateLimits,
  RateLimitWindow,
} from "./api";
import { planLabel } from "./App";

/**
 * トレイのカスタムパネル（CleanMyMac 風のフレームレス小窓）の中身。
 * tray-panel ウィンドウ（tauri.conf.json）専用の画面で、main.tsx が
 * URL クエリ（?panel=tray）で通常の App と出し分けて描画する。
 *
 * 2026-07-26、ネイティブメニューでは見づらいというユーザー要望を受けて追加した。
 * 表示制御（トグル・位置決め・blur での自動 hide）は tray.rs / lib.rs 側で行い、
 * このコンポーネントはウィンドウが表示されるたびに（`focus` イベントで）最新化するだけでよい。
 *
 * データ取得は2系統に分ける（どちらかの失敗が他方を巻き込まないように）:
 * - ログイン中アカウント: getRateLimits/getAccountProfile（クロスプラットフォーム、
 *   トップバーの UsagePopover と同じ経路）。表示名は登録済みアカウントの display_name を優先する
 * - その他のアカウント: getAccounts/getAccountsUsage（macOS 限定）。失敗時は静かに欄ごと隠す
 *
 * 使用率の警告色しきい値は「80% 超で警告色、100% で赤」という今回の指示に合わせており、
 * トップバー側の UsageCardView（85%/60% しきい値）とは意図的に別基準にしている
 * （このパネルは常時ちら見えする常駐 UI のため早めに警告色を出す設計判断）。
 */
export function TrayPanel() {
  const [rate, setRate] = useState<RateLimits | null>(null);
  const [profile, setProfile] = useState<AccountProfile | null>(null);
  const [liveError, setLiveError] = useState<string | null>(null);
  const [loadingLive, setLoadingLive] = useState(true);

  const [accountsState, setAccountsState] = useState<AccountsState | null>(null);
  const [usageByName, setUsageByName] = useState<Record<string, AccountUsage>>({});
  const [switchingName, setSwitchingName] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState<string | null>(null);

  const load = useCallback(() => {
    setLiveError(null);
    setLoadingLive(true);
    Promise.all([api.getRateLimits(), api.getAccountProfile()])
      .then(([r, p]) => {
        setRate(r);
        setProfile(p);
      })
      .catch((e) => setLiveError(String(e)))
      .finally(() => setLoadingLive(false));

    // 他アカウントの切り替え導線は macOS 限定機能。失敗しても「その他のアカウント」欄が
    // 出ないだけにして、ログイン中アカウントの表示は妨げない
    api
      .getAccounts()
      .then((s) => {
        setAccountsState(s);
        return api.getAccountsUsage().catch(() => [] as AccountUsage[]);
      })
      .then((usage) => {
        const map: Record<string, AccountUsage> = {};
        for (const u of usage) map[u.name] = u;
        setUsageByName(map);
      })
      .catch(() => setAccountsState(null));
  }, []);

  // パネルはウィンドウの show/hide で使い回す（マウントは初回だけ）ため、
  // 表示されるたびに OS フォーカスが戻る（tray.rs 側で show() 直後に set_focus() する）
  // ことを利用して、ブラウザの focus イベントで最新化する
  useEffect(() => {
    document.documentElement.classList.add("tray-panel-mode");
    load();
    window.addEventListener("focus", load);
    return () => {
      document.documentElement.classList.remove("tray-panel-mode");
      window.removeEventListener("focus", load);
    };
  }, [load]);

  const live = accountsState?.accounts.find((a) => a.is_live) ?? null;
  const liveName = live
    ? accountLabel(live)
    : (profile?.account?.display_name ?? profile?.account?.full_name ?? "(名前不明)");
  const liveEmail = profile?.account?.email ?? "";
  const others = accountsState?.accounts.filter((a) => !a.is_live) ?? [];

  const doSwitch = (name: string) => {
    setSwitchingName(name);
    setSwitchError(null);
    api
      .switchAccount(name, true)
      .then((outcome) => {
        if (outcome.status === "needs_import") {
          setSwitchError(
            "現在ログイン中のアカウントが未登録のため切り替えられません。アプリのアカウント画面から取り込んでください。"
          );
          return;
        }
        if (outcome.status === "sessions_running") {
          // force=true で呼ぶため通常は発生しないが、念のため案内する
          setSwitchError("アプリのアカウント画面から操作してください。");
          return;
        }
        if (outcome.warning) setSwitchError(outcome.warning);
        load();
      })
      .catch((e) => setSwitchError(String(e)))
      .finally(() => setSwitchingName(null));
  };

  return (
    <div className="tray-panel">
      <div className="tray-panel-body">
        {liveError && <p className="error-box tray-panel-error">{liveError}</p>}
        {switchError && <p className="error-box tray-panel-error">{switchError}</p>}

        <section className="tray-panel-live">
          {loadingLive && !profile ? (
            <p className="muted">読み込み中…</p>
          ) : profile ? (
            <>
              <p className="tray-panel-name">{liveName}</p>
              <p className="muted tray-panel-email">
                {liveEmail || "(メール未取得)"}
                {profile && ` ・ ${planLabel(profile)}`}
              </p>
              <div className="tray-panel-gauges">
                <TrayGaugeRow label="5h" data={rate?.five_hour ?? null} />
                <TrayGaugeRow label="週次" data={rate?.seven_day ?? null} />
              </div>
            </>
          ) : null}
        </section>

        {others.length > 0 && (
          <section className="tray-panel-others">
            <h3 className="tray-panel-section-title">その他のアカウント</h3>
            <ul className="tray-panel-list">
              {others.map((a) => (
                <li key={a.name} className="tray-panel-row">
                  <div className="tray-panel-row-info">
                    <span className="tray-panel-row-name">{accountLabel(a)}</span>
                    <TrayMiniGauge usage={usageByName[a.name]} hasCredentials={a.has_credentials} />
                  </div>
                  <button
                    type="button"
                    className="acct-btn acct-btn-primary tray-panel-switch"
                    disabled={!a.has_credentials || switchingName !== null}
                    title={!a.has_credentials ? "未取り込み" : "このアカウントに切り替える"}
                    onClick={() => doSwitch(a.name)}
                  >
                    {switchingName === a.name ? <span className="diag-spinner" /> : "切り替え"}
                  </button>
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>

      <footer className="tray-panel-footer">
        <button type="button" className="tray-panel-footer-btn" onClick={() => api.showMainWindow()}>
          アプリを開く
        </button>
        <button type="button" className="tray-panel-footer-btn" onClick={load}>
          ステータス更新
        </button>
        <button type="button" className="tray-panel-footer-btn" onClick={() => api.quitApp()}>
          終了
        </button>
      </footer>
    </div>
  );
}

/** 「80% 超で警告色、100% で赤」というこのパネル専用のしきい値 */
function usageLevel(pct: number): "low" | "mid" | "high" {
  if (pct >= 100) return "high";
  if (pct > 80) return "mid";
  return "low";
}

/** ISO 日時を、今日中なら時刻だけ・それ以外は日付つきで短く表示する
 * （tray.rs のネイティブメニューで使っていた reset_local と同じ考え方。
 * 幅の狭いパネルなので、常に日付まで出す App.tsx の formatReset とはあえて分けている） */
function formatResetShort(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  return sameDay
    ? d.toLocaleTimeString("ja-JP", { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString("ja-JP", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

/** ログイン中アカウントの5h/週次ゲージ（1行分） */
function TrayGaugeRow({ label, data }: { label: string; data: RateLimitWindow | null }) {
  const pct = data?.utilization != null ? Math.round(data.utilization) : null;
  const level = usageLevel(pct ?? 0);
  return (
    <div className={`usage-limit level-${level}`}>
      <div className="usage-limit-head">
        <span>{label}</span>
        <span className="gauge-pct">
          {pct == null ? "-" : `${pct}%`}
          {data?.resets_at && <span className="tray-panel-reset"> ・ {formatResetShort(data.resets_at)}</span>}
        </span>
      </div>
      <span className="gauge-bar wide">
        <span className="gauge-fill" style={{ width: `${pct ?? 0}%` }} />
      </span>
    </div>
  );
}

/** 「その他のアカウント」1行分のミニゲージ（5h/週の数値 + 小さいバー）。
 * stale（キャッシュ返し）のときは薄字にして、ツールチップで取得時刻を示す */
function TrayMiniGauge({ usage, hasCredentials }: { usage?: AccountUsage; hasCredentials: boolean }) {
  if (!hasCredentials) {
    return <span className="muted tray-panel-mini-empty">未取り込み</span>;
  }
  if (!usage || usage.five_pct == null) {
    return <span className="muted tray-panel-mini-empty">使用率不明</span>;
  }
  const five = usage.five_probably_reset ? 0 : Math.round(usage.five_pct);
  const seven = usage.seven_pct != null ? Math.round(usage.seven_pct) : null;
  const title = usage.stale && usage.fetched_at != null ? `${relativeTime(usage.fetched_at)}時点` : undefined;
  return (
    <span className={`tray-panel-mini${usage.stale ? " stale" : ""}`} title={title}>
      <span className={`tray-mini-bar level-${usageLevel(five)}`}>
        <span className="tray-mini-fill" style={{ width: `${five}%` }} />
      </span>
      <span className="tray-mini-text">
        5h {five}%{seven != null ? ` ・週 ${seven}%` : ""}
      </span>
    </span>
  );
}
