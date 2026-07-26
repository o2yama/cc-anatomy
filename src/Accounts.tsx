import { useCallback, useEffect, useRef, useState } from "react";
import { api, Account, AccountsState, accountLabel } from "./api";

const PLAN_LABEL: Record<string, string> = {
  claude_max: "Max",
  claude_pro: "Pro",
};

const LOGIN_TIMEOUT_MS = 5 * 60 * 1000;
const POLL_INTERVAL_MS = 2000;

/**
 * 「起動中セッションがあります」確認ダイアログの「今後この確認を表示しない」設定。
 * `cc-anatomy.showGlobal`（ProjectOverview.tsx）と同じ localStorage の流儀（"1"/未設定）。
 * 対象は sessions_running の確認のみ。ずれ検知警告（同一性検証の mismatched）と
 * needs_import（未登録ログインの取り込み確認）はデータ喪失・安全性に関わるため対象外で、
 * 常に表示する（2026-07-26 ユーザー承認: 12時間の実機検証で巻き戻り未観測、
 * sync-back + 同一性検証の防御があるため毎回の確認は過剰と判断）。
 * リセットしたい場合はブラウザの devtools 等で該当キーを削除する（専用 UI は用意していない）
 */
const SKIP_SESSIONS_CONFIRM_KEY = "cc-anatomy.skipSessionsConfirm";
const skipSessionsConfirmEnabled = () =>
  localStorage.getItem(SKIP_SESSIONS_CONFIRM_KEY) === "1";

/** 「取り込みますか？」確認ダイアログの対象。switch は特定アカウントへの切り替え、
 * login はこれから始める claude auth login の事前 sync-back で未登録ログインを検知したケース */
type PendingConfirm =
  | { kind: "switch"; name: string; liveEmail: string | null }
  | { kind: "login"; liveEmail: string | null };

/** 「起動中セッションがあるが続行するか」の確認ダイアログの対象。
 * 続行を選ぶと同じ操作を force=true で再実行する */
type SessionsConfirm =
  | { kind: "switch"; name: string; count: number }
  | { kind: "login"; count: number };

/**
 * アカウント切り替えビュー（Keychain スワップ方式）。
 *
 * 「切り替え」「アカウント追加（ブラウザログイン）」は PC 全体のログイン情報
 * （ライブ Keychain + ~/.claude.json）を書き換える。実行中の Claude Code セッションが
 * 自分のトークンをライブへ書き戻して結果を踏み潰しうるため、外部セッションが1件以上あると
 * まず確認ダイアログを挟む（force=true で続行を選べる。ユーザー環境ではシェルセッションが
 * 常時複数開いており「0件」を強制すると機能が使えなくなるため、ハードブロックではなく
 * 確認方式にしている）。本アプリ自身の環境診断/タスク抽出の実行中だけは待てば済むため
 * 常にハードブロックされる（バックエンドからのエラーとして表示）。
 *
 * 確認ダイアログ（起動中セッション続行 / 未登録ログインの取り込み）はどちらも
 * モーダル中央のオーバーレイカードに統一して表示する（バナー先頭に出すと、
 * どのボタン操作に対する確認か分かりにくくなるため）。
 *
 * アカウント名はクリックしてインライン編集できる（表示名のみ変更。Keychain 照合キーである
 * 内部識別子 name は不変。表示は accountLabel() = display_name ?? name で統一する）。
 *
 * 2026-07-25 ユーザー決定で監視用長期トークンの仕組みを全廃した。使用量は現在ライブの
 * アカウントのみをツールバー/メニューバーで表示し、ここでは切り替え管理のみを行う。
 */
export function AccountsOverlay({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const [state, setState] = useState<AccountsState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [pendingConfirm, setPendingConfirm] = useState<PendingConfirm | null>(null);
  const [sessionsConfirm, setSessionsConfirm] = useState<SessionsConfirm | null>(null);
  // ダイアログを開くたびにチェックを外した状態に戻す（前回のチェックを引き継がない）
  const [skipFutureChecked, setSkipFutureChecked] = useState(false);

  // 表示名のインライン編集。editingName は編集中アカウントの内部識別子(name)
  const [editingName, setEditingName] = useState<string | null>(null);
  const [editingValue, setEditingValue] = useState("");
  // Escape での取り消しは blur 経由で走らせる（onBlur と onKeyDown の二重発火を区別するため）
  const cancelingEditRef = useRef(false);

  // ドラッグ&ドロップでの並び替え。サーバーの accounts.json の並びを楽観的に更新し、
  // reorder_accounts の失敗時のみ reload() で巻き戻す
  const [displayAccounts, setDisplayAccounts] = useState<Account[]>([]);
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null);

  // Flow B: claude auth login のログイン待ち
  const [loginPending, setLoginPending] = useState(false);
  const loginPollRef = useRef<number | null>(null);

  const reload = useCallback(() => {
    api
      .getAccounts()
      .then(setState)
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    if (open) reload();
  }, [open, reload]);

  // サーバーから再取得するたびに表示順をリセットする（並び替え失敗時の巻き戻しもこれで行う）
  useEffect(() => {
    setDisplayAccounts(state?.accounts ?? []);
  }, [state]);

  const stopLoginPolling = useCallback(() => {
    if (loginPollRef.current !== null) {
      window.clearInterval(loginPollRef.current);
      loginPollRef.current = null;
    }
  }, []);
  const cancelLogin = useCallback(() => {
    stopLoginPolling();
    setLoginPending(false);
  }, [stopLoginPolling]);

  // 閉じても止めないと、ログイン待ちのまま2秒おきに Keychain を叩き続け、
  // ユーザーが見ていないところで勝手にアカウントを取り込んでしまう
  useEffect(() => {
    if (!open) cancelLogin();
  }, [open, cancelLogin]);
  useEffect(() => stopLoginPolling, [stopLoginPolling]);

  const switchBusy = busy || loginPending;

  const pollForCompletion = (baseline: string) => {
    setLoginPending(true);
    const deadline = Date.now() + LOGIN_TIMEOUT_MS;
    loginPollRef.current = window.setInterval(() => {
      if (Date.now() > deadline) {
        cancelLogin();
        setError("ログインが5分以内に完了しなかったため中止しました。");
        return;
      }
      api
        .pollAddAccountLogin(baseline)
        .then((result) => {
          if (result.status === "waiting") return;
          cancelLogin();
          setNotice(
            `「${result.account.email || accountLabel(result.account)}」を取り込みました。本人のアカウントか確認してください。`
          );
          reload();
        })
        .catch((e) => {
          cancelLogin();
          setError(String(e));
        });
    }, POLL_INTERVAL_MS);
  };

  const startAddLogin = (force = false): Promise<void> => {
    setError(null);
    setNotice(null);
    setBusy(true);
    stopLoginPolling();
    return api
      .startAddAccountLogin(force)
      .then((outcome) => {
        if (outcome.status === "needs_import") {
          setPendingConfirm({ kind: "login", liveEmail: outcome.live_email });
          return;
        }
        if (outcome.status === "sessions_running") {
          // 「今後表示しない」が有効なら確認を出さず自動で force=true 再試行する
          if (skipSessionsConfirmEnabled()) {
            return startAddLogin(true);
          }
          setSessionsConfirm({ kind: "login", count: outcome.count });
          return;
        }
        setSessionsConfirm(null);
        if (outcome.warning) setNotice(outcome.warning);
        pollForCompletion(outcome.baseline);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const importLive = () => {
    setError(null);
    setNotice(null);
    setBusy(true);
    return api
      .importLiveAccount()
      .then((acc) => {
        setNotice(`「${acc.email || accountLabel(acc)}」を取り込みました。`);
        reload();
        return acc;
      })
      .catch((e) => {
        setError(String(e));
        throw e;
      })
      .finally(() => setBusy(false));
  };

  const doSwitch = (name: string, force = false): Promise<void> => {
    setError(null);
    setNotice(null);
    setBusy(true);
    return api
      .switchAccount(name, force)
      .then((outcome) => {
        if (outcome.status === "needs_import") {
          setPendingConfirm({ kind: "switch", name, liveEmail: outcome.live_email });
          return;
        }
        if (outcome.status === "sessions_running") {
          // 「今後表示しない」が有効なら確認を出さず自動で force=true 再試行する
          if (skipSessionsConfirmEnabled()) {
            return doSwitch(name, true);
          }
          setSessionsConfirm({ kind: "switch", name, count: outcome.count });
          return;
        }
        setPendingConfirm(null);
        setSessionsConfirm(null);
        setNotice(
          outcome.warning ??
            "切り替えました。実行中の Claude Code セッションには反映されません。新しく起動したセッションから有効です。"
        );
        reload();
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const confirmImportThenContinue = () => {
    if (!pendingConfirm) return;
    const confirm = pendingConfirm;
    setPendingConfirm(null);
    importLive()
      .then(() => {
        if (confirm.kind === "switch") {
          doSwitch(confirm.name);
        } else {
          startAddLogin();
        }
      })
      .catch(() => {
        /* エラーは importLive 内で表示済み */
      });
  };

  const confirmSessionsAndContinue = () => {
    if (!sessionsConfirm) return;
    const confirm = sessionsConfirm;
    // 「やめる」の場合は保存しない。「続行する」を押したときだけチェック状態を反映する
    if (skipFutureChecked) {
      localStorage.setItem(SKIP_SESSIONS_CONFIRM_KEY, "1");
    }
    setSessionsConfirm(null);
    setSkipFutureChecked(false);
    if (confirm.kind === "switch") {
      doSwitch(confirm.name, true);
    } else {
      startAddLogin(true);
    }
  };

  const remove = (name: string) => {
    setError(null);
    setBusy(true);
    setConfirmRemove(null);
    api
      .removeAccount(name)
      .then(reload)
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const startEditingName = (a: Account) => {
    setEditingName(a.name);
    setEditingValue(accountLabel(a));
  };

  const commitRename = (name: string) => {
    if (cancelingEditRef.current) {
      cancelingEditRef.current = false;
      setEditingName(null);
      return;
    }
    setEditingName(null);
    api
      .renameAccount(name, editingValue)
      .then(reload)
      .catch((e) => setError(String(e)));
  };

  const handleDrop = (dropIndex: number) => {
    setDragOverIndex(null);
    if (dragIndex === null || dragIndex === dropIndex) {
      setDragIndex(null);
      return;
    }
    const next = [...displayAccounts];
    const [moved] = next.splice(dragIndex, 1);
    next.splice(dropIndex, 0, moved);
    setDragIndex(null);
    setDisplayAccounts(next); // 楽観的に確定
    api
      .reorderAccounts(next.map((a) => a.name))
      .catch((e) => {
        setError(String(e));
        reload(); // 失敗時は再取得して巻き戻す
      });
  };

  if (!open) return null;

  const accounts = state?.accounts ?? [];
  const hasLegacyAccounts = accounts.some((a) => !a.has_credentials);
  const runningSessionsCount = state?.running_sessions ?? 0;
  // 確認ダイアログは対象アカウントの内部識別子(name)しか持たないため、表示名を引き直す
  const labelFor = (name: string) => {
    const found = accounts.find((a) => a.name === name);
    return found ? accountLabel(found) : name;
  };
  // 名前編集中・busy中・確認ダイアログ表示中はドラッグ操作を無効にする
  const dragDisabled =
    switchBusy || busy || editingName !== null || pendingConfirm !== null || sessionsConfirm !== null;

  return (
    <div className="drawer-overlay" onClick={onClose}>
      <div className="diagnosis-panel" onClick={(e) => e.stopPropagation()}>
        <div className="drawer-head">
          <div>
            <h2>アカウント</h2>
            <p className="muted">
              使用するClaudeサブスクリプションアカウントの設定・切り替えができます。
            </p>
          </div>
          <button className="close-btn" onClick={onClose}>
            ✕
          </button>
        </div>

        <div className="drawer-body">
          {error && <p className="acct-error">{error}</p>}
          {notice && <p className="muted">{notice}</p>}

          {runningSessionsCount > 0 && (
            <p className="muted">
              起動中の Claude Code セッションが{runningSessionsCount}
              件あります。切り替え・追加を行うと確認ダイアログが出ます。
            </p>
          )}

          {hasLegacyAccounts && (
            <p className="muted">
              「未取り込み」のアカウントは保存済みのログイン情報がありません。「＋
              アカウントを追加」からこのアカウントでログインするとログイン情報が取り込まれます（ブラウザでは登録したいアカウントを選んでください。別アカウントでログインすると、別のアカウントとして新規に取り込まれます）。
            </p>
          )}

          {state && (
            <div className="acct-banner">
              <div>
                <strong>現在のログイン: {state.live_email ?? "検出できません"}</strong>
                {state.live_email && !state.live_registered && (
                  <p className="muted">
                    このアカウントはまだ登録されていません。取り込むと保存され、切り替え先として選べるようになります。
                  </p>
                )}
              </div>
              {state.live_email && (
                <button className="acct-btn acct-btn-primary" disabled={busy} onClick={() => importLive()}>
                  {state.live_registered ? "セッション更新" : "取り込む"}
                </button>
              )}
            </div>
          )}

          {accounts.length === 0 && !loginPending && (
            <p className="muted">
              まだアカウントが登録されていません。「アカウントを追加」からブラウザでログインしてください。
            </p>
          )}

          <ul className="acct-list">
            {displayAccounts.map((a, idx) => (
              <li
                key={a.name}
                draggable={!dragDisabled}
                onDragStart={(e) => {
                  const target = e.target as HTMLElement;
                  // input/button からのドラッグ開始は編集・誤操作と競合するため抑止する
                  if (dragDisabled || target.closest("input, button")) {
                    e.preventDefault();
                    return;
                  }
                  setDragIndex(idx);
                  e.dataTransfer.effectAllowed = "move";
                  // WebKit（macOS の WKWebView）は setData が無いとドラッグ自体を開始しない
                  e.dataTransfer.setData("text/plain", a.name);
                }}
                onDragOver={(e) => {
                  if (dragIndex === null) return;
                  e.preventDefault();
                  if (dragOverIndex !== idx) setDragOverIndex(idx);
                }}
                onDragLeave={() => setDragOverIndex((v) => (v === idx ? null : v))}
                onDrop={(e) => {
                  e.preventDefault();
                  handleDrop(idx);
                }}
                onDragEnd={() => {
                  setDragIndex(null);
                  setDragOverIndex(null);
                }}
                className={[
                  a.is_live ? "acct-item live" : "acct-item",
                  dragIndex === idx ? "acct-item-dragging" : "",
                  dragOverIndex === idx && dragIndex !== idx ? "acct-item-drag-over" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
              >
                <div className="acct-info">
                  <span className="acct-name-row">
                    {editingName === a.name ? (
                      <input
                        className="acct-name-input"
                        autoFocus
                        draggable={false}
                        value={editingValue}
                        onChange={(e) => setEditingValue(e.target.value)}
                        onBlur={() => commitRename(a.name)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.currentTarget.blur();
                          } else if (e.key === "Escape") {
                            cancelingEditRef.current = true;
                            e.currentTarget.blur();
                          }
                        }}
                      />
                    ) : (
                      <span
                        className="acct-name"
                        title="クリックして表示名を変更"
                        onClick={() => startEditingName(a)}
                      >
                        {accountLabel(a)}
                      </span>
                    )}
                    {a.is_live && (
                      <span
                        className="acct-live"
                        title="Claude Code が現在ログイン中。連携なしの起動中セッションはこのアカウントを消費します"
                      >
                        ログイン中
                      </span>
                    )}
                    {!a.has_credentials && <span className="acct-warn">未取り込み</span>}
                  </span>
                  <span className="muted acct-email">
                    {a.email || "(メール未取得)"}
                    {a.plan ? ` ・ ${PLAN_LABEL[a.plan] ?? a.plan}` : ""}
                  </span>
                </div>

                <div className="acct-actions">
                  {confirmRemove === a.name ? (
                    <>
                      <span className="acct-confirm muted">
                        削除すると、追加ボタンから元のアカウントでログインし直す必要があります
                      </span>
                      <button className="acct-btn acct-btn-ghost" onClick={() => setConfirmRemove(null)}>
                        やめる
                      </button>
                      <button
                        className="acct-btn acct-btn-danger"
                        disabled={busy}
                        onClick={() => remove(a.name)}
                      >
                        削除する
                      </button>
                    </>
                  ) : (
                    <>
                      <button
                        className="acct-btn acct-btn-primary"
                        disabled={switchBusy || a.is_live || !a.has_credentials}
                        title={
                          !a.has_credentials
                            ? "追加ボタンからこのアカウントでログインすると資格情報が取り込まれます"
                            : a.is_live
                              ? "現在ログイン中です"
                              : "このアカウントに切り替える"
                        }
                        onClick={() => doSwitch(a.name)}
                      >
                        切り替える
                      </button>
                      <button
                        className="acct-btn acct-btn-ghost"
                        disabled={busy}
                        onClick={() => setConfirmRemove(a.name)}
                        title="登録を削除（Claude 側のアカウントは消えません）"
                      >
                        削除
                      </button>
                    </>
                  )}
                </div>
              </li>
            ))}
          </ul>

          {loginPending ? (
            <div className="acct-pending">
              <span className="diag-spinner" />
              <div>
                <strong>ブラウザ認証を待っています</strong>
                <p className="muted">
                  Terminal でログインを完了してください。完了すると自動で取り込みます。
                </p>
              </div>
              <button className="acct-btn acct-btn-ghost" onClick={cancelLogin}>
                中止
              </button>
            </div>
          ) : (
            <button className="acct-btn acct-btn-primary" disabled={switchBusy} onClick={() => startAddLogin()}>
              ＋ アカウントを追加（ブラウザでログイン）
            </button>
          )}

        </div>

        {(pendingConfirm || sessionsConfirm) && (
          <div
            className="acct-modal-overlay"
            onClick={(e) => {
              e.stopPropagation();
              setPendingConfirm(null);
              setSessionsConfirm(null);
              setSkipFutureChecked(false);
            }}
          >
            <div className="acct-modal-card" onClick={(e) => e.stopPropagation()}>
              {pendingConfirm ? (
                <>
                  <strong>
                    {pendingConfirm.kind === "switch"
                      ? `「${labelFor(pendingConfirm.name)}」に切り替えます`
                      : "ブラウザでログインします"}
                  </strong>
                  <p className="muted">
                    現在のログイン
                    {pendingConfirm.liveEmail ? `（${pendingConfirm.liveEmail}）` : ""}
                    は未登録です。取り込まずに進むと失われます。取り込みますか？
                  </p>
                  <div className="acct-modal-actions">
                    <button className="acct-btn acct-btn-ghost" onClick={() => setPendingConfirm(null)}>
                      やめる
                    </button>
                    <button
                      className="acct-btn acct-btn-primary"
                      disabled={busy}
                      onClick={confirmImportThenContinue}
                    >
                      取り込んで{pendingConfirm.kind === "switch" ? "切り替える" : "続ける"}
                    </button>
                  </div>
                </>
              ) : sessionsConfirm ? (
                <>
                  <strong>
                    {sessionsConfirm.kind === "switch"
                      ? `「${labelFor(sessionsConfirm.name)}」に切り替えます`
                      : "ブラウザでログインします"}
                  </strong>
                  <p className="muted">
                    起動中の Claude Code セッションが{sessionsConfirm.count}
                    件あります。続行すると、実行中セッションが古いトークンを書き戻して切り替えが巻き戻ったり、保存済みアカウントが後で「＋
                    アカウントを追加」から改めてログインし直す必要になる可能性があります。全セッション終了を推奨しますが、続行しますか？
                  </p>
                  <label className="acct-modal-checkbox">
                    <input
                      type="checkbox"
                      checked={skipFutureChecked}
                      onChange={(e) => setSkipFutureChecked(e.target.checked)}
                    />
                    今後この確認を表示しない
                  </label>
                  <div className="acct-modal-actions">
                    <button
                      className="acct-btn acct-btn-ghost"
                      onClick={() => {
                        setSessionsConfirm(null);
                        setSkipFutureChecked(false);
                      }}
                    >
                      やめる
                    </button>
                    <button
                      className="acct-btn acct-btn-primary"
                      disabled={busy}
                      onClick={confirmSessionsAndContinue}
                    >
                      続行する
                    </button>
                  </div>
                </>
              ) : null}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
