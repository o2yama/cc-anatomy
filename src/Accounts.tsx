import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  DndContext,
  DragOverlay,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { restrictToParentElement, restrictToVerticalAxis } from "@dnd-kit/modifiers";
import {
  api,
  Account,
  AccountsState,
  AccountUsage,
  AccountsUpdatedEvent,
  accountLabel,
  describeAccountError,
  ownerErrorKind,
  isForceSwitchEligible,
} from "./api";

const PLAN_LABEL: Record<string, string> = {
  claude_max: "Max",
  claude_pro: "Pro",
};

const LOGIN_TIMEOUT_MS = 5 * 60 * 1000;
const POLL_INTERVAL_MS = 2000;
/** 再ログイン導線の mismatch 誤検知対策。Keychain（完了検知）と ~/.claude.json（照合）の
 * 書き込み順の隙間を待ってから1回だけ再確認する（2026-07-26 レビュー M-6b） */
const MISMATCH_RETRY_DELAY_MS = 3000;
/** 使用量の常時監視（claude setup-token）は完全に任意の後付け機能なので、
 * ログイン待ち（5分）より短いタイムアウトでスキップ扱いにする */
const MONITOR_TIMEOUT_MS = 90 * 1000;

/**
 * 「起動中セッションがあります」確認ダイアログの「今後この確認を表示しない」設定。
 * `cc-anatomy.showGlobal`（ProjectOverview.tsx）と同じ localStorage の流儀（"1"/未設定）。
 * 対象は sessions_running の確認のみ。ずれ検知警告（同一性検証の mismatched）と
 * needs_import（未登録ログインの取り込み確認）はデータ喪失・安全性に関わるため対象外で、
 * 常に表示する（2026-07-26 ユーザー承認: 12時間の実機検証で巻き戻り未観測、
 * sync-back + 同一性検証の防御があるため毎回の確認は過剰と判断）。
 * リセットしたい場合はブラウザの devtools 等で該当キーを削除する（専用 UI は用意していない）
 */
export const SKIP_SESSIONS_CONFIRM_KEY = "cc-anatomy.skipSessionsConfirm";
export const skipSessionsConfirmEnabled = () =>
  localStorage.getItem(SKIP_SESSIONS_CONFIRM_KEY) === "1";

/** 「取り込みますか？」確認ダイアログの対象。switch は特定アカウントへの切り替え、
 * login はこれから始める claude auth login の事前 sync-back で未登録ログインを検知したケース。
 * targetName は「再ログイン」導線（登録済みカードからの起動）のときだけ入り、
 * 確認後に同じ対象で startAddLogin を再開するために持ち回す */
type PendingConfirm =
  | { kind: "switch"; name: string; liveEmail: string | null }
  | { kind: "login"; liveEmail: string | null; targetName?: string };

/** 「起動中セッションがあるが続行するか」の確認ダイアログの対象。
 * 続行を選ぶと同じ操作を force=true で再実行する。trust はこの確認が発生した時点の
 * trustUnverified 値を持ち回る（2026-08-08 issue #3 再レビュー: owner確認→セッション確認と
 * 進んだ後に「続行する」を押した際、trustUnverified を落として再度 owner確認に戻る
 * 二度手間を防ぐ） */
type SessionsConfirm =
  | { kind: "switch"; name: string; count: number; trust: boolean }
  | { kind: "login"; count: number; targetName?: string; trust: boolean };

/** 「持ち主を確認できない（token 期限切れ／通信不能）が続行するか」の確認ダイアログの
 * 対象（2026-08-08 issue #3、レビュー案A）。続行を選ぶと同じ操作を trustUnverified=true で
 * 再実行する（force はこの確認が発生した時点の値をそのまま持ち回る。force と
 * trustUnverified は独立した同意なので、sessionsConfirm 側の「続行する」がここを
 * 兼ねることはない）。バックエンドは sync-back の書き込みを一切行わずスキップするだけで、
 * 別アカウントの資格情報で上書きすることはない（accounts.rs::SyncBack::SkippedUnverified 参照）。
 * message は describeAccountError で得た原文（TokenExpired/NetworkError の回復手段説明）を
 * そのまま持ち回る */
type OwnerConfirm =
  | { kind: "switch"; name: string; message: string; force: boolean }
  | { kind: "login"; message: string; targetName?: string; force: boolean };

/** 使用率の1行分のテキスト（例: "5h 9% ・ 週 52%"）。リセット時刻を過ぎている想定なら
 * 「5h リセット済み」に置き換える。値が無ければ null（呼び出し側は行ごと出さない） */
function usageText(usage: AccountUsage): string | null {
  if (usage.five_pct == null) return null;
  const five = usage.five_probably_reset ? "5h リセット済み" : `5h ${Math.round(usage.five_pct)}%`;
  const seven = usage.seven_pct != null ? ` ・ 週 ${Math.round(usage.seven_pct)}%` : "";
  return `${five}${seven}`;
}

/** 1行分の名前・email・バッジ・使用率表示。行本体（AccountRow）とドラッグ中のプレビュー
 * （AccountRowPreview）で共有する。編集中はクリック対象の名前が input に差し替わる。
 * 使用率は get_accounts とは別コマンド（get_accounts_usage）で非同期に埋まるため任意（undefined 可） */
function AccountRowContent({
  account,
  usage,
  editing = false,
  editingValue = "",
  onEditingValueChange,
  onStartEditing,
  onCommitRename,
  onCancelEditingViaEscape,
  monitorBusy = false,
  onStartMonitor,
}: {
  account: Account;
  usage?: AccountUsage;
  editing?: boolean;
  editingValue?: string;
  onEditingValueChange?: (v: string) => void;
  onStartEditing?: () => void;
  onCommitRename?: () => void;
  onCancelEditingViaEscape?: () => void;
  /** この行の「常時監視を設定」が実行中（Terminal でのブラウザ承認待ち） */
  monitorBusy?: boolean;
  /** 未指定（ドラッグ中プレビュー等）なら常時監視の導線自体を出さない */
  onStartMonitor?: () => void;
}) {
  const usageLine = usage ? usageText(usage) : null;
  return (
    <div className="acct-info">
      <span className="acct-name-row">
        {editing ? (
          <input
            className="acct-name-input"
            autoFocus
            draggable={false}
            value={editingValue}
            onChange={(e) => onEditingValueChange?.(e.target.value)}
            onBlur={onCommitRename}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.currentTarget.blur();
              } else if (e.key === "Escape") {
                onCancelEditingViaEscape?.();
                e.currentTarget.blur();
              }
            }}
          />
        ) : (
          <span className="acct-name" title="クリックして表示名を変更" onClick={onStartEditing}>
            {accountLabel(account)}
          </span>
        )}
        {account.is_live && <span className="acct-live">ログイン中</span>}
        {!account.has_credentials && <span className="acct-warn">未取り込み</span>}
        {account.has_monitor_token && (
          <span className="acct-live acct-monitor-badge" title="使用量を常時監視しています">
            常時監視中
          </span>
        )}
      </span>
      <span className="muted acct-email">
        {account.email || "(メール未取得)"}
        {account.plan ? ` ・ ${PLAN_LABEL[account.plan] ?? account.plan}` : ""}
      </span>
      {usageLine && <span className="muted acct-usage">{usageLine}</span>}
      {/* 常時監視は切り替え機能とは独立の任意機能。過密にならないよう、未設定のときだけ
          小さなテキストリンクとして出す（設定済みは上のバッジで示すので導線は消える） */}
      {!account.has_monitor_token && onStartMonitor && (
        <button
          type="button"
          className="acct-monitor-link"
          disabled={monitorBusy}
          onClick={(e) => {
            e.stopPropagation();
            onStartMonitor();
          }}
        >
          {monitorBusy ? "承認待ち…" : "常時監視を設定"}
        </button>
      )}
    </div>
  );
}

/** DragOverlay に浮かせる、掴んだカードの静的なコピー（クリック等の副作用は持たない） */
function AccountRowPreview({ account }: { account: Account }) {
  return (
    <div className={account.is_live ? "acct-item acct-drag-preview live" : "acct-item acct-drag-preview"}>
      <span className="acct-drag-handle">⠿</span>
      <AccountRowContent account={account} />
      <div className="acct-actions" />
    </div>
  );
}

/**
 * 一覧の1行。ドラッグはハンドル（⠿）からのみ開始できるよう、
 * useSortable の attributes/listeners はハンドルにだけ付与する
 * （行全体に付けると名前編集クリックやボタン操作と衝突するため）。
 */
function AccountRow({
  account,
  usage,
  dragDisabled,
  editing,
  editingValue,
  onEditingValueChange,
  onStartEditing,
  onCommitRename,
  onCancelEditingViaEscape,
  confirmingRemove,
  onRequestRemove,
  onCancelRemove,
  onConfirmRemove,
  onSwitch,
  onRelogin,
  switchButtonDisabled,
  busy,
  monitorBusy,
  onStartMonitor,
}: {
  account: Account;
  usage?: AccountUsage;
  dragDisabled: boolean;
  editing: boolean;
  editingValue: string;
  onEditingValueChange: (v: string) => void;
  onStartEditing: () => void;
  onCommitRename: () => void;
  onCancelEditingViaEscape: () => void;
  confirmingRemove: boolean;
  onRequestRemove: () => void;
  onCancelRemove: () => void;
  onConfirmRemove: () => void;
  onSwitch: () => void;
  /** 資格情報が使えないカード（has_credentials=false）の「再ログイン」導線 */
  onRelogin: () => void;
  switchButtonDisabled: boolean;
  busy: boolean;
  monitorBusy: boolean;
  onStartMonitor: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: account.name,
    disabled: dragDisabled,
  });
  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  return (
    <li
      ref={setNodeRef}
      style={style}
      className={[
        account.is_live ? "acct-item live" : "acct-item",
        isDragging ? "acct-item-dragging" : "",
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <button
        type="button"
        className="acct-drag-handle"
        disabled={dragDisabled}
        aria-label="ドラッグして並び替え"
        title="ドラッグして並び替え"
        {...attributes}
        {...listeners}
      >
        ⠿
      </button>

      <AccountRowContent
        account={account}
        usage={usage}
        editing={editing}
        editingValue={editingValue}
        onEditingValueChange={onEditingValueChange}
        onStartEditing={onStartEditing}
        onCommitRename={onCommitRename}
        onCancelEditingViaEscape={onCancelEditingViaEscape}
        monitorBusy={monitorBusy}
        onStartMonitor={onStartMonitor}
      />

      <div className="acct-actions">
        {confirmingRemove ? (
          <>
            <span className="acct-confirm muted">
              削除すると、追加ボタンから元のアカウントでログインし直す必要があります
            </span>
            <button className="acct-btn acct-btn-ghost" onClick={onCancelRemove}>
              やめる
            </button>
            <button className="acct-btn acct-btn-danger" disabled={busy} onClick={onConfirmRemove}>
              削除する
            </button>
          </>
        ) : (
          <>
            {account.has_credentials ? (
              <button
                className="acct-btn acct-btn-primary"
                disabled={switchButtonDisabled || account.is_live}
                title={account.is_live ? "現在ログイン中です" : "このアカウントに切り替える"}
                onClick={onSwitch}
              >
                切り替える
              </button>
            ) : (
              <button
                className="acct-btn acct-btn-primary"
                disabled={switchButtonDisabled || account.is_live || !account.can_relogin}
                title={
                  account.is_live
                    // 60秒ごとの自動更新（tray.rs）がライブと一致した登録済みアカウントの
                    // 資格情報を自動で最新化するため、手動操作は不要（2026-07-26 レビュー L-12）
                    ? "現在ログイン中のアカウントです。1分以内に自動で資格情報が更新されます"
                    : !account.can_relogin
                      // org_id・email のどちらも無い旧登録は照合しようがない（2026-07-26 レビュー M-7）
                      ? "照合に使える情報（組織ID・メールアドレス）が無いため再ログインでは紐づけできません。「＋アカウントを追加」から新規登録してください"
                      : "ブラウザで再ログインすると、このアカウントの資格情報を取り込み直します（別アカウントでログインした場合は取り込まずエラーになります）"
                }
                onClick={onRelogin}
              >
                再ログイン
              </button>
            )}
            <button
              className="acct-btn acct-btn-ghost"
              disabled={busy}
              onClick={onRequestRemove}
              title="登録を削除（Claude 側のアカウントは消えません）"
            >
              削除
            </button>
          </>
        )}
      </div>
    </li>
  );
}

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
 * 一覧の並び替えは @dnd-kit/core + @dnd-kit/sortable によるドラッグ&ドロップ（2026-07-26
 * ユーザー評価を受けて自前 HTML5 DnD 実装から作り直した）。ドラッグはカード左端のハンドル
 * （⠿）からのみ開始し、名前編集・ボタン操作とは競合しない。並び替えはドロップ確定時に
 * 楽観的更新 → reorder_accounts、失敗時は reload() で巻き戻す。
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
  // アカウントごとの使用率。get_accounts とは別コマンドで、一覧表示をブロックせず
  // モーダルを開いた後に非同期で埋める（切り替え前にどのアカウントが空いているか見えるように）
  const [usageByName, setUsageByName] = useState<Record<string, AccountUsage>>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [pendingConfirm, setPendingConfirm] = useState<PendingConfirm | null>(null);
  const [sessionsConfirm, setSessionsConfirm] = useState<SessionsConfirm | null>(null);
  const [ownerConfirm, setOwnerConfirm] = useState<OwnerConfirm | null>(null);
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
  const [activeDragName, setActiveDragName] = useState<string | null>(null);
  const sensors = useSensors(
    // distance: 6 で、クリック（名前編集ボタン等）と誤って競合しないようにする
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } })
  );

  // Flow B: claude auth login のログイン待ち
  const [loginPending, setLoginPending] = useState(false);
  const loginPollRef = useRef<number | null>(null);
  // mismatch 再確認の setTimeout（pollForCompletion 内）。id を追跡してキャンセル・画面
  // クローズ時に必ず clearTimeout する（2026-07-26 レビュー M-A1）。追跡しないと、
  // ユーザーが画面を閉じた・キャンセルした後に書き込みAPIである pollAddAccountLogin が
  // 勝手に発火し、取り込み・監視フローが始まってしまう
  const mismatchRetryRef = useRef<number | null>(null);
  // "polling" 中の pollForCompletion 呼び出しだけが応答を処理してよい、という一方向ガード。
  // interval は前回 invoke の完了を待たず2秒ごとに発火するため複数の pollAddAccountLogin が
  // 同時に in-flight になりうる。ガード無しだと複数の応答がばらばらの順で戻ってきて
  // handleMismatch/settle が二重実行されうる（2026-07-26 レビュー M-A3）。cancelLogin が
  // "idle" に戻すことで、キャンセル済み・画面クローズ後に届く在り来たりの応答も無視される
  const loginPhaseRef = useRef<"idle" | "polling" | "retrying">("idle");

  // 使用量の常時監視（claude setup-token、任意機能）の紐づけ待ち。
  // 「＋アカウントを追加」の統合フロー・ステップ2（monitorFlowName）と、
  // 登録済み行の単独「常時監視を設定」（rowMonitorName）の2箇所で使う。
  // どちらも切り替え機能とは独立で、失敗・タイムアウトしても他の操作をブロックしない
  const [monitorFlowName, setMonitorFlowName] = useState<string | null>(null);
  const monitorFlowPollRef = useRef<number | null>(null);
  const [rowMonitorName, setRowMonitorName] = useState<string | null>(null);
  const rowMonitorPollRef = useRef<number | null>(null);

  const reload = useCallback(() => {
    api
      .getAccounts()
      .then(setState)
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    if (open) reload();
  }, [open, reload]);

  // 定期更新ループ（tray.rs）が自動取り込みを行ったら "accounts-updated" が飛んでくる。
  // 開いている間だけ購読し、画面表示時にしか同期されなかった状態を解消する（2026-07-26）。
  // warning はライブ乗っ取り検知時の案内。旧・手動「セッション更新」ボタン押下時は
  // outcome.warning として見えていたが、自動化の過程で握り潰されていたため復元する
  // （2026-07-26 レビュー High-2b）
  useEffect(() => {
    if (!open) return;
    const unlisten = listen<AccountsUpdatedEvent>("accounts-updated", (e) => {
      if (e.payload.warning) setNotice(e.payload.warning);
      reload();
    });
    return () => {
      unlisten.then((un) => un());
    };
  }, [open, reload]);

  // 使用率は一覧表示をブロックしないよう別コマンドで取りに行く。取得できなくても
  // 一覧自体は成立するので、失敗は静かに無視する（行ごとの使用率表示が出ないだけ）
  useEffect(() => {
    if (!open) return;
    api
      .getAccountsUsage()
      .then((list) => {
        const map: Record<string, AccountUsage> = {};
        for (const u of list) map[u.name] = u;
        setUsageByName(map);
      })
      .catch(() => {});
  }, [open]);

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
  const clearMismatchRetry = useCallback(() => {
    if (mismatchRetryRef.current !== null) {
      window.clearTimeout(mismatchRetryRef.current);
      mismatchRetryRef.current = null;
    }
  }, []);
  const cancelLogin = useCallback(() => {
    stopLoginPolling();
    clearMismatchRetry();
    loginPhaseRef.current = "idle";
    setLoginPending(false);
  }, [stopLoginPolling, clearMismatchRetry]);

  const stopMonitorFlowPolling = useCallback(() => {
    if (monitorFlowPollRef.current !== null) {
      window.clearInterval(monitorFlowPollRef.current);
      monitorFlowPollRef.current = null;
    }
    setMonitorFlowName(null);
  }, []);
  const stopRowMonitorPolling = useCallback(() => {
    if (rowMonitorPollRef.current !== null) {
      window.clearInterval(rowMonitorPollRef.current);
      rowMonitorPollRef.current = null;
    }
    setRowMonitorName(null);
  }, []);

  // 閉じても止めないと、待ちのまま2秒おきに Keychain を叩き続け、
  // ユーザーが見ていないところで勝手に取り込み・監視の紐づけが進んでしまう
  useEffect(() => {
    if (!open) {
      cancelLogin();
      stopMonitorFlowPolling();
      stopRowMonitorPolling();
    }
  }, [open, cancelLogin, stopMonitorFlowPolling, stopRowMonitorPolling]);
  useEffect(() => stopLoginPolling, [stopLoginPolling]);
  useEffect(() => clearMismatchRetry, [clearMismatchRetry]);
  useEffect(() => stopMonitorFlowPolling, [stopMonitorFlowPolling]);
  useEffect(() => stopRowMonitorPolling, [stopRowMonitorPolling]);

  const switchBusy = busy || loginPending;

  /** setup-token の紐づけ完了をポーリングする（統合フローのステップ2・単独の「常時監視を設定」共通）。
   * onSettle は「紐づいた」「タイムアウト（スキップ扱い）」「照合不一致（別アカウントで承認された）」
   * のいずれでも呼ばれる。監視は完全に任意の後付け機能なので、タイムアウトを「失敗」として
   * busy 表示のまま固まらせず、必ず終わらせる */
  const pollMonitorLinking = (
    name: string,
    pollRef: { current: number | null },
    stop: () => void,
    onSettle: (
      result: { kind: "linked" } | { kind: "timeout" } | { kind: "mismatch"; label: string; email: string }
    ) => void
  ) => {
    const deadline = Date.now() + MONITOR_TIMEOUT_MS;
    pollRef.current = window.setInterval(() => {
      if (Date.now() > deadline) {
        stop();
        onSettle({ kind: "timeout" });
        return;
      }
      api
        .pollMonitorSetup(name)
        .then((result) => {
          if (result.status === "waiting") return;
          stop();
          if (result.status === "mismatch") {
            onSettle({ kind: "mismatch", label: result.expected_label, email: result.expected_email });
          } else {
            onSettle({ kind: "linked" });
          }
        })
        .catch(() => {
          stop();
          onSettle({ kind: "timeout" });
        });
    }, POLL_INTERVAL_MS);
  };

  /** 誤紐づけ（照合不一致）の案内文言。「常時監視を設定」導線の両方で共有する */
  const monitorMismatchMessage = (label: string, email: string) =>
    `承認されたアカウントが「${label}」と一致しません。ブラウザで ${email} にログインしてからやり直してください。`;

  /** 「＋アカウントを追加」統合フローのステップ2を開始する（Flow B 完了直後に呼ぶ） */
  const beginMonitorFlow = (name: string) => {
    setMonitorFlowName(name);
    pollMonitorLinking(name, monitorFlowPollRef, stopMonitorFlowPolling, (result) => {
      if (result.kind === "linked") {
        setNotice("使用量の常時監視を設定しました。");
      } else if (result.kind === "mismatch") {
        setError(monitorMismatchMessage(result.label, result.email));
      } else {
        setNotice("使用量監視の設定はスキップされました。後から「常時監視を設定」から追加できます。");
      }
      reload();
    });
  };

  /** ステップ2の「スキップ」ボタン。アカウント追加自体はすでに成功しているので、
   * ここでの中止は監視の設定を諦めるだけで、追加の成否には一切影響しない */
  const skipMonitorFlow = () => {
    stopMonitorFlowPolling();
    setNotice("使用量監視の設定をスキップしました。後から「常時監視を設定」から追加できます。");
  };

  /** 登録済みアカウント行の「常時監視を設定」単独フロー */
  const startRowMonitor = (name: string) => {
    setError(null);
    setRowMonitorName(name);
    api
      .startMonitorSetup(name)
      .then(() => {
        pollMonitorLinking(name, rowMonitorPollRef, stopRowMonitorPolling, (result) => {
          if (result.kind === "linked") {
            setNotice("使用量の常時監視を設定しました。");
          } else if (result.kind === "mismatch") {
            setError(monitorMismatchMessage(result.label, result.email));
          } else {
            setNotice("使用量監視の設定がタイムアウトしました。もう一度お試しください。");
          }
          reload();
        });
      })
      .catch((e) => {
        setRowMonitorName(null);
        setError(String(e));
      });
  };

  /** 再ログイン導線での誤紐づけ検知メッセージ。setup-token 側の monitorMismatchMessage とは
   * 文言を分ける（「承認された」ではなく「ログインした」。ブラウザ操作の主体が違うため）。
   * email が取れていない旧登録向けにフォールバック文言も用意する（2026-07-26 レビュー L-9） */
  const loginMismatchMessage = (label: string, email: string) =>
    email
      ? `ログインしたアカウントが「${label}」と一致しません。ブラウザで ${email} にログインしてからやり直してください。`
      : `ログインしたアカウントが「${label}」と一致しません。正しいアカウントでログインしてからやり直してください。`;

  /** 完了・成功時の通知＋後続処理（reload・常時監視ステップ2）をまとめる */
  const settleLoginDone = (account: Account, targetName?: string) => {
    setNotice(
      targetName
        ? `「${accountLabel(account)}」の再ログインが完了しました。`
        : `「${account.email || accountLabel(account)}」を取り込みました。本人のアカウントか確認してください。`
    );
    reload();
    // 統合フロー・ステップ2: 続けて使用量の常時監視（任意）を設定する。
    // ここでの成否はアカウント追加の成否に一切影響しない
    beginMonitorFlow(account.name);
  };

  /** targetName があれば「再ログイン」導線（組織ID/email 照合あり）のポーリング。
   *
   * mismatch は「取り込みが行われていない」だけで、ライブの Keychain / ~/.claude.json
   * 自体はすでに書き換わっている（誤ったアカウントで実際にログインしてしまっている）ため
   * reload() は必要（2026-07-26 レビュー L-8。旧コメント「何も変わっていない」は誤り）。
   *
   * Keychain（完了検知）と ~/.claude.json（照合）は別々に書き込まれるため、その隙間に
   * 2秒ポーリングが入ると正しいログインでも一時的に mismatch と判定されうる。即エラーに
   * せず、3秒後に1回だけ再確認し、それでも不一致のときだけエラー表示する
   * （2026-07-26 レビュー M-6b）。再確認が waiting のときは通常のインターバルポーリングへ
   * 戻し、5分デッドラインの管理下に留める（2026-07-26 レビュー M-A2）。
   * loginPhaseRef による二重実行防止・mismatchRetryRef の追跡は cancelLogin 側を参照
   * （2026-07-26 レビュー M-A1/M-A3） */
  const pollForCompletion = (baseline: string, targetName?: string) => {
    setLoginPending(true);
    loginPhaseRef.current = "polling";
    const deadline = Date.now() + LOGIN_TIMEOUT_MS;

    const startInterval = () => {
      loginPollRef.current = window.setInterval(() => {
        if (loginPhaseRef.current !== "polling") return;
        if (Date.now() > deadline) {
          cancelLogin();
          setError("ログインが5分以内に完了しなかったため中止しました。");
          return;
        }
        api
          .pollAddAccountLogin(baseline)
          .then((result) => {
            // すでに確定済み・キャンセル済み（別の in-flight 応答が先に処理した、
            // ユーザーが中止した、画面を閉じた等）なら、この応答は無視する
            if (loginPhaseRef.current !== "polling") return;
            if (result.status === "waiting") return;
            if (result.status === "mismatch") {
              loginPhaseRef.current = "retrying";
              handleMismatch();
              return;
            }
            cancelLogin();
            settleLoginDone(result.account, targetName);
          })
          .catch((e) => {
            if (loginPhaseRef.current !== "polling") return;
            cancelLogin();
            setError(String(e));
          });
      }, POLL_INTERVAL_MS);
    };

    // 最初の mismatch では確定させず、3秒後の再確認結果だけを見る。インターバルだけ止め、
    // loginPending は維持したまま再確認を待つ（ユーザーには引き続き「ログインを待っています」
    // の表示のままにする）
    const handleMismatch = () => {
      stopLoginPolling();
      mismatchRetryRef.current = window.setTimeout(() => {
        mismatchRetryRef.current = null;
        if (loginPhaseRef.current !== "retrying") return; // cancelLogin 済み
        api
          .pollAddAccountLogin(baseline)
          .then((retry) => {
            if (loginPhaseRef.current !== "retrying") return;
            if (retry.status === "waiting") {
              loginPhaseRef.current = "polling";
              startInterval();
              return;
            }
            if (retry.status === "mismatch") {
              cancelLogin();
              reload();
              setError(loginMismatchMessage(retry.expected_label, retry.expected_email));
              return;
            }
            cancelLogin();
            settleLoginDone(retry.account, targetName);
          })
          .catch((e) => {
            if (loginPhaseRef.current !== "retrying") return;
            cancelLogin();
            setError(String(e));
          });
      }, MISMATCH_RETRY_DELAY_MS);
    };

    startInterval();
  };

  /** targetName 省略時は「＋アカウントを追加」の汎用フロー、指定時は登録済みカードの
   * 「再ログイン」導線（ログイン結果の組織IDが targetName と一致しなければ取り込まない）。
   * force（セッション確認スキップ）と trustUnverified（持ち主未確認でも続行、issue #3）は
   * 独立した引数（major-2） */
  const startAddLogin = (targetName?: string, force = false, trustUnverified = false): Promise<void> => {
    setError(null);
    setNotice(null);
    setBusy(true);
    stopLoginPolling();
    return api
      .startAddAccountLogin(force, trustUnverified, targetName)
      .then((outcome) => {
        if (outcome.status === "needs_import") {
          setPendingConfirm({ kind: "login", liveEmail: outcome.live_email, targetName });
          return;
        }
        if (outcome.status === "sessions_running") {
          // 「今後表示しない」が有効なら確認を出さず自動で force=true 再試行する。
          // trustUnverified はここでは変えない（セッション確認の同意が持ち主未確認への
          // 同意を兼ねてはいけない）
          if (skipSessionsConfirmEnabled()) {
            return startAddLogin(targetName, true, trustUnverified);
          }
          setSessionsConfirm({ kind: "login", count: outcome.count, targetName, trust: trustUnverified });
          return;
        }
        setSessionsConfirm(null);
        if (outcome.warning) setNotice(outcome.warning);
        pollForCompletion(outcome.baseline, targetName);
      })
      .catch((e) => {
        // issue #3: 持ち主未確認（token 期限切れ／通信不能）は続行を選べる。
        // すでに trustUnverified=true で失敗しているときは再提案しない
        // （Other 等、trustUnverified でも解消しない別要因）
        if (!trustUnverified && isForceSwitchEligible(ownerErrorKind(e))) {
          setOwnerConfirm({ kind: "login", message: describeAccountError(e), targetName, force });
          return;
        }
        setError(describeAccountError(e));
      })
      .finally(() => setBusy(false));
  };

  /** 登録済みだが資格情報が使えないカード（has_credentials=false）の「再ログイン」導線 */
  const startRelogin = (name: string) => startAddLogin(name);

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

  /** force（セッション確認スキップ）と trustUnverified（持ち主未確認でも続行、issue #3）は
   * 独立した引数（major-2） */
  const doSwitch = (name: string, force = false, trustUnverified = false): Promise<void> => {
    setError(null);
    setNotice(null);
    setBusy(true);
    return api
      .switchAccount(name, force, trustUnverified)
      .then((outcome) => {
        if (outcome.status === "needs_import") {
          setPendingConfirm({ kind: "switch", name, liveEmail: outcome.live_email });
          return;
        }
        if (outcome.status === "sessions_running") {
          // 「今後表示しない」が有効なら確認を出さず自動で force=true 再試行する。
          // trustUnverified はここでは変えない（セッション確認の同意が持ち主未確認への
          // 同意を兼ねてはいけない）
          if (skipSessionsConfirmEnabled()) {
            return doSwitch(name, true, trustUnverified);
          }
          setSessionsConfirm({ kind: "switch", name, count: outcome.count, trust: trustUnverified });
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
      .catch((e) => {
        // issue #3: 持ち主未確認（token 期限切れ／通信不能）は続行を選べる。
        // すでに trustUnverified=true で失敗しているときは再提案しない
        // （Other 等、trustUnverified でも解消しない別要因）
        if (!trustUnverified && isForceSwitchEligible(ownerErrorKind(e))) {
          setOwnerConfirm({ kind: "switch", name, message: describeAccountError(e), force });
          return;
        }
        setError(describeAccountError(e));
      })
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
          startAddLogin(confirm.targetName);
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
    // confirm.trust を引き継ぐ（issue #3 再レビュー: owner確認→セッション確認と進んだ場合、
    // ここで trust を落とすと再度 owner確認に戻ってしまう二度手間になる）
    if (confirm.kind === "switch") {
      doSwitch(confirm.name, true, confirm.trust);
    } else {
      startAddLogin(confirm.targetName, true, confirm.trust);
    }
  };

  /** issue #3: 「持ち主を確認できないが続行する」を選んだら trustUnverified=true で
   * 再実行する。force はこの確認が発生した時点の値をそのまま持ち回る（major-2:
   * セッション確認への同意が持ち主未確認への同意を兼ねてはいけないため、force は変えない） */
  const confirmOwnerAndContinue = () => {
    if (!ownerConfirm) return;
    const confirm = ownerConfirm;
    setOwnerConfirm(null);
    if (confirm.kind === "switch") {
      doSwitch(confirm.name, confirm.force, true);
    } else {
      startAddLogin(confirm.targetName, confirm.force, true);
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

  const handleDragStart = (event: DragStartEvent) => {
    setActiveDragName(String(event.active.id));
  };

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    setActiveDragName(null);
    if (!over || active.id === over.id) return;
    const oldIndex = displayAccounts.findIndex((a) => a.name === active.id);
    const newIndex = displayAccounts.findIndex((a) => a.name === over.id);
    if (oldIndex === -1 || newIndex === -1) return;
    const next = arrayMove(displayAccounts, oldIndex, newIndex);
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
  // 確認ダイアログは対象アカウントの内部識別子(name)しか持たないため、表示名を引き直す
  const labelFor = (name: string) => {
    const found = accounts.find((a) => a.name === name);
    return found ? accountLabel(found) : name;
  };
  // 名前編集中・busy中・確認ダイアログ表示中はドラッグ操作を無効にする
  const dragDisabled =
    switchBusy ||
    busy ||
    editingName !== null ||
    pendingConfirm !== null ||
    sessionsConfirm !== null ||
    ownerConfirm !== null;
  const activeDragAccount = displayAccounts.find((a) => a.name === activeDragName) ?? null;

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

          {hasLegacyAccounts && (
            <p className="muted">
              「未取り込み」のアカウントは保存済みのログイン情報がありません。行の「再ログイン」からブラウザでログインすると資格情報を取り込み直せます（別アカウントでログインした場合は誤って紐づけず、エラーになります）。組織ID・メールアドレスのどちらも登録が無い旧アカウントは照合できないため「再ログイン」は使えません。別アカウントとして新規に登録したい場合は「＋
              アカウントを追加」から行ってください。
            </p>
          )}

          {/* 登録済み・ライブ一致・整合済みの場合は60秒ごとの自動更新（tray.rs）が資格情報を
              最新化するため、ここでの手動操作は不要（旧「セッション更新」ボタンは廃止・
              自動化した。2026-07-26）。未登録のライブセッション、または直前のスワップが
              中途半端な状態のまま（inconsistent）のときだけ、一覧の先頭に取り込み導線を出す。
              inconsistent は live_registered=true でも起こりうる（switch_account のロール
              バック失敗時等）ため、live_registered だけで出し分けると詰む（2026-07-26 レビュー
              High-1） */}
          {state && state.live_email && (!state.live_registered || state.inconsistent) && (
            <div className="acct-banner">
              <div>
                <strong>現在のログイン: {state.live_email}</strong>
                <p className="muted">
                  {state.inconsistent
                    ? "直前の切り替えが中途半端な状態のままです。取り込むと解消し、以後の切り替え・追加・再ログインができるようになります。"
                    : "このアカウントはまだ登録されていません。取り込むと保存され、切り替え先として選べるようになります。"}
                </p>
              </div>
              <button className="acct-btn acct-btn-primary" disabled={busy} onClick={() => importLive()}>
                このセッションを取り込む
              </button>
            </div>
          )}

          {accounts.length === 0 && !loginPending && (
            <p className="muted">
              まだアカウントが登録されていません。「アカウントを追加」からブラウザでログインしてください。
            </p>
          )}

          <DndContext
            sensors={sensors}
            collisionDetection={closestCenter}
            modifiers={[restrictToVerticalAxis, restrictToParentElement]}
            onDragStart={handleDragStart}
            onDragEnd={handleDragEnd}
            onDragCancel={() => setActiveDragName(null)}
          >
            <SortableContext
              items={displayAccounts.map((a) => a.name)}
              strategy={verticalListSortingStrategy}
            >
              <ul className="acct-list">
                {displayAccounts.map((a) => (
                  <AccountRow
                    key={a.name}
                    account={a}
                    usage={usageByName[a.name]}
                    dragDisabled={dragDisabled}
                    editing={editingName === a.name}
                    editingValue={editingValue}
                    onEditingValueChange={setEditingValue}
                    onStartEditing={() => startEditingName(a)}
                    onCommitRename={() => commitRename(a.name)}
                    onCancelEditingViaEscape={() => {
                      cancelingEditRef.current = true;
                    }}
                    confirmingRemove={confirmRemove === a.name}
                    onRequestRemove={() => setConfirmRemove(a.name)}
                    onCancelRemove={() => setConfirmRemove(null)}
                    onConfirmRemove={() => remove(a.name)}
                    onSwitch={() => doSwitch(a.name)}
                    onRelogin={() => startRelogin(a.name)}
                    switchButtonDisabled={switchBusy}
                    busy={busy}
                    monitorBusy={rowMonitorName === a.name || monitorFlowName === a.name}
                    onStartMonitor={() => startRowMonitor(a.name)}
                  />
                ))}
              </ul>
            </SortableContext>
            <DragOverlay>
              {activeDragAccount ? <AccountRowPreview account={activeDragAccount} /> : null}
            </DragOverlay>
          </DndContext>

          {loginPending ? (
            <div className="acct-pending">
              <span className="diag-spinner" />
              <div>
                <strong>1/2 ブラウザでログインを待っています</strong>
                <p className="muted">
                  Terminal でログインを完了してください。完了すると自動で取り込みます。
                </p>
              </div>
              <button className="acct-btn acct-btn-ghost" onClick={cancelLogin}>
                中止
              </button>
            </div>
          ) : monitorFlowName ? (
            <div className="acct-pending">
              <span className="diag-spinner" />
              <div>
                <strong>2/2 使用量監視の承認を待っています（任意）</strong>
                <p className="muted">
                  Terminal でブラウザ承認を完了してください。不要ならスキップしても、
                  アカウントの追加・切り替えには影響しません。
                </p>
              </div>
              <button className="acct-btn acct-btn-ghost" onClick={skipMonitorFlow}>
                スキップ
              </button>
            </div>
          ) : (
            <button
              className="acct-btn acct-btn-primary"
              disabled={switchBusy}
              onClick={() => startAddLogin(undefined)}
            >
              ＋ アカウントを追加（ブラウザでログイン）
            </button>
          )}

        </div>

        {(pendingConfirm || sessionsConfirm || ownerConfirm) && (
          <div
            className="acct-modal-overlay"
            onClick={(e) => {
              e.stopPropagation();
              setPendingConfirm(null);
              setSessionsConfirm(null);
              setOwnerConfirm(null);
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
              ) : ownerConfirm ? (
                <>
                  <strong>
                    {ownerConfirm.kind === "switch"
                      ? `「${labelFor(ownerConfirm.name)}」に切り替えます`
                      : "ブラウザでログインします"}
                  </strong>
                  <p className="muted">
                    持ち主を確認できませんが切り替えは可能です。直前のセッションのログイン情報は今回同期されません。元のアカウントに戻す際、再ログインが必要になる場合があります。続行しますか？
                  </p>
                  <p className="muted acct-owner-confirm-detail">{ownerConfirm.message}</p>
                  <div className="acct-modal-actions">
                    <button className="acct-btn acct-btn-ghost" onClick={() => setOwnerConfirm(null)}>
                      やめる
                    </button>
                    <button
                      className="acct-btn acct-btn-primary"
                      disabled={busy}
                      onClick={confirmOwnerAndContinue}
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
