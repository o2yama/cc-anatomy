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
  /** 使用量の常時監視用に claude setup-token の長期トークンが紐づいているか（任意機能）。
   * 切り替え機能とは完全に独立で、これが無くても切り替え・使用量取得は成立する */
  has_monitor_token: boolean;
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
}

/** Flow B: claude auth login の完了検知ポーリング結果 */
export type PollResult =
  | { status: "waiting" }
  | { status: "done"; account: Account };

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

export const api = {
  listProjects: () => invoke<ProjectInfo[]>("list_projects"),
  getHomeDir: () => invoke<string>("get_home_dir"),
  getProjectEnv: (project: string, path: string | null) =>
    invoke<ProjectEnv>("get_project_env", { project, path }),
  readDoc: (path: string) => invoke<FileDoc>("read_doc", { path }),
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
  getAccounts: () => invoke<AccountsState>("get_accounts"),
  importLiveAccount: () => invoke<Account>("import_live_account"),
  startAddAccountLogin: (force = false) =>
    invoke<StartLoginOutcome>("start_add_account_login", { force }),
  pollAddAccountLogin: (baseline: string) =>
    invoke<PollResult>("poll_add_account_login", { baseline }),
  switchAccount: (name: string, force = false) =>
    invoke<SwitchOutcome>("switch_account", { name, force }),
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
  getRateLimits: () => unwrapUsageFetch<RateLimits>("get_rate_limits"),
  getAccountProfile: () => unwrapUsageFetch<AccountProfile>("get_account_profile"),
};

/** get_rate_limits/get_account_profile の生の戻り値（Rust の UsageFetch と同じ形）。
 * "expired" は access token の期限切れ（Claude Code 側の自動 refresh 待ちの正常な状態）で、
 * エラーではない */
type UsageFetchRaw =
  | { status: "ok"; body: string }
  | { status: "expired" }
  | { status: "error"; message: string };

/** ライブ access token が期限切れであることを示す（2026-07-26 追加）。
 * 「再ログインが必要かもしれません」という誤誘導なエラー文言を廃止し、代わりにこの型で
 * 呼び出し側が「エラーではなく待ち状態」と判別できるようにする。
 * 呼び出し側は catch した値が `instanceof UsageExpiredError` かで分岐し、
 * エラーボックスではなく薄字の案内（例:「最新の使用量は Claude Code を一度使うと取得できます」）
 * を出すこと */
export class UsageExpiredError extends Error {
  constructor() {
    super("ライブの access token が期限切れです（Claude Code を一度使うと更新されます）");
    this.name = "UsageExpiredError";
  }
}

/** get_rate_limits/get_account_profile 共通の unwrap。呼び出し側の型（Promise<RateLimits> /
 * Promise<AccountProfile>）は変えず、期限切れだけ UsageExpiredError として投げ分ける */
async function unwrapUsageFetch<T>(command: string): Promise<T> {
  const raw = await invoke<UsageFetchRaw>(command);
  if (raw.status === "ok") return JSON.parse(raw.body) as T;
  if (raw.status === "expired") throw new UsageExpiredError();
  throw new Error(raw.message);
}

export interface RateLimitWindow {
  utilization: number | null;
  resets_at: string | null;
}

/** usage API の limits 配列の1エントリ（モデル別枠は scope.model に入る） */
export interface LimitEntry {
  kind: string;
  group: string | null;
  percent: number | null;
  severity: string | null;
  resets_at: string | null;
  scope: { model?: { display_name?: string | null } | null } | null;
  is_active: boolean;
}

export interface RateLimits {
  five_hour: RateLimitWindow | null;
  seven_day: RateLimitWindow | null;
  limits?: LimitEntry[];
  extra_usage?: {
    is_enabled: boolean;
    monthly_limit: number | null;
    used_credits: number | null;
    utilization: number | null;
  } | null;
  spend?: {
    used: { amount_minor: number; currency: string; exponent: number } | null;
    limit: unknown;
    percent: number | null;
    enabled: boolean;
  } | null;
}

export interface AccountProfile {
  account?: {
    full_name?: string | null;
    display_name?: string | null;
    email?: string | null;
    has_claude_max?: boolean;
    has_claude_pro?: boolean;
  } | null;
  organization?: {
    name?: string | null;
    organization_type?: string | null;
    rate_limit_tier?: string | null;
    subscription_status?: string | null;
  } | null;
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
