const API_BASE = import.meta.env.VITE_API_BASE ?? "";

export interface UserAgent {
  agent_id: string;
  name: string;
  description?: string | null;
}

export interface MetricsHourBucket {
  hour: string;
  label: string;
  trace_count: number;
  session_count: number;
  total_cost: number;
  total_tokens: number;
  avg_latency_ms: number | null;
}

export interface AgentMetricsSummary {
  trace_count: number;
  latency_ms_p50: number | null;
  latency_ms_p99: number | null;
  cost_summary?: {
    total?: { cost?: number };
    prompt?: { cost?: number };
    completion?: { cost?: number };
  };
  sessions_in_range: number;
}

export interface AgentMetricsTimeseries {
  agent_id: string;
  hours: number;
  start_time: string;
  end_time: string;
  series: MetricsHourBucket[];
  summary: AgentMetricsSummary;
}

async function authFetch(url: string, token: string) {
  const res = await fetch(url, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(
      (body as { detail?: string }).detail ?? `Request failed (${res.status})`
    );
  }
  return res.json();
}

export async function fetchUserAgents(token: string): Promise<UserAgent[]> {
  const data = (await authFetch(
    `${API_BASE}/api/v1/registry/user/agents`,
    token
  )) as { data: UserAgent[] };
  return data.data ?? [];
}

export async function fetchAgentMetricsTimeseries(
  token: string,
  agentId: string,
  hours = 24
): Promise<AgentMetricsTimeseries> {
  const params = new URLSearchParams({ hours: String(hours) });
  const data = (await authFetch(
    `${API_BASE}/api/v1/observability/agent/${encodeURIComponent(agentId)}/metrics/timeseries?${params}`,
    token
  )) as { data: AgentMetricsTimeseries };
  return data.data;
}
