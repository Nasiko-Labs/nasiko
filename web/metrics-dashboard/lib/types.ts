/** Shapes from Nasiko `GET /api/v1/registry/user/agents` (SimpleUserAgentsResponse). */

export type NasikoSimpleUserAgent = {
  agent_id: string;
  name: string;
  icon_url?: string | null;
  tags?: string[];
  description?: string | null;
};

export type NasikoSimpleUserAgentsResponse = {
  data: NasikoSimpleUserAgent[];
  status_code: number;
  message: string;
};

/** Normalized agent for the metrics UI dropdown. */
export type MetricsAgentOption = {
  agentId: string;
  name: string;
  description?: string | null;
  tags: string[];
};

/** Nasiko `GET /api/v1/observability/agent/{id}/stats` → `data.project`. */
export type NasikoAgentProjectStats = {
  trace_count?: number;
  latency_ms_p50?: number | null;
  latency_ms_p99?: number | null;
  cost_summary?: {
    total?: { cost?: number };
    prompt?: { cost?: number };
    completion?: { cost?: number };
  };
  span_annotation_names?: string[];
  id?: string;
};

export type NasikoAgentStatsResponse = {
  data: {
    project: NasikoAgentProjectStats;
  };
};

/** Nasiko `GET /api/v1/observability/session/list` session node (snake_case). */
export type NasikoObservabilitySession = {
  agent_id?: string;
  session_id?: string;
  num_traces?: number;
  start_time?: string;
  end_time?: string;
  trace_latency_ms_p50?: number | null;
  trace_latency_ms_p99?: number | null;
  last_output?: { value?: string } | string | null;
  session_annotation_summaries?: Array<{
    name?: string;
    mean_score?: number | null;
    label_fractions?: Array<{ label?: string; fraction?: number }>;
  }>;
};

export type NasikoObservabilitySessionsResponse = {
  data: {
    sessions: NasikoObservabilitySession[];
    total_agents?: number;
    successful_agents?: number;
  };
};

export type MetricsSourceMeta = {
  trace_count: number;
  latency_ms_p50: number | null;
  latency_ms_p99: number | null;
  project_id?: string | null;
  cost_usd?: number | null;
  session_count?: number;
  uptime?: {
    successful_checks: number;
    recorded_checks: number;
    expected_checks_24h: number;
    poll_interval_minutes: number;
    current_health: boolean;
  };
  /**
   * Tier B — User-feedback / quality scores aggregated from
   * `session_annotation_summaries[*].mean_score` (e.g. thumbs/ratings).
   * `null` mean_score = no annotated sessions in window.
   */
  quality?: {
    mean_score: number | null;
    sample_count: number;
    /**
     * Top label fractions across all annotated sessions in the window
     * (sorted descending by fraction; capped to 4 entries).
     */
    top_labels: Array<{ label: string; fraction: number }>;
    /**
     * The annotation name we surfaced (first that produced a score).
     * Hint for the UI when there are multiple annotation streams.
     */
    primary_annotation?: string | null;
  };
  /**
   * Tier B — Orchestration depth: how many spans/traces per user request,
   * a proxy for tool-call complexity. From `session.num_traces`.
   */
  orchestration?: {
    avg_traces_per_session: number | null;
    total_sessions: number;
    total_traces: number;
  };
  /**
   * Tier B — Prompt vs completion cost split from Phoenix `cost_summary`.
   * Either half may be null if Phoenix didn't report it.
   */
  cost_breakdown?: {
    prompt_usd: number | null;
    completion_usd: number | null;
    total_usd: number | null;
  };
};

export type MetricsHourlyPoint = {
  ts: string;
  latency_ms: number | null;
  success: number | null;
  error: number | null;
};

/** Dashboard JSON from `GET /api/metrics` (B4 contract; B5–B6 fill series/uptime). */
export type AgentMetricsResponse = {
  agent: string;
  window: string;
  avg_latency_ms: number | null;
  success: number;
  error: number;
  uptime_pct: number | null;
  series_24h: MetricsHourlyPoint[];
  source?: MetricsSourceMeta;
  /** True when Phoenix project missing or no traces in window. */
  no_data?: boolean;
  message?: string;
};

/** Row for sessions table (from `/api/sessions`). */
export type MetricsSessionRow = {
  session_id: string;
  start_time: string | null;
  num_traces: number;
  latency_ms_p50: number | null;
  status: "ok" | "error" | "unknown";
  preview: string | null;
};

export type SessionsListResponse = {
  agent: string;
  window: string;
  sessions: MetricsSessionRow[];
};

export type SessionTraceRef = {
  id?: string;
  trace_id: string;
  latency_ms?: number | null;
};

export type SessionDetailResponse = {
  session_id: string;
  num_traces: number;
  traces: SessionTraceRef[];
  project_id: string | null;
};

export type TraceSpanNode = {
  id: string;
  span_id?: string;
  name?: string;
  span_kind?: string;
  status_code?: string;
  latency_ms?: number | null;
  children?: TraceSpanNode[];
};

export type TraceGraphNode = {
  id: string;
  position: { x: number; y: number };
  data: {
    label: string;
    latency_ms: number | null;
    status_code: string | null;
    span_kind: string | null;
    /** n8n-style workflow node type for the trace canvas */
    node_kind?: "user" | "llm" | "agent" | "tool" | "step";
    tone?: "ok" | "error" | "neutral";
    subtitle?: string;
  };
};

export type TraceGraphEdge = {
  id: string;
  source: string;
  target: string;
};

export type TraceDetailResponse = {
  agent: string;
  trace_id: string;
  latency_ms: number | null;
  num_spans: number;
  nodes: TraceGraphNode[];
  edges: TraceGraphEdge[];
  truncated: boolean;
};

/**
 * Challenge 1 — Platform / activity log entries synthesized server-side from
 * existing observability + registry data. We never call Nasiko logging APIs
 * directly; each row is derived from a session lifecycle event or a registry
 * snapshot.
 */
export type LogLevel = "INFO" | "WARNING" | "ERROR";

export type SyntheticLogRow = {
  id: string;
  ts: string; // ISO-8601
  level: LogLevel;
  service: string; // e.g. "agent:a2a-translator", "platform", "registry"
  message: string;
  agent_id?: string | null;
  session_id?: string | null;
  latency_ms?: number | null;
  detail?: Record<string, unknown> | null;
};

export type LogsResponse = {
  window: string;
  generated_at: string; // ISO-8601
  total: number;
  source_counts: {
    sessions: number;
    registry: number;
  };
  logs: SyntheticLogRow[];
};
