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
  name: string;
  email: string;
  plan: string;
  active: boolean;
  /** Claude Code が現在 /login しているアカウント（起動中セッションが消費する先） */
  is_live: boolean;
  /** Keychain 側のトークンが失われている（手動削除・1年の期限切れ等）。切り替えできない */
  missing_token: boolean;
}

export interface AccountsState {
  accounts: Account[];
  /** .zshrc に読み込み行が入っているか。無いと切り替えても新しいシェルに効かない */
  shell_integration: boolean;
  /** 起動中の claude CLI セッション数。切り替えの反映には再起動が要る */
  running_sessions: number;
}

export interface AccountUsageDetail {
  name: string;
  email: string;
  plan: string;
  active: boolean;
  is_live: boolean;
  usage: RateLimits | null;
  error: string | null;
}

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
  getAccountsUsage: () => invoke<AccountUsageDetail[]>("get_accounts_usage"),
  addAccountInTerminal: (name: string) =>
    invoke<void>("add_account_in_terminal", { name }),
  claimPendingAccount: (name: string) =>
    invoke<Account | null>("claim_pending_account", { name }),
  switchAccount: (name: string) => invoke<void>("switch_account", { name }),
  removeAccount: (name: string) => invoke<void>("remove_account", { name }),
  installShellIntegration: () => invoke<void>("install_shell_integration"),
  getRateLimits: async (): Promise<RateLimits> => {
    const raw = await invoke<string>("get_rate_limits");
    return JSON.parse(raw) as RateLimits;
  },
  getAccountProfile: async (): Promise<AccountProfile> => {
    const raw = await invoke<string>("get_account_profile");
    return JSON.parse(raw) as AccountProfile;
  },
};

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
