import React, { useState } from "react";
import { AskWarroomResponse, CompetitiveSignal } from "../types";
import { askWarroom } from "../api";

interface AskWarroomViewProps {
  signals: CompetitiveSignal[];
}

export const AskWarroomView: React.FC<AskWarroomViewProps> = ({ signals }) => {
  const [question, setQuestion] = useState("Why does Cashfree's RiskShield matter to PayFlow?");
  const [isLoading, setIsLoading] = useState(false);
  const [result, setResult] = useState<AskWarroomResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const starterQuestions = [
    "Why does Cashfree's RiskShield matter to PayFlow?",
    "What are the most important competitive signals across the current landscape?",
    "How does Razorpay's fraud prevention compare to PayFlow's FraudShield?",
  ];

  const handleAsk = async (qToAsk?: string) => {
    const q = (qToAsk || question).trim();
    if (!q) return;

    setIsLoading(true);
    setError(null);
    try {
      const resp = await askWarroom(q, signals);
      setResult(resp);
    } catch (err: any) {
      setError(err.message || "Failed to query Ask WARROOM.");
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <div className="ask-page">
      <div className="page-header">
        <div>
          <h1 className="page-title">Ask WARROOM</h1>
          <p className="page-subtitle">
            Evidence-grounded competitive intelligence reasoning engine powered by DronaHQ.
          </p>
        </div>
      </div>

      <div className="ask-container">
        {/* Input Card */}
        <div className="ask-input-box">
          <div style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--text-muted)", fontSize: "0.78rem", fontWeight: 700, textTransform: "uppercase" }}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="11" cy="11" r="8" />
              <line x1="21" y1="21" x2="16.65" y2="16.65" />
            </svg>
            Competitive Query
          </div>

          <textarea
            className="ask-textarea"
            placeholder="Ask anything about current competitive signals, threats, or overlap..."
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            rows={3}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                handleAsk();
              }
            }}
          />

          <div className="ask-actions">
            <div className="starter-chips">
              <span style={{ fontSize: "0.72rem", color: "var(--text-muted)", fontWeight: 600 }}>Suggested:</span>
              {starterQuestions.map((sq, i) => (
                <button
                  key={i}
                  type="button"
                  className="starter-chip"
                  onClick={() => {
                    setQuestion(sq);
                    handleAsk(sq);
                  }}
                >
                  {sq}
                </button>
              ))}
            </div>

            <button
              className="primary-btn"
              disabled={isLoading || !question.trim()}
              onClick={() => handleAsk()}
            >
              {isLoading ? (
                <>
                  <span className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />
                  Synthesizing...
                </>
              ) : (
                <>
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                    <line x1="22" y1="2" x2="11" y2="13" />
                    <polygon points="22 2 15 22 11 13 2 9 22 2" />
                  </svg>
                  Ask WARROOM
                </>
              )}
            </button>
          </div>
        </div>

        {error && (
          <div style={{ padding: "14px 18px", backgroundColor: "var(--danger-light)", color: "var(--danger)", borderRadius: "var(--radius-md)", fontSize: "0.85rem", border: "1px solid var(--danger-border)" }}>
            {error}
          </div>
        )}

        {/* Intelligence Report Result */}
        {result && (
          <div className="ask-result-card">
            {/* Header / Meta */}
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", borderBottom: "1px solid var(--border-light)", paddingBottom: 14 }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span className="badge product" style={{ fontWeight: 700 }}>
                  WARROOM INTELLIGENCE REPORT
                </span>
                <span className="metric-badge green">
                  Confidence: {result.confidence.toUpperCase()}
                </span>
              </div>
              <span style={{ fontSize: "0.78rem", color: "var(--text-muted)" }}>
                Grounding: {result.evidence.source_urls.length} Verified Sources
              </span>
            </div>

            {/* Answer */}
            <div>
              <div className="field-label" style={{ marginBottom: 6 }}>
                Strategic Synthesis
              </div>
              <div className="ask-answer">
                {result.answer}
              </div>
            </div>

            {/* Key Points */}
            {result.key_points && result.key_points.length > 0 && (
              <div>
                <div className="field-label" style={{ marginBottom: 8 }}>
                  Key Takeaways & Recommended Investigations ({result.key_points.length})
                </div>
                <ul className="sim-bullet-list">
                  {result.key_points.map((point, i) => (
                    <li key={i} className="sim-bullet-item" style={{ fontSize: "0.88rem" }}>
                      {point}
                    </li>
                  ))}
                </ul>
              </div>
            )}

            {/* Relevant Signals */}
            {result.relevant_signals && result.relevant_signals.length > 0 && (
              <div>
                <div className="field-label" style={{ marginBottom: 8 }}>
                  Mapped Competitive Signals
                </div>
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                  {result.relevant_signals.map((rs, i) => (
                    <div
                      key={i}
                      className="panel-card"
                      style={{ padding: "8px 12px", display: "flex", alignItems: "center", gap: 8, fontSize: "0.8rem" }}
                    >
                      <span className="badge product">{rs.competitor}</span>
                      <span style={{ fontWeight: 600 }}>{rs.headline || "Activity"}</span>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Evidence URLs */}
            {result.evidence.source_urls.length > 0 && (
              <div>
                <div className="field-label" style={{ marginBottom: 8 }}>
                  Grounded Evidence Sources ({result.evidence.source_urls.length})
                </div>
                <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  {result.evidence.source_urls.map((url, i) => (
                    <a
                      key={i}
                      href={url}
                      target="_blank"
                      rel="noreferrer"
                      style={{ fontSize: "0.8rem", color: "var(--primary)", display: "flex", alignItems: "center", gap: 6 }}
                    >
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                        <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                      </svg>
                      {url}
                    </a>
                  ))}
                </div>
              </div>
            )}

            {/* Limitations / Caveats */}
            {result.limitations && result.limitations.length > 0 && (
              <div className="sim-caveat" style={{ margin: 0 }}>
                <strong>Scope & Limitations:</strong> {result.limitations.join(" ")}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
