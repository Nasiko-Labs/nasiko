"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { fetchLogs, MetricsApiError } from "@/lib/api-client";
import {
  clearDevAccessToken,
  getAccessTokenFromStorage,
  setDevAccessToken,
} from "@/lib/auth-client";
import { countByLevel, filterLogs } from "@/lib/logs";
import type { LogLevel, LogsResponse, SyntheticLogRow } from "@/lib/types";
import { LogsTable } from "@/components/LogsTable";
import { LogsToolbar } from "@/components/LogsToolbar";
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

const ALL_LEVELS: LogLevel[] = ["INFO", "WARNING", "ERROR"];

export function LogsView() {
  const [token, setToken] = useState<string | null>(null);
  const [data, setData] = useState<LogsResponse | null>(null);
  const [state, setState] = useState<LoadState>("idle");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);
  const [devTokenInput, setDevTokenInput] = useState("");
  const [autoRefresh, setAutoRefresh] = useState(false);
  const [now, setNow] = useState(() => new Date());

  // Filters
  const [activeLevels, setActiveLevels] = useState<Set<LogLevel>>(
    () => new Set<LogLevel>(ALL_LEVELS),
  );
  const [search, setSearch] = useState("");

  const refreshRef = useRef<() => void>(() => {});

  const resolveToken = useCallback(() => {
    const t = getAccessTokenFromStorage();
    setToken(t);
    return t;
  }, []);

  useEffect(() => {
    resolveToken();
  }, [resolveToken]);

  const refresh = useCallback(async () => {
    const accessToken = resolveToken();
    if (!accessToken) {
      setState("auth");
      return;
    }
    setState("loading");
    setErrorMessage(null);
    try {
      const body = await fetchLogs(accessToken, "24h");
      setData(body);
      setLastUpdated(new Date());
      setState("ready");
    } catch (err) {
      if (err instanceof MetricsApiError && err.status === 401) {
        setState("auth");
        setErrorMessage("Session expired. Sign in again or paste a new token.");
        return;
      }
      setState("error");
      setErrorMessage(err instanceof Error ? err.message : "Failed to load logs");
    }
  }, [resolveToken]);

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

  useEffect(() => {
    const id = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(id);
  }, []);

  useEffect(() => {
    if (!autoRefresh) return;
    const id = window.setInterval(() => refreshRef.current(), AUTO_REFRESH_MS);
    return () => window.clearInterval(id);
  }, [autoRefresh]);

  const handleDevTokenSave = () => {
    if (!devTokenInput.trim()) return;
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

  const toggleLevel = useCallback((level: LogLevel) => {
    setActiveLevels((prev) => {
      const next = new Set(prev);
      if (next.has(level)) {
        next.delete(level);
        if (next.size === 0) {
          // Don't allow zero-state filter — re-enable all.
          for (const l of ALL_LEVELS) next.add(l);
        }
      } else {
        next.add(level);
      }
      return next;
    });
  }, []);

  const setAllLevels = useCallback(() => {
    setActiveLevels(new Set<LogLevel>(ALL_LEVELS));
  }, []);

  const allLogs: SyntheticLogRow[] = useMemo(
    () => data?.logs ?? [],
    [data],
  );
  const levelCounts = useMemo(() => countByLevel(allLogs), [allLogs]);
  const filtered = useMemo(
    () => filterLogs(allLogs, { levels: activeLevels, search }),
    [allLogs, activeLevels, search],
  );

  const isLoading = state === "loading";
  const relativeLabel = formatRelative(lastUpdated, now);
  const absoluteLabel = lastUpdated ? lastUpdated.toLocaleTimeString() : "—";

  if (state === "auth") {
    return (
      <main id="main" className="dashboard" aria-labelledby="logs-title">
        <header className="dashboard__header">
          <p className="dashboard__eyebrow">Nasiko · Titan Builder Challenge</p>
          <h1 id="logs-title" className="dashboard__title">Platform Logs</h1>
        </header>
        <div className="state-box" role="region" aria-labelledby="auth-heading">
          <h2 id="auth-heading">Sign in required</h2>
          <p>
            Open the main Nasiko app on the <strong>same host</strong> as this page
            so your session token is available, or paste a JWT for local dev.
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
              <code>localhost:9100</code>, key <code>nasiko-credentials-…</code>,
              field <code>access_token</code>.
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

  return (
    <>
      <a href="#main" className="skip-link">Skip to logs</a>
      <main id="main" className="dashboard" aria-labelledby="logs-title">
        <header className="dashboard__header">
          <p className="dashboard__eyebrow">Nasiko · Titan Builder Challenge</p>
          <h1 id="logs-title" className="dashboard__title">Platform Logs</h1>
          <p className="dashboard__subtitle">
            Recent activity across your agents and the platform — last 24 hours.
            Each row is synthesized from observability sessions or the agent
            registry; no separate log pipeline required.
          </p>
        </header>

        <nav className="dashboard-nav" aria-label="Section navigation">
          <Link href="/" className="dashboard-nav__btn">
            ← Agent metrics
          </Link>
          <span
            className="dashboard-nav__btn dashboard-nav__btn--active"
            aria-current="page"
          >
            Platform logs
          </span>
        </nav>

        <LogsToolbar
          activeLevels={activeLevels}
          levelCounts={levelCounts}
          onToggleLevel={toggleLevel}
          onSelectAll={setAllLevels}
          search={search}
          onSearchChange={setSearch}
          isLoading={isLoading}
          autoRefresh={autoRefresh}
          onAutoRefreshChange={setAutoRefresh}
          onRefresh={() => void refresh()}
          status={state}
          relativeLabel={relativeLabel}
          absoluteLabel={absoluteLabel}
          lastUpdated={lastUpdated}
        />

        {state === "error" && errorMessage && (
          <div
            className="state-box state-box--error"
            role="alert"
            aria-live="assertive"
            style={{ marginBottom: "1rem" }}
          >
            <h2>Could not load logs</h2>
            <p>{errorMessage}</p>
            <button type="button" className="btn btn--primary" onClick={() => void refresh()}>
              Retry
            </button>
          </div>
        )}

        {state === "ready" && filtered.length === 0 && allLogs.length === 0 && (
          <div className="state-box" role="status" aria-live="polite">
            <h2>No activity yet</h2>
            <p>
              Once people start using your agents, every request and registry
              event will show up here.
            </p>
            <p>
              <a className="btn btn--primary" href={NASIKO_APP_URL}>
                Open main app
              </a>
            </p>
          </div>
        )}

        {state === "ready" && filtered.length === 0 && allLogs.length > 0 && (
          <div className="state-box" role="status" aria-live="polite">
            <h2>No logs match this filter</h2>
            <p>
              {allLogs.length.toLocaleString()} log{allLogs.length === 1 ? "" : "s"} in
              the window. Try a different level or clear the search.
            </p>
            <button type="button" className="btn" onClick={setAllLevels}>
              Reset filters
            </button>
          </div>
        )}

        {filtered.length > 0 && (
          <LogsTable rows={filtered} loading={isLoading} />
        )}

        <p className="footer-meta" aria-live="polite">
          {data
            ? `${filtered.length.toLocaleString()} of ${data.total.toLocaleString()} logs · sessions ${data.source_counts.sessions.toLocaleString()} · registry ${data.source_counts.registry.toLocaleString()}`
            : "—"}
          {lastUpdated && ` · Last updated ${lastUpdated.toLocaleString()}`}
        </p>
      </main>
    </>
  );
}
