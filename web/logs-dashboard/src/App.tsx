import { useEffect, useMemo, useState } from "react";

type LogLevel = "INFO" | "WARNING" | "ERROR";
type LevelFilter = "ALL" | LogLevel;

type PlatformLog = {
  timestamp: string;
  level: LogLevel;
  logger: string;
  message: string;
  exception?: string;
};

type LogsResponse = {
  items: PlatformLog[];
  levels: LogLevel[];
  count: number;
};

const levelOptions: LevelFilter[] = ["ALL", "INFO", "WARNING", "ERROR"];

function formatTimestamp(value: string): string {
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(value));
}

export function App() {
  const [logs, setLogs] = useState<PlatformLog[]>([]);
  const [level, setLevel] = useState<LevelFilter>("ALL");
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);

  async function loadLogs(selectedLevel = level) {
    setLoading(true);
    setError(null);

    const params = new URLSearchParams({ limit: "200" });
    if (selectedLevel !== "ALL") {
      params.set("level", selectedLevel);
    }

    const controller = new AbortController();
    const timeoutId = window.setTimeout(() => controller.abort(), 8000);

    try {
      const response = await fetch(
        `${import.meta.env.BASE_URL}api/v1/platform/logs?${params.toString()}`,
        { signal: controller.signal },
      );
      if (!response.ok) {
        throw new Error(`Request failed with ${response.status}`);
      }
      const data = (await response.json()) as LogsResponse;
      setLogs(data.items);
      setLastUpdated(new Date());
    } catch (caughtError) {
      setError(
        caughtError instanceof DOMException && caughtError.name === "AbortError"
          ? "Log API request timed out"
          : caughtError instanceof Error
          ? caughtError.message
          : "Unable to load platform logs",
      );
    } finally {
      window.clearTimeout(timeoutId);
      setLoading(false);
    }
  }

  useEffect(() => {
    void loadLogs();
    const timer = window.setInterval(() => void loadLogs(), 15000);
    return () => window.clearInterval(timer);
  }, []);

  const filteredLogs = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase();
    return logs.filter((log) => {
      if (level !== "ALL" && log.level !== level) {
        return false;
      }

      if (!normalizedQuery) {
        return true;
      }

      return [log.timestamp, log.level, log.logger, log.message, log.exception ?? ""]
        .join(" ")
        .toLowerCase()
        .includes(normalizedQuery);
    });
  }, [level, logs, query]);

  const counts = useMemo(() => {
    return logs.reduce(
      (acc, log) => {
        acc[log.level] += 1;
        return acc;
      },
      { INFO: 0, WARNING: 0, ERROR: 0 } satisfies Record<LogLevel, number>,
    );
  }, [logs]);

  const activeFilterLabel =
    level === "ALL" ? "All platform logs" : `${level} logs only`;

  function handleLevelChange(nextLevel: LevelFilter) {
    setLevel(nextLevel);
    void loadLogs(nextLevel);
  }

  return (
    <main className="shell">
      <section className="topbar">
        <div>
          <p className="eyebrow">Nasiko Platform</p>
          <h1>Recent Logs</h1>
          <p className="lede">
            Current platform events from backend services and agent operations.
          </p>
        </div>
        <div className="topbarActions">
          <span className="liveBadge">Live</span>
          <span className="updated" aria-live="polite">
            {lastUpdated
              ? `Updated ${formatTimestamp(lastUpdated.toISOString())}`
              : "Waiting"}
          </span>
          <button
            className="refreshButton"
            onClick={() => void loadLogs()}
            disabled={loading}
          >
            Refresh
          </button>
        </div>
      </section>

      <section className="summaryGrid" aria-label="Log summary">
        <div className="summaryTile total">
          <span>Total</span>
          <strong>{logs.length}</strong>
        </div>
        <div className="summaryTile info">
          <span>Info</span>
          <strong>{counts.INFO}</strong>
        </div>
        <div className="summaryTile warning">
          <span>Warnings</span>
          <strong>{counts.WARNING}</strong>
        </div>
        <div className="summaryTile error">
          <span>Errors</span>
          <strong>{counts.ERROR}</strong>
        </div>
      </section>

      <section className="toolbar" aria-label="Log filters">
        <div>
          <p className="toolbarLabel">{activeFilterLabel}</p>
          <div className="segmentedControl">
            {levelOptions.map((option) => (
              <button
                key={option}
                className={level === option ? "active" : ""}
                onClick={() => handleLevelChange(option)}
              >
                {option}
              </button>
            ))}
          </div>
        </div>
        <label className="searchBox">
          <span>Search</span>
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Message or logger"
            aria-label="Search logs"
          />
        </label>
      </section>

      <section className="tableWrap">
        <div className="tableHeader">
          <div>
            <h2>Log Events</h2>
            <p>
              Showing {filteredLogs.length} of {logs.length} captured events
            </p>
          </div>
          <span className={`filterPill ${level.toLowerCase()}`}>{level}</span>
        </div>
        {error ? <div className="state errorState">{error}</div> : null}
        {!error && loading && logs.length === 0 ? (
          <div className="state">
            <span className="spinner" />
            Loading logs...
          </div>
        ) : null}
        {!error && !loading && filteredLogs.length === 0 ? (
          <div className="state emptyState">
            <strong>No matching logs</strong>
            <span>Adjust the level filter or search text.</span>
          </div>
        ) : null}
        {filteredLogs.length > 0 ? (
          <table>
            <thead>
              <tr>
                <th>Timestamp</th>
                <th>Level</th>
                <th>Logger</th>
                <th>Message</th>
              </tr>
            </thead>
            <tbody>
              {filteredLogs.map((log, index) => (
                <tr key={`${log.timestamp}-${log.logger}-${index}`}>
                  <td className="timestamp">{formatTimestamp(log.timestamp)}</td>
                  <td>
                    <span className={`levelBadge ${log.level.toLowerCase()}`}>
                      {log.level}
                    </span>
                  </td>
                  <td className="logger">{log.logger}</td>
                  <td className="message">
                    <span>{log.message}</span>
                    {log.exception ? <pre>{log.exception}</pre> : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : null}
      </section>
    </main>
  );
}
