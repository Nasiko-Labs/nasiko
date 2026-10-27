import React, { useState, useEffect } from "react";
import { CompetitiveSignal, ResponseSimulation } from "../types";
import { simulateSignal } from "../api";

interface ResponsePlansViewProps {
  signals: CompetitiveSignal[];
  selectedSignal: CompetitiveSignal | null;
  onSelectSignal: (signal: CompetitiveSignal) => void;
}

export const ResponsePlansView: React.FC<ResponsePlansViewProps> = ({
  signals,
  selectedSignal,
  onSelectSignal,
}) => {
  const [currentSignal, setCurrentSignal] = useState<CompetitiveSignal | null>(
    selectedSignal || (signals.length > 0 ? signals[0] : null)
  );
  const [simulation, setSimulation] = useState<ResponseSimulation | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (selectedSignal) {
      setCurrentSignal(selectedSignal);
    } else if (!currentSignal && signals.length > 0) {
      setCurrentSignal(signals[0]);
    }
  }, [selectedSignal, signals]);

  const handleSimulate = async (sigToSimulate?: CompetitiveSignal) => {
    const target = sigToSimulate || currentSignal;
    if (!target) return;

    setIsLoading(true);
    setError(null);
    try {
      const sim = await simulateSignal(target);
      setSimulation(sim);
    } catch (err: any) {
      setError(err.message || "Failed to generate response simulation.");
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <div className="response-plans-page">
      <div className="page-header">
        <div>
          <h1 className="page-title">Response Plans</h1>
          <p className="page-subtitle">
            AI strategic decision-support simulations across Product, Sales, Marketing, and Strategy.
          </p>
        </div>
      </div>

      {/* Signal Selection Box */}
      <div className="panel-card" style={{ padding: "18px 24px", marginBottom: 24 }}>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: 16 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 12, flex: 1, minWidth: 280 }}>
            <span style={{ fontSize: "0.82rem", fontWeight: 700, color: "var(--text-secondary)", whiteSpace: "nowrap" }}>
              Selected Signal:
            </span>
            {signals.length === 0 ? (
              <span style={{ fontSize: "0.85rem", color: "var(--text-muted)" }}>
                No signals available. Please run a landscape scan first.
              </span>
            ) : (
              <select
                className="filter-select"
                style={{ flex: 1, maxWidth: 500 }}
                value={currentSignal ? `${currentSignal.competitor} - ${currentSignal.headline}` : ""}
                onChange={(e) => {
                  const match = signals.find(
                    (s) => `${s.competitor} - ${s.headline}` === e.target.value
                  );
                  if (match) {
                    setCurrentSignal(match);
                    onSelectSignal(match);
                    setSimulation(null);
                  }
                }}
              >
                {signals.map((s, idx) => (
                  <option key={idx} value={`${s.competitor} - ${s.headline}`}>
                    [{s.competitor}] {s.headline} ({s.significance || "Informational"})
                  </option>
                ))}
              </select>
            )}
          </div>

          <button
            className="primary-btn"
            disabled={!currentSignal || isLoading}
            onClick={() => handleSimulate()}
          >
            {isLoading ? (
              <>
                <span className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />
                Simulating Plan...
              </>
            ) : (
              <>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
                  <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
                </svg>
                Generate Response Plan
              </>
            )}
          </button>
        </div>

        {currentSignal && (
          <div style={{ marginTop: 14, paddingTop: 14, borderTop: "1px solid var(--border-light)", fontSize: "0.84rem", color: "var(--text-secondary)" }}>
            <strong>Signal Context:</strong> {currentSignal.summary || currentSignal.headline}
          </div>
        )}
      </div>

      {error && (
        <div style={{ padding: "14px 18px", backgroundColor: "var(--danger-light)", color: "var(--danger)", borderRadius: "var(--radius-md)", marginBottom: 20, fontSize: "0.85rem", border: "1px solid var(--danger-border)" }}>
          {error}
        </div>
      )}

      {/* 4 Pillars Display */}
      {simulation ? (
        <div style={{ display: "flex", flexDirection: "column", gap: 24 }}>
          {/* Signal context banner */}
          <div className="panel-card" style={{ padding: "16px 20px", backgroundColor: "#f8fafc", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
            <div>
              <span className="badge product" style={{ marginRight: 8 }}>
                {simulation.signal_context.competitor}
              </span>
              <span style={{ fontWeight: 700, fontSize: "0.95rem" }}>
                {simulation.signal_context.headline}
              </span>
            </div>
            {simulation.confidence && (
              <span className="metric-badge green">
                Confidence: {simulation.confidence.toUpperCase()}
              </span>
            )}
          </div>

          <div className="sim-grid">
            {/* 1. Product Pillar */}
            <div className="sim-pillar">
              <div className="sim-pillar-header">
                <span className="badge product">PRODUCT</span>
                <h3 className="sim-pillar-title">Product Investigation</h3>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                <div>
                  <div className="field-label">Recommended Actions</div>
                  <ul className="sim-bullet-list" style={{ marginTop: 6 }}>
                    {simulation.product.actions.map((act, i) => (
                      <li key={i} className="sim-bullet-item">{act}</li>
                    ))}
                  </ul>
                </div>
                {simulation.product.investigation_questions.length > 0 && (
                  <div>
                    <div className="field-label">Investigation Questions</div>
                    <ul className="sim-bullet-list" style={{ marginTop: 6 }}>
                      {simulation.product.investigation_questions.map((q, i) => (
                        <li key={i} className="sim-bullet-item">{q}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {simulation.product.caveats.length > 0 && (
                  <div className="sim-caveat">
                    <strong>Caveats:</strong> {simulation.product.caveats.join(" ")}
                  </div>
                )}
              </div>
            </div>

            {/* 2. Sales Pillar */}
            <div className="sim-pillar">
              <div className="sim-pillar-header">
                <span className="badge sales">SALES</span>
                <h3 className="sim-pillar-title">Sales Battlecard</h3>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                {simulation.sales.trigger && (
                  <div>
                    <div className="field-label">When a Prospect Says</div>
                    <div style={{ fontSize: "0.85rem", fontStyle: "italic", color: "var(--text-main)", marginTop: 4, background: "#fff1f2", padding: "8px 12px", borderRadius: "var(--radius-sm)" }}>
                      "{simulation.sales.trigger}"
                    </div>
                  </div>
                )}
                {simulation.sales.talk_track && (
                  <div>
                    <div className="field-label">Recommended Talk Track</div>
                    <div style={{ fontSize: "0.85rem", color: "var(--text-secondary)", marginTop: 4, lineHeight: 1.5 }}>
                      {simulation.sales.talk_track}
                    </div>
                  </div>
                )}
                {simulation.sales.questions.length > 0 && (
                  <div>
                    <div className="field-label">Discovery Questions</div>
                    <ul className="sim-bullet-list" style={{ marginTop: 6 }}>
                      {simulation.sales.questions.map((q, i) => (
                        <li key={i} className="sim-bullet-item">{q}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {simulation.sales.caveats.length > 0 && (
                  <div className="sim-caveat">
                    <strong>Caveats:</strong> {simulation.sales.caveats.join(" ")}
                  </div>
                )}
              </div>
            </div>

            {/* 3. Marketing Pillar */}
            <div className="sim-pillar">
              <div className="sim-pillar-header">
                <span className="badge marketing">MARKETING</span>
                <h3 className="sim-pillar-title">Positioning & Messaging</h3>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                {simulation.marketing.actions.length > 0 && (
                  <div>
                    <div className="field-label">Marketing Actions</div>
                    <ul className="sim-bullet-list" style={{ marginTop: 6 }}>
                      {simulation.marketing.actions.map((act, i) => (
                        <li key={i} className="sim-bullet-item">{act}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {simulation.marketing.messaging_angles.length > 0 && (
                  <div>
                    <div className="field-label">Positioning Angles</div>
                    <ul className="sim-bullet-list" style={{ marginTop: 6 }}>
                      {simulation.marketing.messaging_angles.map((ang, i) => (
                        <li key={i} className="sim-bullet-item">{ang}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {simulation.marketing.caveats.length > 0 && (
                  <div className="sim-caveat">
                    <strong>Caveats:</strong> {simulation.marketing.caveats.join(" ")}
                  </div>
                )}
              </div>
            </div>

            {/* 4. Strategy Pillar */}
            <div className="sim-pillar">
              <div className="sim-pillar-header">
                <span className="badge strategy">STRATEGY</span>
                <h3 className="sim-pillar-title">Leadership Inquiries</h3>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                {simulation.strategy.questions.length > 0 && (
                  <div>
                    <div className="field-label">Strategic Questions for Leadership</div>
                    <ul className="sim-bullet-list" style={{ marginTop: 6 }}>
                      {simulation.strategy.questions.map((q, i) => (
                        <li key={i} className="sim-bullet-item">{q}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {simulation.strategy.actions.length > 0 && (
                  <div>
                    <div className="field-label">Strategic Moves</div>
                    <ul className="sim-bullet-list" style={{ marginTop: 6 }}>
                      {simulation.strategy.actions.map((act, i) => (
                        <li key={i} className="sim-bullet-item">{act}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {simulation.strategy.caveats.length > 0 && (
                  <div className="sim-caveat">
                    <strong>Caveats:</strong> {simulation.strategy.caveats.join(" ")}
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* Evidence Grounding */}
          <div className="panel-card" style={{ padding: "18px 22px" }}>
            <div className="field-label" style={{ marginBottom: 8 }}>
              Verified Source Evidence Grounding
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {simulation.evidence.source_urls.length > 0 ? (
                simulation.evidence.source_urls.map((url, i) => (
                  <a
                    key={i}
                    href={url}
                    target="_blank"
                    rel="noreferrer"
                    style={{ fontSize: "0.82rem", color: "var(--primary)", display: "flex", alignItems: "center", gap: 6 }}
                  >
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                      <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                    </svg>
                    {url}
                  </a>
                ))
              ) : (
                <span style={{ fontSize: "0.82rem", color: "var(--text-muted)" }}>
                  Grounding verified from active scan context
                </span>
              )}
            </div>
          </div>
        </div>
      ) : (
        <div className="empty-state">
          <div className="empty-icon">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
            </svg>
          </div>
          <h2 className="empty-title">Ready to simulate competitive response</h2>
          <p className="empty-desc">
            Select an active competitor signal above and click "Generate Response Plan" to simulate multi-department impact and talk tracks.
          </p>
        </div>
      )}
    </div>
  );
};
