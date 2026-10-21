import React from "react";
import { CompetitiveSignal } from "../types";

interface SignalDetailModalProps {
  signal: CompetitiveSignal | null;
  onClose: () => void;
  onSimulate: (signal: CompetitiveSignal) => void;
}

export const SignalDetailModal: React.FC<SignalDetailModalProps> = ({
  signal,
  onClose,
  onSimulate,
}) => {
  if (!signal) return null;

  const severityClass = (signal.significance || "informational").toLowerCase();
  const compClass =
    signal.competitor.toLowerCase().includes("cashfree") ? "cf" :
    signal.competitor.toLowerCase().includes("razorpay") ? "rz" : "pu";

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-content" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span className={`comp-avatar ${compClass}`}>
              {signal.competitor.slice(0, 2).toUpperCase()}
            </span>
            <div>
              <div className="modal-title">{signal.competitor} Signal Detail</div>
              <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 4 }}>
                <span className={`badge ${severityClass}`}>
                  {signal.significance || "INFORMATIONAL"}
                </span>
                {signal.category && (
                  <span className="badge gray">
                    {signal.category.toUpperCase()}
                  </span>
                )}
                {signal.confidence && (
                  <span className="badge gray">
                    Confidence: {signal.confidence.toUpperCase()}
                  </span>
                )}
              </div>
            </div>
          </div>
          <button className="icon-btn" onClick={onClose} title="Close">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
        </div>

        <div className="modal-body">
          {/* Headline & Summary */}
          <div className="modal-field">
            <span className="field-label">Development Headline</span>
            <div className="field-value" style={{ fontWeight: 600, fontSize: "1rem" }}>
              {signal.headline || "Competitor Update"}
            </div>
          </div>

          <div className="modal-field">
            <span className="field-label">Signal Summary & Strategic Significance</span>
            <div className="field-value" style={{ lineHeight: 1.6, color: "var(--text-secondary)" }}>
              {signal.summary || "No detailed summary provided."}
            </div>
          </div>

          {/* Product & Customer Overlap */}
          {signal.overlap && (
            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>
              <div className="modal-field">
                <span className="field-label">Affected Internal Products</span>
                <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 4 }}>
                  {signal.overlap.products.length > 0 ? (
                    signal.overlap.products.map((p, i) => (
                      <span key={i} className="badge product">{p}</span>
                    ))
                  ) : (
                    <span style={{ fontSize: "0.85rem", color: "var(--text-muted)" }}>General Portfolio</span>
                  )}
                </div>
              </div>

              <div className="modal-field">
                <span className="field-label">Target Customer Segments</span>
                <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 4 }}>
                  {signal.overlap.customer_segments.length > 0 ? (
                    signal.overlap.customer_segments.map((s, i) => (
                      <span key={i} className="badge gray">{s}</span>
                    ))
                  ) : (
                    <span style={{ fontSize: "0.85rem", color: "var(--text-muted)" }}>All Segments</span>
                  )}
                </div>
              </div>
            </div>
          )}

          {/* Departmental Impacts */}
          {signal.impact && (
            <div className="modal-field">
              <span className="field-label">Cross-Department Impact Assessment</span>
              <div className="impact-grid" style={{ marginTop: 6 }}>
                <div className="impact-box">
                  <div className="impact-dept">Product</div>
                  <div className={`impact-val ${(signal.impact.product || "LOW").toLowerCase()}`}>
                    {signal.impact.product || "LOW"}
                  </div>
                </div>
                <div className="impact-box">
                  <div className="impact-dept">Sales</div>
                  <div className={`impact-val ${(signal.impact.sales || "LOW").toLowerCase()}`}>
                    {signal.impact.sales || "LOW"}
                  </div>
                </div>
                <div className="impact-box">
                  <div className="impact-dept">Marketing</div>
                  <div className={`impact-val ${(signal.impact.marketing || "LOW").toLowerCase()}`}>
                    {signal.impact.marketing || "LOW"}
                  </div>
                </div>
                <div className="impact-box">
                  <div className="impact-dept">Strategy</div>
                  <div className={`impact-val ${(signal.impact.strategy || "LOW").toLowerCase()}`}>
                    {signal.impact.strategy || "LOW"}
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* Recommended Actions */}
          {signal.recommended_actions && signal.recommended_actions.length > 0 && (
            <div className="modal-field">
              <span className="field-label">Actionable Recommendations & Battlecards</span>
              <ul style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 6, paddingLeft: 18 }}>
                {signal.recommended_actions.map((act, i) => (
                  <li key={i} style={{ fontSize: "0.86rem", color: "var(--text-secondary)", lineHeight: 1.5 }}>
                    {act}
                  </li>
                ))}
              </ul>
            </div>
          )}

          {/* Verified Evidence URLs */}
          <div className="modal-field">
            <span className="field-label">Grounding Evidence ({signal.source_urls.length} Sources)</span>
            <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 6 }}>
              {signal.source_urls.length > 0 ? (
                signal.source_urls.map((url, i) => (
                  <a
                    key={i}
                    href={url}
                    target="_blank"
                    rel="noreferrer"
                    style={{
                      fontSize: "0.82rem",
                      color: "var(--primary)",
                      textDecoration: "none",
                      display: "flex",
                      alignItems: "center",
                      gap: 6,
                      wordBreak: "break-all",
                    }}
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
                  Extracted from market intelligence synthesis
                </span>
              )}
            </div>
          </div>
        </div>

        <div className="modal-footer">
          <button className="secondary-btn" onClick={onClose}>
            Close
          </button>
          <button
            className="primary-btn"
            onClick={() => {
              onClose();
              onSimulate(signal);
            }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
              <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
            </svg>
            Simulate Response Plan
          </button>
        </div>
      </div>
    </div>
  );
};
