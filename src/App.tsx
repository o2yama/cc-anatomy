import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  AccountUsage,
  AccountsUpdatedEvent,
  describeAccountError,
  formatEpoch,
  isForceSwitchEligible,
  OtherAccountOverview,
  ownerErrorKind,
  ProjectInfo,
  SearchHit,
  Transcript,
  UsageOverview,
} from "./api";
import {
  ProjectTree,
  TreeSelection,
  selectionKey,
  buildTree,
  collapsiblePaths,
} from "./ProjectTree";
import { ProjectOverview } from "./ProjectOverview";
import { DiagnosisOverlay } from "./Diagnosis";
import { AccountsOverlay, SKIP_SESSIONS_CONFIRM_KEY, skipSessionsConfirmEnabled } from "./Accounts";
import { useIsMac } from "./platform";
import "./App.css";

export default function App() {
  const [searchOpen, setSearchOpen] = useState(false);
  const [diagOpen, setDiagOpen] = useState(false);
  const [acctOpen, setAcctOpen] = useState(false);
  // 環境診断・アカウント切り替えは macOS 限定機能（Keychain / Terminal.app 依存）
  const isMac = useIsMac();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        setSearchOpen((v) => !v);
      }
      if (e.key === "Escape") setSearchOpen(false);
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  return (
    <div className="app">
      <header className="topbar">
        <h1>CC Anatomy</h1>
        {isMac && (
        <>
        <button
          className="icon-btn"
          title="環境診断"
          onClick={() => setDiagOpen(true)}
        >
          <svg
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            {/* 心拍波形アイコン（診断のメタファー） */}
            <polyline points="2 12 6 12 9 5 14 19 17 12 22 12" />
          </svg>
        </button>
        <button
          className="icon-btn"
          title="アカウント切り替え"
          onClick={() => setAcctOpen(true)}
        >
          <svg
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            {/* 人物アイコン（アカウントのメタファー） */}
            <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
            <circle cx="12" cy="7" r="4" />
          </svg>
        </button>
        </>
        )}
        <UsagePopover />
        <button
          className="icon-btn"
          title="横断検索 (⌘K)"
          onClick={() => setSearchOpen(true)}
        >
          <svg
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.2"
            strokeLinecap="round"
          >
            <circle cx="11" cy="11" r="7" />
            <line x1="21" y1="21" x2="16.5" y2="16.5" />
          </svg>
        </button>
      </header>
      <main>
        <SessionsView />
      </main>
      {searchOpen && <GlobalSearchOverlay onClose={() => setSearchOpen(false)} />}
      {/* 実行中に閉じても診断を継続できるよう、常にマウントして open で表示を切り替える
          （macOS 限定機能のため非 macOS ではマウント自体を省く） */}
      {isMac && <DiagnosisOverlay open={diagOpen} onClose={() => setDiagOpen(false)} />}
      {isMac && <AccountsOverlay open={acctOpen} onClose={() => setAcctOpen(false)} />}
    </div>
  );
}

/** ドットゲージのドット数・丸め規則は tray.rs の render_dots_pixels と同一にする
 * （0%→0個、100%→32個、それ以外は round 後 1〜31 個にクランプ）。
 * pct は事前に整数（Math.round 済み）で渡すこと */
const DOT_COUNT = 32;

function filledDotCount(pct: number): number {
  const p = Math.min(100, Math.max(0, pct));
  if (p === 0) return 0;
  if (p === 100) return DOT_COUNT;
  const rounded = Math.round((p * DOT_COUNT) / 100);
  return Math.min(DOT_COUNT - 1, Math.max(1, rounded));
}

function DotGauge({ pct }: { pct: number }) {
  const filled = filledDotCount(pct);
  return (
    <span className="dot-gauge">
      {Array.from({ length: DOT_COUNT }, (_, i) => (
        <span key={i} className={`dot-gauge-dot${i < filled ? " filled" : ""}`} />
      ))}
    </span>
  );
}

/** epoch 秒をローカル時刻表示に変換する。tray.rs の reset_local と同じ規則
 * （今日中なら時刻だけ、それ以外は日付つき） */
function resetLocal(epochSec: number): string {
  const d = new Date(epochSec * 1000);
  const now = new Date();
  const time = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  return sameDay ? time : `${d.getMonth() + 1}/${d.getDate()} ${time}`;
}

/** tray.rs の reset_suffix と同じ規則（「（14:00 復活）」の形） */
function resetSuffix(epoch: number | null): string {
  if (epoch == null) return "";
  return `（${resetLocal(epoch)} 復活）`;
}

/** その他アカウント1件分の使用率テキスト。tray.rs の compact_usage_segments と同じ内容
 * （白=数値、グレー=リセット時刻）を span の色分けで再現する */
function OtherAccountStats({ usage }: { usage: AccountUsage | null }) {
  if (!usage || usage.five_pct == null) {
    return <p className="other-acct-stats muted">未取得</p>;
  }
  // リセット時刻を過ぎている想定なら実質 0% とみなす（tray.rs と同じ規則）
  const fiveVal = usage.five_probably_reset ? 0 : Math.round(usage.five_pct);
  const sevenVal = Math.round(usage.seven_pct ?? 0);
  return (
    <p className="other-acct-stats">
      <span className="stat-value">5h: {fiveVal}%</span>
      <span className="muted">{resetSuffix(usage.five_reset)}</span>
      <span className="muted"> / </span>
      <span className="stat-value">週次: {sevenVal}%</span>
      <span className="muted">{resetSuffix(usage.seven_reset)}</span>
    </p>
  );
}

/** その他アカウント1行分（名前・使用率・切り替えボタン）。has_credentials=false は
 * 資格情報スナップショットが無く切り替え不可（tray.rs の「未取り込み（切り替え不可）」と同じ） */
function OtherAccountRow({
  account,
  busy,
  onSwitch,
}: {
  account: OtherAccountOverview;
  busy: boolean;
  onSwitch: (name: string) => void;
}) {
  return (
    <div className="other-acct-row">
      <p className="other-acct-name">{account.display_name}</p>
      {account.has_credentials ? (
        <>
          <OtherAccountStats usage={account.usage} />
          <button
            className="acct-btn acct-btn-ghost switch-btn"
            disabled={busy}
            onClick={() => onSwitch(account.name)}
          >
            ⇄ このアカウントへ切り替え
          </button>
        </>
      ) : (
        <p className="muted">未取り込み（切り替え不可）</p>
      )}
    </div>
  );
}

/** 起動中セッションがある場合の続行確認。Accounts.tsx の sessionsConfirm と同じ文言・
 * 「今後表示しない」設定（localStorage キー共有）を踏襲する。
 * ポップオーバー（`.usage-anchor`、31×27px）の内側に描画すると
 * `.acct-modal-overlay` の `position: absolute; inset: 0` がその小さな箱に潰れてしまうため、
 * body 直下へ createPortal し、専用クラスで position: fixed 化する */
function SessionsConfirmModal({
  name,
  count,
  busy,
  onCancel,
  onConfirm,
}: {
  name: string;
  count: number;
  busy: boolean;
  onCancel: () => void;
  onConfirm: (skipFuture: boolean) => void;
}) {
  const [skipFuture, setSkipFuture] = useState(false);
  return createPortal(
    <div className="acct-modal-overlay usage-sessions-modal-overlay" onClick={onCancel}>
      <div className="acct-modal-card" onClick={(e) => e.stopPropagation()}>
        <strong>「{name}」に切り替えます</strong>
        <p className="muted">
          起動中の Claude Code セッションがあります。続行すると、実行中セッションが古いトークンを
          書き戻して切り替えが巻き戻ったり、保存済みアカウントが後で「＋
          アカウントを追加」から改めてログインし直す必要になる可能性があります（{count}件）。
          全セッション終了を推奨しますが、続行しますか？
        </p>
        <label className="acct-modal-checkbox">
          <input
            type="checkbox"
            checked={skipFuture}
            onChange={(e) => setSkipFuture(e.target.checked)}
          />
          今後この確認を表示しない
        </label>
        <div className="acct-modal-actions">
          <button className="acct-btn acct-btn-ghost" onClick={onCancel}>
            やめる
          </button>
          <button
            className="acct-btn acct-btn-primary"
            disabled={busy}
            onClick={() => onConfirm(skipFuture)}
          >
            続行する
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}

/** 「持ち主を確認できないが続行するか」の確認（issue #3、レビュー案A）。続行すると
 * sync-back を書き込まずスキップするだけ（trustUnverified=true。Rust 側の
 * SyncBack::SkippedUnverified 参照）で、別アカウントの資格情報を上書きすることはない。
 * Accounts.tsx の ownerConfirm と同じ文言。SessionsConfirmModal と同様に body 直下へ
 * createPortal する */
function OwnerConfirmModal({
  name,
  message,
  busy,
  onCancel,
  onConfirm,
}: {
  name: string;
  message: string;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return createPortal(
    <div className="acct-modal-overlay usage-sessions-modal-overlay" onClick={onCancel}>
      <div className="acct-modal-card" onClick={(e) => e.stopPropagation()}>
        <strong>「{name}」に切り替えます</strong>
        <p className="muted">
          持ち主を確認できませんが切り替えは可能です。直前のセッションのログイン情報は今回同期されません。元のアカウントに戻す際、再ログインが必要になる場合があります。続行しますか？
        </p>
        <p className="muted acct-owner-confirm-detail">{message}</p>
        <div className="acct-modal-actions">
          <button className="acct-btn acct-btn-ghost" onClick={onCancel}>
            やめる
          </button>
          <button className="acct-btn acct-btn-primary" disabled={busy} onClick={onConfirm}>
            続行する
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}

/** メニューバートレイパネルと同一の表示内容・数値・フォーマットを再現する使用量ポップオーバー
 * （2026-07-31 UsageCard/UsageCardView ベースの独自表示から作り替え）。
 * データは get_usage_overview（tray::fetch_raw_status 共有）から取得するため、
 * トレイのメニューと数値・優先順位が完全に一致する */
function UsagePopover() {
  const [open, setOpen] = useState(false);
  const [overview, setOverview] = useState<UsageOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // 切り替え失敗は取得エラー（error、全置換）とは別枠で目立たせる（M-2）
  const [switchError, setSwitchError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // trust はこの確認が発生した時点の trustUnverified 値を持ち回る（issue #3 再レビュー:
  // owner確認→セッション確認と進んだ後の「続行する」で trust を落とさないため）
  const [sessionsConfirm, setSessionsConfirm] = useState<
    { name: string; label: string; count: number; trust: boolean } | null
  >(null);
  // issue #3: 持ち主未確認（token 期限切れ／通信不能）で続行を選べる確認。
  // force はこの確認が発生した時点の値を持ち回る（force と trustUnverified は独立、major-2）
  const [ownerConfirm, setOwnerConfirm] = useState<
    { name: string; label: string; message: string; force: boolean } | null
  >(null);

  const load = useCallback(() => {
    setError(null);
    api
      .getUsageOverview()
      .then(setOverview)
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    if (open) load();
  }, [open, load]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setOpen(false);
        setSessionsConfirm(null);
        setOwnerConfirm(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  // 定期更新ループ（tray.rs）が自動取り込みを行った場合、表示中のポップオーバーも
  // 追随させる（Accounts.tsx の同名 listen と同じイベント）
  useEffect(() => {
    if (!open) return;
    const unlisten = listen<AccountsUpdatedEvent>("accounts-updated", () => {
      load();
    });
    return () => {
      unlisten.then((un) => un());
    };
  }, [open, load]);

  /** トレイの switch_from_tray と同じ確認フロー（needs_import は案内のみ、
   * sessions_running は確認モーダル＋「今後表示しない」設定を踏襲）。
   * force（セッション確認スキップ）と trustUnverified（持ち主未確認でも続行、issue #3）は
   * 独立した引数（major-2） */
  const doSwitch = (name: string, force = false, trustUnverified = false): Promise<void> => {
    setNotice(null);
    setSwitchError(null);
    setBusy(true);
    return api
      .switchAccount(name, force, trustUnverified)
      .then((outcome) => {
        if (outcome.status === "needs_import") {
          setNotice(
            "現在ログイン中のアカウントが未登録のため切り替えられません。アカウント画面から操作してください。"
          );
          return;
        }
        if (outcome.status === "sessions_running") {
          // trustUnverified はここでは変えない（セッション確認の同意が持ち主未確認への
          // 同意を兼ねてはいけない）
          if (skipSessionsConfirmEnabled()) {
            return doSwitch(name, true, trustUnverified);
          }
          const label = overview?.others.find((a) => a.name === name)?.display_name ?? name;
          setSessionsConfirm({ name, label, count: outcome.count, trust: trustUnverified });
          return;
        }
        setSessionsConfirm(null);
        setNotice(
          outcome.warning ??
            "切り替えました。実行中の Claude Code セッションには反映されません。"
        );
        load();
      })
      .catch((e) => {
        // issue #3: 持ち主未確認（token 期限切れ／通信不能）は続行を選べる。
        // すでに trustUnverified=true で失敗しているときは再提案しない
        // （Other 等、trustUnverified でも解消しない別要因）
        if (!trustUnverified && isForceSwitchEligible(ownerErrorKind(e))) {
          const label = overview?.others.find((a) => a.name === name)?.display_name ?? name;
          setOwnerConfirm({ name, label, message: describeAccountError(e), force });
          return;
        }
        setSwitchError(describeAccountError(e));
      })
      .finally(() => setBusy(false));
  };

  /** issue #3: force はこの確認が発生した時点の値をそのまま持ち回る（major-2） */
  const confirmOwnerAndContinue = () => {
    if (!ownerConfirm) return;
    const target = ownerConfirm;
    setOwnerConfirm(null);
    doSwitch(target.name, target.force, true);
  };

  const confirmSessionsAndContinue = (skipFuture: boolean) => {
    if (!sessionsConfirm) return;
    if (skipFuture) localStorage.setItem(SKIP_SESSIONS_CONFIRM_KEY, "1");
    const target = sessionsConfirm;
    setSessionsConfirm(null);
    // target.trust を引き継ぐ（issue #3 再レビュー: owner確認→セッション確認と進んだ場合、
    // ここで trust を落とすと再度 owner確認に戻ってしまう二度手間になる）
    doSwitch(target.name, true, target.trust);
  };

  // tray.rs の live_header と同じ規則：使用量取得が失敗（live_error）した場合は
  // live_name の有無によらず固定文言にする
  const liveHeader = overview
    ? overview.live && overview.live_name
      ? `ログイン中: ${overview.live_name}`
      : "ログイン中アカウント"
    : "取得中…";

  return (
    <div className="usage-anchor">
      <button
        className="icon-btn"
        title="アカウントとリソース使用状況"
        onClick={() => {
          setOpen((v) => !v);
          setSessionsConfirm(null);
          setOwnerConfirm(null);
        }}
      >
        <svg
          width="15"
          height="15"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          {/* スピードメーター風アイコン */}
          <path d="M12 21a9 9 0 1 1 9-9" />
          <path d="M12 12l5-3" />
          <circle cx="12" cy="12" r="1" />
        </svg>
      </button>
      {open && (
        <>
          <div
            className="menu-backdrop"
            onMouseDown={() => {
              setOpen(false);
              setSessionsConfirm(null);
              setOwnerConfirm(null);
            }}
          />
          <div className="usage-popover">
            {error ? (
              <p className="error-box">{error}</p>
            ) : (
              <>
                <p className="usage-live-header">{liveHeader}</p>
                {overview?.live ? (
                  <div className="usage-gauges">
                    <div className="usage-gauge-row">
                      <DotGauge pct={Math.round(overview.live.five_pct)} />
                      <span>
                        5H {Math.round(overview.live.five_pct)}%
                        {resetSuffix(overview.live.five_reset)}
                      </span>
                    </div>
                    <div className="usage-gauge-row">
                      <DotGauge pct={Math.round(overview.live.seven_pct)} />
                      <span>
                        週次 {Math.round(overview.live.seven_pct)}%
                        {resetSuffix(overview.live.seven_reset)}
                      </span>
                    </div>
                    {overview.live_note &&
                      // S-3（2026-08-22）: 注記が2行（取得時刻＋Expiredのときの復帰案内）に
                      // なることがあるため、live_error と同じ「\n で分割して1行1<p>」の
                      // 既存機構に乗せる（overview.live_error のレンダリングと同一パターン）
                      overview.live_note.split("\n").map((line, i) => (
                        <p key={i} className="muted usage-live-note">
                          {line}
                        </p>
                      ))}
                  </div>
                ) : overview?.live_error ? (
                  overview.live_error
                    .split("\n")
                    .map((line, i) => (
                      <p key={i} className="muted">
                        {line}
                      </p>
                    ))
                ) : (
                  <p className="muted">取得中…</p>
                )}

                {overview && overview.others.length > 0 && (
                  <>
                    <hr />
                    {overview.others.map((a) => (
                      <OtherAccountRow
                        key={a.name}
                        account={a}
                        busy={busy}
                        onSwitch={doSwitch}
                      />
                    ))}
                  </>
                )}

                {notice && <p className="usage-note muted">{notice}</p>}
              </>
            )}
            {switchError && <p className="error-box">{switchError}</p>}
            <hr />
            <button className="acct-btn acct-btn-ghost usage-refresh-btn" onClick={load}>
              ステータス更新
            </button>
          </div>
          {sessionsConfirm && (
            <SessionsConfirmModal
              name={sessionsConfirm.label}
              count={sessionsConfirm.count}
              busy={busy}
              onCancel={() => setSessionsConfirm(null)}
              onConfirm={confirmSessionsAndContinue}
            />
          )}
          {ownerConfirm && (
            <OwnerConfirmModal
              name={ownerConfirm.label}
              message={ownerConfirm.message}
              busy={busy}
              onCancel={() => setOwnerConfirm(null)}
              onConfirm={confirmOwnerAndContinue}
            />
          )}
        </>
      )}
    </div>
  );
}

function SessionsView() {
  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [home, setHome] = useState<string | null>(null);
  const [selected, setSelected] = useState<TreeSelection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [loading, setLoading] = useState(false);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [extractTarget, setExtractTarget] = useState<TreeSelection | null>(
    null
  );
  // ProjectOverview のドロワーが未保存かどうか。サイドバー選択変更・再読み込みは
  // このドロワーを問答無用で捨てて別プロジェクトへ移動するため、ここで確認を挟む
  const [overviewDirty, setOverviewDirty] = useState(false);

  const guardUnsaved = (message: string): boolean => {
    if (!overviewDirty) return true;
    return window.confirm(message);
  };

  const selectProject = (next: TreeSelection) => {
    if (!guardUnsaved("未保存の変更があります。破棄して別のプロジェクトを開きますか？")) {
      return;
    }
    setSelected(next);
  };

  const reload = () => {
    if (!guardUnsaved("未保存の変更があります。破棄して再読み込みしますか？")) {
      return;
    }
    setReloadKey((k) => k + 1);
  };

  useEffect(() => {
    setLoading(true);
    Promise.all([api.listProjects(), api.getHomeDir()])
      .then(([ps, h]) => {
        setProjects(ps);
        setHome(h);
        setError(null);
        // 選択はディレクトリ（実績なしの階層もありうる）なので、
        // プロジェクト一覧との突合はせず未選択のときだけ初期値を入れる
        setSelected((s) =>
          s ?? (ps[0] ? { project: ps[0].project, path: ps[0].path } : null)
        );
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [reloadKey]);

  if (error) return <ErrorBox message={error} />;

  const tree = home ? buildTree(projects, home) : null;
  const collapsibles = tree ? collapsiblePaths(tree.root) : [];
  const allCollapsed =
    collapsibles.length > 0 && collapsibles.every((p) => collapsed.has(p));
  const toggleAll = () =>
    setCollapsed(allCollapsed ? new Set() : new Set(collapsibles));

  return (
    <div className="sessions-layout">
      <aside className="project-list">
        <div className="sidebar-head">
          <span>プロジェクト</span>
          <button
            className="icon-btn"
            title={allCollapsed ? "すべて展開" : "すべて折りたたむ"}
            onClick={toggleAll}
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              {allCollapsed ? (
                <>
                  <polyline points="7 13 12 18 17 13" />
                  <polyline points="7 6 12 11 17 6" />
                </>
              ) : (
                <>
                  <polyline points="7 11 12 6 17 11" />
                  <polyline points="7 18 12 13 17 18" />
                </>
              )}
            </svg>
          </button>
          <button
            className={`icon-btn ${loading ? "spinning" : ""}`}
            title="再読み込み"
            onClick={reload}
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M21 12a9 9 0 1 1-2.64-6.36" />
              <polyline points="21 3 21 9 15 9" />
            </svg>
          </button>
        </div>
        {tree && (
          <ProjectTree
            tree={tree}
            collapsed={collapsed}
            setCollapsed={setCollapsed}
            selected={selected}
            onSelect={selectProject}
            onExtractTasks={setExtractTarget}
          />
        )}
      </aside>
      <section className="session-pane">
        {selected && (
          <>
            <div className="pane-header">
              <span className="pane-path">
                {selected.path ?? selected.project}
              </span>
            </div>
            <ProjectOverview
              key={`${selectionKey(selected)}-${reloadKey}`}
              project={selected.project}
              path={selected.path}
              onDirtyChange={setOverviewDirty}
            />
          </>
        )}
      </section>
      {extractTarget && (
        <TaskExtractDrawer
          target={extractTarget}
          onClose={() => setExtractTarget(null)}
        />
      )}
    </div>
  );
}

/** 右クリック「タスク抽出」の結果表示。開いた時点で claude CLI 抽出を開始する */
function TaskExtractDrawer({
  target,
  onClose,
}: {
  target: TreeSelection;
  onClose: () => void;
}) {
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setResult(null);
    setError(null);
    api
      .extractTasks(target.project)
      .then(setResult)
      .catch((e) => setError(String(e)));
  }, [target.project]);

  return (
    <div className="drawer-overlay" onClick={onClose}>
      <div className="drawer" onClick={(e) => e.stopPropagation()}>
        <div className="drawer-head">
          <h3>未完了タスク: {target.project}</h3>
          <button className="icon-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="drawer-body">
          {error ? (
            <ErrorBox message={error} />
          ) : result === null ? (
            <p className="muted">
              claude-mem のサマリー履歴からタスクを抽出中…（数十秒かかります）
            </p>
          ) : (
            <pre className="task-result">{result}</pre>
          )}
        </div>
      </div>
    </div>
  );
}

function SearchHitCard({
  hit,
  showProject,
  onOpen,
}: {
  hit: SearchHit;
  showProject: boolean;
  onOpen?: () => void;
}) {
  return (
    <article className="session-card">
      <div className="session-head">
        <div>
          <p className="session-title">{hit.request ?? "(記録なし)"}</p>
          <p className="session-meta">
            {showProject && `${hit.project} · `}
            {formatEpoch(hit.created_at_epoch)}
          </p>
          {hit.completed && (
            <p className="summary-field">
              <span className="field-label">完了</span>
              {hit.completed}
            </p>
          )}
        </div>
        {onOpen && (
          <button className="open-btn" onClick={onOpen}>
            会話を開く
          </button>
        )}
      </div>
    </article>
  );
}

function TranscriptDrawer({
  sessionId,
  onClose,
}: {
  sessionId: string;
  onClose: () => void;
}) {
  const [transcript, setTranscript] = useState<Transcript | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .getTranscript(sessionId)
      .then(setTranscript)
      .catch((e) => setError(String(e)));
  }, [sessionId]);

  return (
    <div className="drawer-overlay" onClick={onClose}>
      <div className="drawer" onClick={(e) => e.stopPropagation()}>
        <div className="drawer-head">
          <div>
            <h2>会話ログ</h2>
            {transcript?.cwd && <p className="muted">{transcript.cwd}</p>}
          </div>
          <button className="close-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="drawer-body">
          {error && <ErrorBox message={error} />}
          {!transcript && !error && <p className="muted">読み込み中…</p>}
          {transcript?.messages.map((m, i) => (
            <div key={i} className={`msg msg-${m.role}`}>
              <span className="msg-role">
                {m.role === "user" ? "You" : "Claude"}
              </span>
              <pre className="msg-text">{m.text}</pre>
            </div>
          ))}
          {transcript?.truncated && (
            <p className="muted">（長すぎるため以降は省略）</p>
          )}
          {transcript && transcript.messages.length === 0 && (
            <p className="muted">テキストメッセージがありません</p>
          )}
        </div>
      </div>
    </div>
  );
}

function GlobalSearchOverlay({ onClose }: { onClose: () => void }) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [openTranscript, setOpenTranscript] = useState<string | null>(null);

  const run = () => {
    if (!query.trim()) return;
    setError(null);
    api
      .searchSummaries(query)
      .then(setHits)
      .catch((e) => setError(String(e)));
  };

  return (
    <div className="drawer-overlay" onClick={onClose}>
      <div className="search-panel" onClick={(e) => e.stopPropagation()}>
        <div className="search-bar">
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && run()}
            placeholder="全プロジェクトのサマリーを全文検索（FTS5構文可）"
          />
          <button onClick={run}>検索</button>
          <button className="clear-btn" onClick={onClose}>
            閉じる
          </button>
        </div>
        {error && <ErrorBox message={error} />}
        <div className="search-panel-results">
          {hits && (
            <div className="session-list">
              {hits.map((h, i) => (
                <SearchHitCard
                  key={i}
                  hit={h}
                  showProject
                  onOpen={
                    h.content_session_id
                      ? () => setOpenTranscript(h.content_session_id)
                      : undefined
                  }
                />
              ))}
              {hits.length === 0 && <p className="muted">ヒットなし</p>}
            </div>
          )}
          {!hits && (
            <p className="muted">
              Enterで検索。例: <code>Tauri AND 設計</code>
            </p>
          )}
        </div>
      </div>
      {openTranscript && (
        <TranscriptDrawer
          sessionId={openTranscript}
          onClose={() => setOpenTranscript(null)}
        />
      )}
    </div>
  );
}

function ErrorBox({ message }: { message: string }) {
  return <div className="error-box">{message}</div>;
}
