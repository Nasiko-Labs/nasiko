import { CheckCircle2, Info, RefreshCw, Search, TriangleAlert, XCircle } from "lucide-react";
import {
  LEVEL_FILTERS,
  TIME_WINDOWS,
  type LevelFilter,
  type PlatformLog,
  type TimeWindow
} from "../types";
import { levelCount } from "../utils/logs";

type CommandBarProps = {
  autoRefresh: boolean;
  isRefreshing: boolean;
  logs: PlatformLog[];
  query: string;
  selectedLevel: LevelFilter;
  selectedService: string;
  services: string[];
  timeWindow: TimeWindow;
  onAutoRefreshChange: (enabled: boolean) => void;
  onLevelChange: (level: LevelFilter) => void;
  onQueryChange: (query: string) => void;
  onRefresh: () => void;
  onServiceChange: (service: string) => void;
  onTimeWindowChange: (window: TimeWindow) => void;
};

const levelIcons = {
  ALL: CheckCircle2,
  INFO: Info,
  WARNING: TriangleAlert,
  ERROR: XCircle
};

export function CommandBar({
  autoRefresh,
  isRefreshing,
  logs,
  onAutoRefreshChange,
  onLevelChange,
  onQueryChange,
  onRefresh,
  onServiceChange,
  onTimeWindowChange,
  query,
  selectedLevel,
  selectedService,
  services,
  timeWindow
}: CommandBarProps) {
  return (
    <section className="command-bar" aria-label="Log controls">
      <label className="query-field">
        <Search size={16} aria-hidden="true" />
        <span className="sr-only">Search logs</span>
        <input
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder="Search or use level:error service:kong"
          type="search"
          value={query}
        />
      </label>

      <div className="level-tabs" aria-label="Filter logs by level">
        {LEVEL_FILTERS.map((level) => {
          const Icon = levelIcons[level];
          const count = level === "ALL" ? logs.length : levelCount(logs, level);

          return (
            <button
              aria-pressed={selectedLevel === level}
              key={level}
              onClick={() => onLevelChange(level)}
              type="button"
            >
              <Icon size={14} aria-hidden="true" />
              <span>{level}</span>
              <strong>{count}</strong>
            </button>
          );
        })}
      </div>

      <label className="service-select">
        <span>Service</span>
        <select onChange={(event) => onServiceChange(event.target.value)} value={selectedService}>
          <option value="ALL">All services</option>
          {services.map((service) => (
            <option key={service} value={service}>
              {service}
            </option>
          ))}
        </select>
      </label>

      <div className="time-tabs" aria-label="Filter logs by time window">
        {TIME_WINDOWS.map((window) => (
          <button
            aria-pressed={timeWindow === window}
            key={window}
            onClick={() => onTimeWindowChange(window)}
            type="button"
          >
            {window}
          </button>
        ))}
      </div>

      <label className="auto-refresh-toggle">
        <input
          checked={autoRefresh}
          onChange={(event) => onAutoRefreshChange(event.target.checked)}
          type="checkbox"
        />
        <span>Live tail</span>
      </label>

      <button
        className={isRefreshing ? "icon-button is-refreshing" : "icon-button"}
        disabled={isRefreshing}
        onClick={onRefresh}
        title="Refresh"
        type="button"
      >
        <RefreshCw size={15} aria-hidden="true" />
      </button>
    </section>
  );
}
