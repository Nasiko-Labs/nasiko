// Per-agent SLO (Service Level Objective) configuration and status computation.
// SLOs are purely advisory — they surface p95 compliance in the dashboard and API
// without changing any routing or queueing behaviour.

export type SloStatus = 'ok' | 'breaching' | 'unknown';

export interface SloConfig {
  sloP95Ms?: number;   // target p95 latency in milliseconds
  sloLabel?: string;   // human-readable label, e.g. "Interactive" or "Batch"
}

export interface SloSnapshot {
  sloP95Ms?:  number;
  sloLabel?:  string;
  sloStatus:  SloStatus;
}

const configs = new Map<string, SloConfig>();

/** Merge SLO fields for an agent (all fields optional). */
export function configure(agentId: string, patch: Partial<SloConfig>): void {
  const current = configs.get(agentId) ?? {};
  configs.set(agentId, { ...current, ...patch });
}

export function getConfig(agentId: string): SloConfig {
  return configs.get(agentId) ?? {};
}

/**
 * Compute the current SLO status for an agent.
 *   - sloP95Ms not set           → unknown (no SLO defined)
 *   - sloP95Ms set, no p95 data  → unknown (not enough data)
 *   - p95 <= sloP95Ms            → ok
 *   - p95 >  sloP95Ms            → breaching
 */
export function computeSnapshot(agentId: string, p95LatencyMs: number | null): SloSnapshot {
  const cfg = getConfig(agentId);

  if (cfg.sloP95Ms === undefined) {
    return { sloStatus: 'unknown' };
  }

  if (p95LatencyMs === null) {
    return { sloP95Ms: cfg.sloP95Ms, sloLabel: cfg.sloLabel, sloStatus: 'unknown' };
  }

  return {
    sloP95Ms:  cfg.sloP95Ms,
    sloLabel:  cfg.sloLabel,
    sloStatus: p95LatencyMs <= cfg.sloP95Ms ? 'ok' : 'breaching',
  };
}
