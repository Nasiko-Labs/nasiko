/**
 * B5: Roll up Phoenix sessions + project stats into dashboard KPIs and hourly series.
 */

import type {
  AgentMetricsResponse,
  MetricsHourlyPoint,
  MetricsSourceMeta,
  NasikoAgentProjectStats,
  NasikoObservabilitySession,
} from "@/lib/types";

const ERROR_OUTPUT_PATTERN =
  /\b(error|401|403|500|failed|failure|invalid api key|rate limit)\b|sorry,\s*an error|error occurred while processing|error code:/i;

export function hourStartIso(tsMs: number): string {
  const d = new Date(tsMs);
  d.setUTCMinutes(0, 0, 0);
  return d.toISOString();
}

export function buildHourlyBuckets(hours: number, now = Date.now()): string[] {
  const keys: string[] = [];
  for (let i = hours - 1; i >= 0; i--) {
    keys.push(hourStartIso(now - i * 60 * 60 * 1000));
  }
  return keys;
}

function sessionOutputText(session: NasikoObservabilitySession): string {
  const raw = session.last_output;
  if (typeof raw === "string") {
    return raw;
  }
  return raw?.value ?? "";
}

export function isSessionError(session: NasikoObservabilitySession): boolean {
  const output = sessionOutputText(session);
  if (typeof output === "string" && ERROR_OUTPUT_PATTERN.test(output)) {
    return true;
  }
  const summaries = session.session_annotation_summaries ?? [];
  for (const summary of summaries) {
    const name = (summary.name ?? "").toLowerCase();
    if (name.includes("error") || name.includes("fail")) {
      return true;
    }
    for (const lf of summary.label_fractions ?? []) {
      const label = (lf.label ?? "").toLowerCase();
      if (label.includes("error") || label.includes("fail")) {
        return (lf.fraction ?? 0) > 0.5;
      }
    }
  }
  return false;
}

export function parseSessionStartMs(session: NasikoObservabilitySession): number | null {
  const raw = session.start_time;
  if (!raw) {
    return null;
  }
  const ms = Date.parse(raw);
  return Number.isFinite(ms) ? ms : null;
}

export type RollupCounts = { success: number; error: number };

export function rollupSessionCounts(
  sessions: NasikoObservabilitySession[],
): RollupCounts {
  let success = 0;
  let error = 0;
  for (const session of sessions) {
    const traces = session.num_traces ?? 0;
    if (traces <= 0) {
      continue;
    }
    if (isSessionError(session)) {
      error += traces;
    } else {
      success += traces;
    }
  }
  return { success, error };
}

export function buildSeries24h(
  sessions: NasikoObservabilitySession[],
  hours: number,
  now = Date.now(),
): MetricsHourlyPoint[] {
  const bucketKeys = buildHourlyBuckets(hours, now);
  const buckets = new Map<
    string,
    { latencySum: number; latencyN: number; success: number; error: number }
  >();

  for (const key of bucketKeys) {
    buckets.set(key, { latencySum: 0, latencyN: 0, success: 0, error: 0 });
  }

  const windowStart = now - hours * 60 * 60 * 1000;
  const windowEnd = now;

  for (const session of sessions) {
    const startMs = parseSessionStartMs(session);
    if (startMs == null || startMs < windowStart || startMs > windowEnd) {
      continue;
    }
    const key = hourStartIso(startMs);
    const bucket = buckets.get(key);
    if (!bucket) {
      continue;
    }

    const traces = session.num_traces ?? 0;
    const failed = isSessionError(session);
    if (failed) {
      bucket.error += traces;
    } else {
      bucket.success += traces;
    }

    const latency = session.trace_latency_ms_p50;
    if (latency != null && Number.isFinite(latency) && traces > 0) {
      bucket.latencySum += latency;
      bucket.latencyN += 1;
    }
  }

  return bucketKeys.map((ts) => {
    const b = buckets.get(ts)!;
    const hasActivity = b.success > 0 || b.error > 0;
    return {
      ts,
      latency_ms:
        b.latencyN > 0 ? Math.round(b.latencySum / b.latencyN) : null,
      success: hasActivity ? b.success : null,
      error: hasActivity ? b.error : null,
    };
  });
}

/**
 * Tier B — Aggregate session annotation summaries (mean_score + label_fractions)
 * across the window. Picks the first annotation stream that yields a score and
 * sticks with it (mixing rubrics is meaningless). Returns null mean_score
 * when no sessions in the window have annotations.
 */
export function aggregateQualityFromSessions(
  sessions: NasikoObservabilitySession[],
): NonNullable<MetricsSourceMeta["quality"]> | null {
  let primary: string | null = null;
  let scoreSum = 0;
  let scoreN = 0;
  const labelTotals = new Map<string, { fractionSum: number; n: number }>();

  for (const session of sessions) {
    const summaries = session.session_annotation_summaries ?? [];
    if (summaries.length === 0) continue;

    // Lock onto the first annotation we see that produces a numeric score.
    type Summary = (typeof summaries)[number];
    let chosen: Summary | null = primary
      ? summaries.find((s) => s.name === primary) ?? null
      : null;
    if (!chosen) {
      chosen =
        summaries.find(
          (s) =>
            typeof s.mean_score === "number" && Number.isFinite(s.mean_score),
        ) ?? summaries[0] ?? null;
      if (chosen?.name) {
        primary = chosen.name;
      }
    }
    if (!chosen) continue;

    if (
      typeof chosen.mean_score === "number" &&
      Number.isFinite(chosen.mean_score)
    ) {
      scoreSum += chosen.mean_score;
      scoreN += 1;
    }

    for (const lf of chosen.label_fractions ?? []) {
      const label = (lf.label ?? "").trim();
      const fraction = lf.fraction ?? 0;
      if (!label || !Number.isFinite(fraction)) continue;
      const cur = labelTotals.get(label) ?? { fractionSum: 0, n: 0 };
      cur.fractionSum += fraction;
      cur.n += 1;
      labelTotals.set(label, cur);
    }
  }

  if (scoreN === 0 && labelTotals.size === 0) {
    return null;
  }

  const top_labels = Array.from(labelTotals.entries())
    .map(([label, { fractionSum, n }]) => ({
      label,
      fraction: n > 0 ? fractionSum / n : 0,
    }))
    .sort((a, b) => b.fraction - a.fraction)
    .slice(0, 4);

  return {
    mean_score: scoreN > 0 ? scoreSum / scoreN : null,
    sample_count: scoreN,
    top_labels,
    primary_annotation: primary,
  };
}

/**
 * Tier B — Orchestration depth: mean traces-per-session in the window.
 * Returns null when no sessions had num_traces > 0 (avoids divide-by-zero
 * and meaningless 0.0 readings).
 */
export function aggregateOrchestrationFromSessions(
  sessions: NasikoObservabilitySession[],
): NonNullable<MetricsSourceMeta["orchestration"]> {
  let totalSessions = 0;
  let totalTraces = 0;
  for (const session of sessions) {
    const traces = session.num_traces ?? 0;
    if (traces <= 0) continue;
    totalSessions += 1;
    totalTraces += traces;
  }
  return {
    avg_traces_per_session:
      totalSessions > 0 ? totalTraces / totalSessions : null,
    total_sessions: totalSessions,
    total_traces: totalTraces,
  };
}

/**
 * Tier B — Prompt vs completion cost split from Phoenix `cost_summary`.
 * Returns `null` when neither side reported a number — useful for hiding the
 * mini-viz cleanly.
 */
export function buildCostBreakdown(
  project: NasikoAgentProjectStats,
): NonNullable<MetricsSourceMeta["cost_breakdown"]> | null {
  const prompt = project.cost_summary?.prompt?.cost ?? null;
  const completion = project.cost_summary?.completion?.cost ?? null;
  const total = project.cost_summary?.total?.cost ?? null;
  const numeric = (v: number | null): number | null =>
    typeof v === "number" && Number.isFinite(v) ? v : null;
  const promptN = numeric(prompt);
  const completionN = numeric(completion);
  const totalN = numeric(total);
  if (promptN == null && completionN == null && totalN == null) {
    return null;
  }
  let totalUsd: number | null = totalN;
  if (totalUsd == null) {
    const inferred = (promptN ?? 0) + (completionN ?? 0);
    totalUsd = inferred > 0 ? inferred : null;
  }
  return {
    prompt_usd: promptN,
    completion_usd: completionN,
    total_usd: totalUsd,
  };
}

export function averageLatencyFromSessions(
  sessions: NasikoObservabilitySession[],
): number | null {
  let sum = 0;
  let n = 0;
  for (const session of sessions) {
    const latency = session.trace_latency_ms_p50;
    if (latency != null && Number.isFinite(latency) && (session.num_traces ?? 0) > 0) {
      sum += latency;
      n += 1;
    }
  }
  if (n === 0) {
    return null;
  }
  return Math.round(sum / n);
}

export type BuildAgentMetricsInput = {
  agentId: string;
  window: string;
  hours: number;
  project: NasikoAgentProjectStats;
  sessions: NasikoObservabilitySession[];
  uptimePct: number | null;
  uptimeMeta?: MetricsSourceMeta["uptime"];
  now?: number;
};

export function buildAgentMetrics(input: BuildAgentMetricsInput): AgentMetricsResponse {
  const { agentId, window, hours, project, sessions, uptimePct, now = Date.now() } =
    input;

  const sessionCounts = rollupSessionCounts(sessions);
  const traceCount = project.trace_count ?? 0;
  const success =
    sessionCounts.success + sessionCounts.error > 0
      ? sessionCounts.success
      : traceCount;
  const error =
    sessionCounts.success + sessionCounts.error > 0 ? sessionCounts.error : 0;

  const avgFromSessions = averageLatencyFromSessions(sessions);
  const avgLatencyMs =
    avgFromSessions ??
    (project.latency_ms_p50 != null && Number.isFinite(project.latency_ms_p50)
      ? Math.round(project.latency_ms_p50)
      : null);

  const quality = aggregateQualityFromSessions(sessions);
  const orchestration = aggregateOrchestrationFromSessions(sessions);
  const costBreakdown = buildCostBreakdown(project);

  return {
    agent: agentId,
    window,
    avg_latency_ms: avgLatencyMs,
    success,
    error,
    uptime_pct: uptimePct,
    series_24h: buildSeries24h(sessions, hours, now),
    source: {
      trace_count: traceCount,
      latency_ms_p50: project.latency_ms_p50 ?? null,
      latency_ms_p99: project.latency_ms_p99 ?? null,
      project_id: project.id ?? null,
      cost_usd: project.cost_summary?.total?.cost ?? null,
      session_count: sessions.length,
      ...(input.uptimeMeta ? { uptime: input.uptimeMeta } : {}),
      ...(quality ? { quality } : {}),
      ...(orchestration.total_sessions > 0 ? { orchestration } : {}),
      ...(costBreakdown ? { cost_breakdown: costBreakdown } : {}),
    },
  };
}
