import React, { useState } from "react";
import { CompetitiveSignal, CompetitorResearch } from "../types";

interface CompetitorsViewProps {
  signals: CompetitiveSignal[];
  research: CompetitorResearch[];
  onSelectSignal: (signal: CompetitiveSignal) => void;
}

interface CompetitorMeta {
  name: string;
  initials: string;
  colorClass: string;
  tagline: string;
  website: string;
  about: string;
  focusAreas: string[];
}

const COMPETITORS_META: Record<string, CompetitorMeta> = {
  Cashfree: {
    name: "Cashfree",
    initials: "CF",
    colorClass: "cf",
    tagline: "Payments and banking technology for Indian and global businesses",
    website: "https://www.cashfree.com",
    about:
      "Cashfree Payments provides payment gateway solutions, instant payouts, automated reconciliation, and real-time risk management (RiskShield) for merchants.",
    focusAreas: ["Payment Gateway", "RiskShield", "Reconciliation", "Payouts", "UPI Autopay"],
  },
  Razorpay: {
    name: "Razorpay",
    initials: "RZ",
    colorClass: "rz",
    tagline: "Full-stack financial services and payments infrastructure",
    website: "https://razorpay.com",
    about:
      "Razorpay offers online payment gateways, automated chargeback protection (Chargeback Shield / Thirdwatch), corporate cards, and SME business banking.",
    focusAreas: ["Payment Gateway", "Chargeback Shield", "Thirdwatch", "Fraud Prevention", "Business Banking"],
  },
  PayU: {
    name: "PayU",
    initials: "PU",
    colorClass: "pu",
    tagline: "Global online payment processing and merchant risk management",
    website: "https://payu.in",
    about:
      "PayU provides multi-channel payment solutions, enterprise checkout experiences, and fraud mitigation tools for online platforms.",
    focusAreas: ["Online Payments", "Anti-Fraud Solutions", "Enterprise Checkout", "Cross-Border Settlements"],
  },
};

export const CompetitorsView: React.FC<CompetitorsViewProps> = ({
  signals,
  research,
  onSelectSignal,
}) => {
  const [selectedCompName, setSelectedCompName] = useState("Cashfree");
  const [activeSubTab, setActiveSubTab] = useState<"overview" | "signals" | "evidence">("overview");

  const meta = COMPETITORS_META[selectedCompName] || COMPETITORS_META["Cashfree"];
  const compSignals = signals.filter(
    (s) => s.competitor.toLowerCase() === selectedCompName.toLowerCase()
  );
  const compResearch = research.find(
    (r) => r.competitor.toLowerCase() === selectedCompName.toLowerCase()
  );

  const criticalCount = compSignals.filter(
    (s) => (s.significance || "").toUpperCase() === "CRITICAL"
  ).length;
  const significantCount = compSignals.filter(
    (s) => (s.significance || "").toUpperCase() === "SIGNIFICANT"
  ).length;
  const informationalCount = compSignals.length - criticalCount - significantCount;

  return (
    <div className="competitors-page">
      <div className="page-header">
        <div>
          <h1 className="page-title">Competitors</h1>
          <p className="page-subtitle">
            Track and analyze key competitors in your market.
          </p>
        </div>
      </div>

      {/* Top 3 Metric Cards */}
      <div className="metrics-grid" style={{ gridTemplateColumns: "repeat(3, 1fr)" }}>
        <div className="metric-card">
          <div className="metric-card-top">
            <div className="metric-icon-wrap primary">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                <circle cx="9" cy="7" r="4" />
                <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                <path d="M16 3.13a4 4 0 0 1 0 7.75" />
              </svg>
            </div>
            <span className="metric-label">Total Competitors</span>
          </div>
          <div className="metric-value-row">
            <span className="metric-number">3</span>
            <span className="metric-badge green">
              <span className="pulse-dot" style={{ width: 6, height: 6 }} /> Active monitoring
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
            <span className="metric-label">Total Signals</span>
          </div>
          <div className="metric-value-row">
            <span className="metric-number">{signals.length}</span>
            <span className="metric-badge green">Current scan</span>
          </div>
        </div>

        <div className="metric-card">
          <div className="metric-card-top">
            <div className="metric-icon-wrap danger">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <circle cx="12" cy="12" r="10" />
                <line x1="12" y1="8" x2="12" y2="12" />
                <line x1="12" y1="16" x2="12.01" y2="16" />
              </svg>
            </div>
            <span className="metric-label">Critical & Significant</span>
          </div>
          <div className="metric-value-row">
            <span className="metric-number">
              {signals.filter((s) => ["CRITICAL", "SIGNIFICANT"].includes((s.significance || "").toUpperCase())).length}
            </span>
            <span className="metric-badge red">Require attention</span>
          </div>
        </div>
      </div>

      {/* Two Pane Layout */}
      <div className="competitors-layout">
        {/* Left Column: Competitor selection cards */}
        <div className="comp-list-col">
          {["Cashfree", "Razorpay", "PayU"].map((name) => {
            const comp = COMPETITORS_META[name];
            const isSelected = selectedCompName.toLowerCase() === name.toLowerCase();
            const compSigs = signals.filter(
              (s) => s.competitor.toLowerCase() === name.toLowerCase()
            );

            return (
              <div
                key={name}
                className="comp-card"
                style={{
                  borderColor: isSelected ? "var(--primary)" : "var(--border-color)",
                  boxShadow: isSelected ? "0 0 0 2px rgba(37, 99, 235, 0.15)" : "var(--shadow-sm)",
                }}
                onClick={() => setSelectedCompName(name)}
              >
                <div className="comp-card-header">
                  <div className="comp-avatar-tag">
                    <span className={`comp-avatar ${comp.colorClass}`}>{comp.initials}</span>
                    <span style={{ fontSize: "1rem", fontWeight: 700 }}>{name}</span>
                  </div>
                  <span className="metric-badge green" style={{ fontSize: "0.74rem" }}>
                    <span className="pulse-dot" style={{ width: 5, height: 5 }} /> Active
                  </span>
                </div>

                <div style={{ fontSize: "0.78rem", color: "var(--text-muted)", marginBottom: 10 }}>
                  {comp.tagline}
                </div>

                <div style={{ display: "flex", gap: 4, flexWrap: "wrap", marginBottom: 12 }}>
                  {comp.focusAreas.slice(0, 3).map((f, i) => (
                    <span key={i} className="badge gray" style={{ fontSize: "0.7rem" }}>
                      {f}
                    </span>
                  ))}
                </div>

                <div className="comp-card-footer">
                  <span>{compSigs.length} signals in scan</span>
                  <span style={{ fontWeight: 600, color: "var(--primary)" }}>Select &gt;</span>
                </div>
              </div>
            );
          })}
        </div>

        {/* Right Column: Selected Competitor Detail */}
        <div className="comp-detail-panel">
          {/* Header */}
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: 18 }}>
            <div style={{ display: "flex", gap: 14, alignItems: "center" }}>
              <span
                className={`comp-avatar ${meta.colorClass}`}
                style={{ width: 44, height: 44, fontSize: "1.1rem" }}
              >
                {meta.initials}
              </span>
              <div>
                <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  <h2 style={{ fontSize: "1.3rem", fontWeight: 800 }}>{meta.name}</h2>
                  <span className="metric-badge green">
                    <span className="pulse-dot" style={{ width: 6, height: 6 }} /> Active monitoring
                  </span>
                </div>
                <div style={{ fontSize: "0.82rem", color: "var(--text-muted)", marginTop: 2 }}>
                  {meta.tagline}
                </div>
              </div>
            </div>

            <a
              href={meta.website}
              target="_blank"
              rel="noreferrer"
              className="secondary-btn"
              style={{ fontSize: "0.78rem" }}
            >
              Open Website
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
                <polyline points="15 3 21 3 21 9" />
                <line x1="10" y1="14" x2="21" y2="3" />
              </svg>
            </a>
          </div>

          {/* Sub-tabs */}
          <div style={{ display: "flex", gap: 18, borderBottom: "1px solid var(--border-color)", marginBottom: 20 }}>
            <button
              style={{
                background: "none",
                border: "none",
                borderBottom: activeSubTab === "overview" ? "2px solid var(--primary)" : "2px solid transparent",
                padding: "8px 4px",
                fontWeight: activeSubTab === "overview" ? 700 : 500,
                color: activeSubTab === "overview" ? "var(--primary)" : "var(--text-muted)",
                cursor: "pointer",
                fontSize: "0.86rem",
              }}
              onClick={() => setActiveSubTab("overview")}
            >
              Overview
            </button>
            <button
              style={{
                background: "none",
                border: "none",
                borderBottom: activeSubTab === "signals" ? "2px solid var(--primary)" : "2px solid transparent",
                padding: "8px 4px",
                fontWeight: activeSubTab === "signals" ? 700 : 500,
                color: activeSubTab === "signals" ? "var(--primary)" : "var(--text-muted)",
                cursor: "pointer",
                fontSize: "0.86rem",
              }}
              onClick={() => setActiveSubTab("signals")}
            >
              Recent Signals ({compSignals.length})
            </button>
            <button
              style={{
                background: "none",
                border: "none",
                borderBottom: activeSubTab === "evidence" ? "2px solid var(--primary)" : "2px solid transparent",
                padding: "8px 4px",
                fontWeight: activeSubTab === "evidence" ? 700 : 500,
                color: activeSubTab === "evidence" ? "var(--primary)" : "var(--text-muted)",
                cursor: "pointer",
                fontSize: "0.86rem",
              }}
              onClick={() => setActiveSubTab("evidence")}
            >
              Evidence ({compResearch?.results.length || 0})
            </button>
          </div>

          {/* Tab Content */}
          {activeSubTab === "overview" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 20 }}>
              <div>
                <div className="field-label">About {meta.name}</div>
                <div style={{ fontSize: "0.88rem", color: "var(--text-secondary)", lineHeight: 1.6, marginTop: 4 }}>
                  {meta.about}
                </div>
              </div>

              <div>
                <div className="field-label">Key Focus Areas</div>
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginTop: 8 }}>
                  {meta.focusAreas.map((area, i) => (
                    <span key={i} className="badge product" style={{ padding: "4px 10px", fontSize: "0.78rem" }}>
                      {area}
                    </span>
                  ))}
                </div>
              </div>

              {/* Signals summary row */}
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>
                <div className="panel-card" style={{ padding: 16 }}>
                  <div className="field-label">Signal Breakdown</div>
                  <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 10 }}>
                    <div style={{ display: "flex", justifyContent: "space-between", fontSize: "0.84rem" }}>
                      <span>Total signals in scan</span>
                      <strong>{compSignals.length}</strong>
                    </div>
                    <div style={{ display: "flex", justifyContent: "space-between", fontSize: "0.84rem" }}>
                      <span style={{ color: "var(--danger)" }}>• Critical</span>
                      <strong>{criticalCount}</strong>
                    </div>
                    <div style={{ display: "flex", justifyContent: "space-between", fontSize: "0.84rem" }}>
                      <span style={{ color: "var(--warning)" }}>• Significant</span>
                      <strong>{significantCount}</strong>
                    </div>
                    <div style={{ display: "flex", justifyContent: "space-between", fontSize: "0.84rem" }}>
                      <span style={{ color: "var(--text-muted)" }}>• Informational</span>
                      <strong>{informationalCount}</strong>
                    </div>
                  </div>
                </div>

                <div className="panel-card" style={{ padding: 16 }}>
                  <div className="field-label">PayFlow Products Impacted</div>
                  <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 10 }}>
                    {compSignals.flatMap((s) => s.overlap?.products || []).length > 0 ? (
                      Array.from(new Set(compSignals.flatMap((s) => s.overlap?.products || []))).map((p, i) => (
                        <span key={i} className="badge product" style={{ fontSize: "0.78rem" }}>
                          {p}
                        </span>
                      ))
                    ) : (
                      <span style={{ fontSize: "0.84rem", color: "var(--text-muted)" }}>
                        General Payment Gateway & Merchant Services
                      </span>
                    )}
                  </div>
                </div>
              </div>
            </div>
          )}

          {activeSubTab === "signals" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              {compSignals.length === 0 ? (
                <div style={{ padding: 24, textAlign: "center", color: "var(--text-muted)" }}>
                  No signals detected for {meta.name} in current scan.
                </div>
              ) : (
                compSignals.map((sig, idx) => (
                  <div
                    key={idx}
                    className="panel-card"
                    style={{ padding: 16, cursor: "pointer" }}
                    onClick={() => onSelectSignal(sig)}
                  >
                    <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 6 }}>
                      <span className={`badge ${(sig.significance || "informational").toLowerCase()}`}>
                        {sig.significance || "INFORMATIONAL"}
                      </span>
                      <span style={{ fontSize: "0.75rem", color: "var(--text-muted)" }}>Current scan</span>
                    </div>
                    <div style={{ fontWeight: 700, fontSize: "0.92rem", color: "var(--text-main)" }}>
                      {sig.headline}
                    </div>
                    <div style={{ fontSize: "0.82rem", color: "var(--text-secondary)", marginTop: 4 }}>
                      {sig.summary}
                    </div>
                    <div style={{ marginTop: 10, display: "flex", justifyContent: "flex-end" }}>
                      <button className="table-action-btn review">Review Signal &gt;</button>
                    </div>
                  </div>
                ))
              )}
            </div>
          )}

          {activeSubTab === "evidence" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              {!compResearch || compResearch.results.length === 0 ? (
                <div style={{ padding: 24, textAlign: "center", color: "var(--text-muted)" }}>
                  No research evidence collected for {meta.name}.
                </div>
              ) : (
                compResearch.results.map((res, idx) => (
                  <div key={idx} className="panel-card" style={{ padding: 16 }}>
                    <div style={{ fontWeight: 600, fontSize: "0.9rem", color: "var(--text-main)" }}>
                      {res.title}
                    </div>
                    <div style={{ fontSize: "0.82rem", color: "var(--text-secondary)", marginTop: 6, lineHeight: 1.5 }}>
                      "{res.snippet}"
                    </div>
                    <div style={{ marginTop: 10 }}>
                      <a
                        href={res.url}
                        target="_blank"
                        rel="noreferrer"
                        style={{ fontSize: "0.8rem", color: "var(--primary)", wordBreak: "break-all" }}
                      >
                        {res.url}
                      </a>
                    </div>
                  </div>
                ))
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
