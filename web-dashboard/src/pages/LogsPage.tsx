import { useCallback, useEffect, useState } from "react";
import { fetchPlatformLogs, LogLevel, PlatformLogEntry } from "../api/client";
import { useAuth } from "../context/AuthContext";
import { LogsTable } from "../components/LogsTable";
import { NasikoShell } from "../components/NasikoShell";

export function LogsPage() {
  const { token, logout } = useAuth();
  const [levelFilter, setLevelFilter] = useState<LogLevel>("");
  const [logs, setLogs] = useState<PlatformLogEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(true);

  const loadLogs = useCallback(async () => {
    if (!token) return;
    setLoading(true);
    setError(null);
    try {
      const data = await fetchPlatformLogs(token, levelFilter);
      setLogs(data.logs);
      setTotal(data.total);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load logs");
    } finally {
      setLoading(false);
    }
  }, [token, levelFilter]);

  useEffect(() => {
    void loadLogs();
  }, [loadLogs]);

  useEffect(() => {
    if (!autoRefresh || !token) return;
    const id = window.setInterval(() => void loadLogs(), 5000);
    return () => window.clearInterval(id);
  }, [autoRefresh, token, loadLogs]);

  const actions = (
    <>
      <label className="toggle">
        <input
          type="checkbox"
          checked={autoRefresh}
          onChange={(e) => setAutoRefresh(e.target.checked)}
        />
        Auto-refresh
      </label>
      <button type="button" className="btn ghost" onClick={() => void loadLogs()}>
        Refresh
      </button>
      <button type="button" className="btn ghost" onClick={logout}>
        Sign out
      </button>
    </>
  );

  return (
    <NasikoShell pageTitle="Platform Logs" activeNav="logs" actions={actions}>
      <div className="page-header">
        <h1>Platform logs</h1>
        <p className="subtitle">
          Recent platform activity with timestamps and level filtering.
        </p>
      </div>
      <main>
        <div className="toolbar">
          <div className="filters" role="group" aria-label="Log level filter">
            {(["", "INFO", "WARNING", "ERROR"] as const).map((level) => (
              <button
                key={level || "all"}
                type="button"
                className={`filter-btn ${levelFilter === level ? "active" : ""} ${level ? `level-${level.toLowerCase()}` : ""}`}
                onClick={() => setLevelFilter(level)}
              >
                {level || "All"}
              </button>
            ))}
          </div>
          <span className="count">
            {loading ? "Loading…" : `${logs.length} shown · ${total} total`}
          </span>
        </div>
        {error && <div className="banner error">{error}</div>}
        <LogsTable logs={logs} loading={loading} />
      </main>
    </NasikoShell>
  );
}
