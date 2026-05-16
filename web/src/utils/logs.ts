import {
  LOG_LEVELS,
  type LevelFilter,
  type LogLevel,
  type LogStats,
  type ParsedQuery,
  type PlatformLog,
  type PlatformLogApiItem,
  type QuickFilter,
  type TimeWindow
} from "../types";

export const TIME_WINDOW_MS: Record<TimeWindow, number> = {
  "15m": 15 * 60 * 1000,
  "1h": 60 * 60 * 1000,
  "24h": 24 * 60 * 60 * 1000
};

const QUERY_FIELD_ALIASES = {
  level: "level",
  lvl: "level",
  service: "service",
  svc: "service",
  route: "route",
  path: "route",
  trace: "trace",
  traceid: "trace",
  request: "request",
  req: "request",
  requestid: "request",
  pod: "pod",
  source: "source",
  src: "source"
} as const;

type QueryFieldAlias = keyof typeof QUERY_FIELD_ALIASES;

export function formatClock(value: string) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false
  }).format(new Date(value));
}

export function formatFullTimestamp(value: string) {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "medium"
  }).format(new Date(value));
}

export function getLatestTimestamp(logs: PlatformLog[]) {
  if (logs.length === 0) {
    return Date.now();
  }

  return Math.max(...logs.map((log) => new Date(log.timestamp).getTime()));
}

export function getServices(logs: PlatformLog[]) {
  return Array.from(new Set(logs.map((log) => log.service))).sort();
}

export function normalizeApiLogs(items: PlatformLogApiItem[]): PlatformLog[] {
  return items.map((item, index) => {
    const id = item.id ?? `api-log-${index}`;
    const logger = item.logger ?? item.service ?? "platform";
    const service = item.service ?? serviceFromLogger(logger);
    const traceId = item.traceId ?? item.trace_id ?? id;
    const requestId = item.requestId ?? item.request_id ?? id;

    return {
      id,
      timestamp: item.timestamp,
      level: item.level,
      logger,
      service,
      route: item.route ?? logger,
      message: item.message,
      exception: item.exception,
      traceId,
      requestId,
      latencyMs: item.latencyMs ?? item.latency_ms ?? 0,
      statusCode: item.statusCode ?? item.status_code,
      pod: item.pod ?? service,
      source: item.source ?? logger,
      commit: item.commit ?? "runtime"
    };
  });
}

export function getLogStats(logs: PlatformLog[]): LogStats {
  const counts = logs.reduce<Record<LogLevel, number>>(
    (accumulator, log) => {
      accumulator[log.level] += 1;
      return accumulator;
    },
    { INFO: 0, WARNING: 0, ERROR: 0 }
  );

  return {
    total: logs.length,
    errors: counts.ERROR,
    warnings: counts.WARNING,
    info: counts.INFO,
    p95LatencyMs: percentile(logs.map((log) => log.latencyMs), 0.95)
  };
}

export function parseQuery(query: string): ParsedQuery {
  const tokens = query.match(/"[^"]+"|\S+/g) ?? [];

  return tokens.reduce<ParsedQuery>(
    (parsed, rawToken) => {
      const token = rawToken.replace(/^"|"$/g, "");
      const separatorIndex = token.indexOf(":");

      if (separatorIndex > 0) {
        const key = token.slice(0, separatorIndex).toLowerCase();
        const value = token.slice(separatorIndex + 1).trim();

        if (key === "slow") {
          const slowAboveMs = Number(value.replace(/[^\d.]/g, ""));
          if (Number.isFinite(slowAboveMs)) {
            parsed.slowAboveMs = slowAboveMs;
          }
          return parsed;
        }

        if (isQueryFieldAlias(key) && value.length > 0) {
          parsed.fields[QUERY_FIELD_ALIASES[key]] = value.toLowerCase();
          return parsed;
        }
      }

      if (token.length > 0) {
        parsed.terms.push(token.toLowerCase());
      }

      return parsed;
    },
    { fields: {}, terms: [] }
  );
}

export function filterLogs(
  logs: PlatformLog[],
  options: {
    level: LevelFilter;
    service: string;
    timeWindow: TimeWindow;
    query: string;
    quickFilter: QuickFilter;
  }
) {
  const latestTimestamp = getLatestTimestamp(logs);
  const minTimestamp = latestTimestamp - TIME_WINDOW_MS[options.timeWindow];
  const parsedQuery = parseQuery(options.query);

  return logs.filter((log) => {
    return (
      new Date(log.timestamp).getTime() >= minTimestamp &&
      matchesLevel(log, options.level) &&
      matchesService(log, options.service) &&
      matchesQuickFilter(log, options.quickFilter) &&
      matchesParsedQuery(log, parsedQuery)
    );
  });
}

export function createEventJson(log: PlatformLog) {
  return JSON.stringify(
    {
      timestamp: log.timestamp,
      level: log.level,
      service: log.service,
      route: log.route,
      message: log.message,
      trace_id: log.traceId,
      request_id: log.requestId,
      latency_ms: log.latencyMs,
      status_code: log.statusCode,
      pod: log.pod,
      source: log.source,
      commit: log.commit
    },
    null,
    2
  );
}

export function createKubectlCommand(log: PlatformLog) {
  return `kubectl logs -n nasiko pod/${log.pod} --since=15m | rg "${log.traceId}|${log.requestId}"`;
}

export function createReplayCommand(log: PlatformLog) {
  const routeMatch = log.route.match(/^([A-Z]+)\s+(\S+)/);
  const method = routeMatch?.[1] ?? "GET";
  const path = routeMatch?.[2] ?? "/";
  const normalizedPath = path.startsWith("/") ? path : "/";

  return [
    `curl -i -X ${method}`,
    `"http://localhost:9100${normalizedPath}"`,
    `-H "Authorization: Bearer $NASIKO_TOKEN"`,
    `-H "X-Request-ID: ${log.requestId}"`
  ].join(" \\\n  ");
}

function matchesLevel(log: PlatformLog, level: LevelFilter) {
  return level === "ALL" || log.level === level;
}

function matchesService(log: PlatformLog, service: string) {
  return service === "ALL" || log.service === service;
}

function matchesQuickFilter(log: PlatformLog, filter: QuickFilter) {
  switch (filter) {
    case "errors":
      return log.level === "ERROR";
    case "slow":
      return log.latencyMs >= 1000;
    case "gateway":
      return log.service.includes("gateway") || log.route.includes("/agents/");
    case "builds":
      return log.service.includes("worker") || log.route.includes("build");
    case "all":
    default:
      return true;
  }
}

function matchesParsedQuery(log: PlatformLog, parsedQuery: ParsedQuery) {
  if (parsedQuery.slowAboveMs !== undefined && log.latencyMs < parsedQuery.slowAboveMs) {
    return false;
  }

  const fieldChecks = [
    ["level", log.level],
    ["service", log.service],
    ["route", log.route],
    ["trace", log.traceId],
    ["request", log.requestId],
    ["pod", log.pod],
    ["source", log.source]
  ] as const;

  for (const [field, value] of fieldChecks) {
    const expected = parsedQuery.fields[field];
    if (expected && !value.toLowerCase().includes(expected)) {
      return false;
    }
  }

  const haystack = [
    log.id,
    log.timestamp,
    log.level,
    log.service,
    log.logger,
    log.route,
    log.message,
    log.exception,
    log.traceId,
    log.requestId,
    log.pod,
    log.source,
    log.commit,
    log.statusCode
  ]
    .join(" ")
    .toLowerCase();

  return parsedQuery.terms.every((term) => haystack.includes(term));
}

function percentile(values: number[], probability: number) {
  if (values.length === 0) {
    return 0;
  }

  const sorted = [...values].sort((a, b) => a - b);
  const index = Math.ceil(sorted.length * probability) - 1;
  return sorted[Math.max(0, index)];
}

function isQueryFieldAlias(value: string): value is QueryFieldAlias {
  return value in QUERY_FIELD_ALIASES;
}

function serviceFromLogger(loggerName: string) {
  const parts = loggerName.split(".");
  if (parts[0] === "app" && parts[1]) {
    return parts[1];
  }

  return parts[0] || "platform";
}

export function levelCount(logs: PlatformLog[], level: LogLevel) {
  return logs.filter((log) => log.level === level).length;
}

export function isKnownLevel(value: string): value is LogLevel {
  return LOG_LEVELS.includes(value as LogLevel);
}
