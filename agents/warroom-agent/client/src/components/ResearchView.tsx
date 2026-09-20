import React from "react";
import { CompetitorResearch } from "../types";

interface ResearchViewProps {
  research: CompetitorResearch[];
}

export const ResearchView: React.FC<ResearchViewProps> = ({ research }) => {
  const getCompClass = (comp: string) => {
    const c = comp.toLowerCase();
    if (c.includes("cashfree")) return "cf";
    if (c.includes("razorpay")) return "rz";
    return "pu";
  };

  const totalResults = research.reduce((acc, r) => acc + r.results.length, 0);

  return (
    <div className="research-page">
      <div className="page-header">
        <div>
          <h1 className="page-title">Live Research</h1>
          <p className="page-subtitle">
            Verified public web evidence and snippets retrieved via Anakin.io live search engine.
          </p>
        </div>
        <span className="badge product" style={{ padding: "6px 12px", fontSize: "0.82rem" }}>
          {totalResults} Verified Sources Across 3 Competitors
        </span>
      </div>

      {research.length === 0 ? (
        <div className="empty-state">
          <div className="empty-icon">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="11" cy="11" r="8" />
              <line x1="21" y1="21" x2="16.65" y2="16.65" />
            </svg>
          </div>
          <h2 className="empty-title">No research collected yet</h2>
          <p className="empty-desc">
            Initiate a landscape scan to trigger real-time search across monitored competitors.
          </p>
        </div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 28 }}>
          {research.map((compGroup, gIdx) => {
            const compClass = getCompClass(compGroup.competitor);

            return (
              <div key={gIdx} className="panel-card" style={{ padding: 24 }}>
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 18, borderBottom: "1px solid var(--border-color)", paddingBottom: 12 }}>
                  <div className="comp-avatar-tag">
                    <span className={`comp-avatar ${compClass}`} style={{ width: 28, height: 28, fontSize: "0.8rem" }}>
                      {compGroup.competitor.slice(0, 2).toUpperCase()}
                    </span>
                    <span style={{ fontSize: "1.1rem", fontWeight: 700 }}>
                      {compGroup.competitor} Research
                    </span>
                  </div>
                  <span style={{ fontSize: "0.8rem", color: "var(--text-muted)", fontWeight: 600 }}>
                    {compGroup.results.length} Primary Sources
                  </span>
                </div>

                <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
                  {compGroup.results.map((res, rIdx) => (
                    <div
                      key={rIdx}
                      style={{
                        padding: "16px 18px",
                        backgroundColor: "#fafbfc",
                        border: "1px solid var(--border-light)",
                        borderRadius: "var(--radius-md)",
                        display: "flex",
                        flexDirection: "column",
                        gap: 8,
                      }}
                    >
                      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", gap: 12 }}>
                        <div style={{ fontWeight: 700, fontSize: "0.94rem", color: "var(--text-main)" }}>
                          {res.title}
                        </div>
                        {res.date && (
                          <span style={{ fontSize: "0.75rem", color: "var(--text-muted)", whiteSpace: "nowrap" }}>
                            {res.date}
                          </span>
                        )}
                      </div>

                      <div style={{ fontSize: "0.84rem", color: "var(--text-secondary)", lineHeight: 1.55 }}>
                        "{res.snippet}"
                      </div>

                      <div style={{ marginTop: 4 }}>
                        <a
                          href={res.url}
                          target="_blank"
                          rel="noreferrer"
                          style={{
                            fontSize: "0.8rem",
                            color: "var(--primary)",
                            display: "inline-flex",
                            alignItems: "center",
                            gap: 6,
                            textDecoration: "none",
                            wordBreak: "break-all",
                          }}
                        >
                          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                            <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                            <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                          </svg>
                          {res.url}
                        </a>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
