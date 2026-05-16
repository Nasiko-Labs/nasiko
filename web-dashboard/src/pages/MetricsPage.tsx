import { useCallback, useEffect, useState } from "react";
import {
  AgentMetricsTimeseries,
  fetchAgentMetricsTimeseries,
  fetchUserAgents,
  UserAgent,
} from "../api/metrics";
import { useAuth } from "../context/AuthContext";
import { MetricsChart } from "../components/MetricsChart";
import { StatCard } from "../components/StatCard";
import { NasikoShell } from "../components/NasikoShell";

function formatCost(value: number | undefined | null): string {
  if (value == null) return "$0.00";
  if (value < 0.01) return `$${value.toFixed(4)}`;
  return `$${value.toFixed(2)}`;
}

function formatLatency(ms: number | null | undefined): string {
  if (ms == null) return "N/A";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function formatTokens(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function MetricsPage() {
  const { token, logout } = useAuth();
  const [agents, setAgents] = useState<UserAgent[]>([]);
  const [selectedId, setSelectedId] = useState("");
  const [metrics, setMetrics] = useState<AgentMetricsTimeseries | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    void (async () => {
      try {
        const list = await fetchUserAgents(token);
        setAgents(list);
        if (list.length > 0) {
          setSelectedId((prev) => prev || list[0].agent_id);
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : "Failed to load agents");
      }
    })();
  }, [token]);

  const loadMetrics = useCallback(async () => {
    if (!token || !selectedId) return;
    setLoading(true);
    setError(null);
    try {
      const data = await fetchAgentMetricsTimeseries(token, selectedId, 24);
      setMetrics(data);
    } catch (e) {
      setMetrics(null);
      setError(e instanceof Error ? e.message : "Failed to load metrics");
    } finally {
      setLoading(false);
    }
  }, [token, selectedId]);

  useEffect(() => {
    void loadMetrics();
  }, [loadMetrics]);

  const summary = metrics?.summary;
  const totalCost = summary?.cost_summary?.total?.cost ?? 0;
  const series = metrics?.series ?? [];

  const actions = (
    <>
      <button type="button" className="btn ghost" onClick={() => void loadMetrics()}>
        Refresh
      </button>
      <button type="button" className="btn ghost" onClick={logout}>
        Sign out
      </button>
    </>
  );

  return (
    <NasikoShell pageTitle="Agent Metrics" activeNav="metrics" actions={actions}>
      <div className="page-header">
        <h1>Agent metrics</h1>
        <p className="subtitle">
          Per-agent performance and cost over the last 24 hours (Phoenix observability).
        </p>
      </div>

      <div className="metrics-toolbar card">
        <label className="agent-select-label">
          Agent
          <select
            className="agent-select"
            value={selectedId}
            onChange={(e) => setSelectedId(e.target.value)}
            disabled={agents.length === 0}
          >
            {agents.length === 0 ? (
              <option value="">No agents available</option>
            ) : (
              agents.map((a) => (
                <option key={a.agent_id} value={a.agent_id}>
                  {a.name} ({a.agent_id})
                </option>
              ))
            )}
          </select>
        </label>
        {metrics && (
          <span className="count">
            {loading
              ? "Loading…"
              : `${metrics.summary.sessions_in_range} sessions · 24h window`}
          </span>
        )}
      </div>

      {error && <div className="banner error">{error}</div>}

      {agents.length === 0 && !error && (
        <p className="empty">
          Deploy an agent and generate traffic to see metrics. Phoenix must be running.
        </p>
      )}

      {summary && (
        <div className="stat-grid">
          <StatCard label="Traces (24h)" value={String(summary.trace_count)} />
          <StatCard
            label="P50 latency"
            value={formatLatency(summary.latency_ms_p50)}
          />
          <StatCard
            label="P99 latency"
            value={formatLatency(summary.latency_ms_p99)}
          />
          <StatCard label="Total cost" value={formatCost(totalCost)} />
        </div>
      )}

      {series.length > 0 && (
        <div className="charts-grid">
          <MetricsChart
            title="Traces per hour"
            data={series}
            valueKey="trace_count"
          />
          <MetricsChart
            title="Cost per hour (USD)"
            data={series}
            valueKey="total_cost"
            formatValue={(n) => formatCost(n)}
            color="#38bdf8"
          />
          <MetricsChart
            title="Tokens per hour"
            data={series}
            valueKey="total_tokens"
            formatValue={formatTokens}
            color="#64748b"
          />
          <MetricsChart
            title="Sessions per hour"
            data={series}
            valueKey="session_count"
            color="#94a3b8"
          />
        </div>
      )}
    </NasikoShell>
  );
}
