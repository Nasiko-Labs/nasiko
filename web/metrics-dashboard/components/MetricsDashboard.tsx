"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import Link from "next/link";
import {
  fetchAgentMetrics,
  fetchAgentSessions,
  fetchAgents,
  fetchSessionDetail,
  fetchTraceGraph,
  MetricsApiError,
} from "@/lib/api-client";
import {
  clearDevAccessToken,
  getAccessTokenFromStorage,
  setDevAccessToken,
} from "@/lib/auth-client";
import { pickPrimaryTrace } from "@/lib/sessions";
import type {
  AgentMetricsResponse,
  MetricsAgentOption,
  MetricsSessionRow,
  TraceDetailResponse,
} from "@/lib/types";
import { AgentsOverviewTable } from "@/components/AgentsOverviewTable";
import { DashboardNav, type DashboardView } from "@/components/DashboardNav";
import { KpiCards } from "@/components/KpiCards";
import { TrafficChart } from "@/components/TrafficChart";
import { MetricsTimeRangeBar } from "@/components/MetricsTimeRangeBar";
import { RequestTracesWorkspace } from "@/components/RequestTracesWorkspace";
import { aggregateFleetMetrics, type FleetAgentRow } from "@/lib/fleet-rollup";
import { defaultTimeRange, type MetricsTimeRange } from "@/lib/time-range";
import "./metrics-dashboard.css";

const NASIKO_APP_URL =
  process.env.NEXT_PUBLIC_NASIKO_APP_URL ?? "http://localhost:9100/app/";

const AUTO_REFRESH_MS = 60_000;

type LoadState = "idle" | "loading" | "ready" | "error" | "auth";

function formatRelative(date: Date | null, now: Date): string {
  if (!date) return "never";
  const deltaSec = Math.max(0, Math.round((now.getTime() - date.getTime()) / 1000));
  if (deltaSec < 5) return "just now";
  if (deltaSec < 60) return `${deltaSec}s ago`;
  const min = Math.round(deltaSec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  return `${hr}h ago`;
}

export function MetricsDashboard() {
  const [token, setToken] = useState<string | null>(null);
  const [agents, setAgents] = useState<MetricsAgentOption[]>([]);
  const [selectedAgentId, setSelectedAgentId] = useState<string>("");
  const [metrics, setMetrics] = useState<AgentMetricsResponse | null>(null);
  const [state, setState] = useState<LoadState>("idle");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);
  const [devTokenInput, setDevTokenInput] = useState("");
  const [autoRefresh, setAutoRefresh] = useState(false);
  const [now, setNow] = useState(() => new Date());
  const [sessions, setSessions] = useState<MetricsSessionRow[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [trace, setTrace] = useState<TraceDetailResponse | null>(null);
  const [traceLoading, setTraceLoading] = useState(false);
  const [traceError, setTraceError] = useState<string | null>(null);
  const [traceLoadingSessionId, setTraceLoadingSessionId] = useState<string | null>(
    null,
  );
  const [dashboardView, setDashboardView] = useState<DashboardView>("agent");
  const [fleetMetrics, setFleetMetrics] = useState<AgentMetricsResponse | null>(
    null,
  );
  const [fleetRows, setFleetRows] = useState<FleetAgentRow[]>([]);
  const [timeRange, setTimeRange] = useState<MetricsTimeRange>(() =>
    defaultTimeRange(),
  );

  const refreshRef = useRef<() => void>(() => {});

  const resolveToken = useCallback(() => {
    const t = getAccessTokenFromStorage();
    setToken(t);
    return t;
  }, []);

  useEffect(() => {
    resolveToken();
  }, [resolveToken]);

  const loadAgents = useCallback(async (accessToken: string) => {
    const list = await fetchAgents(accessToken);
    setAgents(list);
    if (list.length > 0) {
      setSelectedAgentId((prev) =>
        prev && list.some((a) => a.agentId === prev) ? prev : list[0].agentId,
      );
    }
    return list;
  }, []);

  const loadSessions = useCallback(
    async (
      accessToken: string,
      agentId: string,
      range: MetricsTimeRange,
    ) => {
      if (!agentId) {
        setSessions([]);
        return;
      }
      setSessionsLoading(true);
      try {
        const data = await fetchAgentSessions(accessToken, agentId, range);
        setSessions(data.sessions);
      } catch {
        setSessions([]);
      } finally {
        setSessionsLoading(false);
      }
    },
    [],
  );

  const loadMetrics = useCallback(
    async (
      accessToken: string,
      agentId: string,
      range: MetricsTimeRange,
    ) => {
      if (!agentId) {
        setMetrics(null);
        return;
      }
      const data = await fetchAgentMetrics(accessToken, agentId, range);
      setMetrics(data);
      setLastUpdated(new Date());
      await loadSessions(accessToken, agentId, range);
    },
    [loadSessions],
  );

  const handleViewTrace = useCallback(
    async (sessionId: string) => {
      const accessToken = resolveToken();
      if (!accessToken || !selectedAgentId) {
        return;
      }
      setSelectedSessionId(sessionId);
      setTraceLoading(true);
      setTraceLoadingSessionId(sessionId);
      setTraceError(null);
      setTrace(null);
      try {
        const session = await fetchSessionDetail(
          accessToken,
          selectedAgentId,
          sessionId,
        );
        const primary = pickPrimaryTrace(session.traces);
        if (!primary?.trace_id) {
          setTraceError("No traces found for this session.");
          return;
        }
        const preview =
          sessions.find((s) => s.session_id === sessionId)?.preview ?? null;
        const graph = await fetchTraceGraph(
          accessToken,
          selectedAgentId,
          primary.trace_id,
          session.project_id ?? metrics?.source?.project_id,
          "24h",
          preview,
        );
        setTrace(graph);
      } catch (err) {
        setTraceError(
          err instanceof Error ? err.message : "Failed to load trace graph",
        );
      } finally {
        setTraceLoading(false);
        setTraceLoadingSessionId(null);
      }
    },
    [metrics?.source?.project_id, resolveToken, selectedAgentId, sessions],
  );

  const loadMetricsForAgent = useCallback(
    async (accessToken: string, agentId: string) => {
      setState("loading");
      setErrorMessage(null);
      try {
        await loadMetrics(accessToken, agentId, timeRange);
        setState("ready");
      } catch (err) {
        if (err instanceof MetricsApiError && err.status === 401) {
          setState("auth");
          setErrorMessage("Session expired. Sign in again or paste a new token.");
          return;
        }
        setState("error");
        setErrorMessage(
          err instanceof Error ? err.message : "Failed to load metrics",
        );
      }
    },
    [loadMetrics, timeRange],
  );

  const loadFleetMetrics = useCallback(
    async (
      accessToken: string,
      agentList: MetricsAgentOption[],
      range: MetricsTimeRange,
    ) => {
      if (agentList.length === 0) {
        setFleetMetrics(null);
        setFleetRows([]);
        setLastUpdated(new Date());
        return;
      }
      const results = await Promise.all(
        agentList.map((agent) =>
          fetchAgentMetrics(accessToken, agent.agentId, range),
        ),
      );
      const { aggregated, rows } = aggregateFleetMetrics(
        agentList,
        results,
        range.window,
      );
      setFleetMetrics(aggregated);
      setFleetRows(rows);
      setLastUpdated(new Date());
    },
    [],
  );

  const loadFleetForView = useCallback(
    async (accessToken: string) => {
      setState("loading");
      setErrorMessage(null);
      try {
        const list = await loadAgents(accessToken);
        await loadFleetMetrics(accessToken, list, timeRange);
        setState("ready");
      } catch (err) {
        if (err instanceof MetricsApiError && err.status === 401) {
          setState("auth");
          setErrorMessage("Session expired. Sign in again or paste a new token.");
          return;
        }
        setState("error");
        setErrorMessage(
          err instanceof Error ? err.message : "Failed to load fleet metrics",
        );
      }
    },
    [loadAgents, loadFleetMetrics, timeRange],
  );

  const handleApplyTimeRange = useCallback(
    async (range: MetricsTimeRange) => {
      setTimeRange(range);
      setSelectedSessionId(null);
      setTrace(null);
      setTraceError(null);
      const accessToken = resolveToken();
      if (!accessToken) {
        return;
      }
      setState("loading");
      setErrorMessage(null);
      try {
        if (dashboardView === "fleet") {
          const list =
            agents.length > 0 ? agents : await loadAgents(accessToken);
          await loadFleetMetrics(accessToken, list, range);
        } else if (selectedAgentId) {
          await loadMetrics(accessToken, selectedAgentId, range);
        }
        setState("ready");
      } catch (err) {
        if (err instanceof MetricsApiError && err.status === 401) {
          setState("auth");
          setErrorMessage("Session expired. Sign in again or paste a new token.");
          return;
        }
        setState("error");
        setErrorMessage(
          err instanceof Error ? err.message : "Failed to load data",
        );
      }
    },
    [
      agents,
      dashboardView,
      loadAgents,
      loadFleetMetrics,
      loadMetrics,
      resolveToken,
      selectedAgentId,
    ],
  );

  const handleDashboardViewChange = useCallback(
    (view: DashboardView) => {
      setDashboardView(view);
      const accessToken = resolveToken();
      if (!accessToken) {
        return;
      }
      if (view === "fleet") {
        void loadFleetForView(accessToken);
      } else if (selectedAgentId) {
        void loadMetricsForAgent(accessToken, selectedAgentId);
      }
    },
    [
      loadFleetForView,
      loadMetricsForAgent,
      resolveToken,
      selectedAgentId,
    ],
  );

  const handleDrillToAgent = useCallback(
    (agentId: string) => {
      setSelectedAgentId(agentId);
      setSelectedSessionId(null);
      setTrace(null);
      setTraceError(null);
      setDashboardView("agent");
      const accessToken = resolveToken();
      if (accessToken) {
        void loadMetricsForAgent(accessToken, agentId);
      }
    },
    [loadMetricsForAgent, resolveToken],
  );

  const refresh = useCallback(async () => {
    const accessToken = resolveToken();
    if (!accessToken) {
      setState("auth");
      return;
    }

    setState("loading");
    setErrorMessage(null);

    try {
      const list = await loadAgents(accessToken);

      if (dashboardView === "fleet") {
        await loadFleetMetrics(accessToken, list, timeRange);
        setState("ready");
        return;
      }

      const agentId =
        selectedAgentId && list.some((a) => a.agentId === selectedAgentId)
          ? selectedAgentId
          : (list[0]?.agentId ?? "");

      if (!agentId) {
        setMetrics(null);
        setState("ready");
        return;
      }

      await loadMetrics(accessToken, agentId, timeRange);
      setState("ready");
    } catch (err) {
      if (err instanceof MetricsApiError && err.status === 401) {
        setState("auth");
        setErrorMessage("Session expired. Sign in again or paste a new token.");
        return;
      }
      setState("error");
      setErrorMessage(err instanceof Error ? err.message : "Failed to load data");
    }
  }, [
    dashboardView,
    loadAgents,
    loadFleetMetrics,
    loadMetrics,
    resolveToken,
    selectedAgentId,
    timeRange,
  ]);

  // Keep a stable ref so the auto-refresh interval doesn't re-bind on every render.
  useEffect(() => {
    refreshRef.current = () => {
      void refresh();
    };
  }, [refresh]);

  useEffect(() => {
    if (!token) {
      setState("auth");
      return;
    }
    void refresh();
  }, [token]); // eslint-disable-line react-hooks/exhaustive-deps

  // "X seconds ago" ticker — independent of fetch cadence; cheap state update.
  useEffect(() => {
    const id = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(id);
  }, []);

  // Auto-refresh interval — opt-in via toolbar toggle.
  useEffect(() => {
    if (!autoRefresh) return;
    const id = window.setInterval(() => refreshRef.current(), AUTO_REFRESH_MS);
    return () => window.clearInterval(id);
  }, [autoRefresh]);

  const handleDevTokenSave = () => {
    if (!devTokenInput.trim()) {
      return;
    }
    setDevAccessToken(devTokenInput.trim());
    setToken(devTokenInput.trim());
    setState("loading");
  };

  const handleDevTokenClear = () => {
    clearDevAccessToken();
    setDevTokenInput("");
    setToken(null);
    setState("auth");
  };

  if (state === "auth") {
    return (
      <main id="main" className="dashboard" aria-labelledby="dashboard-title">
        <header className="dashboard__header">
          <p className="dashboard__eyebrow">Nasiko · Titan Builder Challenge</p>
          <h1 id="dashboard-title" className="dashboard__title">Agent Metrics</h1>
        </header>
        <div className="state-box" role="region" aria-labelledby="auth-heading">
          <h2 id="auth-heading">Sign in required</h2>
          <p>
            Open the main Nasiko app on the <strong>same host</strong> as this page
            so your session token is available, or paste a JWT for local dev on port
            3003.
          </p>
          <p>
            <a href={NASIKO_APP_URL}>Sign in at {NASIKO_APP_URL}</a>
          </p>
          <div className="auth-panel">
            <label htmlFor="dev-token">Dev token (local port 3003 only)</label>
            <input
              id="dev-token"
              type="password"
              autoComplete="off"
              placeholder="Paste access_token from nasiko-credentials-*.json"
              value={devTokenInput}
              onChange={(e) => setDevTokenInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleDevTokenSave();
              }}
            />
            <small>
              From DevTools → Application → Local Storage on{" "}
              <code>localhost:9100</code>, key <code>nasiko-credentials-…</code>, field{" "}
              <code>access_token</code>.
            </small>
            <div className="auth-panel__actions">
              <button
                type="button"
                className="btn btn--primary"
                onClick={handleDevTokenSave}
                disabled={!devTokenInput.trim()}
              >
                Use token
              </button>
              <button type="button" className="btn" onClick={handleDevTokenClear}>
                Clear
              </button>
            </div>
          </div>
        </div>
      </main>
    );
  }

  const isFleetView = dashboardView === "fleet";
  const displayMetrics = isFleetView ? fleetMetrics : metrics;
  const emptyAgents = state === "ready" && agents.length === 0;
  const noData = displayMetrics?.no_data === true;
  const isLoading = state === "loading";
  const relativeLabel = formatRelative(lastUpdated, now);
  const absoluteLabel = lastUpdated ? lastUpdated.toLocaleTimeString() : "—";

  return (
    <>
      <a href="#main" className="skip-link">Skip to dashboard</a>
      <main id="main" className="dashboard" aria-labelledby="dashboard-title">
        <header className="dashboard__header">
          <p className="dashboard__eyebrow">Nasiko agent performance</p>
          <h1 id="dashboard-title" className="dashboard__title">
            {isFleetView ? "How are your agents doing?" : "How is your agent doing?"}
          </h1>
          <p className="dashboard__subtitle">
            {isFleetView
              ? "Fleet-wide health for the last 24 hours — totals across every deployed agent."
              : "A simple health check for the last 24 hours — speed, success rate, and availability."}
          </p>
        </header>

        <div className="dashboard-sections">
          <DashboardNav view={dashboardView} onChange={handleDashboardViewChange} />
          <Link href="/logs" className="dashboard-nav__link">
            Platform logs →
          </Link>
        </div>

        <div className="toolbar" role="toolbar" aria-label="Dashboard controls">
          {!isFleetView && (
          <div className="field">
            <label htmlFor="agent-select">Agent</label>
            <select
              id="agent-select"
              value={selectedAgentId}
              disabled={isLoading || agents.length === 0}
              onChange={(e) => {
                const id = e.target.value;
                setSelectedAgentId(id);
                setSelectedSessionId(null);
                setTrace(null);
                setTraceError(null);
                if (token && id) {
                  void loadMetricsForAgent(token, id);
                }
              }}
            >
              {agents.length === 0 ? (
                <option value="">No agents deployed</option>
              ) : (
                agents.map((agent) => (
                  <option key={agent.agentId} value={agent.agentId}>
                    {agent.name}
                  </option>
                ))
              )}
            </select>
          </div>
          )}

          <div className="toolbar__actions">
            <button
              type="button"
              className="btn btn--primary"
              disabled={isLoading || (!isFleetView && !selectedAgentId)}
              onClick={() => void refresh()}
              aria-label={isLoading ? "Refreshing metrics" : "Refresh metrics now"}
            >
              {isLoading ? "Refreshing…" : "Refresh"}
            </button>

            <label className="toggle">
              <input
                type="checkbox"
                checked={autoRefresh}
                onChange={(e) => setAutoRefresh(e.target.checked)}
                aria-describedby="auto-refresh-hint"
              />
              <span>Auto refresh</span>
            </label>
            <span id="auto-refresh-hint" className="sr-only">
              When enabled, the dashboard reloads every 60 seconds.
            </span>

            <a className="btn" href={NASIKO_APP_URL}>
              Main app
            </a>
          </div>

          <div
            className="status-pill"
            aria-live="polite"
            data-state={state}
            title={lastUpdated ? `Last updated ${lastUpdated.toLocaleString()}` : "No data yet"}
          >
            <span className={`status-pill__dot status-pill__dot--${state}`} aria-hidden="true" />
            <span className="status-pill__text">
              {isLoading
                ? "Loading…"
                : lastUpdated
                  ? `Updated ${relativeLabel}`
                  : "Awaiting data"}
            </span>
            {lastUpdated && (
              <span className="status-pill__abs" aria-hidden="true">
                · {absoluteLabel}
              </span>
            )}
          </div>
        </div>

        {state === "error" && errorMessage && (
          <div
            className="state-box state-box--error"
            role="alert"
            aria-live="assertive"
            style={{ marginBottom: "1rem" }}
          >
            <h2>Could not load metrics</h2>
            <p>{errorMessage}</p>
            <button type="button" className="btn btn--primary" onClick={() => void refresh()}>
              Retry
            </button>
          </div>
        )}

        {emptyAgents && (
          <div className="state-box" role="status" aria-live="polite" style={{ marginBottom: "1rem" }}>
            <h2>No agents deployed yet</h2>
            <p>
              Once you deploy an agent in the main Nasiko app, it shows up here.
              The dashboard then tracks its latency, success/error counts, and uptime
              over the last 24 hours.
            </p>
            <p>
              <a className="btn btn--primary" href={NASIKO_APP_URL}>Open main app</a>
            </p>
          </div>
        )}

        <MetricsTimeRangeBar
          range={timeRange}
          onApply={(range) => void handleApplyTimeRange(range)}
          disabled={isLoading || emptyAgents}
        />

        {noData && (
          <div className="state-box" role="status" aria-live="polite" style={{ marginBottom: "1rem" }}>
            <h2>
              {isFleetView ? "No activity across agents yet" : "No customer activity yet"}
            </h2>
            <p>
              {displayMetrics?.message ??
                (isFleetView
                  ? "Once your agents handle requests, fleet totals and per-agent breakdowns appear here."
                  : "Once people start using this agent in the main app, you will see speed, success rate, and recent requests here.")}
            </p>
            <p>
              <a className="btn" href={NASIKO_APP_URL}>
                {isFleetView ? "Open main app" : "Chat with the agent"}
              </a>
            </p>
          </div>
        )}

        <section aria-labelledby="kpi-heading" aria-busy={isLoading}>
          <h2 id="kpi-heading" className="section-heading">
            At a glance
          </h2>
          <KpiCards metrics={displayMetrics} loading={isLoading} noData={noData} />
        </section>

        {isFleetView ? (
          <AgentsOverviewTable
            rows={fleetRows}
            loading={isLoading}
            onSelectAgent={handleDrillToAgent}
          />
        ) : (
          <>
            <TrafficChart
              series={metrics?.series_24h ?? []}
              loading={isLoading}
              noData={noData}
            />

            <RequestTracesWorkspace
              sessions={sessions}
              globalRange={timeRange}
              loading={sessionsLoading || isLoading}
              selectedSessionId={selectedSessionId}
              traceLoadingSessionId={traceLoadingSessionId}
              trace={trace}
              traceLoading={traceLoading}
              traceError={traceError}
              onSelectSession={(id) => void handleViewTrace(id)}
            />
          </>
        )}

        <p className="footer-meta" aria-live="polite">
          Window {displayMetrics?.window ?? timeRange.window}
          {isFleetView
            ? agents.length > 0
              ? ` · All agents (${agents.length})`
              : ""
            : displayMetrics?.agent
              ? ` · ${displayMetrics.agent}`
              : ""}
          {lastUpdated && ` · Last updated ${lastUpdated.toLocaleString()}`}
        </p>
      </main>
    </>
  );
}
