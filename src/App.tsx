import { useCallback, useEffect, useMemo, useState } from "react";
import {
  api,
  formatEpoch,
  AccountProfile,
  LimitEntry,
  ProjectInfo,
  RateLimits,
  SearchHit,
  SessionInfo,
  Transcript,
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
import { AccountsOverlay } from "./Accounts";
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

/** limits 配列の kind をユーザー向けラベルに変換 */
function limitLabel(l: LimitEntry): string {
  const model = l.scope?.model?.display_name;
  if (l.kind === "session") return "セッション（5時間枠）";
  if (l.kind === "weekly_all") return "週間（全体）";
  if (l.kind === "weekly_scoped") return model ? `週間（${model}）` : "週間（モデル別）";
  return model ? `${l.kind}（${model}）` : l.kind;
}

/** "default_claude_max_20x" → "Max 20x" のようにプラン名に整形 */
function planLabel(p: AccountProfile): string {
  const tier = p.organization?.rate_limit_tier ?? "";
  const m = tier.match(/claude_(\w+?)_(\d+x)/);
  if (m) return `${m[1][0].toUpperCase()}${m[1].slice(1)} ${m[2]}`;
  if (p.account?.has_claude_max) return "Max";
  if (p.account?.has_claude_pro) return "Pro";
  return p.organization?.organization_type ?? "不明";
}

function formatReset(iso: string | null): string {
  if (!iso) return "";
  return new Date(iso).toLocaleString("ja-JP", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** カルーセルの1枚に渡す統一形（登録アカウント / ライブログインの両方をこの形に寄せる） */
interface UsageCard {
  name: string;
  email: string;
  planLabel: string;
  isLive: boolean;
  usage: RateLimits | null;
  error: string | null;
}

function planLabelRaw(plan: string): string {
  if (plan === "claude_max") return "Max";
  if (plan === "claude_pro") return "Pro";
  return plan || "不明";
}

/** 1アカウント分の使用量（枠ごとのゲージ）を描画する */
function UsageCardView({ card }: { card: UsageCard }) {
  if (card.error) return <p className="error-box">{card.error}</p>;
  if (!card.usage) return <p className="muted">取得中…</p>;
  const limits = card.usage.limits ?? [];
  return (
    <>
      <div className="usage-account">
        <p className="usage-name">
          {card.name}
          {card.isLive && <span className="acct-live">ログイン中</span>}
        </p>
        <p className="muted">{card.email}</p>
        <p className="usage-plan">
          <span className="count-badge">{card.planLabel}</span>
        </p>
      </div>
      <hr />
      <div className="usage-limits">
        {limits.map((l, i) => {
          const pct = Math.min(100, Math.round(l.percent ?? 0));
          const level = pct >= 85 ? "high" : pct >= 60 ? "mid" : "low";
          return (
            <div key={i} className={`usage-limit level-${level}`}>
              <div className="usage-limit-head">
                <span>{limitLabel(l)}</span>
                <span className="gauge-pct">{pct}%</span>
              </div>
              <span className="gauge-bar wide">
                <span className="gauge-fill" style={{ width: `${pct}%` }} />
              </span>
              <p className="muted usage-reset">
                {formatReset(l.resets_at)} にリセット
                {l.severity && l.severity !== "normal"
                  ? ` · ${l.severity}`
                  : ""}
              </p>
            </div>
          );
        })}
      </div>
      {limits.length > 0 &&
        !limits.some((l) => l.kind === "weekly_scoped") && (
          <p className="muted usage-note">
            モデル別（Fable
            等）の内訳は、アカウント切り替え中は取得できません（長期トークンの権限制限）。
          </p>
        )}
    </>
  );
}

/** 各アカウントの使用状況を左右の矢印で見比べられるポップオーバー */
function UsagePopover() {
  const [open, setOpen] = useState(false);
  const [cards, setCards] = useState<UsageCard[] | null>(null);
  const [idx, setIdx] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    setError(null);
    setCards(null);
    api
      .getAccountsUsage()
      .then((accts) => {
        if (accts.length > 0) {
          setCards(
            accts.map((a) => ({
              name: a.name,
              email: a.email,
              planLabel: planLabelRaw(a.plan),
              isLive: a.is_live,
              usage: a.usage,
              error: a.error,
            }))
          );
          // 選択中アカウントを最初に表示する
          const activeIdx = accts.findIndex((a) => a.active);
          setIdx(activeIdx >= 0 ? activeIdx : 0);
          return;
        }
        // アカウント未登録時はライブログインを1枚だけ表示する
        return Promise.all([
          api.getRateLimits(),
          api.getAccountProfile(),
        ]).then(([u, p]) => {
          setCards([
            {
              name:
                p.account?.display_name ?? p.account?.full_name ?? "(名前不明)",
              email: p.account?.email ?? "",
              planLabel: planLabel(p),
              // アカウント未登録時はライブログインそのものを表示している
              isLive: true,
              usage: u,
              error: null,
            },
          ]);
          setIdx(0);
        });
      })
      .catch((e) => setError(String(e)));
  }, []);

  // 取得は open の立ち上がりだけで走らせる。cards を依存に入れると、load が cards を
  // 更新→再取得…の無限ループになりポップオーバーがちらつくため分離する
  useEffect(() => {
    if (open) load();
  }, [open, load]);

  // キー操作は最新の cards を参照する必要があるので別 effect にする（再取得は起こさない）
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
      if (cards && cards.length > 1) {
        if (e.key === "ArrowLeft")
          setIdx((i) => (i - 1 + cards.length) % cards.length);
        if (e.key === "ArrowRight") setIdx((i) => (i + 1) % cards.length);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, cards]);

  const count = cards?.length ?? 0;
  const prev = () => setIdx((i) => (i - 1 + count) % count);
  const next = () => setIdx((i) => (i + 1) % count);
  const card = cards?.[idx];

  return (
    <div className="usage-anchor">
      <button
        className="icon-btn"
        title="アカウントとリソース使用状況"
        onClick={() => setOpen((v) => !v)}
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
          <div className="menu-backdrop" onMouseDown={() => setOpen(false)} />
          <div className="usage-popover">
            {error ? (
              <p className="error-box">{error}</p>
            ) : !cards || !card ? (
              <p className="muted">取得中…</p>
            ) : (
              <>
                {count > 1 && (
                  <div className="usage-carousel-nav">
                    <button
                      className="icon-btn"
                      title="前のアカウント（←）"
                      onClick={prev}
                    >
                      ‹
                    </button>
                    <span className="usage-carousel-dots">
                      {cards.map((_, i) => (
                        <span
                          key={i}
                          className={i === idx ? "dot on" : "dot"}
                        />
                      ))}
                    </span>
                    <button
                      className="icon-btn"
                      title="次のアカウント（→）"
                      onClick={next}
                    >
                      ›
                    </button>
                  </div>
                )}
                <UsageCardView card={card} />
                <hr />
                <p className="usage-extra muted">
                  追加クレジット:{" "}
                  {card.usage?.extra_usage?.is_enabled
                    ? `有効（使用 ${card.usage.extra_usage.used_credits ?? 0}）`
                    : "無効"}
                </p>
              </>
            )}
          </div>
        </>
      )}
    </div>
  );
}

function SessionsView() {
  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [home, setHome] = useState<string | null>(null);
  const [selected, setSelected] = useState<TreeSelection | null>(null);
  const [paneTab, setPaneTab] = useState<"overview" | "sessions">("overview");
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [loading, setLoading] = useState(false);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [extractTarget, setExtractTarget] = useState<TreeSelection | null>(
    null
  );

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
            onClick={() => setReloadKey((k) => k + 1)}
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
            onSelect={setSelected}
            onExtractTasks={setExtractTarget}
          />
        )}
      </aside>
      <section className="session-pane">
        {selected && (
          <>
            <div className="pane-header">
              <div className="pane-tabs">
                <button
                  className={paneTab === "overview" ? "active" : ""}
                  onClick={() => setPaneTab("overview")}
                >
                  概要
                </button>
                <button
                  className={paneTab === "sessions" ? "active" : ""}
                  onClick={() => setPaneTab("sessions")}
                >
                  セッション
                </button>
              </div>
              <span className="pane-path">
                {selected.path ?? selected.project}
              </span>
            </div>
            {paneTab === "overview" ? (
              <ProjectOverview
                key={`${selectionKey(selected)}-${reloadKey}`}
                project={selected.project}
                path={selected.path}
              />
            ) : (
              <SessionList
                key={`${selectionKey(selected)}-${reloadKey}`}
                project={selected.project}
              />
            )}
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

function SessionList({ project }: { project: string }) {
  const [sessions, setSessions] = useState<SessionInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [openTranscript, setOpenTranscript] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[] | null>(null);

  useEffect(() => {
    setSessions(null);
    setQuery("");
    setHits(null);
    api
      .listSessions(project)
      .then(setSessions)
      .catch((e) => setError(String(e)));
  }, [project]);

  const runSearch = () => {
    if (!query.trim()) {
      setHits(null);
      return;
    }
    api
      .searchSummaries(query, project)
      .then(setHits)
      .catch((e) => setError(String(e)));
  };

  const withContent = useMemo(
    () =>
      (sessions ?? []).filter(
        (s) => s.summaries.length > 0 || (s.user_prompt ?? "").trim() !== ""
      ),
    [sessions]
  );

  if (error) return <ErrorBox message={error} />;
  if (!sessions) return <p className="muted">読み込み中…</p>;

  return (
    <div className="session-list">
      <div className="search-bar">
        <input
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            if (e.target.value.trim() === "") setHits(null);
          }}
          onKeyDown={(e) => e.key === "Enter" && runSearch()}
          placeholder={`${project} 内のサマリーを検索`}
        />
        {hits !== null && (
          <button
            className="clear-btn"
            onClick={() => {
              setQuery("");
              setHits(null);
            }}
          >
            クリア
          </button>
        )}
      </div>
      {hits !== null ? (
        <>
          <p className="muted">{hits.length}件ヒット</p>
          {hits.map((h, i) => (
            <SearchHitCard
              key={i}
              hit={h}
              showProject={false}
              onOpen={
                h.content_session_id
                  ? () => setOpenTranscript(h.content_session_id)
                  : undefined
              }
            />
          ))}
        </>
      ) : (
        <>
          {withContent.map((s) => (
            <SessionCard
              key={s.content_session_id}
              session={s}
              onOpen={() => setOpenTranscript(s.content_session_id)}
            />
          ))}
          {withContent.length === 0 && (
            <p className="muted">表示できるセッションがありません</p>
          )}
        </>
      )}
      {openTranscript && (
        <TranscriptDrawer
          sessionId={openTranscript}
          onClose={() => setOpenTranscript(null)}
        />
      )}
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

function SessionCard({
  session,
  onOpen,
}: {
  session: SessionInfo;
  onOpen: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const latest = session.summaries[session.summaries.length - 1];
  const title =
    latest?.request ?? session.user_prompt ?? "(依頼内容の記録なし)";

  return (
    <article className="session-card">
      <div className="session-head" onClick={() => setExpanded(!expanded)}>
        <div>
          <p className="session-title">{title}</p>
          <p className="session-meta">
            {formatEpoch(session.started_at_epoch)}
            {session.summaries.length > 1 &&
              ` · サマリー${session.summaries.length}件`}
          </p>
        </div>
        <button
          className="open-btn"
          onClick={(e) => {
            e.stopPropagation();
            onOpen();
          }}
        >
          会話を開く
        </button>
      </div>
      {expanded && (
        <div className="summary-detail">
          {session.summaries.length === 0 && (
            <p className="muted">
              このセッションの claude-mem サマリーはありません
            </p>
          )}
          {session.summaries.map((sum, i) => (
            <div key={i} className="summary-block">
              <SummaryField label="依頼" value={sum.request} />
              <SummaryField label="調査" value={sum.investigated} />
              <SummaryField label="学び" value={sum.learned} />
              <SummaryField label="完了" value={sum.completed} />
              <SummaryField label="次の一手" value={sum.next_steps} />
              <SummaryField label="編集ファイル" value={sum.files_edited} />
            </div>
          ))}
        </div>
      )}
    </article>
  );
}

function SummaryField({
  label,
  value,
}: {
  label: string;
  value: string | null;
}) {
  if (!value || value.trim() === "") return null;
  return (
    <p className="summary-field">
      <span className="field-label">{label}</span>
      {value}
    </p>
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
