import { useEffect, useRef, useState } from "react";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { markdown } from "@codemirror/lang-markdown";
import { languages } from "@codemirror/language-data";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";
import { api } from "./api";

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
  /** 未保存変更の有無を親（ドロワーの離脱ガード）に伝える */
  onDirtyChange?: (dirty: boolean) => void;
}

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
  onDirtyChange,
}: DocEditorProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const readOnly = path === null || truncated;

  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [epoch, setEpoch] = useState<number | null>(modifiedEpoch);

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
          </div>
        </div>
      )}
      {readOnly && (
        <div className="doc-editor-toolbar doc-editor-toolbar-readonly">
          <span className="doc-editor-status">
            {truncated
              ? "ファイルが大きいため読み取り専用です（切り詰められた内容のため保存不可）"
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
    </div>
  );
}
