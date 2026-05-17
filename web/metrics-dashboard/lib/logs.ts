/**
 * Challenge 1 — Synthesize a "platform activity log" from existing
 * observability data (sessions) + registry snapshot. No Nasiko API changes.
 *
 * Each session contributes one or two log rows depending on whether it
 * succeeded or errored. The registry contributes one INFO row per visible
 * agent ("Agent online" at the time the snapshot was taken).
 *
 * Pure functions — no I/O — so this is easy to unit test.
 */

import type {
  LogLevel,
  NasikoObservabilitySession,
  NasikoSimpleUserAgent,
  SyntheticLogRow,
} from "@/lib/types";
import { isSessionError } from "@/lib/rollup";

/** Slow request threshold: sessions with p50 latency above this become WARNING. */
export const WARNING_LATENCY_MS = 5_000;

function sessionPreview(session: NasikoObservabilitySession): string {
  const raw = session.last_output;
  const text = typeof raw === "string" ? raw : raw?.value;
  const trimmed = (text ?? "").trim();
  if (!trimmed) return "";
  if (trimmed.length <= 140) return trimmed;
  return `${trimmed.slice(0, 137)}…`;
}

function sessionService(session: NasikoObservabilitySession): string {
  return session.agent_id ? `agent:${session.agent_id}` : "agent";
}

function endTs(session: NasikoObservabilitySession): string | null {
  return (
    session.end_time ?? session.start_time ?? null
  );
}

function safeIso(ts: string | null | undefined): string | null {
  if (!ts) return null;
  const ms = Date.parse(ts);
  return Number.isFinite(ms) ? new Date(ms).toISOString() : null;
}

/**
 * Convert one session into 1–2 log rows:
 *   - Always emit a "started" INFO row (timestamp = start_time).
 *   - On completion, emit either INFO (success) or ERROR (failure).
 *   - If success but p50 latency > WARNING_LATENCY_MS, downgrade to WARNING.
 */
export function rowsFromSession(
  session: NasikoObservabilitySession,
): SyntheticLogRow[] {
  const rows: SyntheticLogRow[] = [];
  const startIso = safeIso(session.start_time);
  const endIso = safeIso(endTs(session));
  const service = sessionService(session);
  const preview = sessionPreview(session);
  const errored = isSessionError(session);
  const latency = session.trace_latency_ms_p50 ?? null;
  const slow =
    latency != null && Number.isFinite(latency) && latency > WARNING_LATENCY_MS;
  const sessionId = session.session_id ?? null;

  if (startIso) {
    rows.push({
      id: `sess:${sessionId ?? startIso}:start`,
      ts: startIso,
      level: "INFO",
      service,
      message: `Request received${session.agent_id ? ` by ${session.agent_id}` : ""}`,
      agent_id: session.agent_id ?? null,
      session_id: sessionId,
      latency_ms: null,
    });
  }

  if (endIso) {
    let level: LogLevel = errored ? "ERROR" : slow ? "WARNING" : "INFO";
    let message: string;
    if (errored) {
      message = preview
        ? `Request failed — ${preview}`
        : `Request failed`;
    } else if (slow) {
      message = `Slow response (${Math.round((latency as number) / 1000)}s)${
        preview ? ` — ${preview}` : ""
      }`;
    } else if (preview) {
      message = `Completed — ${preview}`;
    } else {
      message = `Completed`;
    }
    // Demote slow rows from ERROR only if errored was false (we already set level above)
    if (errored && slow && latency != null) {
      message = `${message} (${Math.round((latency as number) / 1000)}s)`;
      level = "ERROR";
    }
    rows.push({
      id: `sess:${sessionId ?? endIso}:end`,
      ts: endIso,
      level,
      service,
      message,
      agent_id: session.agent_id ?? null,
      session_id: sessionId,
      latency_ms: latency,
    });
  }

  return rows;
}

/** Registry snapshot: one INFO row per agent currently online. */
export function rowsFromRegistry(
  agents: NasikoSimpleUserAgent[],
  snapshotAtIso: string,
): SyntheticLogRow[] {
  return agents.map((agent) => ({
    id: `reg:${agent.agent_id}:${snapshotAtIso}`,
    ts: snapshotAtIso,
    level: "INFO" as const,
    service: "registry",
    message: `Agent online · ${agent.name ?? agent.agent_id}`,
    agent_id: agent.agent_id,
    detail: {
      tags: agent.tags ?? [],
      description: agent.description ?? null,
    },
  }));
}

/** Cap so we don't ship a 50k-row payload to the browser. */
export const DEFAULT_MAX_LOGS = 500;

export type SynthesizeInput = {
  sessions: NasikoObservabilitySession[];
  agents: NasikoSimpleUserAgent[];
  generatedAt: Date;
  maxRows?: number;
};

/**
 * Top-level synthesizer. Combines all sources, sorts newest-first, and caps
 * the result. Returns rows ready for `/api/logs` response.
 */
export function synthesizeLogs(input: SynthesizeInput): {
  logs: SyntheticLogRow[];
  source_counts: { sessions: number; registry: number };
} {
  const generatedAtIso = input.generatedAt.toISOString();
  const sessionRows = input.sessions.flatMap(rowsFromSession);
  const registryRows = rowsFromRegistry(input.agents, generatedAtIso);

  const all = [...sessionRows, ...registryRows].sort((a, b) =>
    b.ts.localeCompare(a.ts),
  );

  const max = input.maxRows ?? DEFAULT_MAX_LOGS;
  return {
    logs: all.slice(0, max),
    source_counts: {
      sessions: sessionRows.length,
      registry: registryRows.length,
    },
  };
}

/** Client-side filter helpers (called from the LogsView). */
export type LogsFilter = {
  levels?: ReadonlySet<LogLevel>;
  search?: string;
};

export function filterLogs(
  rows: SyntheticLogRow[],
  filter: LogsFilter,
): SyntheticLogRow[] {
  const needle = filter.search?.trim().toLowerCase() ?? "";
  return rows.filter((row) => {
    if (filter.levels && filter.levels.size > 0 && !filter.levels.has(row.level)) {
      return false;
    }
    if (needle) {
      const hay =
        `${row.message} ${row.service} ${row.agent_id ?? ""} ${row.session_id ?? ""}`.toLowerCase();
      if (!hay.includes(needle)) return false;
    }
    return true;
  });
}

/** Compact level counts for the toolbar badges. */
export function countByLevel(
  rows: SyntheticLogRow[],
): Record<LogLevel, number> {
  const counts: Record<LogLevel, number> = { INFO: 0, WARNING: 0, ERROR: 0 };
  for (const row of rows) {
    counts[row.level] += 1;
  }
  return counts;
}
