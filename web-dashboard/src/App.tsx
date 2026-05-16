import { useCallback, useEffect, useState } from "react";
import {
  clearStoredToken,
  fetchPlatformLogs,
  getStoredToken,
  login,
  LogLevel,
  PlatformLogEntry,
} from "./api/client";
import { LoginPanel } from "./components/LoginPanel";
import { LogsTable } from "./components/LogsTable";
import { NasikoShell } from "./components/NasikoShell";

export default function App() {
  const [token, setToken] = useState<string | null>(() => getStoredToken());
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

  const handleLogin = async (accessKey: string, accessSecret: string) => {
    const jwt = await login(accessKey, accessSecret);
    setToken(jwt);
  };

  const handleLogout = () => {
    clearStoredToken();
    setToken(null);
    setLogs([]);
    setTotal(0);
  };

  const topbarActions = token ? (
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
      <button type="button" className="btn ghost" onClick={handleLogout}>
        Sign out
      </button>
    </>
  ) : undefined;

  return (
    <NasikoShell actions={topbarActions}>
      {!token ? (
        <LoginPanel onLogin={handleLogin} />
      ) : (
        <>
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
        </>
      )}
    </NasikoShell>
  );
}
