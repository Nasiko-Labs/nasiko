import React from "react";
import { CompanyContext } from "../types";

interface IntegrationsViewProps {
  company: CompanyContext | null;
}

export const IntegrationsView: React.FC<IntegrationsViewProps> = ({ company }) => {
  return (
    <div className="integrations-page">
      <div className="page-header">
        <div>
          <h1 className="page-title">Integrations & A2A Architecture</h1>
          <p className="page-subtitle">
            Connected engines, protocol interfaces, and company context profile.
          </p>
        </div>
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 24, maxWidth: 900 }}>
        {/* Nasiko A2A Card */}
        <div className="panel-card" style={{ padding: 24 }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: 14 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <span className="badge product" style={{ padding: "4px 8px", fontWeight: 700 }}>A2A 1.0</span>
              <h2 style={{ fontSize: "1.1rem", fontWeight: 700 }}>Nasiko Agent-to-Agent Protocol</h2>
            </div>
            <span className="metric-badge green">
              <span className="pulse-dot" style={{ width: 6, height: 6 }} /> Compliant
            </span>
          </div>
          <p style={{ fontSize: "0.85rem", color: "var(--text-secondary)", lineHeight: 1.6, marginBottom: 16 }}>
            WARROOM implements the open Linux Foundation Agent-to-Agent (A2A 1.0) JSON-RPC specification. Any autonomous agent or control plane can discover capabilities via the well-known AgentCard and dispatch SendMessage tasks.
          </p>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
            <div style={{ padding: 12, backgroundColor: "#fafbfc", border: "1px solid var(--border-light)", borderRadius: "var(--radius)" }}>
              <div style={{ fontSize: "0.74rem", fontWeight: 700, color: "var(--text-muted)", textTransform: "uppercase" }}>Discovery Endpoint</div>
              <code style={{ fontSize: "0.82rem", color: "var(--primary)", fontFamily: "var(--font-mono)" }}>
                GET /.well-known/agent-card.json
              </code>
            </div>

            <div style={{ padding: 12, backgroundColor: "#fafbfc", border: "1px solid var(--border-light)", borderRadius: "var(--radius)" }}>
              <div style={{ fontSize: "0.74rem", fontWeight: 700, color: "var(--text-muted)", textTransform: "uppercase" }}>Task RPC Endpoint</div>
              <code style={{ fontSize: "0.82rem", color: "var(--primary)", fontFamily: "var(--font-mono)" }}>
                POST /a2a (SendMessage)
              </code>
            </div>
          </div>

          <div style={{ marginTop: 14, paddingTop: 14, borderTop: "1px solid var(--border-light)", display: "flex", gap: 8 }}>
            <span className="badge gray">Skill: competitive-scan</span>
            <span className="badge gray">Skill: response-simulate</span>
            <span className="badge gray">Skill: ask-warroom</span>
          </div>
        </div>

        {/* Engine Integrations */}
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 20 }}>
          <div className="panel-card" style={{ padding: 20 }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 10 }}>
              <h3 style={{ fontSize: "0.95rem", fontWeight: 700 }}>Anakin.io Search Engine</h3>
              <span className="metric-badge green">Connected</span>
            </div>
            <p style={{ fontSize: "0.82rem", color: "var(--text-secondary)", lineHeight: 1.5 }}>
              Executes live web searches across competitor news, documentation, and product releases. Provides grounded snippets, timestamps, and verifiable source URLs.
            </p>
            <div style={{ marginTop: 12, fontSize: "0.76rem", color: "var(--text-muted)" }}>
              Monitored: Cashfree, Razorpay, PayU
            </div>
          </div>

          <div className="panel-card" style={{ padding: 20 }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 10 }}>
              <h3 style={{ fontSize: "0.95rem", fontWeight: 700 }}>DronaHQ Reasoning Agent</h3>
              <span className="metric-badge green">Connected</span>
            </div>
            <p style={{ fontSize: "0.82rem", color: "var(--text-secondary)", lineHeight: 1.5 }}>
              Processes multi-competitor evidence against PayFlow's product portfolio and customer segments. Generates rich signals, impact assessments, and talk tracks.
            </p>
            <div style={{ marginTop: 12, fontSize: "0.76rem", color: "var(--text-muted)" }}>
              Mode: Synchronous Standard Webhook
            </div>
          </div>
        </div>

        {/* Company Context Profile */}
        <div className="panel-card" style={{ padding: 24 }}>
          <h3 style={{ fontSize: "1rem", fontWeight: 700, marginBottom: 14 }}>
            Active Company Profile: {company?.name || "PayFlow"}
          </h3>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>
            <div>
              <div className="field-label" style={{ marginBottom: 6 }}>Internal Product Lineup</div>
              <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                {(company?.products || ["Payment Gateway", "Reconciliation", "FraudShield"]).map((p, i) => (
                  <span key={i} className="badge product">{p}</span>
                ))}
              </div>
            </div>

            <div>
              <div className="field-label" style={{ marginBottom: 6 }}>Target Customer Segments</div>
              <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                {(company?.target_customers || ["SMB", "Mid-Market"]).map((s, i) => (
                  <span key={i} className="badge gray">{s}</span>
                ))}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
