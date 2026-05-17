/**
 * Window helpers and empty-state metrics (rollup in `lib/rollup.ts`).
 */

import type { AgentMetricsResponse, MetricsHourlyPoint } from "@/lib/types";
import { buildHourlyBuckets } from "@/lib/rollup";

const SUPPORTED_WINDOWS = new Set(["1h", "24h", "7d"]);

export function parseMetricsWindow(
  raw: string | null,
): { window: string; hours: number } | { error: string } {
  const window = (raw ?? "24h").trim().toLowerCase();
  if (!SUPPORTED_WINDOWS.has(window)) {
    return {
      error: `Unsupported window "${window}". Supported: ${[...SUPPORTED_WINDOWS].join(", ")}`,
    };
  }
  const hours =
    window === "1h" ? 1 : window === "7d" ? 24 * 7 : 24;
  return { window, hours };
}

/** ISO-8601 UTC for Nasiko `start_time` query param. */
export function computeStartTimeIso(hours: number, now = Date.now()): string {
  return new Date(now - hours * 60 * 60 * 1000).toISOString();
}

function buildEmptySeries24h(now = Date.now()): MetricsHourlyPoint[] {
  return buildHourlyBuckets(24, now).map((ts) => ({
    ts,
    latency_ms: null,
    success: null,
    error: null,
  }));
}

/** Metrics when Phoenix has no project for this agent yet (no traces ingested). */
export function emptyAgentMetrics(
  agentId: string,
  window: string,
  message: string,
): AgentMetricsResponse {
  return {
    agent: agentId,
    window,
    avg_latency_ms: null,
    success: 0,
    error: 0,
    uptime_pct: null,
    series_24h: buildEmptySeries24h(),
    no_data: true,
    message,
  };
}

