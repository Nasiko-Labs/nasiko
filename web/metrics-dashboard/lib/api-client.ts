import { authHeaders } from "@/lib/auth-client";
import { timeRangeQueryString, type MetricsTimeRange } from "@/lib/time-range";
import type {
  AgentMetricsResponse,
  LogsResponse,
  MetricsAgentOption,
  SessionDetailResponse,
  SessionsListResponse,
  TraceDetailResponse,
} from "@/lib/types";

export class MetricsApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
    this.name = "MetricsApiError";
  }
}

async function parseJson<T>(response: Response): Promise<T> {
  const text = await response.text();
  if (!text) {
    return {} as T;
  }
  return JSON.parse(text) as T;
}

export async function fetchAgents(
  token: string,
): Promise<MetricsAgentOption[]> {
  const response = await fetch("/api/agents", {
    headers: authHeaders(token),
    cache: "no-store",
  });
  const body = await parseJson<{
    agents?: MetricsAgentOption[];
    error?: string;
  }>(response);

  if (!response.ok) {
    throw new MetricsApiError(body.error ?? "Failed to load agents", response.status);
  }
  return body.agents ?? [];
}

export async function fetchAgentMetrics(
  token: string,
  agentId: string,
  timeRange?: MetricsTimeRange,
): Promise<AgentMetricsResponse> {
  const qs = timeRange
    ? timeRangeQueryString(timeRange)
    : "window=24h";
  const url = `/api/metrics?agent=${encodeURIComponent(agentId)}&${qs}`;
  const response = await fetch(url, {
    headers: authHeaders(token),
    cache: "no-store",
  });
  const body = await parseJson<AgentMetricsResponse & { error?: string }>(response);

  if (!response.ok) {
    throw new MetricsApiError(body.error ?? "Failed to load metrics", response.status);
  }
  return body;
}

export async function fetchAgentSessions(
  token: string,
  agentId: string,
  timeRange?: MetricsTimeRange,
): Promise<SessionsListResponse> {
  const qs = timeRange
    ? timeRangeQueryString(timeRange)
    : "window=24h";
  const url = `/api/sessions?agent=${encodeURIComponent(agentId)}&${qs}`;
  const response = await fetch(url, {
    headers: authHeaders(token),
    cache: "no-store",
  });
  const body = await parseJson<SessionsListResponse & { error?: string }>(response);
  if (!response.ok) {
    throw new MetricsApiError(body.error ?? "Failed to load sessions", response.status);
  }
  return body;
}

export async function fetchSessionDetail(
  token: string,
  agentId: string,
  sessionId: string,
  window = "24h",
): Promise<SessionDetailResponse> {
  const url = `/api/sessions/${encodeURIComponent(sessionId)}?agent=${encodeURIComponent(agentId)}&window=${encodeURIComponent(window)}`;
  const response = await fetch(url, {
    headers: authHeaders(token),
    cache: "no-store",
  });
  const body = await parseJson<SessionDetailResponse & { error?: string }>(response);
  if (!response.ok) {
    throw new MetricsApiError(body.error ?? "Failed to load session", response.status);
  }
  return body;
}

export async function fetchLogs(
  token: string,
  window = "24h",
  signal?: AbortSignal,
): Promise<LogsResponse> {
  const response = await fetch(
    `/api/logs?window=${encodeURIComponent(window)}`,
    {
      headers: authHeaders(token),
      cache: "no-store",
      signal,
    },
  );
  const body = await parseJson<LogsResponse & { error?: string }>(response);
  if (!response.ok) {
    throw new MetricsApiError(body.error ?? "Failed to load logs", response.status);
  }
  return body;
}

export async function fetchTraceGraph(
  token: string,
  agentId: string,
  traceId: string,
  projectId?: string | null,
  window = "24h",
  userPreview?: string | null,
): Promise<TraceDetailResponse> {
  const params = new URLSearchParams({
    agent: agentId,
    traceId,
    window,
  });
  if (projectId) {
    params.set("projectId", projectId);
  }
  if (userPreview?.trim()) {
    params.set("userPreview", userPreview.trim());
  }
  const response = await fetch(`/api/traces?${params}`, {
    headers: authHeaders(token),
    cache: "no-store",
  });
  const body = await parseJson<TraceDetailResponse & { error?: string }>(response);
  if (!response.ok) {
    throw new MetricsApiError(body.error ?? "Failed to load trace", response.status);
  }
  return body;
}
