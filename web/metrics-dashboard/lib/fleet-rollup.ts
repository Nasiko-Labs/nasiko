import type { AgentMetricsResponse, MetricsAgentOption } from "@/lib/types";

export type FleetAgentRow = {
  agentId: string;
  name: string;
  metrics: AgentMetricsResponse;
};

export function aggregateFleetMetrics(
  agents: MetricsAgentOption[],
  perAgent: AgentMetricsResponse[],
  window: string,
): { aggregated: AgentMetricsResponse; rows: FleetAgentRow[] } {
  const rows: FleetAgentRow[] = agents.map((agent, i) => ({
    agentId: agent.agentId,
    name: agent.name,
    metrics: perAgent[i] ?? emptyRow(agent.agentId, window),
  }));

  let success = 0;
  let error = 0;
  let cost = 0;
  let latencyWeighted = 0;
  let latencyWeight = 0;
  const uptimeValues: number[] = [];
  let agentsWithData = 0;

  for (const m of perAgent) {
    if (!m.no_data) {
      agentsWithData += 1;
    }
    success += m.success ?? 0;
    error += m.error ?? 0;
    const weight = (m.success ?? 0) + (m.error ?? 0);
    if (m.avg_latency_ms != null && weight > 0) {
      latencyWeighted += m.avg_latency_ms * weight;
      latencyWeight += weight;
    }
    if (m.uptime_pct != null && Number.isFinite(m.uptime_pct)) {
      uptimeValues.push(m.uptime_pct);
    }
    if (m.source?.cost_usd != null && Number.isFinite(m.source.cost_usd)) {
      cost += m.source.cost_usd;
    }
  }

  const avgLatencyMs =
    latencyWeight > 0 ? Math.round(latencyWeighted / latencyWeight) : null;

  const uptimePct =
    uptimeValues.length > 0
      ? Math.round(
          (uptimeValues.reduce((a, b) => a + b, 0) / uptimeValues.length) * 10,
        ) / 10
      : null;

  const aggregated: AgentMetricsResponse = {
    agent: "__all__",
    window,
    avg_latency_ms: avgLatencyMs,
    success,
    error,
    uptime_pct: uptimePct,
    series_24h: [],
    no_data: agentsWithData === 0,
    message:
      agentsWithData === 0
        ? "No agent has traffic in the last 24 hours yet."
        : undefined,
    source: {
      trace_count: perAgent.reduce((n, m) => n + (m.source?.trace_count ?? 0), 0),
      latency_ms_p50: null,
      latency_ms_p99: null,
      cost_usd: cost > 0 ? cost : null,
      session_count: perAgent.reduce(
        (n, m) => n + (m.source?.session_count ?? 0),
        0,
      ),
    },
  };

  return { aggregated, rows };
}

function emptyRow(agentId: string, window: string): AgentMetricsResponse {
  return {
    agent: agentId,
    window,
    avg_latency_ms: null,
    success: 0,
    error: 0,
    uptime_pct: null,
    series_24h: [],
    no_data: true,
  };
}
