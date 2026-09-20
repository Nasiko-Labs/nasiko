import React from "react";
import { CompetitiveSignal, CompetitorResearch } from "../types";

interface EvidenceViewProps {
  signals: CompetitiveSignal[];
  research: CompetitorResearch[];
  onSelectSignal: (signal: CompetitiveSignal) => void;
}

export const EvidenceView: React.FC<EvidenceViewProps> = ({
  signals,
  research,
  onSelectSignal,
}) => {
  // Build flattened evidence rows from real data
  const evidenceItems: {
    competitor: string;
    title: string;
    url: string;
    domain: string;
    relatedSignal?: CompetitiveSignal;
    snippet?: string;
  }[] = [];

  research.forEach((r) => {
    r.results.forEach((res) => {
      let domain = "";
      try {
        domain = new URL(res.url).hostname.replace("www.", "");
      } catch {
        domain = "web";
      }

      const matchingSignal = signals.find(
        (s) =>
          s.competitor.toLowerCase() === r.competitor.toLowerCase() &&
          s.source_urls.includes(res.url)
      );

      evidenceItems.push({
        competitor: r.competitor,
        title: res.title,
        url: res.url,
        domain,
        relatedSignal: matchingSignal || signals.find((s) => s.competitor.toLowerCase() === r.competitor.toLowerCase()),
        snippet: res.snippet,
      });
    });
  });

  const getCompClass = (comp: string) => {
    const c = comp.toLowerCase();
    if (c.includes("cashfree")) return "cf";
    if (c.includes("razorpay")) return "rz";
    return "pu";
  };

  return (
    <div className="evidence-page">
      <div className="page-header">
        <div>
          <h1 className="page-title">Evidence Vault</h1>
          <p className="page-subtitle">
            Ground truth primary sources, documentation, and news indexed during live competitor monitoring.
          </p>
        </div>
        <span className="badge gray" style={{ padding: "6px 12px", fontSize: "0.82rem" }}>
          {evidenceItems.length} Verified Sources
        </span>
      </div>

      {evidenceItems.length === 0 ? (
        <div className="empty-state">
          <div className="empty-icon">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
              <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
            </svg>
          </div>
          <h2 className="empty-title">No evidence sources available</h2>
          <p className="empty-desc">
            Run a landscape scan to gather verified web sources and ground competitive signals.
          </p>
        </div>
      ) : (
        <div className="panel-card">
          <div className="data-table-wrap">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Competitor</th>
                  <th>Source & Context</th>
                  <th>Domain</th>
                  <th>Associated Signal</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {evidenceItems.map((item, idx) => (
                  <tr key={idx}>
                    <td style={{ verticalAlign: "top" }}>
                      <div className="comp-avatar-tag">
                        <span className={`comp-avatar ${getCompClass(item.competitor)}`}>
                          {item.competitor.slice(0, 2).toUpperCase()}
                        </span>
                        <span>{item.competitor}</span>
                      </div>
                    </td>

                    <td style={{ maxWidth: 360 }}>
                      <div style={{ fontWeight: 600, fontSize: "0.86rem", color: "var(--text-main)" }}>
                        {item.title}
                      </div>
                      {item.snippet && (
                        <div style={{ fontSize: "0.76rem", color: "var(--text-muted)", marginTop: 4, lineHeight: 1.4 }}>
                          "{item.snippet.slice(0, 140)}..."
                        </div>
                      )}
                    </td>

                    <td style={{ whiteSpace: "nowrap" }}>
                      <span className="badge gray" style={{ fontFamily: "var(--font-mono)", fontSize: "0.72rem" }}>
                        {item.domain}
                      </span>
                    </td>

                    <td style={{ maxWidth: 240 }}>
                      {item.relatedSignal ? (
                        <div
                          style={{
                            fontSize: "0.8rem",
                            color: "var(--primary)",
                            cursor: "pointer",
                            fontWeight: 500,
                          }}
                          onClick={() => onSelectSignal(item.relatedSignal!)}
                        >
                          {item.relatedSignal.headline || "View Signal"}
                        </div>
                      ) : (
                        <span style={{ fontSize: "0.78rem", color: "var(--text-muted)" }}>
                          Market Context
                        </span>
                      )}
                    </td>

                    <td style={{ whiteSpace: "nowrap" }}>
                      <a
                        href={item.url}
                        target="_blank"
                        rel="noreferrer"
                        className="table-action-btn"
                        style={{ display: "inline-flex", alignItems: "center", gap: 4, textDecoration: "none" }}
                      >
                        Open Source
                        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                          <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
                          <polyline points="15 3 21 3 21 9" />
                          <line x1="10" y1="14" x2="21" y2="3" />
                        </svg>
                      </a>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
};
