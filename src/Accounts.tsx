import { useCallback, useEffect, useRef, useState } from "react";
import { api, Account, AccountsState } from "./api";

const PLAN_LABEL: Record<string, string> = {
  claude_max: "Max",
  claude_pro: "Pro",
};

/**
 * アカウント切り替えビュー。
 *
 * トークンは Keychain にのみ置き、UI・アプリのファイルには一切持ち込まない。
 * 追加は Terminal.app での `claude setup-token`（ブラウザ認証）に委ね、
 * 完了を Keychain のポーリングで検知する。
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
  const [busy, setBusy] = useState(false);
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  const [pending, setPending] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const pollRef = useRef<number | null>(null);

  const reload = useCallback(() => {
    api
      .getAccounts()
      .then(setState)
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    if (open) reload();
  }, [open, reload]);

  const stopPolling = useCallback(() => {
    if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }, []);

  const cancelPending = useCallback(() => {
    stopPolling();
    setPending(null);
  }, [stopPolling]);

  // 閉じても止めないと、ログイン待ちのまま 2 秒おきに Keychain を叩き続け、
  // ユーザーが見ていないところで勝手にアカウントを取り込んでしまう
  useEffect(() => {
    if (!open) cancelPending();
  }, [open, cancelPending]);
  useEffect(() => stopPolling, [stopPolling]);

  const startAdd = () => {
    const name = newName.trim();
    setError(null);
    setBusy(true);
    stopPolling();
    api
      .addAccountInTerminal(name)
      .then(() => {
        setAdding(false);
        setNewName("");
        setPending(name);
        // Terminal 側のログイン完了は通知されないので、Keychain に現れるまで待つ。
        // 放置された待ち受けが永久に回らないよう上限を設ける
        const deadline = Date.now() + 10 * 60 * 1000;
        pollRef.current = window.setInterval(() => {
          if (Date.now() > deadline) {
            cancelPending();
            setError("ログインが10分以内に完了しなかったため中止しました。");
            return;
          }
          api
            .claimPendingAccount(name)
            .then((acc: Account | null) => {
              if (!acc) return;
              cancelPending();
              reload();
            })
            .catch((e) => {
              cancelPending();
              setError(String(e));
            });
        }, 2000);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const switchTo = (name: string) => {
    setError(null);
    setBusy(true);
    api
      .switchAccount(name)
      .then(reload)
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
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

  const installShell = () => {
    setError(null);
    setBusy(true);
    api
      .installShellIntegration()
      .then(reload)
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  if (!open) return null;

  const accounts = state?.accounts ?? [];

  return (
    <div className="drawer-overlay" onClick={onClose}>
      <div className="diagnosis-panel" onClick={(e) => e.stopPropagation()}>
        <div className="drawer-head">
          <div>
            <h2>アカウント</h2>
            <p className="muted">
              登録したアカウントの使用状況を確認できます（トークンは Keychain にのみ保存）
            </p>
          </div>
          <button className="close-btn" onClick={onClose}>
            ✕
          </button>
        </div>

        <div className="drawer-body">
          {error && <p className="acct-error">{error}</p>}

          {state && !state.shell_integration && (
            <div className="acct-banner">
              <div>
                <strong>シェル連携（任意）</strong>
                <p className="muted">
                  選択中アカウントを、新しく開くターミナルの Claude Code の既定にしたい場合は
                  .zshrc に読み込み行を追加します。すでに起動中のセッションには反映されません。
                </p>
              </div>
              <button
                className="diag-primary"
                disabled={busy}
                onClick={installShell}
              >
                設定する
              </button>
            </div>
          )}

          {accounts.length === 0 && !pending && (
            <p className="muted">
              まだアカウントが登録されていません。「アカウントを追加」から
              ブラウザでログインしてください。
            </p>
          )}

          <ul className="acct-list">
            {accounts.map((a) => (
              <li
                key={a.name}
                className={a.active ? "acct-item active" : "acct-item"}
              >
                <button
                  className="acct-radio"
                  disabled={busy || a.active || a.missing_token}
                  onClick={() => switchTo(a.name)}
                  title={
                    a.missing_token
                      ? "トークンが失われています。削除して登録し直してください"
                      : a.active
                        ? "選択中（このアプリの表示と診断で使うアカウント）"
                        : "このアカウントを選択"
                  }
                >
                  <span className={a.active ? "dot on" : "dot"} />
                  <span className="acct-meta">
                    <span className="acct-name">
                      {a.name}
                      {a.is_live && (
                        <span
                          className="acct-live"
                          title="Claude Code が現在ログイン中。連携なしの起動中セッションはこのアカウントを消費します"
                        >
                          ログイン中
                        </span>
                      )}
                      {a.missing_token && (
                        <span className="acct-warn">要再登録</span>
                      )}
                    </span>
                    <span className="muted acct-email">
                      {a.missing_token
                        ? "Keychain にトークンがありません"
                        : `${a.email}${a.plan ? ` ・ ${PLAN_LABEL[a.plan] ?? a.plan}` : ""}`}
                    </span>
                  </span>
                </button>
                {confirmRemove === a.name ? (
                  <>
                    <span className="acct-confirm muted">
                      {a.active && "選択中です。"}削除すると再ログインが必要です
                    </span>
                    <button
                      className="acct-remove danger"
                      disabled={busy}
                      onClick={() => remove(a.name)}
                    >
                      削除する
                    </button>
                    <button
                      className="acct-remove"
                      onClick={() => setConfirmRemove(null)}
                    >
                      やめる
                    </button>
                  </>
                ) : (
                  <button
                    className="acct-remove"
                    disabled={busy}
                    onClick={() => setConfirmRemove(a.name)}
                    title="登録を削除（Claude 側のアカウントは消えません）"
                  >
                    削除
                  </button>
                )}
              </li>
            ))}
          </ul>

          {pending && (
            <div className="acct-pending">
              <span className="diag-spinner" />
              <div>
                <strong>「{pending}」のログイン待ちです</strong>
                <p className="muted">
                  Terminal でブラウザ認証を完了してください。完了すると自動で
                  取り込みます。
                </p>
              </div>
              <button className="acct-remove" onClick={cancelPending}>
                中止
              </button>
            </div>
          )}

          {adding ? (
            <div className="acct-add">
              <input
                autoFocus
                value={newName}
                placeholder="アカウント名（例: work / personal）"
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && newName.trim()) startAdd();
                  if (e.key === "Escape") setAdding(false);
                }}
              />
              <button
                className="diag-primary"
                disabled={busy || !newName.trim()}
                onClick={startAdd}
              >
                Terminal でログイン
              </button>
              <button className="acct-remove" onClick={() => setAdding(false)}>
                やめる
              </button>
            </div>
          ) : (
            !pending && (
              <button
                className="diag-primary"
                disabled={busy}
                onClick={() => setAdding(true)}
              >
                ＋ アカウントを追加
              </button>
            )
          )}

          {accounts.length > 0 && (
            <p className="acct-note muted">
              各アカウントの残り枠はメニューバーとツールバーの使用状況アイコンで確認できます。
              起動中の Claude Code
              セッションの消費先はこのアプリからは変更できません（セッション切り替えは非対応）。
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
