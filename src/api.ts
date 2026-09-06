import { invoke } from "@tauri-apps/api/core";

export interface ProjectInfo {
  project: string;
  path: string | null;
  session_count: number;
  summary_count: number;
  last_activity_epoch: number;
  last_request: string | null;
}

export interface SummaryEntry {
  request: string | null;
  investigated: string | null;
  learned: string | null;
  completed: string | null;
  next_steps: string | null;
  files_edited: string | null;
  created_at_epoch: number;
}

export interface SessionInfo {
  content_session_id: string;
  user_prompt: string | null;
  started_at_epoch: number;
  status: string;
  summaries: SummaryEntry[];
}

export interface SearchHit {
  project: string;
  content_session_id: string | null;
  request: string | null;
  completed: string | null;
  created_at_epoch: number;
}

export interface TranscriptMessage {
  role: string;
  text: string;
  timestamp: string | null;
}

export interface Transcript {
  session_id: string;
  cwd: string | null;
  messages: TranscriptMessage[];
  truncated: boolean;
}

export interface InventoryItem {
  name: string;
  description: string;
  path: string;
  modified_epoch: number;
}

export interface FileDoc {
  path: string;
  content: string;
  truncated: boolean;
  modified_epoch: number;
}

export interface ScopedItem {
  name: string;
  description: string;
  path: string;
  scope: "project" | "global";
}

export interface McpServer {
  name: string;
  scope: "project" | "global";
  source: string;
  config: string;
}

export interface HookInfo {
  event: string;
  matcher_count: number;
  scope: "project" | "global";
  config: string;
}

export interface RuleFile {
  name: string;
  path: string;
}

export interface ObservationItem {
  id: number;
  title: string | null;
  subtitle: string | null;
  type: string;
  narrative: string | null;
  facts: string[];
  files_modified: string[];
  created_at_epoch: number;
}

export interface ProjectEnv {
  path: string | null;
  has_claude_mem: boolean;
  claude_mds: FileDoc[];
  memory_md: FileDoc | null;
  memory_files: RuleFile[];
  observations: ObservationItem[];
  next_steps: string | null;
  mcp_servers: McpServer[];
  agents: ScopedItem[];
  skills: ScopedItem[];
  commands: ScopedItem[];
  hooks: HookInfo[];
  rules: RuleFile[];
}

export interface DiagnosisFinding {
  id: string;
  severity: "high" | "medium" | "low";
  category: string;
  title: string;
  detail: string;
  fix_prompt: string;
  target_paths: string[];
}

export interface DiagnosisReport {
  summary: string;
  findings: DiagnosisFinding[];
}

/** 実行中は "diagnosis-progress" イベント（{kind, label}）が随時 emit される */
export interface DiagnosisProgress {
  kind: "tool" | "text" | "info";
  label: string;
}

/** "doc-analysis-progress" イベント。diagnosis-progress と同型。
 * Rust 側（doc_analysis.rs の emit_progress）が実際に emit するのは "tool" と "text" のみ */
export interface DocAnalysisProgress {
  kind: "tool" | "text";
  label: string;
}

export interface Account {
  /** 内部識別子。Keychain サービス名・照合キーに使うため不変。表示には accountLabel() を使うこと */
  name: string;
  /** ユーザーが自由に付けられる表示名。null なら name をそのまま表示する */
  display_name: string | null;
  email: string;
  plan: string;
  /** Claude Code が現在 /login しているアカウント（起動中セッションが消費する先。＝選択中） */
  is_live: boolean;
  /** ライブ資格情報のスナップショットが登録済みか。無いと「切り替え」できない */
  has_credentials: boolean;
  /** スナップショット内の refresh token 有効期限（epoch ミリ秒）。取得不能なら null */
  refresh_token_expires_at: number | null;
  /** 使用量の常時監視用に claude setup-token の長期トークンが紐づいているか（任意機能）。
   * 切り替え機能とは完全に独立で、これが無くても切り替え・使用量取得は成立する */
  has_monitor_token: boolean;
  /** 「再ログイン」導線が使えるか（org_id か email のどちらかが登録されているか）。
   * false の旧登録は照合しようがなく、再ログインを開始しても拒否される */
  can_relogin: boolean;
}

/** 表示名のフォールバック規則。Rust 側の resolve_display_name と同じ規則
 * （display_name があればそれ、無ければ内部識別子 name） */
export function accountLabel(a: Pick<Account, "name" | "display_name">): string {
  return a.display_name?.trim() || a.name;
}

export interface AccountsState {
  accounts: Account[];
  /** 現在 PC にログイン中のアカウントの email（取得できなければ null） */
  live_email: string | null;
  /** 現在のログインがすでにどれかのアカウントとして登録済みか */
  live_registered: boolean;
  /** 起動中の claude CLI セッション数。切り替えの反映には再起動が要る */
  running_sessions: number;
  /** 直前のスワップが中途半端な状態のまま残っている。true の間は切り替え・追加・
   * 再ログインがすべてエラーになり、「このセッションを取り込む」でしか解消できない。
   * live_registered の値に関わらず起こりうる */
  inconsistent: boolean;
}

/** "accounts-updated" イベント（tray.rs の定期更新ループから emit）のペイロード。
 * warning はライブ乗っ取り検知時の案内（ある場合だけ表示する） */
export interface AccountsUpdatedEvent {
  warning: string | null;
}

/** アカウント1件分の使用率。監視用長期トークンは復活させず、保存済みスナップショットの
 * access token（期限内のときだけ）で /api/oauth/usage を照会した結果。
 * stale=true はキャッシュ返し（今回は新規取得できなかった）を示す */
export interface AccountUsage {
  name: string;
  five_pct: number | null;
  seven_pct: number | null;
  five_reset: number | null;
  seven_reset: number | null;
  /** 取得時刻（epoch 秒）。cache が無ければ null */
  fetched_at: number | null;
  stale: boolean;
  /** 5h 枠のリセット時刻を過ぎている想定（実質 0% とみなせる） */
  five_probably_reset: boolean;
  /** スナップショット内の refresh token 有効期限（epoch ミリ秒）。使用率取得の成否とは
   * 無関係に読めた値をそのまま返す。取得不能なら null */
  refresh_token_expires_at: number | null;
}

/** Flow B: claude auth login の完了検知ポーリング結果。
 * mismatch は「再ログイン」導線（target 指定あり）で、ログイン結果の組織IDが対象
 * アカウントと一致しなかった場合。誤紐づけを避けるため取り込みは行われていない */
export type PollResult =
  | { status: "waiting" }
  | { status: "done"; account: Account }
  | { status: "mismatch"; expected_label: string; expected_email: string };

/** Flow C: Keychain スワップ切り替えの結果 */
export type SwitchOutcome =
  | { status: "switched"; warning: string | null }
  | { status: "needs_import"; live_email: string | null }
  | { status: "sessions_running"; count: number };

/** Flow B 開始（claude auth login を Terminal で起動）の結果。事前 sync-back を含む */
export type StartLoginOutcome =
  | { status: "started"; baseline: string; warning: string | null }
  | { status: "needs_import"; live_email: string | null }
  | { status: "sessions_running"; count: number };

/** 使用量の常時監視（claude setup-token、任意機能）の紐づけ完了検知ポーリング結果。
 * 「＋アカウントを追加」のステップ2、または既存アカウントの「常時監視を設定」の両方で使う。
 * mismatch はブラウザ側が期待していたアカウントと別アカウントで承認していた場合
 * （org_id 照合の不一致）。トークンは紐づけ済みではなく破棄されている */
export type MonitorSetupPoll =
  | { status: "waiting" }
  | { status: "linked" }
  | { status: "mismatch"; expected_label: string; expected_email: string };

/** switch_account / start_add_account_login の事前 sync-back（持ち主確認）失敗時、
 * Rust 側（accounts.rs の OwnerError）がメッセージ先頭へ埋め込む機械可読プレフィックス
 * （wire format の契約は docs/dev-log.md 参照）。それ以外の（分類対象外の）エラー
 * メッセージには付かない。Rust 側の `strip_owner_error_tag`（tray.rs 用、コマンド境界を
 * 越えない経路向け）と対になる一覧なので、増減したら両方揃えること。
 * YAGNI: OWNER_MISMATCH は resolve_live_owner が実際には送出しないため定義しない
 * （2026-08-08 レビューで一度追加したが未使用のため撤去） */
const OWNER_ERROR_PREFIXES = ["TOKEN_EXPIRED:", "NETWORK_ERROR:", "OTHER:"] as const;
export type OwnerErrorKind = "TOKEN_EXPIRED" | "NETWORK_ERROR" | "OTHER";

/** catch で受け取った失敗（unknown）が OwnerError 由来ならその種別を返す（分類できない
 * エラーは null）。「持ち主未確認でも続行を選べる導線を出してよいか」の判定に使う
 * （2026-08-08 issue #3、レビュー案A: TokenExpired/NetworkError は「今は確認できないだけ」で
 * 続行時は sync-back を書き込まずスキップするだけなので安全
 * [accounts.rs::SyncBack::SkippedUnverified 参照]。Other は応答の構文エラー等、
 * 真に予期しない失敗のため対象外） */
export function ownerErrorKind(e: unknown): OwnerErrorKind | null {
  const raw = String(e);
  for (const prefix of OWNER_ERROR_PREFIXES) {
    if (raw.startsWith(prefix)) {
      return prefix.slice(0, -1) as OwnerErrorKind;
    }
  }
  return null;
}

/** switchAccount / startAddAccountLogin の catch で受け取った失敗（unknown）を表示用文言に
 * 変換する。回復手段の文言自体は Rust 側（OwnerError::message）が持つので、ここでは
 * 既知のプレフィックスを剥がして本文だけを返すだけでよい（parseAlreadyRunning と同じ
 * startsWith 方式）。プレフィックスが無い、または本文が空なら raw をそのまま返す */
export function describeAccountError(e: unknown): string {
  const raw = String(e);
  for (const prefix of OWNER_ERROR_PREFIXES) {
    if (raw.startsWith(prefix)) {
      const message = raw.slice(prefix.length);
      return message || raw;
    }
  }
  return raw;
}

/** ownerErrorKind が TOKEN_EXPIRED/NETWORK_ERROR のとき、「持ち主を確認できませんが
 * 切り替え自体は可能です」の force 続行導線を出してよいか（issue #3 スコープ）。
 * OTHER・null（分類対象外のエラー）は対象外 */
export const isForceSwitchEligible = (kind: OwnerErrorKind | null): boolean =>
  kind === "TOKEN_EXPIRED" || kind === "NETWORK_ERROR";

export const api = {
  listProjects: () => invoke<ProjectInfo[]>("list_projects"),
  getHomeDir: () => invoke<string>("get_home_dir"),
  getProjectEnv: (project: string, path: string | null) =>
    invoke<ProjectEnv>("get_project_env", { project, path }),
  readDoc: (path: string) => invoke<FileDoc>("read_doc", { path }),
  writeDoc: (path: string, content: string, expectedModifiedEpoch: number | null) =>
    invoke<FileDoc>("write_doc", {
      path,
      content,
      expectedModifiedEpoch,
    }),
  listSessions: (project: string) =>
    invoke<SessionInfo[]>("list_sessions", { project }),
  searchSummaries: (query: string, project?: string) =>
    invoke<SearchHit[]>("search_summaries", { query, project: project ?? null }),
  getTranscript: (sessionId: string) =>
    invoke<Transcript>("get_transcript", { sessionId }),
  listSkills: () => invoke<InventoryItem[]>("list_skills"),
  listAgents: () => invoke<InventoryItem[]>("list_agents"),
  /** "macos" | "windows" | "linux"（Rust の std::env::consts::OS） */
  getPlatform: () => invoke<string>("get_platform"),
  openInFinder: (path: string) => invoke<void>("open_in_finder", { path }),
  openInCmux: (path: string) => invoke<void>("open_in_cmux", { path }),
  openInTerminal: (path: string) => invoke<void>("open_in_terminal", { path }),
  extractTasks: (project: string) =>
    invoke<string>("extract_tasks", { project }),
  runDiagnosis: () => invoke<DiagnosisReport>("run_diagnosis"),
  cancelDiagnosis: () => invoke<void>("cancel_diagnosis"),
  runFixesInTerminal: (prompts: string[]) =>
    invoke<void>("run_fixes_in_terminal", { prompts }),
  analyzeDoc: (path: string, content: string, projectDir: string | null) =>
    invoke<string>("analyze_doc", { path, content, projectDir }),
  /** キャンセル可否に関わらず fire-and-forget で呼ばれることがある（unmount 時等）ので
   * 呼び出し側は catch を省略しないこと。実行中の分析が無ければ Rust 側がエラーを返す */
  cancelDocAnalysis: () => invoke<void>("cancel_doc_analysis"),
  getAccounts: () => invoke<AccountsState>("get_accounts"),
  importLiveAccount: () => invoke<Account>("import_live_account"),
  /** target が指定された場合は登録済みカードの「再ログイン」導線。ログイン結果の組織IDが
   * target と一致しなければ取り込まず mismatch を返す（誤紐づけ防止）。省略時は従来どおり
   * 「＋アカウントを追加」の汎用フロー（対象を問わず取り込む）。
   * `force`（外部セッション確認スキップ）と `trustUnverified`（2026-08-08 issue #3:
   * 持ち主未確認でも sync-back をスキップして続行する同意）は独立した引数。
   * セッション確認への同意（force）が持ち主未確認への同意を兼ねてはいけないため、
   * 呼び出し側で混同しないこと（Accounts.tsx/App.tsx の sessionsConfirm/ownerConfirm 参照） */
  startAddAccountLogin: (force = false, trustUnverified = false, targetName?: string) =>
    invoke<StartLoginOutcome>("start_add_account_login", {
      force,
      trustUnverified,
      targetName: targetName ?? null,
    }),
  pollAddAccountLogin: (baseline: string) =>
    invoke<PollResult>("poll_add_account_login", { baseline }),
  /** クライアント側のタイムアウト・画面クローズ等、pollAddAccountLogin を呼ばずに
   * ログイン待ちを打ち切るときに、トレイと共有の進行中フラグを明示的に解放する
   * （2026-09-06 レビュー M-1）。このウィンドウが取得していなくても no-op で安全 */
  releaseLoginLock: () => invoke<void>("release_login_lock"),
  /** force/trustUnverified の意味は startAddAccountLogin と同じ（独立した引数） */
  switchAccount: (name: string, force = false, trustUnverified = false) =>
    invoke<SwitchOutcome>("switch_account", { name, force, trustUnverified }),
  removeAccount: (name: string) => invoke<void>("remove_account", { name }),
  renameAccount: (name: string, displayName: string) =>
    invoke<void>("rename_account", { name, displayName }),
  reorderAccounts: (names: string[]) =>
    invoke<void>("reorder_accounts", { names }),
  /** 登録済み全アカウントの使用率一括取得。get_accounts とは別コマンドで、
   * 一覧表示をブロックせずモーダルを開いた後に非同期で埋める想定 */
  getAccountsUsage: () => invoke<AccountUsage[]>("get_accounts_usage"),
  /** 登録済みアカウントへ使用量の常時監視（claude setup-token、任意機能）を設定する。
   * Terminal を開いて setup-token を実行するだけで、完了は pollMonitorSetup で検知する */
  startMonitorSetup: (name: string) => invoke<void>("start_monitor_setup", { name }),
  pollMonitorSetup: (name: string) => invoke<MonitorSetupPoll>("poll_monitor_setup", { name }),
  /** 右上の使用量ポップオーバー用。トレイと同じ土台（tray::fetch_raw_status）から
   * 組み立てられるため、数値・フォーマットはトレイのメニュー表示と一致する */
  getUsageOverview: () => invoke<UsageOverview>("get_usage_overview"),
};

/** アプリ内使用量ポップオーバー（トレイと共有する `tray::UsageOverview`）向け。
 * ライブアカウントの使用率。トレイの `LiveUsage` と同じ形 */
export interface LiveUsage {
  five_pct: number;
  seven_pct: number;
  five_reset: number | null;
  seven_reset: number | null;
}

/** その他アカウント1件分（トレイの `OtherAccountEntry` と同じ形）。
 * usage が null、または usage.five_pct が null なら「未取得」表示にする */
export interface OtherAccountOverview {
  name: string;
  display_name: string;
  has_credentials: boolean;
  usage: AccountUsage | null;
}

/** get_usage_overview の戻り値。live が取れなかったとき、live_error に原因（token 期限切れ／
 * 通信不能／その他）に応じて文言が変わる2行を改行区切りで持つ（固定文言ではない。
 * tray.rs::usage_advisory 参照）。live が取れていても、その値の取得時刻が古い
 * （USAGE_STALE_NOTE_SECS 超）ときは live_note に取得時刻の注記が入る（live_error とは排他）。
 * 2026-08-22（S-3）: live_note は原因が Expired のときだけ復帰案内の行が付き、
 * 改行区切りで2行になることがある */
export interface UsageOverview {
  live_name: string | null;
  /** live_name の内部識別子（startAddAccountLogin に渡すキー）。ライブが未登録なら null */
  live_internal_name: string | null;
  live: LiveUsage | null;
  live_error: string | null;
  live_note: string | null;
  /** ライブアカウント自身のスナップショットの refresh token 有効期限（epoch ミリ秒）。
   * 取得できない/未登録なら null。`refreshExpiryDisplay` と組み合わせて期限警告を出す */
  live_refresh_token_expires_at: number | null;
  others: OtherAccountOverview[];
}

/** analyzeDoc が「すでに実行中」で失敗したかどうかの判定。文字列マッチではなく
 * Rust 側が付与する "ALREADY_RUNNING:" プレフィックスで判定し、表示用にはプレフィックスを
 * 剥がしたメッセージを返す（doc_analysis.rs 参照） */
export function parseAlreadyRunning(e: unknown): { alreadyRunning: boolean; message: string } {
  const raw = String(e);
  const prefix = "ALREADY_RUNNING:";
  if (raw.startsWith(prefix)) {
    return { alreadyRunning: true, message: raw.slice(prefix.length) };
  }
  return { alreadyRunning: false, message: raw };
}

export function formatEpoch(epochMs: number): string {
  if (!epochMs) return "-";
  // claude-mem は ms 単位の epoch、ファイル mtime は秒単位が来るため桁で判定
  const ms = epochMs < 1e12 ? epochMs * 1000 : epochMs;
  const d = new Date(ms);
  return d.toLocaleString("ja-JP", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function relativeTime(epochMs: number): string {
  if (!epochMs) return "-";
  const ms = epochMs < 1e12 ? epochMs * 1000 : epochMs;
  const diff = Date.now() - ms;
  const min = Math.floor(diff / 60000);
  if (min < 60) return `${min}分前`;
  const hour = Math.floor(min / 60);
  if (hour < 24) return `${hour}時間前`;
  const day = Math.floor(hour / 24);
  if (day < 30) return `${day}日前`;
  const month = Math.floor(day / 30);
  if (month < 12) return `${month}ヶ月前`;
  return `${Math.floor(month / 12)}年前`;
}
