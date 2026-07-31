import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { markdown } from "@codemirror/lang-markdown";
import { languages } from "@codemirror/language-data";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";
import { api, DocAnalysisProgress, parseAlreadyRunning } from "./api";
import { useIsMac } from "./platform";

/** 見出しを太字・大きめに、強調(**text**)を太字にする。App.css のダーク配色に合わせる */
const markdownHighlight = HighlightStyle.define([
  { tag: tags.heading1, fontWeight: "700", fontSize: "1.4em", color: "var(--text)" },
  { tag: tags.heading2, fontWeight: "700", fontSize: "1.25em", color: "var(--text)" },
  { tag: tags.heading3, fontWeight: "700", fontSize: "1.12em", color: "var(--text)" },
  {
    tag: [tags.heading4, tags.heading5, tags.heading6],
    fontWeight: "700",
    color: "var(--text)",
  },
  { tag: tags.strong, fontWeight: "700", color: "var(--text)" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strikethrough, textDecoration: "line-through", color: "var(--text-dim)" },
  { tag: tags.link, color: "var(--accent)" },
  { tag: tags.url, color: "var(--accent)" },
  {
    tag: [tags.monospace, tags.processingInstruction],
    fontFamily: '"SF Mono", Menlo, monospace',
    color: "var(--accent)",
  },
  { tag: tags.quote, color: "var(--text-dim)", fontStyle: "italic" },
]);

/** App.css の CSS 変数（--bg 等）に馴染ませたダークテーマ */
const editorTheme = EditorView.theme(
  {
    "&": {
      color: "var(--text)",
      backgroundColor: "var(--bg)",
      height: "100%",
      fontSize: "12.5px",
    },
    ".cm-content": {
      fontFamily: '"SF Mono", Menlo, monospace',
      caretColor: "var(--accent)",
      padding: "10px 12px",
    },
    ".cm-gutters": {
      backgroundColor: "var(--bg-panel)",
      color: "var(--text-dim)",
      border: "none",
    },
    ".cm-activeLine": { backgroundColor: "var(--bg-hover)" },
    ".cm-activeLineGutter": { backgroundColor: "var(--bg-hover)" },
    ".cm-selectionBackground, ::selection": {
      backgroundColor: "var(--accent-soft) !important",
    },
    "&.cm-focused .cm-cursor": { borderLeftColor: "var(--accent)" },
    ".cm-scroller": { overflow: "auto" },
  },
  { dark: true }
);

function isMarkdownPath(path: string | null): boolean {
  return !!path && path.toLowerCase().endsWith(".md");
}

export interface DocEditorProps {
  /** 実ファイルに紐づかないコンテンツ（MCP設定・hooks・記憶メモ等）は null にし常に読み取り専用にする */
  path: string | null;
  content: string;
  truncated?: boolean;
  modifiedEpoch?: number | null;
  /** AI分析の実行 cwd。プロジェクトルート（null ならファイルの親ディレクトリを Rust 側で使う） */
  projectDir?: string | null;
  /** 未保存変更の有無を親（ドロワーの離脱ガード）に伝える */
  onDirtyChange?: (dirty: boolean) => void;
}

type AnalysisPhase = "idle" | "running" | "done" | "error";

/**
 * CLAUDE.md 等の設定ドキュメントをその場で編集・保存する CodeMirror 6 エディタ。
 * 親側が `key` に detail の識別子（path 等）を渡し、ドキュメント切替時は毎回 remount させる
 * 前提で作っている（内部状態を props 差分で同期する複雑さを避けるため）。
 */
export function DocEditor({
  path,
  content,
  truncated = false,
  modifiedEpoch = null,
  projectDir = null,
  onDirtyChange,
}: DocEditorProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const readOnly = path === null || truncated;
  const isMac = useIsMac();
  // AI分析は macOS 限定（claude CLI のヘッドレス実行に依存）かつ、非 truncated の
  // 実ファイルのみ（切り詰められた内容を分析対象にしない）。readOnly（保存不可）とは独立の条件
  const canAnalyze = isMac && path !== null && !truncated;

  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [epoch, setEpoch] = useState<number | null>(modifiedEpoch);

  const [analysisPhase, setAnalysisPhase] = useState<AnalysisPhase>("idle");
  const [analysisLog, setAnalysisLog] = useState<DocAnalysisProgress[]>([]);
  const [analysisResult, setAnalysisResult] = useState<string | null>(null);
  const [analysisError, setAnalysisError] = useState<string | null>(null);
  const [analysisAlreadyRunning, setAnalysisAlreadyRunning] = useState(false);
  const [copyLabel, setCopyLabel] = useState("コピー");
  // ユーザー操作によるキャンセルかどうかを catch 側で判別するための参照。
  // 「失敗」と「キャンセルしました」の表示を分けるために使う（catch はエラー理由を持たないため）
  const cancelledRef = useRef(false);
  // unmount 時、実行中の分析があれば孤児プロセスにしないよう中止する
  const analysisPhaseRef = useRef(analysisPhase);
  useEffect(() => {
    analysisPhaseRef.current = analysisPhase;
  });
  const copyTimeoutRef = useRef<number | null>(null);

  useEffect(() => {
    const un = listen<DocAnalysisProgress>("doc-analysis-progress", (e) => {
      // ログは直近5件だけ見せれば十分（実行状況が伝わればよく、全量は不要）
      setAnalysisLog((l) => [...l.slice(-4), e.payload]);
    });
    return () => {
      un.then((f) => f());
      if (analysisPhaseRef.current === "running") {
        api.cancelDocAnalysis().catch(() => {});
      }
    };
  }, []);

  useEffect(() => {
    return () => {
      if (copyTimeoutRef.current !== null) {
        window.clearTimeout(copyTimeoutRef.current);
      }
    };
  }, []);

  const runAnalysis = () => {
    if (!path || !viewRef.current) return;
    const value = viewRef.current.state.doc.toString();
    cancelledRef.current = false;
    setAnalysisPhase("running");
    setAnalysisLog([]);
    setAnalysisResult(null);
    setAnalysisError(null);
    setAnalysisAlreadyRunning(false);
    api
      .analyzeDoc(path, value, projectDir)
      .then((result) => {
        setAnalysisResult(result);
        setAnalysisPhase("done");
      })
      .catch((e) => {
        if (cancelledRef.current) {
          setAnalysisError("キャンセルしました");
          setAnalysisAlreadyRunning(false);
        } else {
          const { alreadyRunning, message } = parseAlreadyRunning(e);
          setAnalysisError(message);
          setAnalysisAlreadyRunning(alreadyRunning);
        }
        setAnalysisPhase("error");
      });
  };

  const cancelAnalysis = () => {
    cancelledRef.current = true;
    api.cancelDocAnalysis().catch(() => {});
  };

  /** エラーパネルの「実行中の分析を中止」導線。すでに実行中の別の分析を止めるだけで、
   * 自分自身の analysisPhase は動かさない（そもそも自分は error のまま） */
  const cancelStuckAnalysis = () => {
    api.cancelDocAnalysis().catch(() => {});
    setAnalysisAlreadyRunning(false);
    setAnalysisError("実行中だった分析を中止しました。もう一度お試しください。");
  };

  const closeAnalysis = () => {
    setAnalysisPhase("idle");
    setAnalysisResult(null);
    setAnalysisError(null);
    setAnalysisAlreadyRunning(false);
  };

  const copyAnalysis = () => {
    if (!analysisResult) return;
    navigator.clipboard
      .writeText(analysisResult)
      .then(() => {
        setCopyLabel("コピーしました");
        copyTimeoutRef.current = window.setTimeout(() => setCopyLabel("コピー"), 1500);
      })
      .catch(() => {
        setCopyLabel("コピーに失敗しました");
        copyTimeoutRef.current = window.setTimeout(() => setCopyLabel("コピー"), 1500);
      });
  };

  const save = async () => {
    if (!viewRef.current || !path || readOnly || saving) return;
    const value = viewRef.current.state.doc.toString();
    setSaving(true);
    setSaveError(null);
    try {
      const doc = await api.writeDoc(path, value, epoch);
      setEpoch(doc.modified_epoch);
      setDirty(false);
      onDirtyChange?.(false);
    } catch (e) {
      const msg = String(e);
      if (msg.includes("conflict")) {
        setConflict(true);
      } else {
        setSaveError(msg);
      }
    } finally {
      setSaving(false);
    }
  };

  // キーマップ／エディタ生成の useEffect はマウント時に一度だけ束ねる（Cmd+S から
  // 常に最新の save を呼べるよう、実体は saveRef 経由で参照する）
  const saveRef = useRef(save);
  useEffect(() => {
    saveRef.current = save;
  });

  const discard = () => {
    if (!viewRef.current) return;
    viewRef.current.dispatch({
      changes: { from: 0, to: viewRef.current.state.doc.length, insert: content },
    });
    setDirty(false);
    setSaveError(null);
    onDirtyChange?.(false);
  };

  const reloadDiscardingLocal = async () => {
    if (!path || !viewRef.current) return;
    try {
      const doc = await api.readDoc(path);
      viewRef.current.dispatch({
        changes: { from: 0, to: viewRef.current.state.doc.length, insert: doc.content },
      });
      setEpoch(doc.modified_epoch);
      setConflict(false);
      setDirty(false);
      onDirtyChange?.(false);
    } catch (e) {
      setSaveError(String(e));
    }
  };

  useEffect(() => {
    if (!hostRef.current) return;

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        setDirty(true);
        onDirtyChange?.(true);
      }
    });
    const saveKeymap = keymap.of([
      {
        key: "Mod-s",
        preventDefault: true,
        run: () => {
          saveRef.current();
          return true;
        },
      },
    ]);

    const extensions = [
      basicSetup,
      EditorView.lineWrapping,
      saveKeymap,
      updateListener,
      EditorView.editable.of(!readOnly),
      EditorState.readOnly.of(readOnly),
      editorTheme,
    ];
    if (isMarkdownPath(path)) {
      extensions.push(markdown({ codeLanguages: languages }));
      extensions.push(syntaxHighlighting(markdownHighlight));
    }

    const view = new EditorView({
      state: EditorState.create({ doc: content, extensions }),
      parent: hostRef.current,
    });
    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 親が key で doc ごとに remount する前提
  }, []);

  return (
    <div className="doc-editor">
      {!readOnly && (
        <div className="doc-editor-toolbar">
          <span
            className={`doc-editor-dirty-dot ${dirty ? "is-dirty" : ""}`}
            title={dirty ? "未保存の変更があります" : "保存済み"}
          >
            ●
          </span>
          <span className="doc-editor-status">
            {saving ? "保存中…" : dirty ? "未保存の変更" : "保存済み"}
          </span>
          <div className="doc-editor-actions">
            <button
              className="doc-editor-btn"
              onClick={discard}
              disabled={!dirty || saving}
            >
              破棄
            </button>
            <button
              className="doc-editor-btn doc-editor-btn-primary"
              onClick={save}
              disabled={!dirty || saving}
            >
              保存
            </button>
            {canAnalyze && (
              <AiAnalysisButton
                analyzing={analysisPhase === "running"}
                onClick={runAnalysis}
              />
            )}
          </div>
        </div>
      )}
      {readOnly && (
        <div className="doc-editor-toolbar doc-editor-toolbar-readonly">
          <span className="doc-editor-status">
            {truncated
              ? "ファイルが大きいため読み取り専用です（切り詰められた内容のため保存・AI分析ともに不可）"
              : "このコンテンツは編集できません"}
          </span>
        </div>
      )}
      {conflict && (
        <div className="doc-editor-conflict">
          <p>別の場所で変更されています。このまま保存すると相手の変更が失われます。</p>
          <div className="doc-editor-conflict-actions">
            <button className="doc-editor-btn" onClick={reloadDiscardingLocal}>
              読み込み直す（自分の変更を破棄）
            </button>
            <button className="doc-editor-btn" onClick={() => setConflict(false)}>
              閉じる
            </button>
          </div>
        </div>
      )}
      {saveError && !conflict && <p className="doc-editor-error">{saveError}</p>}
      <div ref={hostRef} className="doc-editor-host" />
      {analysisPhase === "running" && (
        <div className="doc-editor-ai-panel">
          <div className="doc-editor-ai-panel-head">
            <span className="doc-editor-ai-spinner" />
            <span className="doc-editor-status">分析中…（数分かかることがあります）</span>
            <button className="doc-editor-btn" onClick={cancelAnalysis}>
              キャンセル
            </button>
          </div>
          <div className="doc-editor-ai-log">
            {analysisLog.map((l, i) => (
              <p key={i} className={`doc-editor-ai-log-line doc-editor-ai-log-${l.kind}`}>
                {l.label}
              </p>
            ))}
          </div>
        </div>
      )}
      {analysisPhase === "error" && (
        <div className="doc-editor-ai-panel">
          <div className="doc-editor-ai-panel-head">
            <span className="doc-editor-status">
              {cancelledRef.current ? "AI分析をキャンセルしました" : "AI分析に失敗しました"}
            </span>
            <div className="doc-editor-actions">
              {analysisAlreadyRunning && (
                <button className="doc-editor-btn" onClick={cancelStuckAnalysis}>
                  実行中の分析を中止
                </button>
              )}
              <button className="doc-editor-btn" onClick={closeAnalysis}>
                閉じる
              </button>
            </div>
          </div>
          <div className="doc-editor-ai-body">
            <p className="doc-editor-error">{analysisError}</p>
          </div>
        </div>
      )}
      {analysisPhase === "done" && analysisResult && (
        <div className="doc-editor-ai-panel">
          <div className="doc-editor-ai-panel-head">
            <span className="doc-editor-status">AI改善提案</span>
            <div className="doc-editor-actions">
              <button className="doc-editor-btn" onClick={copyAnalysis}>
                {copyLabel}
              </button>
              <button className="doc-editor-btn" onClick={closeAnalysis}>
                ✕
              </button>
            </div>
          </div>
          <div className="doc-editor-ai-body">
            <pre className="doc-editor-ai-result">{analysisResult}</pre>
          </div>
        </div>
      )}
    </div>
  );
}

/** ツールバー右端に置く AI 分析トリガー。星型の✦アイコンで既存ボタンのトーンに合わせる */
function AiAnalysisButton({
  analyzing,
  onClick,
}: {
  analyzing: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className="doc-editor-btn doc-editor-ai-btn"
      title="AIに分析・改善してもらう"
      onClick={onClick}
      disabled={analyzing}
    >
      {analyzing ? (
        <span className="doc-editor-ai-spinner" />
      ) : (
        <svg
          width="13"
          height="13"
          viewBox="0 0 24 24"
          fill="currentColor"
          aria-hidden="true"
        >
          <path d="M12 2l1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8L12 2z" />
          <path d="M19 14l0.7 2.3L22 17l-2.3 0.7L19 20l-0.7-2.3L16 17l2.3-0.7L19 14z" />
        </svg>
      )}
    </button>
  );
}
