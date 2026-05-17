import { isSessionError, parseSessionStartMs } from "@/lib/rollup";
import type {
  MetricsSessionRow,
  NasikoObservabilitySession,
  SessionTraceRef,
} from "@/lib/types";

function sessionPreview(session: NasikoObservabilitySession): string | null {
  const raw = session.last_output;
  const text = typeof raw === "string" ? raw : raw?.value;
  if (!text || typeof text !== "string") {
    return null;
  }
  const trimmed = text.trim();
  if (!trimmed) {
    return null;
  }
  return trimmed.length > 120 ? `${trimmed.slice(0, 117)}…` : trimmed;
}

export function toSessionRow(session: NasikoObservabilitySession): MetricsSessionRow | null {
  const sessionId = session.session_id?.trim();
  if (!sessionId) {
    return null;
  }

  const errored = isSessionError(session);
  const traces = session.num_traces ?? 0;

  return {
    session_id: sessionId,
    start_time: session.start_time ?? null,
    num_traces: traces,
    latency_ms_p50: session.trace_latency_ms_p50 ?? null,
    status: traces <= 0 ? "unknown" : errored ? "error" : "ok",
    preview: sessionPreview(session),
  };
}

export function normalizeSessions(
  sessions: NasikoObservabilitySession[],
  agentId: string,
): MetricsSessionRow[] {
  return sessions
    .filter((s) => !s.agent_id || s.agent_id === agentId)
    .map(toSessionRow)
    .filter((row): row is MetricsSessionRow => row != null)
    .sort((a, b) => {
      const aMs = a.start_time ? Date.parse(a.start_time) : 0;
      const bMs = b.start_time ? Date.parse(b.start_time) : 0;
      return bMs - aMs;
    });
}

export function extractSessionTraces(
  traces: Array<Record<string, unknown>>,
): SessionTraceRef[] {
  const out: SessionTraceRef[] = [];
  for (const trace of traces) {
    const traceId =
      (typeof trace.trace_id === "string" && trace.trace_id) ||
      (typeof trace.traceId === "string" && trace.traceId);
    if (!traceId) {
      continue;
    }
    const rootSpan = trace.root_span as Record<string, unknown> | undefined;
    const rootSpanAlt = trace.rootSpan as Record<string, unknown> | undefined;
    const latency =
      (rootSpan?.latency_ms as number | undefined) ??
      (rootSpanAlt?.latencyMs as number | undefined) ??
      null;
    out.push({
      id: typeof trace.id === "string" ? trace.id : undefined,
      trace_id: traceId,
      latency_ms: latency,
    });
  }
  return out;
}

/** Prefer the most recent trace with the highest latency (usually the main request). */
export function pickPrimaryTrace(traces: SessionTraceRef[]): SessionTraceRef | null {
  if (traces.length === 0) {
    return null;
  }
  return [...traces].sort(
    (a, b) => (b.latency_ms ?? 0) - (a.latency_ms ?? 0),
  )[0];
}

export { parseSessionStartMs };
