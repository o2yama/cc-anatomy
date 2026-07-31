import { useEffect, useMemo, useState } from "react";
import {
  api,
  relativeTime,
  FileDoc,
  ObservationItem,
  ProjectEnv,
  ScopedItem,
} from "./api";
import { DocEditor } from "./DocEditor";

const OBS_TYPES: Record<string, { icon: string; label: string }> = {
  feature: { icon: "🟣", label: "機能" },
  bugfix: { icon: "🔴", label: "バグ修正" },
  decision: { icon: "⚖️", label: "決定" },
  discovery: { icon: "🔵", label: "発見" },
  change: { icon: "✅", label: "変更" },
  refactor: { icon: "🔄", label: "リファクタ" },
};

function obsIcon(type: string): string {
  return OBS_TYPES[type]?.icon ?? "・";
}

interface Detail {
  title: string;
  subtitle?: string;
  content: string;
  truncated?: boolean;
  /** 実ファイルに紐づく場合のみ設定。無ければ DocEditor は編集不可（閲覧のみ）で開く */
  path?: string;
  modifiedEpoch?: number;
  /** false の場合、AI分析にプロジェクトルートを渡さない（rules・auto-memory等の
   * プロジェクトに紐づかないグローバル文書）。未指定は true 扱い（従来どおりプロジェクトと
   * 突き合わせる） */
  projectScoped?: boolean;
}

/**
 * 空白区切りの全語が含まれたら一致（AND・大文字小文字無視の部分一致）。
 * 名前だけでなく本文・設定JSONも haystack に含めて「全文検索」にする
 */
function matches(query: string, ...fields: (string | null | undefined)[]): boolean {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return true;
  const hay = fields.filter(Boolean).join("\n").toLowerCase();
  return terms.every((t) => hay.includes(t));
}

export function ProjectOverview({
  project,
  path,
}: {
  project: string;
  path: string | null;
}) {
  const [env, setEnv] = useState<ProjectEnv | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [detail, setDetailRaw] = useState<Detail | null>(null);
  // ドロワーで編集中（DocEditor から報告される）かどうか。閉じる／別ファイルを開く
  // 操作をガードするために使う
  const [detailDirty, setDetailDirty] = useState(false);
  // ガード対象の遷移先。null は「閉じる」を意味する
  const [pendingDetail, setPendingDetail] = useState<{ next: Detail | null } | null>(
    null
  );

  /** detail の切替は原則これ経由にし、未保存変更があれば確認を挟む */
  const setDetail = (next: Detail | null) => {
    if (detailDirty) {
      setPendingDetail({ next });
      return;
    }
    setDetailRaw(next);
    setDetailDirty(false);
  };
  const confirmDiscardAndProceed = () => {
    if (!pendingDetail) return;
    setDetailRaw(pendingDetail.next);
    setDetailDirty(false);
    setPendingDetail(null);
  };
  const cancelPendingSwitch = () => setPendingDetail(null);
  // グローバル設定（~/.claude 由来の skills/MCP等）は量が多いのでデフォルト非表示。
  // 選択はプロジェクト切替をまたいで保持したいので localStorage に持つ
  const [showGlobal, setShowGlobal] = useState(
    () => localStorage.getItem("cc-anatomy.showGlobal") === "1"
  );
  const toggleGlobal = (v: boolean) => {
    setShowGlobal(v);
    localStorage.setItem("cc-anatomy.showGlobal", v ? "1" : "0");
  };
  // メモリカードの表示段階: ヘッダクリックで完全開閉、もっと見るで3件⇔全件
  const [memCollapsed, setMemCollapsed] = useState(false);
  const [memExpanded, setMemExpanded] = useState(false);

  useEffect(() => {
    setEnv(null);
    // プロジェクト切替は未保存変更ガードの対象外にする（トップレベルの画面遷移のため）
    setDetailRaw(null);
    setDetailDirty(false);
    setPendingDetail(null);
    api
      .getProjectEnv(project, path)
      .then(setEnv)
      .catch((e) => setError(String(e)));
  }, [project, path]);

  const openFile = (
    title: string,
    filePath: string,
    scope: "project" | "global" = "project"
  ) => {
    api
      .readDoc(filePath)
      .then((doc: FileDoc) =>
        setDetail({
          title,
          subtitle: doc.path,
          content: doc.content,
          truncated: doc.truncated,
          path: doc.path,
          modifiedEpoch: doc.modified_epoch,
          projectScoped: scope === "project",
        })
      )
      .catch((e) =>
        setDetail({ title, subtitle: filePath, content: String(e) })
      );
  };

  if (error) return <div className="error-box">{error}</div>;
  if (!env) return <p className="muted">読み込み中…</p>;

  const mcpServers = env.mcp_servers.filter(
    (s) => showGlobal || s.scope === "project"
  );
  const hooks = env.hooks.filter((h) => showGlobal || h.scope === "project");
  // rules は ~/.claude/rules 由来でグローバル扱い
  const rules = showGlobal ? env.rules : [];
  const scopedFilter = (items: ScopedItem[]) =>
    items.filter((i) => showGlobal || i.scope === "project");

  return (
    <div className="overview">
      <div className="overview-grid">
        <Card title="CLAUDE.md" badge={env.claude_mds.length > 0 ? "あり" : "なし"}>
          {(query) => {
            const docs = env.claude_mds.filter((d) =>
              matches(query, d.path, d.content)
            );
            return (
              <>
                {env.claude_mds.length === 0 && (
                  <p className="muted">
                    {env.path
                      ? "このプロジェクトに CLAUDE.md はありません"
                      : "パス未解決のため確認できません"}
                  </p>
                )}
                <ul className="item-list">
                  {docs.map((doc) => (
                    <li
                      key={doc.path}
                      className="clickable"
                      onClick={() =>
                        setDetail({
                          title: "CLAUDE.md",
                          subtitle: doc.path,
                          content: doc.content,
                          truncated: doc.truncated,
                          path: doc.path,
                          modifiedEpoch: doc.modified_epoch,
                        })
                      }
                    >
                      <span className="item-name">{shortPath(doc.path)}</span>
                      <span className="item-sub">
                        更新 {relativeTime(doc.modified_epoch)}
                      </span>
                    </li>
                  ))}
                  <NoMatch shown={docs.length} total={env.claude_mds.length} />
                </ul>
              </>
            );
          }}
        </Card>

        {env.has_claude_mem && (
        <Card
          title="メモリ（直近の記憶）"
          badge={`${env.observations.length}件`}
          wide
          collapsed={memCollapsed}
          onToggleCollapse={() => setMemCollapsed((v) => !v)}
        >
          {(query) => {
            const allObs = env.observations.filter((o) =>
              matches(
                query,
                o.title,
                o.subtitle,
                o.narrative,
                o.facts.join("\n"),
                o.files_modified.join("\n")
              )
            );
            // 検索中は絞り込み結果を全部見せたいので3件制限しない
            const showAll = memExpanded || query.trim() !== "";
            const obs = showAll ? allObs : allObs.slice(0, 3);
            const memoryMd =
              env.memory_md &&
              matches(query, "MEMORY.md", env.memory_md.content)
                ? env.memory_md
                : null;
            const memFiles = env.memory_files.filter((f) =>
              matches(query, f.name)
            );
            return (
              <>
                {env.next_steps && !query && (
                  <p className="summary-field">
                    <span className="field-label">次の一手</span>
                    {env.next_steps}
                  </p>
                )}
                <MemoryTimeline
                  observations={obs}
                  compact={!showAll}
                  onOpen={(o) =>
                    setDetail({
                      title: `${obsIcon(o.type)} ${o.title ?? "(無題)"}`,
                      subtitle: `${OBS_TYPES[o.type]?.label ?? o.type} · ${formatDate(
                        o.created_at_epoch
                      )}`,
                      content: observationText(o),
                    })
                  }
                  emptyMessage={
                    query
                      ? `「${query}」に一致する記憶なし`
                      : "claude-mem の記録がありません"
                  }
                />
                {!query && allObs.length > 3 && (
                  <button
                    className="more-btn"
                    onClick={() => setMemExpanded((v) => !v)}
                  >
                    {memExpanded
                      ? "閉じる"
                      : `もっと見る（残り ${allObs.length - 3} 件）`}
                  </button>
                )}
                {showAll && (memoryMd || memFiles.length > 0) && (
                  <>
                    <p className="tree-section">auto-memory</p>
                    <ul className="item-list">
                      {memoryMd && (
                        <li
                          className="clickable"
                          onClick={() =>
                            setDetail({
                              title: "MEMORY.md（インデックス）",
                              subtitle: memoryMd.path,
                              content: memoryMd.content,
                              truncated: memoryMd.truncated,
                              path: memoryMd.path,
                              modifiedEpoch: memoryMd.modified_epoch,
                            })
                          }
                        >
                          <span className="item-name">MEMORY.md</span>
                          <span className="item-sub">
                            更新 {relativeTime(memoryMd.modified_epoch)}
                          </span>
                        </li>
                      )}
                      {memFiles.map((f) => (
                        <li
                          key={f.path}
                          className="clickable"
                          onClick={() => openFile(`メモリ: ${f.name}`, f.path, "global")}
                        >
                          <span className="item-name">{f.name}</span>
                        </li>
                      ))}
                    </ul>
                  </>
                )}
              </>
            );
          }}
        </Card>
        )}

        <label className="global-toggle grid-corner">
          <input
            type="checkbox"
            checked={showGlobal}
            onChange={(e) => toggleGlobal(e.target.checked)}
          />
          グローバル設定を表示
        </label>

        <Card title="MCPサーバー" badge={`${mcpServers.length}件`}>
          {(query) => {
            const servers = mcpServers.filter((s) =>
              matches(query, s.name, s.source, s.config)
            );
            return (
              <>
                {mcpServers.length === 0 && (
                  <p className="muted">
                    {env.mcp_servers.length > 0
                      ? "プロジェクト固有のMCPサーバーなし"
                      : "MCPサーバー設定なし"}
                  </p>
                )}
                <ul className="item-list">
                  {servers.map((s, i) => (
                    <li
                      key={i}
                      className="clickable"
                      onClick={() =>
                        setDetail({
                          title: `MCP: ${s.name}`,
                          subtitle: `${s.source}（環境変数・ヘッダはマスク済み）`,
                          content: s.config,
                        })
                      }
                    >
                      <ScopeBadge scope={s.scope} />
                      <span className="item-name">{s.name}</span>
                      <span className="item-sub">{s.source}</span>
                    </li>
                  ))}
                  <NoMatch shown={servers.length} total={mcpServers.length} />
                </ul>
              </>
            );
          }}
        </Card>

        <Card title="Hooks / Rules" badge={`${hooks.length} / ${rules.length}`}>
          {(query) => {
            const shownHooks = hooks.filter((h) =>
              matches(query, h.event, h.config)
            );
            const shownRules = rules.filter((r) => matches(query, r.name));
            return (
              <>
                <ul className="item-list">
                  {shownHooks.map((h, i) => (
                    <li
                      key={i}
                      className="clickable"
                      onClick={() =>
                        setDetail({
                          title: `Hook: ${h.event}`,
                          subtitle: `${h.scope === "project" ? "プロジェクト" : "グローバル"} settings`,
                          content: h.config,
                        })
                      }
                    >
                      <ScopeBadge scope={h.scope} />
                      <span className="item-name">{h.event}</span>
                      <span className="item-sub">matcher {h.matcher_count}件</span>
                    </li>
                  ))}
                  {hooks.length === 0 && (
                    <p className="muted">
                      {env.hooks.length > 0
                        ? "プロジェクト固有のhooksなし"
                        : "hooks設定なし"}
                    </p>
                  )}
                  <NoMatch shown={shownHooks.length} total={hooks.length} />
                </ul>
                {shownRules.length > 0 && (
                  <details className="doc-details" open>
                    <summary>rules（グローバル {shownRules.length}件）</summary>
                    <ul className="item-list">
                      {shownRules.map((r) => (
                        <li
                          key={r.path}
                          className="clickable"
                          onClick={() => openFile(`Rule: ${r.name}`, r.path, "global")}
                        >
                          <span className="item-name">{r.name}</span>
                        </li>
                      ))}
                    </ul>
                  </details>
                )}
              </>
            );
          }}
        </Card>

        <ItemsCard
          title="Agents"
          items={scopedFilter(env.agents)}
          onOpen={openFile}
        />
        <ItemsCard
          title="Skills"
          items={scopedFilter(env.skills)}
          onOpen={openFile}
        />
        <ItemsCard
          title="Commands"
          items={scopedFilter(env.commands)}
          onOpen={openFile}
        />
      </div>
      {detail && (
        <DetailDrawer
          detail={detail}
          projectDir={env.path}
          onClose={() => setDetail(null)}
          onDirtyChange={setDetailDirty}
        />
      )}
      {pendingDetail && (
        <div className="unsaved-guard-overlay">
          <div className="unsaved-guard-box">
            <p>未保存の変更があります。破棄して続けますか？</p>
            <div className="unsaved-guard-actions">
              <button className="doc-editor-btn" onClick={cancelPendingSwitch}>
                キャンセル
              </button>
              <button
                className="doc-editor-btn doc-editor-btn-primary"
                onClick={confirmDiscardAndProceed}
              >
                破棄して続ける
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * children に関数を渡すとヘッダに検索窓が出て、入力中のクエリで描画される。
 * フィルタ自体は各カード側の責務（対象フィールドがカードごとに違うため）。
 * onToggleCollapse を渡すとヘッダクリックで開閉できるカードになる
 */
function Card({
  title,
  badge,
  wide = false,
  collapsed = false,
  onToggleCollapse,
  children,
}: {
  title: string;
  badge?: string;
  wide?: boolean;
  collapsed?: boolean;
  onToggleCollapse?: () => void;
  children: React.ReactNode | ((query: string) => React.ReactNode);
}) {
  const searchable = typeof children === "function";
  const [query, setQuery] = useState("");

  return (
    <section className={`env-card ${wide ? "env-card-wide" : ""}`}>
      <header
        className={`env-card-head ${onToggleCollapse ? "collapsible" : ""}`}
        onClick={onToggleCollapse}
      >
        {onToggleCollapse && (
          <span className={`card-chevron ${collapsed ? "" : "open"}`}>▸</span>
        )}
        <h3>{title}</h3>
        {badge && <span className="count-badge">{badge}</span>}
        {searchable && !collapsed && (
          <input
            className="card-search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="検索"
            spellCheck={false}
            onClick={(e) => e.stopPropagation()}
          />
        )}
      </header>
      {!collapsed && (
        <div className="env-card-body">
          {searchable ? children(query) : children}
        </div>
      )}
    </section>
  );
}

/** 記憶を種類チップでフィルタし、日付ごとにまとめて表示するタイムライン */
function MemoryTimeline({
  observations,
  onOpen,
  emptyMessage = "claude-mem の記録がありません",
  compact = false,
}: {
  observations: ObservationItem[];
  onOpen: (o: ObservationItem) => void;
  emptyMessage?: string;
  /** 3件表示モード。種類チップを出さずリストだけにする */
  compact?: boolean;
}) {
  const [filter, setFilter] = useState<string | null>(null);

  const counts = useMemo(() => {
    const c: Record<string, number> = {};
    for (const o of observations) c[o.type] = (c[o.type] ?? 0) + 1;
    return c;
  }, [observations]);

  const filtered = filter
    ? observations.filter((o) => o.type === filter)
    : observations;

  const groups = useMemo(() => {
    const g: { date: string; items: ObservationItem[] }[] = [];
    for (const o of filtered) {
      const date = formatDate(o.created_at_epoch);
      const last = g[g.length - 1];
      if (last && last.date === date) last.items.push(o);
      else g.push({ date, items: [o] });
    }
    return g;
  }, [filtered]);

  if (observations.length === 0) {
    return <p className="muted">{emptyMessage}</p>;
  }

  return (
    <div className="memory-timeline">
      {!compact && (
        <div className="chip-row">
          <button
            className={`chip ${filter === null ? "active" : ""}`}
            onClick={() => setFilter(null)}
          >
            すべて {observations.length}
          </button>
          {Object.entries(OBS_TYPES)
            .filter(([type]) => counts[type])
            .map(([type, meta]) => (
              <button
                key={type}
                className={`chip ${filter === type ? "active" : ""}`}
                onClick={() => setFilter(filter === type ? null : type)}
              >
                {meta.icon} {meta.label} {counts[type]}
              </button>
            ))}
        </div>
      )}
      {groups.map((g) => (
        <div key={g.date} className="obs-group">
          <p className="obs-date">{g.date}</p>
          <ul className="obs-list">
            {g.items.map((o) => (
              <li key={o.id} className="clickable" onClick={() => onOpen(o)}>
                <span className="obs-icon">{obsIcon(o.type)}</span>
                <span className="obs-title" title={o.subtitle ?? undefined}>
                  {o.title ?? "(無題)"}
                </span>
                <span className="obs-time">{formatTime(o.created_at_epoch)}</span>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

function formatDate(epochMs: number): string {
  const d = new Date(epochMs);
  return d.toLocaleDateString("ja-JP", {
    month: "long",
    day: "numeric",
    weekday: "short",
  });
}

function formatTime(epochMs: number): string {
  return new Date(epochMs).toLocaleTimeString("ja-JP", {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 記憶1件を詳細ドロワー用の読みやすいテキストに整形 */
function observationText(o: ObservationItem): string {
  const parts: string[] = [];
  if (o.subtitle) parts.push(o.subtitle);
  if (o.narrative) parts.push(`【詳細】\n${o.narrative}`);
  if (o.facts.length > 0)
    parts.push(`【ファクト】\n${o.facts.map((f) => `・${f}`).join("\n")}`);
  if (o.files_modified.length > 0)
    parts.push(
      `【変更ファイル】\n${o.files_modified.map((f) => `・${f}`).join("\n")}`
    );
  return parts.join("\n\n") || "(詳細記録なし)";
}

function ItemsCard({
  title,
  items,
  onOpen,
}: {
  title: string;
  items: ScopedItem[];
  onOpen: (title: string, path: string, scope: "project" | "global") => void;
}) {
  const projectItems = items.filter((i) => i.scope === "project");
  const globalItems = items.filter((i) => i.scope === "global");
  return (
    <Card
      title={title}
      badge={
        projectItems.length > 0 && globalItems.length > 0
          ? `P${projectItems.length} + G${globalItems.length}`
          : `${items.length}件`
      }
    >
      {(query) => {
        const filtered = [...projectItems, ...globalItems].filter((i) =>
          matches(query, i.name, i.description, i.path)
        );
        return (
          <ul className="item-list">
            {filtered.map((item) => (
              <ItemRow
                key={item.path}
                item={item}
                onClick={() =>
                  onOpen(
                    `${title.replace(/s$/, "")}: ${item.name}`,
                    item.path,
                    item.scope
                  )
                }
              />
            ))}
            {items.length === 0 && <p className="muted">なし</p>}
            <NoMatch shown={filtered.length} total={items.length} />
          </ul>
        );
      }}
    </Card>
  );
}

/** フィルタで全件消えたときだけ「一致なし」を出す（元から0件のときは出さない） */
function NoMatch({ shown, total }: { shown: number; total: number }) {
  if (total === 0 || shown > 0) return null;
  return <p className="muted">一致する項目がありません</p>;
}

function ItemRow({ item, onClick }: { item: ScopedItem; onClick: () => void }) {
  return (
    <li
      className="clickable"
      title={item.description || item.path}
      onClick={onClick}
    >
      <ScopeBadge scope={item.scope} />
      <span className="item-name">{item.name}</span>
      <span className="item-sub">{firstSentence(item.description)}</span>
    </li>
  );
}

/** ホームを ~ に置換（誤読を避けるため省略はしない。あふれはCSSのellipsisに任せる） */
function shortPath(path: string): string {
  const home = path.match(/^\/Users\/[^/]+/)?.[0];
  return home ? path.replace(home, "~") : path;
}

function firstSentence(text: string): string {
  const t = text.trim().replace(/\s+/g, " ");
  const cut = t.search(/[。．.](\s|$)/);
  const s = cut >= 0 ? t.slice(0, cut + 1) : t;
  return s.length > 80 ? s.slice(0, 80) + "…" : s;
}

function ScopeBadge({ scope }: { scope: "project" | "global" }) {
  return (
    <span className={`scope-badge scope-${scope}`}>
      {scope === "project" ? "P" : "G"}
    </span>
  );
}

function DetailDrawer({
  detail,
  projectDir,
  onClose,
  onDirtyChange,
}: {
  detail: Detail;
  projectDir: string | null;
  onClose: () => void;
  onDirtyChange: (dirty: boolean) => void;
}) {
  // ドキュメントが変わるたびに DocEditor を作り直したいので、識別子を key にする
  const editorKey = detail.path ?? `${detail.title}::${detail.subtitle ?? ""}`;
  // グローバルスコープの文書（rules・auto-memory等）はプロジェクトに紐づかないため、
  // AI分析にプロジェクトルートを渡さない（Rust 側がプロンプトを文書単体の分析に出し分ける）
  const analysisProjectDir = detail.projectScoped === false ? null : projectDir;
  return (
    <div className="drawer-overlay" onClick={onClose}>
      <div className="drawer" onClick={(e) => e.stopPropagation()}>
        <div className="drawer-head">
          <div>
            <h2>{detail.title}</h2>
            {detail.subtitle && <p className="muted">{detail.subtitle}</p>}
          </div>
          <button className="close-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="drawer-body drawer-body-editor">
          <DocEditor
            key={editorKey}
            path={detail.path ?? null}
            content={detail.content}
            truncated={detail.truncated}
            modifiedEpoch={detail.modifiedEpoch ?? null}
            projectDir={analysisProjectDir}
            onDirtyChange={onDirtyChange}
          />
        </div>
      </div>
    </div>
  );
}
