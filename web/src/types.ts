export const LOG_LEVELS = ["INFO", "WARNING", "ERROR"] as const;
export const LEVEL_FILTERS = ["ALL", ...LOG_LEVELS] as const;
export const TIME_WINDOWS = ["15m", "1h", "24h"] as const;

export type LogLevel = (typeof LOG_LEVELS)[number];
export type LevelFilter = (typeof LEVEL_FILTERS)[number];
export type TimeWindow = (typeof TIME_WINDOWS)[number];
export type QuickFilter = "all" | "errors" | "slow" | "gateway" | "builds";

export type PlatformLog = {
  id: string;
  timestamp: string;
  level: LogLevel;
  logger?: string;
  service: string;
  route: string;
  message: string;
  exception?: string;
  traceId: string;
  requestId: string;
  latencyMs: number;
  statusCode?: number;
  pod: string;
  source: string;
  commit: string;
};

export type PlatformLogApiItem = {
  id?: string;
  timestamp: string;
  level: LogLevel;
  logger?: string;
  service?: string;
  route?: string;
  message: string;
  exception?: string;
  trace_id?: string;
  traceId?: string;
  request_id?: string;
  requestId?: string;
  latency_ms?: number;
  latencyMs?: number;
  status_code?: number;
  statusCode?: number;
  pod?: string;
  source?: string;
  commit?: string;
};

export type PlatformLogsResponse = {
  items: PlatformLogApiItem[];
  levels: LogLevel[];
  count: number;
  limit: number;
  generated_at?: string;
};

export type ParsedQuery = {
  terms: string[];
  fields: Partial<Record<"level" | "service" | "route" | "trace" | "request" | "pod" | "source", string>>;
  slowAboveMs?: number;
};

export type LogStats = {
  total: number;
  errors: number;
  warnings: number;
  info: number;
  p95LatencyMs: number;
};
