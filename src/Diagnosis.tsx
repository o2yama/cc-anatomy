import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, DiagnosisFinding, DiagnosisProgress, DiagnosisReport } from "./api";

type Phase = "idle" | "running" | "done" | "error";

const SEVERITY_ORDER: DiagnosisFinding["severity"][] = ["high", "medium", "low"];
const SEVERITY_LABEL: Record<string, string> = {
  high: "要対処",
  medium: "近いうちに",
  low: "お好みで",
};

/**
 * 環境診断ビュー。open=false でも DOM から消すだけでアンマウントはしない
 * （実行中に閉じても診断を継続し、再度開いたときに結果を受け取れるようにする）
 */
export function DiagnosisOverlay({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [log, setLog] = useState<DiagnosisProgress[]>([]);
  const [report, setReport] = useState<DiagnosisReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const [launched, setLaunched] = useState(false);
  const logEndRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const un = listen<DiagnosisProgress>("diagnosis-progress", (e) => {
      // ログは直近200件だけ保持して DOM の肥大化を防ぐ
      setLog((l) => [...l.slice(-199), e.payload]);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [log]);

  const start = () => {
    setPhase("running");
    setLog([]);
    setReport(null);
    setError(null);
    setChecked(new Set());
    setConfirming(false);
    setLaunched(false);
    api
      .runDiagnosis()
      .then((r) => {
        setReport(r);
        // 「要対処」だけ初期選択にして、押すだけで最低限の改善が回る状態にする
        setChecked(
          new Set(r.findings.filter((f) => f.severity === "high").map((f) => f.id))
        );
        setPhase("done");
      })
      .catch((e) => {
        setError(String(e));
        setPhase("error");
      });
  };

  const cancel = () => {
    api.cancelDiagnosis().catch(() => {});
  };

  const toggle = (id: string) => {
    setConfirming(false);
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const runFixes = () => {
    if (!report) return;
    const prompts = report.findings
      .filter((f) => checked.has(f.id))
      .map((f) => f.fix_prompt);
    api
      .runFixesInTerminal(prompts)
      .then(() => {
        setConfirming(false);
        setLaunched(true);
      })
      .catch((e) => {
        setError(String(e));
        setConfirming(false);
      });
  };

  if (!open) return null;

  return (
    <div className="drawer-overlay" onClick={onClose}>
      <div className="diagnosis-panel" onClick={(e) => e.stopPropagation()}>
        <div className="drawer-head">
          <div>
            <h2>環境診断</h2>
            <p className="muted">
              ローカルの Claude Code アカウントで PC 環境全体をスキャンします
            </p>
          </div>
          <button className="close-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="drawer-body">
          {phase === "idle" && (
            <div className="diag-intro">
              <p>
                ホームディレクトリ全体を<strong>読み取り専用</strong>
                でスキャンし、改善ポイントをレポートします。
              </p>
              <ul className="diag-points muted">
                <li>ホーム直下の散らかり / Desktop・Downloads の堆積</li>
                <li>巨大な node_modules・キャッシュなどのディスク大食い</li>
                <li>未コミット・未プッシュのまま放置された Git リポジトリ</li>
                <li>開発環境の残骸・健全性</li>
              </ul>
              <p className="diag-note muted">
                所要時間は数分〜10分程度。ログイン中プランのレートリミットを消費します。
                診断中もファイルは一切変更しません。
              </p>
              <button className="diag-primary" onClick={start}>
                診断を開始
              </button>
            </div>
          )}

          {phase === "running" && (
            <div className="diag-running">
              <p className="diag-status">
                <span className="diag-spinner" />
                スキャン中…（数分かかります。閉じても診断は続きます）
              </p>
              <div className="diag-log">
                {log.map((l, i) => (
                  <p key={i} className={`diag-log-line diag-log-${l.kind}`}>
                    {l.label}
                  </p>
                ))}
                <div ref={logEndRef} />
              </div>
              <button className="clear-btn" onClick={cancel}>
                キャンセル
              </button>
            </div>
          )}

          {phase === "error" && (
            <div className="diag-intro">
              <div className="error-box">{error}</div>
              <button className="diag-primary" onClick={start}>
                再試行
              </button>
            </div>
          )}

          {phase === "done" && report && (
            <div className="diag-report">
              <p className="diag-summary">{report.summary}</p>
              {report.findings.length === 0 && (
                <p className="muted">改善ポイントは見つかりませんでした 🎉</p>
              )}
              {SEVERITY_ORDER.map((sev) => {
                const items = report.findings.filter((f) => f.severity === sev);
                if (items.length === 0) return null;
                return (
                  <section key={sev} className="diag-group">
                    <h3 className={`diag-sev diag-sev-${sev}`}>
                      {SEVERITY_LABEL[sev]}（{items.length}）
                    </h3>
                    {items.map((f) => (
                      <label key={f.id} className="diag-finding">
                        <input
                          type="checkbox"
                          checked={checked.has(f.id)}
                          onChange={() => toggle(f.id)}
                        />
                        <div>
                          <p className="diag-finding-title">
                            <span className="count-badge">{f.category}</span>
                            {f.title}
                          </p>
                          <p className="muted">{f.detail}</p>
                          {f.target_paths.length > 0 && (
                            <p className="diag-paths">
                              {f.target_paths.join(" · ")}
                            </p>
                          )}
                        </div>
                      </label>
                    ))}
                  </section>
                );
              })}
              {/* Terminal 起動失敗（オートメーション権限拒否など）はここに出す */}
              {error && <div className="error-box">{error}</div>}
              {report.findings.length > 0 && (
                <div className="diag-actions">
                  {launched ? (
                    <p className="muted">
                      ターミナルで修正を実行中です。完了後にもう一度診断すると効果を確認できます。
                    </p>
                  ) : confirming ? (
                    <>
                      <span className="muted">
                        Terminal.app が開き、選択した{checked.size}
                        件の修正を claude が実行します。よろしいですか？
                      </span>
                      <button className="diag-primary" onClick={runFixes}>
                        実行する
                      </button>
                      <button
                        className="clear-btn"
                        onClick={() => setConfirming(false)}
                      >
                        やめる
                      </button>
                    </>
                  ) : (
                    <>
                      <button
                        className="diag-primary"
                        disabled={checked.size === 0}
                        onClick={() => {
                          setError(null);
                          setConfirming(true);
                        }}
                      >
                        選択した{checked.size}件をターミナルで修正
                      </button>
                      <button className="clear-btn" onClick={start}>
                        再診断
                      </button>
                    </>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
