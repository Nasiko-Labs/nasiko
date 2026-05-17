/**
 * B6: Agent uptime via Kong health probes (in-memory buffer per dev server process).
 *
 * Formula (ADR 0002): successful checks / expected checks over 24h at poll interval.
 * Until the buffer fills, we use recorded checks as the denominator so early
 * dashboard loads show sensible values (documented in `source.uptime`).
 */

import { getNasikoApiUrl } from "@/lib/nasiko";

/** 5 minutes — matches plan / ADR poll-interval assumption. */
export const HEALTH_POLL_INTERVAL_MS = 5 * 60 * 1000;

const WINDOW_MS = 24 * 60 * 60 * 1000;

type HealthSample = { at: number; ok: boolean };

const buffers = new Map<string, HealthSample[]>();

export type UptimeMeta = {
  successful_checks: number;
  recorded_checks: number;
  expected_checks_24h: number;
  poll_interval_minutes: number;
  current_health: boolean;
};

/** Kong routes Docker agents at `/agents/agent-{agentId}` (strip_path). */
export function kongAgentPathCandidates(agentId: string): string[] {
  const dockerName = `agent-${agentId}`;
  return [
    `/agents/${dockerName}/health`,
    `/agents/${agentId}/health`,
    `/agents/${dockerName}/`,
    `/agents/${agentId}/`,
  ];
}

async function probeUrl(url: string): Promise<boolean> {
  try {
    const response = await fetch(url, {
      method: "GET",
      cache: "no-store",
      signal: AbortSignal.timeout(8000),
    });
    if (response.status >= 200 && response.status < 500) {
      return true;
    }
    if (response.status === 405) {
      return true;
    }
  } catch {
    /* try next */
  }

  try {
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: "{}",
      cache: "no-store",
      signal: AbortSignal.timeout(8000),
    });
    return response.status >= 200 && response.status < 500;
  } catch {
    return false;
  }
}

export async function probeAgentHealth(agentId: string): Promise<boolean> {
  const base = getNasikoApiUrl();
  for (const path of kongAgentPathCandidates(agentId)) {
    const ok = await probeUrl(`${base}${path}`);
    if (ok) {
      return true;
    }
  }
  return false;
}

function pruneBuffer(samples: HealthSample[], now: number): HealthSample[] {
  const cutoff = now - WINDOW_MS;
  return samples.filter((s) => s.at >= cutoff);
}

export async function recordAgentHealth(
  agentId: string,
  now = Date.now(),
): Promise<{ uptimePct: number | null; meta: UptimeMeta }> {
  const existing = buffers.get(agentId) ?? [];
  const pruned = pruneBuffer(existing, now);
  const last = pruned[pruned.length - 1];

  if (!last || now - last.at >= HEALTH_POLL_INTERVAL_MS) {
    const ok = await probeAgentHealth(agentId);
    pruned.push({ at: now, ok });
  }

  buffers.set(agentId, pruned);

  const expected = Math.ceil(WINDOW_MS / HEALTH_POLL_INTERVAL_MS);
  const successful = pruned.filter((s) => s.ok).length;
  const recorded = pruned.length;
  const currentHealth = pruned[pruned.length - 1]?.ok ?? false;

  let uptimePct: number | null = null;
  if (recorded > 0) {
    const denominator = recorded < expected ? recorded : expected;
    uptimePct = Math.round((1000 * successful) / denominator) / 10;
  }

  return {
    uptimePct,
    meta: {
      successful_checks: successful,
      recorded_checks: recorded,
      expected_checks_24h: expected,
      poll_interval_minutes: HEALTH_POLL_INTERVAL_MS / 60_000,
      current_health: currentHealth,
    },
  };
}

/** Test helper — reset in-memory buffer. */
export function resetUptimeBuffers(): void {
  buffers.clear();
}
