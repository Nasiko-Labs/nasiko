"use client";

import type { LogLevel } from "@/lib/types";

type LoadState = "idle" | "loading" | "ready" | "error" | "auth";

type Props = {
  activeLevels: Set<LogLevel>;
  levelCounts: Record<LogLevel, number>;
  onToggleLevel: (level: LogLevel) => void;
  onSelectAll: () => void;
  search: string;
  onSearchChange: (next: string) => void;
  isLoading: boolean;
  autoRefresh: boolean;
  onAutoRefreshChange: (next: boolean) => void;
  onRefresh: () => void;
  status: LoadState;
  relativeLabel: string;
  absoluteLabel: string;
  lastUpdated: Date | null;
};

const LEVELS: { level: LogLevel; label: string; cls: string }[] = [
  { level: "INFO", label: "Info", cls: "log-chip--info" },
  { level: "WARNING", label: "Warning", cls: "log-chip--warn" },
  { level: "ERROR", label: "Error", cls: "log-chip--error" },
];

export function LogsToolbar({
  activeLevels,
  levelCounts,
  onToggleLevel,
  onSelectAll,
  search,
  onSearchChange,
  isLoading,
  autoRefresh,
  onAutoRefreshChange,
  onRefresh,
  status,
  relativeLabel,
  absoluteLabel,
  lastUpdated,
}: Props) {
  const allActive = LEVELS.every((l) => activeLevels.has(l.level));
  return (
    <div className="toolbar" role="toolbar" aria-label="Log filters">
      <div className="log-filter" role="group" aria-label="Log level filter">
        <button
          type="button"
          className={`log-chip log-chip--all ${allActive ? "log-chip--active" : ""}`}
          onClick={onSelectAll}
          aria-pressed={allActive}
        >
          All
        </button>
        {LEVELS.map((l) => {
          const isActive = activeLevels.has(l.level);
          return (
            <button
              key={l.level}
              type="button"
              className={`log-chip ${l.cls} ${isActive ? "log-chip--active" : ""}`}
              onClick={() => onToggleLevel(l.level)}
              aria-pressed={isActive}
            >
              {l.label}
              <span className="log-chip__count" aria-hidden="true">
                {levelCounts[l.level].toLocaleString()}
              </span>
            </button>
          );
        })}
      </div>

      <div className="field" style={{ minWidth: 220, flex: "1 1 240px" }}>
        <label htmlFor="logs-search">Search</label>
        <input
          id="logs-search"
          type="search"
          className="logs-search"
          placeholder="Filter by message, agent, or session…"
          value={search}
          onChange={(e) => onSearchChange(e.target.value)}
        />
      </div>

      <div className="toolbar__actions">
        <button
          type="button"
          className="btn btn--primary"
          disabled={isLoading}
          onClick={onRefresh}
          aria-label={isLoading ? "Refreshing logs" : "Refresh logs now"}
        >
          {isLoading ? "Refreshing…" : "Refresh"}
        </button>
        <label className="toggle">
          <input
            type="checkbox"
            checked={autoRefresh}
            onChange={(e) => onAutoRefreshChange(e.target.checked)}
            aria-describedby="logs-auto-refresh-hint"
          />
          <span>Auto refresh</span>
        </label>
        <span id="logs-auto-refresh-hint" className="sr-only">
          When enabled, logs reload every 60 seconds.
        </span>
      </div>

      <div
        className="status-pill"
        aria-live="polite"
        data-state={status}
        title={lastUpdated ? `Last updated ${lastUpdated.toLocaleString()}` : "No data yet"}
      >
        <span
          className={`status-pill__dot status-pill__dot--${status}`}
          aria-hidden="true"
        />
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
  );
}
