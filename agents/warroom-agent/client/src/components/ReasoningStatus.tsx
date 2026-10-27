import React, { useState } from "react";
import { DronaHQRun } from "../types";

interface ReasoningStatusProps {
  run: DronaHQRun;
}

export const ReasoningStatus: React.FC<ReasoningStatusProps> = ({ run }) => {
  const [showDetails, setShowDetails] = useState(false);

  return (
    <section className="reasoning-panel" aria-label="WARROOM Reasoning Status">
      <div className="section-header">
        <h2 className="section-title">WARROOM REASONING</h2>
        <span style={{ fontFamily: "var(--font-mono)", fontSize: "0.75rem", color: "var(--text-muted)" }}>
          DRONAHQ ENGINE
        </span>
      </div>

      <div className="reasoning-grid">
        <div className="reasoning-item">
          <span className="reasoning-label">RESEARCH LAYER</span>
          <div className="status-badge-collected">
            <span>✓</span>
            <span>COLLECTED</span>
          </div>
        </div>

        <div className="reasoning-item">
          <span className="reasoning-label">AI ANALYSIS PIPELINE</span>
          <div className="status-badge-pending">
            <div className="status-dot" style={{ backgroundColor: "var(--accent-amber)", boxShadow: "none" }} />
            <span>{run.status.toUpperCase()}</span>
          </div>
        </div>
      </div>

      {(run.thread_id || run.run_id) && (
        <div className="reasoning-debug-details">
          <button
            className="debug-toggle"
            onClick={() => setShowDetails(!showDetails)}
            aria-expanded={showDetails}
          >
            <span>{showDetails ? "▼" : "▶"}</span>
            <span>DronaHQ Agent Trace Details</span>
          </button>

          {showDetails && (
            <div className="debug-content">
              {run.thread_id && (
                <div className="debug-row">
                  <span className="debug-label">Thread ID:</span>
                  <span className="debug-val">{run.thread_id}</span>
                </div>
              )}
              {run.run_id && (
                <div className="debug-row">
                  <span className="debug-label">Run ID:</span>
                  <span className="debug-val">{run.run_id}</span>
                </div>
              )}
              {run.message && (
                <div className="debug-row">
                  <span className="debug-label">Message:</span>
                  <span className="debug-val">{run.message}</span>
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
};
