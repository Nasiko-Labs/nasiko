import React from "react";
import { CompetitiveSignal, ScanResponse } from "../types";

interface OverviewViewProps {
  scanData: ScanResponse | null;
  onScan: () => void;
  isScanning: boolean;
  onSelectSignal: (signal: CompetitiveSignal) => void;
  onNavigate: (page: any) => void;
}

export const OverviewView: React.FC<OverviewViewProps> = ({
  scanData,
  onScan,
  isScanning,
  onSelectSignal,
  onNavigate,
}) => {
  const signals = scanData?.signals || [];
  const criticalCount = signals.filter(
    (s) => (s.significance || "").toUpperCase() === "CRITICAL"
  ).length;
  const significantCount = signals.filter(
    (s) => (s.significance || "").toUpperCase() === "SIGNIFICANT"
  ).length;
  const highAttentionCount = criticalCount + significantCount;

  const totalSources = (scanData?.research || []).reduce(
    (acc, r) => acc + r.results.length,
    0
  );

  const getCompClass = (comp: string) => {
    const c = comp.toLowerCase();
    if (c.includes("cashfree")) return "cf";
    if (c.includes("razorpay")) return "rz";
    return "pu";
  };

  const getCompInitials = (comp: string) => comp.slice(0, 2).toUpperCase();

  const getSeverityClass = (sig?: string | null) => {
    const s = (sig || "informational").toLowerCase();
    if (s.includes("critical")) return "critical";
    if (s.includes("significant")) return "significant";
    return "informational";
  };

  return (
    <div className="overview-container">
      <div className="page-header">
        <div>
          <h1 className="page-title">Competitive Landscape</h1>
          <p className="page-subtitle">
            What's changing across the competitors that matter to{" "}
            <strong>{scanData?.company.name || "PayFlow"}</strong>.
          </p>
        </div>
      </div>

      {/* Scanning status banner */}
      {isScanning && (
        <div className="scanning-banner">
          <div className="spinner" />
          <div>
            <div style={{ fontWeight: 600, fontSize: "0.9rem", color: "var(--primary)" }}>
              Scanning competitive landscape in real time...
            </div>
            <div style={{ fontSize: "0.8rem", color: "var(--text-secondary)" }}>
              Querying Anakin.io live research and executing DronaHQ AI strategic reasoning.
            </div>
          </div>
        </div>
      )}

      {/* Empty State if no scan yet */}
      {!scanData && !isScanning ? (
        <div className="empty-state">
          <div className="empty-icon">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="12" cy="12" r="10" />
              <circle cx="12" cy="12" r="6" />
              <circle cx="12" cy="12" r="2" />
            </svg>
          </div>
          <h2 className="empty-title">No competitive scan yet</h2>
          <p className="empty-desc">
            Run a live landscape scan to discover real-time competitor signals, evaluate product overlap, and generate AI response plans.
          </p>
          <button className="primary-btn" onClick={onScan}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
              <polygon points="5 3 19 12 5 21 5 3" />
            </svg>
            Scan Landscape
          </button>
        </div>
      ) : (
        <>
          {/* 4 Metric Cards */}
          <div className="metrics-grid">
            <div className="metric-card">
              <div className="metric-card-top">
                <div className="metric-icon-wrap danger">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                    <circle cx="12" cy="12" r="10" />
                    <line x1="12" y1="8" x2="12" y2="12" />
                    <line x1="12" y1="16" x2="12.01" y2="16" />
                  </svg>
                </div>
                <span className="metric-label">Critical & Significant</span>
              </div>
              <div className="metric-value-row">
                <span className="metric-number">{highAttentionCount}</span>
                <span className="metric-badge red">
                  Require review
                </span>
              </div>
            </div>

            <div className="metric-card">
              <div className="metric-card-top">
                <div className="metric-icon-wrap primary">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                  </svg>
                </div>
                <span className="metric-label">Signals Detected</span>
              </div>
              <div className="metric-value-row">
                <span className="metric-number">{signals.length}</span>
                <span className="metric-badge green">
                  Current scan
                </span>
              </div>
            </div>

            <div className="metric-card">
              <div className="metric-card-top">
                <div className="metric-icon-wrap purple">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                    <circle cx="9" cy="7" r="4" />
                    <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                  </svg>
                </div>
                <span className="metric-label">Competitors Monitored</span>
              </div>
              <div className="metric-value-row">
                <span className="metric-number">3</span>
                <span className="metric-badge green">
                  <span className="pulse-dot" style={{ width: 6, height: 6 }} /> All active
                </span>
              </div>
            </div>

            <div className="metric-card">
              <div className="metric-card-top">
                <div className="metric-icon-wrap slate">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                    <polyline points="14 2 14 8 20 8" />
                  </svg>
                </div>
                <span className="metric-label">Response Plans</span>
              </div>
              <div className="metric-value-row">
                <span className="metric-number">{signals.length}</span>
                <span className="metric-badge amber">
                  Ready to simulate
                </span>
              </div>
            </div>
          </div>

          {/* Two column grid */}
          <div className="overview-grid">
            {/* Left Main Content */}
            <div className="overview-main">
              {/* Priority Signals Table */}
              <div>
                <div className="section-header">
                  <div>
                    <h2 className="section-title">Priority Signals</h2>
                    <p className="section-subtitle">
                      Most relevant competitor developments based on live evidence
                    </p>
                  </div>
                  <button className="link-btn" onClick={() => onNavigate("signals")}>
                    View all signals →
                  </button>
                </div>

                <div className="panel-card">
                  <div className="data-table-wrap">
                    <table className="data-table">
                      <thead>
                        <tr>
                          <th>Severity</th>
                          <th>Competitor</th>
                          <th>Signal</th>
                          <th>Detected</th>
                          <th>Impact</th>
                          <th>Action</th>
                        </tr>
                      </thead>
                      <tbody>
                        {signals.length === 0 ? (
                          <tr>
                            <td colSpan={6} style={{ textAlign: "center", padding: "28px" }}>
                              No signals extracted in current scan.
                            </td>
                          </tr>
                        ) : (
                          signals.map((sig, idx) => {
                            const sevClass = getSeverityClass(sig.significance);
                            const compClass = getCompClass(sig.competitor);
                            const impact = sig.impact || {};

                            return (
                              <tr key={idx} onClick={() => onSelectSignal(sig)}>
                                <td>
                                  <div className="severity-cell">
                                    <span className={`severity-dot ${sevClass}`} />
                                    <span>
                                      {sig.significance || "Informational"}
                                    </span>
                                  </div>
                                </td>
                                <td>
                                  <div className="comp-avatar-tag">
                                    <span className={`comp-avatar ${compClass}`}>
                                      {getCompInitials(sig.competitor)}
                                    </span>
                                    <span>{sig.competitor}</span>
                                  </div>
                                </td>
                                <td style={{ maxWidth: 280 }}>
                                  <div style={{ fontWeight: 600, color: "var(--text-main)", fontSize: "0.85rem" }}>
                                    {sig.headline || "Competitor development"}
                                  </div>
                                  <div
                                    style={{
                                      fontSize: "0.76rem",
                                      color: "var(--text-muted)",
                                      whiteSpace: "nowrap",
                                      overflow: "hidden",
                                      textOverflow: "ellipsis",
                                      marginTop: 2,
                                    }}
                                  >
                                    {sig.summary}
                                  </div>
                                </td>
                                <td style={{ whiteSpace: "nowrap", fontSize: "0.8rem", color: "var(--text-muted)" }}>
                                  Current scan
                                </td>
                                <td>
                                  <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
                                    {impact.product && impact.product !== "LOW" && (
                                      <span className="badge product">Product</span>
                                    )}
                                    {impact.sales && impact.sales !== "LOW" && (
                                      <span className="badge sales">Sales</span>
                                    )}
                                    {impact.marketing && impact.marketing !== "LOW" && (
                                      <span className="badge marketing">Marketing</span>
                                    )}
                                    {impact.strategy && impact.strategy !== "LOW" && (
                                      <span className="badge strategy">Strategy</span>
                                    )}
                                    {(!impact.product || impact.product === "LOW") &&
                                      (!impact.sales || impact.sales === "LOW") &&
                                      (!impact.marketing || impact.marketing === "LOW") &&
                                      (!impact.strategy || impact.strategy === "LOW") && (
                                        <span className="badge gray">General</span>
                                      )}
                                  </div>
                                </td>
                                <td>
                                  <button
                                    className="table-action-btn review"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      onSelectSignal(sig);
                                    }}
                                  >
                                    Review &gt;
                                  </button>
                                </td>
                              </tr>
                            );
                          })
                        )}
                      </tbody>
                    </table>
                  </div>
                </div>
              </div>

              {/* Competitor Overview Row */}
              <div>
                <div className="section-header">
                  <div>
                    <h2 className="section-title">Competitor Overview</h2>
                    <p className="section-subtitle">
                      Latest activity across monitored competitors
                    </p>
                  </div>
                  <button className="link-btn" onClick={() => onNavigate("competitors")}>
                    View all →
                  </button>
                </div>

                <div className="comp-overview-grid">
                  {["Cashfree", "Razorpay", "PayU"].map((compName) => {
                    const compClass = getCompClass(compName);
                    const compSignals = signals.filter(
                      (s) => s.competitor.toLowerCase() === compName.toLowerCase()
                    );
                    const latestSignal = compSignals[0];

                    return (
                      <div
                        key={compName}
                        className="comp-card"
                        onClick={() => onNavigate("competitors")}
                      >
                        <div className="comp-card-header">
                          <div className="comp-avatar-tag">
                            <span className={`comp-avatar ${compClass}`}>
                              {getCompInitials(compName)}
                            </span>
                            <span style={{ fontSize: "0.95rem" }}>{compName}</span>
                          </div>
                          <span className="metric-badge green" style={{ fontSize: "0.72rem" }}>
                            <span className="pulse-dot" style={{ width: 5, height: 5 }} /> Active
                          </span>
                        </div>

                        <div className="comp-card-body">
                          <div style={{ fontSize: "0.72rem", color: "var(--text-muted)", fontWeight: 600 }}>
                            Last meaningful signal
                          </div>
                          <div className="comp-signal-title">
                            {latestSignal?.headline || "Active monitoring in current scan"}
                          </div>
                        </div>

                        <div className="comp-card-footer">
                          <span>Last detected: Current scan</span>
                          <span style={{ fontWeight: 600 }}>
                            {compSignals.length} {compSignals.length === 1 ? "signal" : "signals"}
                          </span>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            </div>

            {/* Right Sidebar */}
            <div className="overview-sidebar">
              {/* Recent Activity Timeline */}
              <div className="panel-card activity-card">
                <div className="section-title" style={{ marginBottom: 16 }}>
                  Recent Activity
                </div>
                <div className="activity-list">
                  <div className="activity-item">
                    <span className="activity-dot red" />
                    <div className="activity-time">Just now</div>
                    <div className="activity-desc">
                      Live competitive scan completed with {signals.length} signal(s) classified.
                    </div>
                  </div>
                  <div className="activity-item">
                    <span className="activity-dot blue" />
                    <div className="activity-time">DronaHQ Engine</div>
                    <div className="activity-desc">
                      Multi-department reasoning synthesized across Product, Sales, and Marketing.
                    </div>
                  </div>
                  <div className="activity-item">
                    <span className="activity-dot green" />
                    <div className="activity-time">Anakin.io Search</div>
                    <div className="activity-desc">
                      {totalSources} verified external source URLs collected and indexed.
                    </div>
                  </div>
                </div>
              </div>

              {/* Quick Info Box */}
              <div className="panel-card quick-info-card">
                <div className="section-title" style={{ marginBottom: 14 }}>
                  Quick Info
                </div>
                <div className="quick-info-row">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ color: "var(--text-muted)", marginTop: 2 }}>
                    <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
                    <circle cx="12" cy="7" r="4" />
                  </svg>
                  <div>
                    <div className="quick-info-label">Target Company</div>
                    <div className="quick-info-val">{scanData?.company.name || "PayFlow"}</div>
                  </div>
                </div>

                <div className="quick-info-row">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ color: "var(--text-muted)", marginTop: 2 }}>
                    <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                    <circle cx="9" cy="7" r="4" />
                    <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                  </svg>
                  <div>
                    <div className="quick-info-label">Monitored Competitors</div>
                    <div className="quick-info-val">Cashfree, Razorpay, PayU</div>
                  </div>
                </div>

                <div className="quick-info-row">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ color: "var(--text-muted)", marginTop: 2 }}>
                    <circle cx="12" cy="12" r="10" />
                    <polyline points="12 6 12 12 16 14" />
                  </svg>
                  <div>
                    <div className="quick-info-label">Sources In Current Scan</div>
                    <div className="quick-info-val">{totalSources} Verified Sources</div>
                  </div>
                </div>

                <div className="quick-info-row">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ color: "var(--success)", marginTop: 2 }}>
                    <polyline points="20 6 9 17 4 12" />
                  </svg>
                  <div>
                    <div className="quick-info-label">Status</div>
                    <div className="quick-info-val" style={{ color: "var(--success)" }}>
                      All systems operational
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  );
};
