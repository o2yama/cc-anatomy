import { useEffect, useState } from "react";
import { api, ProjectInfo, relativeTime } from "./api";

/**
 * ツリーで選択された「階層」。ディレクトリはセッション実績の有無にかかわらず選択でき、
 * その階層の設定（CLAUDE.md / rules / skills …）を開く。
 * path が null なのは claude-mem 上に場所の記録がないプロジェクトのみ。
 */
export interface TreeSelection {
  project: string;
  path: string | null;
}

export function selectionKey(sel: TreeSelection): string {
  return sel.path ?? `?${sel.project}`;
}

export interface DirNode {
  name: string;
  fullPath: string;
  children: DirNode[];
  /** セッション実績のあるディレクトリだけが持つ */
  info?: ProjectInfo;
  /** 配下を含む最終活動時刻。並び順に使う */
  lastActivity: number;
}

export interface Tree {
  root: DirNode;
  others: ProjectInfo[];
}

/**
 * フルパス群からディレクトリツリーを構築する。
 * ホーム配下に収まらないパス・パス不明のものは others に落とす
 * （ツリー上の位置が決められず、偽のパスを作ると設定の読み先を誤るため）。
 */
export function buildTree(projects: ProjectInfo[], home: string): Tree {
  const root: DirNode = { name: "~", fullPath: home, children: [], lastActivity: 0 };
  const others: ProjectInfo[] = [];

  for (const p of projects) {
    if (!p.path) {
      others.push(p);
      continue;
    }
    // ~/.claude 配下はスキル開発やエージェントの作業痕跡であって
    // ユーザーのプロジェクトではないので、ツリーに出さない（記録自体は残す）
    if (p.path.startsWith(home + "/.claude/")) {
      continue;
    }
    if (p.path === home) {
      root.info = p;
      continue;
    }
    if (!p.path.startsWith(home + "/")) {
      others.push(p);
      continue;
    }

    let node = root;
    for (const seg of p.path.slice(home.length + 1).split("/")) {
      let child = node.children.find((c) => c.name === seg);
      if (!child) {
        child = {
          name: seg,
          fullPath: `${node.fullPath}/${seg}`,
          children: [],
          lastActivity: 0,
        };
        node.children.push(child);
      }
      node = child;
    }
    node.info = p;
  }

  computeActivity(root);
  sortTree(root);
  others.sort((a, b) => b.last_activity_epoch - a.last_activity_epoch);
  return { root, others };
}

function computeActivity(node: DirNode): number {
  node.lastActivity = node.children
    .map(computeActivity)
    .reduce((max, v) => Math.max(max, v), node.info?.last_activity_epoch ?? 0);
  return node.lastActivity;
}

/** 最近触った階層を上に。活動のないコンテナだけが名前順に並ぶ */
function sortTree(node: DirNode) {
  node.children.sort(
    (a, b) => b.lastActivity - a.lastActivity || a.name.localeCompare(b.name)
  );
  node.children.forEach(sortTree);
}

/**
 * 一括開閉の対象になるディレクトリのフルパス。
 * ルート（~）は畳むと何も見えなくなるだけなので対象外にする。
 */
export function collapsiblePaths(root: DirNode): string[] {
  const acc: string[] = [];
  const walk = (node: DirNode) => {
    if (node.children.length === 0) return;
    acc.push(node.fullPath);
    node.children.forEach(walk);
  };
  root.children.forEach(walk);
  return acc;
}

interface MenuState {
  x: number;
  y: number;
  sel: TreeSelection;
}

export function ProjectTree({
  tree,
  collapsed,
  setCollapsed,
  selected,
  onSelect,
  onExtractTasks,
}: {
  tree: Tree;
  collapsed: Set<string>;
  setCollapsed: (update: (prev: Set<string>) => Set<string>) => void;
  selected: TreeSelection | null;
  onSelect: (sel: TreeSelection) => void;
  onExtractTasks: (sel: TreeSelection) => void;
}) {
  const { root, others } = tree;
  // パスを復元できなかった（transcript が自動削除された等）プロジェクトの置き場。
  // 普段は見ないのでデフォルトで畳んでおく
  const [othersCollapsed, setOthersCollapsed] = useState(true);
  const [menu, setMenu] = useState<MenuState | null>(null);

  const openMenu = (e: React.MouseEvent, sel: TreeSelection) => {
    e.preventDefault();
    setMenu({ x: e.clientX, y: e.clientY, sel });
  };

  const toggle = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      next.has(path) ? next.delete(path) : next.add(path);
      return next;
    });

  const selKey = selected ? selectionKey(selected) : null;

  return (
    <div className="tree">
      <DirRows
        node={root}
        depth={0}
        collapsed={collapsed}
        toggle={toggle}
        selKey={selKey}
        onSelect={onSelect}
        onContextMenu={openMenu}
      />
      {others.length > 0 && (
        <>
          <button
            className="tree-section-toggle"
            onClick={() => setOthersCollapsed((v) => !v)}
          >
            <Chevron open={!othersCollapsed} />
            <span>削除済みフォルダ</span>
            <span className="count-badge">{others.length}</span>
          </button>
          {!othersCollapsed &&
            others.map((p) => (
              <div
                key={p.project}
                className={`tree-row container ${
                  selKey === selectionKey({ project: p.project, path: p.path })
                    ? "selected"
                    : ""
                }`}
                style={{ paddingLeft: 8 }}
                onContextMenu={(e) =>
                  openMenu(e, { project: p.project, path: p.path })
                }
              >
                <span className="chevron-spacer" />
                <button
                  className="tree-label"
                  onClick={() => onSelect({ project: p.project, path: p.path })}
                  title={`${p.path ?? "パス不明"} · 最終活動 ${relativeTime(
                    p.last_activity_epoch
                  )}`}
                >
                  <span className="dir-name">{p.project}</span>
                  {p.summary_count > 0 && (
                    <span className="count-badge">{p.summary_count}</span>
                  )}
                </button>
              </div>
            ))}
        </>
      )}
      {menu && (
        <ContextMenu
          menu={menu}
          onClose={() => setMenu(null)}
          onExtractTasks={onExtractTasks}
        />
      )}
    </div>
  );
}

function ContextMenu({
  menu,
  onClose,
  onExtractTasks,
}: {
  menu: MenuState;
  onClose: () => void;
  onExtractTasks: (sel: TreeSelection) => void;
}) {
  const { sel } = menu;
  const hasPath = !!sel.path;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const run = (fn: () => Promise<void>) => {
    onClose();
    fn().catch((e) => window.alert(String(e)));
  };

  return (
    <>
      {/* メニュー外クリック・右クリックで閉じるための透明レイヤー */}
      <div
        className="menu-backdrop"
        onMouseDown={onClose}
        onContextMenu={(e) => {
          e.preventDefault();
          onClose();
        }}
      />
      <div
        className="context-menu"
        style={{ left: menu.x, top: menu.y }}
        onContextMenu={(e) => e.preventDefault()}
      >
        <button
          disabled={!hasPath}
          onClick={() => run(() => api.openInFinder(sel.path!))}
        >
          Finder で開く
        </button>
        <button
          disabled={!hasPath}
          onClick={() => run(() => api.openInCmux(sel.path!))}
        >
          cmux で開く
        </button>
        <button
          disabled={!hasPath}
          onClick={() => run(() => api.openInTerminal(sel.path!))}
        >
          Ghostty で開く
        </button>
        <hr />
        <button
          onClick={() => {
            onClose();
            onExtractTasks(sel);
          }}
        >
          タスク抽出（Claude で）
        </button>
      </div>
    </>
  );
}

function DirRows({
  node,
  depth,
  collapsed,
  toggle,
  selKey,
  onSelect,
  onContextMenu,
}: {
  node: DirNode;
  depth: number;
  collapsed: Set<string>;
  toggle: (path: string) => void;
  selKey: string | null;
  onSelect: (sel: TreeSelection) => void;
  onContextMenu: (e: React.MouseEvent, sel: TreeSelection) => void;
}) {
  const hasChildren = node.children.length > 0;
  const isCollapsed = collapsed.has(node.fullPath);
  const basename = node.fullPath.split("/").pop() ?? node.name;
  const selection: TreeSelection = {
    project: node.info?.project ?? basename,
    path: node.fullPath,
  };

  // 名前クリックで「この階層の概要を開く」と「開閉」を兼ねる
  const activate = () => {
    onSelect(selection);
    if (hasChildren) toggle(node.fullPath);
  };

  return (
    <>
      <div
        className={`tree-row ${selKey === node.fullPath ? "selected" : ""} ${
          node.info ? "" : "container"
        }`}
        style={{ paddingLeft: 8 + depth * 14 }}
        onContextMenu={(e) => onContextMenu(e, selection)}
      >
        {hasChildren ? (
          <button
            className="chevron-btn"
            onClick={() => toggle(node.fullPath)}
            aria-label={isCollapsed ? "展開" : "折りたたむ"}
          >
            <Chevron open={!isCollapsed} />
          </button>
        ) : (
          <span className="chevron-spacer" />
        )}
        <button
          className="tree-label"
          onClick={activate}
          title={
            node.info
              ? `${node.fullPath} · 最終活動 ${relativeTime(
                  node.info.last_activity_epoch
                )}`
              : node.fullPath
          }
        >
          <span className="dir-name">{node.name}</span>
          {node.info && node.info.summary_count > 0 && (
            <span className="count-badge">{node.info.summary_count}</span>
          )}
        </button>
      </div>
      {hasChildren &&
        !isCollapsed &&
        node.children.map((c) => (
          <DirRows
            key={c.fullPath}
            node={c}
            depth={depth + 1}
            collapsed={collapsed}
            toggle={toggle}
            selKey={selKey}
            onSelect={onSelect}
            onContextMenu={onContextMenu}
          />
        ))}
    </>
  );
}

function Chevron({ open }: { open: boolean }) {
  return (
    <svg
      className={`chevron ${open ? "open" : ""}`}
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="3"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <polyline points="9 5 16 12 9 19" />
    </svg>
  );
}
