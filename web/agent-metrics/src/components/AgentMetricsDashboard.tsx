import { useCallback, useEffect, useMemo, useState } from "react";
import { clearToken, fetchAgentMetrics } from "../api/client";
import type { AgentMetrics } from "../types";
import MetricsCharts from "./MetricsCharts";

interface AgentMetricsDashboardProps {
  onLogout: () => void;
}

function statusClass(status: string): string {
  const normalized = status.toLowerCase();
  if (normalized === "active") return "active";
  if (normalized === "failed") return "failed";
  return "setup";
}

export default function AgentMetricsDashboard({
  onLogout,
}: AgentMetricsDashboardProps) {
  const [agents, setAgents] = useState<AgentMetrics[]>([]);
  const [selectedAgentId, setSelectedAgentId] = useState<string>("");
  const [periodHours, setPeriodHours] = useState(24);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadMetrics = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const response = await fetchAgentMetrics(periodHours);
      const nextAgents = response.data.agents ?? [];
      setAgents(nextAgents);
      setSelectedAgentId((current) => {
        if (current && nextAgents.some((agent) => agent.agent_id === current)) {
          return current;
        }
        return nextAgents[0]?.agent_id ?? "";
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load metrics");
    } finally {
      setLoading(false);
    }
  }, [periodHours]);

  useEffect(() => {
    void loadMetrics();
  }, [loadMetrics]);

  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.agent_id === selectedAgentId),
    [agents, selectedAgentId]
  );

  function handleLogout() {
    clearToken();
    onLogout();
  }

  return (
    <div className="app-shell">
      <header className="header">
        <div>
          <h1>Agent performance metrics</h1>
          <p>Per-agent stats for the last {periodHours} hours (Phoenix traces).</p>
        </div>
        <div className="header-actions">
          <button className="btn btn-secondary" onClick={() => void loadMetrics()}>
            Refresh
          </button>
          <button className="btn btn-secondary" onClick={handleLogout}>
            Sign out
          </button>
        </div>
      </header>

      {error && <div className="error-banner">{error}</div>}

      <div className="panel">
        <div className="toolbar">
          <div className="field" style={{ marginBottom: 0, minWidth: 220 }}>
            <label htmlFor="agent-select">Agent</label>
            <select
              id="agent-select"
              value={selectedAgentId}
              onChange={(e) => setSelectedAgentId(e.target.value)}
              disabled={agents.length === 0}
            >
              {agents.map((agent) => (
                <option key={agent.agent_id} value={agent.agent_id}>
                  {agent.name} ({agent.agent_id})
                </option>
              ))}
            </select>
          </div>
          <div className="field" style={{ marginBottom: 0, minWidth: 140 }}>
            <label htmlFor="period-hours">Window</label>
            <select
              id="period-hours"
              value={periodHours}
              onChange={(e) => setPeriodHours(Number(e.target.value))}
            >
              <option value={6}>6 hours</option>
              <option value={12}>12 hours</option>
              <option value={24}>24 hours</option>
              <option value={48}>48 hours</option>
            </select>
          </div>
        </div>

        {loading && <p className="empty-state">Loading metrics…</p>}

        {!loading && agents.length === 0 && (
          <p className="empty-state">
            No agents found. Deploy an agent and run a session to generate traces.
          </p>
        )}

        {!loading && selectedAgent && (
          <>
            <div style={{ marginBottom: "1rem" }}>
              <span
                className={`status-pill ${statusClass(selectedAgent.deployment_status)}`}
              >
                {selectedAgent.deployment_status}
              </span>
              {!selectedAgent.has_observability && (
                <span style={{ marginLeft: "0.75rem", color: "var(--muted)" }}>
                  No Phoenix project yet — metrics will populate after sessions.
                </span>
              )}
            </div>

            <div className="stat-grid">
              <div className="stat-card">
                <div className="label">Avg response time</div>
                <div className="value">
                  {selectedAgent.avg_response_time_ms.toLocaleString()} ms
                </div>
              </div>
              <div className="stat-card">
                <div className="label">Success / Error</div>
                <div className="value">
                  {selectedAgent.success_count} / {selectedAgent.error_count}
                </div>
              </div>
              <div className="stat-card">
                <div className="label">Total requests</div>
                <div className="value">{selectedAgent.total_requests}</div>
              </div>
              <div className="stat-card">
                <div className="label">Uptime</div>
                <div className="value">{selectedAgent.uptime_percent}%</div>
              </div>
            </div>

            <MetricsCharts hourly={selectedAgent.hourly} />
          </>
        )}
      </div>

      <p className="footer-note">
        Main Nasiko dashboard:{" "}
        <a href="/app/" target="_blank" rel="noreferrer">
          /app/
        </a>
        {" · "}
        Phoenix UI:{" "}
        <a href="http://localhost:6006" target="_blank" rel="noreferrer">
          localhost:6006
        </a>
      </p>
    </div>
  );
}
