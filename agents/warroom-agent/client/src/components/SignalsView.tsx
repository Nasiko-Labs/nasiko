import React, { useState, useMemo } from "react";
import { CompetitiveSignal } from "../types";

interface SignalsViewProps {
  signals: CompetitiveSignal[];
  onSelectSignal: (signal: CompetitiveSignal) => void;
}

export const SignalsView: React.FC<SignalsViewProps> = ({
  signals,
  onSelectSignal,
}) => {
  const [searchTerm, setSearchTerm] = useState("");
  const [selectedCompetitor, setSelectedCompetitor] = useState("All");
  const [selectedSeverity, setSelectedSeverity] = useState("All");
  const [selectedDept, setSelectedDept] = useState("All");
  const [activeTab, setActiveTab] = useState<"All" | "Critical" | "Significant" | "Informational">("All");

  const filteredSignals = useMemo(() => {
    return signals.filter((s) => {
      // Tab filter
      if (activeTab !== "All") {
        if ((s.significance || "informational").toUpperCase() !== activeTab.toUpperCase()) {
          return false;
        }
      }

      // Severity dropdown
      if (selectedSeverity !== "All") {
        if ((s.significance || "informational").toUpperCase() !== selectedSeverity.toUpperCase()) {
          return false;
        }
      }

      // Competitor dropdown
      if (selectedCompetitor !== "All") {
        if (s.competitor.toLowerCase() !== selectedCompetitor.toLowerCase()) {
          return false;
        }
      }

      // Department dropdown
      if (selectedDept !== "All") {
        const impact = s.impact || {};
        const d = selectedDept.toLowerCase() as keyof typeof impact;
        if (!impact[d] || impact[d] === "LOW") {
          return false;
        }
      }

      // Search term
      if (searchTerm.trim()) {
        const q = searchTerm.toLowerCase();
        const text = `${s.competitor} ${s.headline || ""} ${s.summary || ""} ${s.category || ""}`.toLowerCase();
        if (!text.includes(q)) {
          return false;
        }
      }

      return true;
    });
  }, [signals, activeTab, selectedSeverity, selectedCompetitor, selectedDept, searchTerm]);

  const counts = useMemo(() => {
    return {
      all: signals.length,
      critical: signals.filter((s) => (s.significance || "").toUpperCase() === "CRITICAL").length,
      significant: signals.filter((s) => (s.significance || "").toUpperCase() === "SIGNIFICANT").length,
      informational: signals.filter((s) => (s.significance || "").toUpperCase() === "INFORMATIONAL").length,
    };
  }, [signals]);

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

  const handleReset = () => {
    setSearchTerm("");
    setSelectedCompetitor("All");
    setSelectedSeverity("All");
    setSelectedDept("All");
    setActiveTab("All");
  };

  return (
    <div className="signals-page">
      <div className="page-header">
        <div>
          <h1 className="page-title">Signals</h1>
          <p className="page-subtitle">
            All detected competitor signals across your monitored landscape.
          </p>
        </div>
      </div>

      {/* Filter Toolbar */}
      <div className="panel-card" style={{ padding: "16px 20px", marginBottom: 20 }}>
        <div className="filter-toolbar" style={{ marginBottom: 0 }}>
          <div style={{ position: "relative", minWidth: 220 }}>
            <svg
              className="search-icon"
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
            >
              <circle cx="11" cy="11" r="8" />
              <line x1="21" y1="21" x2="16.65" y2="16.65" />
            </svg>
            <input
              type="text"
              placeholder="Search signals..."
              value={searchTerm}
              onChange={(e) => setSearchTerm(e.target.value)}
              style={{
                width: "100%",
                height: 34,
                padding: "0 10px 0 32px",
                fontSize: "0.82rem",
                border: "1px solid var(--border-color)",
                borderRadius: "var(--radius)",
                outline: "none",
              }}
            />
          </div>

          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <span style={{ fontSize: "0.76rem", fontWeight: 600, color: "var(--text-muted)" }}>Competitor</span>
            <select
              className="filter-select"
              value={selectedCompetitor}
              onChange={(e) => setSelectedCompetitor(e.target.value)}
            >
              <option value="All">All</option>
              <option value="Cashfree">Cashfree</option>
              <option value="Razorpay">Razorpay</option>
              <option value="PayU">PayU</option>
            </select>
          </div>

          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <span style={{ fontSize: "0.76rem", fontWeight: 600, color: "var(--text-muted)" }}>Severity</span>
            <select
              className="filter-select"
              value={selectedSeverity}
              onChange={(e) => setSelectedSeverity(e.target.value)}
            >
              <option value="All">All</option>
              <option value="Critical">Critical</option>
              <option value="Significant">Significant</option>
              <option value="Informational">Informational</option>
            </select>
          </div>

          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <span style={{ fontSize: "0.76rem", fontWeight: 600, color: "var(--text-muted)" }}>Department</span>
            <select
              className="filter-select"
              value={selectedDept}
              onChange={(e) => setSelectedDept(e.target.value)}
            >
              <option value="All">All</option>
              <option value="product">Product</option>
              <option value="sales">Sales</option>
              <option value="marketing">Marketing</option>
              <option value="strategy">Strategy</option>
            </select>
          </div>

          <button className="secondary-btn" onClick={handleReset} style={{ marginLeft: "auto" }}>
            Reset
          </button>
        </div>
      </div>

      {/* Pill tabs */}
      <div className="pill-tabs">
        <button
          className={`pill-tab ${activeTab === "All" ? "active" : ""}`}
          onClick={() => setActiveTab("All")}
        >
          All ({counts.all})
        </button>
        <button
          className={`pill-tab ${activeTab === "Critical" ? "active" : ""}`}
          onClick={() => setActiveTab("Critical")}
        >
          Critical ({counts.critical})
        </button>
        <button
          className={`pill-tab ${activeTab === "Significant" ? "active" : ""}`}
          onClick={() => setActiveTab("Significant")}
        >
          Significant ({counts.significant})
        </button>
        <button
          className={`pill-tab ${activeTab === "Informational" ? "active" : ""}`}
          onClick={() => setActiveTab("Informational")}
        >
          Informational ({counts.informational})
        </button>
      </div>

      {/* Signals Table */}
      <div className="panel-card">
        <div className="data-table-wrap">
          <table className="data-table">
            <thead>
              <tr>
                <th>Severity</th>
                <th>Competitor</th>
                <th>Signal</th>
                <th>Category</th>
                <th>Detected</th>
                <th>Affected Departments</th>
                <th>Status</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {filteredSignals.length === 0 ? (
                <tr>
                  <td colSpan={8} style={{ textAlign: "center", padding: "36px", color: "var(--text-muted)" }}>
                    No signals found matching current filter criteria.
                  </td>
                </tr>
              ) : (
                filteredSignals.map((sig, idx) => {
                  const sevClass = getSeverityClass(sig.significance);
                  const compClass = getCompClass(sig.competitor);
                  const impact = sig.impact || {};

                  return (
                    <tr key={idx} onClick={() => onSelectSignal(sig)}>
                      <td>
                        <div className="severity-cell">
                          <span className={`severity-dot ${sevClass}`} />
                          <span>{sig.significance || "Informational"}</span>
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
                      <td style={{ maxWidth: 300 }}>
                        <div style={{ fontWeight: 600, color: "var(--text-main)", fontSize: "0.86rem" }}>
                          {sig.headline || "Competitor Activity"}
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
                      <td>
                        <span className="badge product">
                          {sig.category || "Product"}
                        </span>
                      </td>
                      <td style={{ fontSize: "0.8rem", color: "var(--text-muted)", whiteSpace: "nowrap" }}>
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
                        <span className={`badge ${sevClass === "critical" || sevClass === "significant" ? "sales" : "gray"}`}>
                          {sevClass === "critical" ? "Review" : sevClass === "significant" ? "Review" : "Monitor"}
                        </span>
                      </td>
                      <td>
                        <button
                          className="table-action-btn"
                          onClick={(e) => {
                            e.stopPropagation();
                            onSelectSignal(sig);
                          }}
                        >
                          View &gt;
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
  );
};
