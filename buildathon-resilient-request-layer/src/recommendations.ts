// Advisory recommendations engine.
// Reads enriched agent stats (the same shape as GET /ops/agents/stats)
// and produces per-agent suggested config changes based on simple heuristics.
// Read-only: never mutates live config.

export interface AgentRecommendation {
  agentId: string;
  suggestedCapacity?: number;
  suggestedRefillRatePerSec?: number;
  suggestedMaxQueueLength?: number;
  suggestedMaxConcurrent?: number;
  rationale: string;
}

// Minimum samples before we produce confident suggestions
const MIN_SAMPLE = 5;

export function computeRecommendations(
  stats: Record<string, unknown>,
): Record<string, AgentRecommendation> {
  const result: Record<string, AgentRecommendation> = {};
  for (const [agentId, a] of Object.entries(stats)) {
    result[agentId] = analyze(agentId, a as Record<string, unknown>);
  }
  return result;
}

function analyze(agentId: string, a: Record<string, unknown>): AgentRecommendation {
  // Safely extract numeric counters
  const total   = num(a.total);
  const queued  = num(a.queued);
  const dropped = num(a.dropped);
  const p95     = a.p95LatencyMs as number | null;

  // SLO fields (flat in the enriched snapshot)
  const sloStatus = String(a.sloStatus ?? 'unknown');
  const sloP95Ms  = a.sloP95Ms as number | undefined;

  // Rate-limiter config
  const rl  = (a.rateLimiter ?? {}) as Record<string, unknown>;
  const cfg = (rl.config    ?? {}) as Record<string, unknown>;
  const capacity         = num(cfg.capacity,         10);
  const refillRatePerSec = num(cfg.refillRatePerSec, 2);

  // Queue live state
  const q              = (a.queue ?? {}) as Record<string, unknown>;
  const maxQueueLength = num(q.maxQueueLength, 50);
  const queueDepth     = num(q.depth,          0);

  // Concurrency guard config
  const conc          = (a.concurrency ?? {}) as Record<string, unknown>;
  const maxConcurrent = num(conc.maxConcurrent, 10);

  // Not enough data to advise
  if (total < MIN_SAMPLE) {
    return {
      agentId,
      rationale: `Collecting data (${total} / ${MIN_SAMPLE} requests needed before advising)`,
    };
  }

  const rec: Partial<AgentRecommendation> = { agentId };
  const reasons: string[] = [];

  // ── Heuristic 1: Drops (most urgent) ──────────────────────────────────────
  // Even a single drop means the queue was full — we should expand both.
  if (dropped > 0) {
    const dropRate = ((dropped / total) * 100).toFixed(1);
    rec.suggestedCapacity       = bump(capacity * 1.5, capacity + 5);
    rec.suggestedMaxQueueLength = bump(maxQueueLength * 1.5, maxQueueLength + 20);
    reasons.push(
      `${dropped} request${dropped > 1 ? 's' : ''} dropped (${dropRate}% drop rate)` +
      ` — raise capacity and queue to absorb burst`,
    );
  }

  // ── Heuristic 2: SLO breach ────────────────────────────────────────────────
  // p95 is above the declared target — more throughput headroom helps.
  if (sloStatus === 'breaching' && sloP95Ms !== undefined && p95 !== null) {
    if (rec.suggestedCapacity === undefined) {
      rec.suggestedCapacity = bump(capacity * 1.4, capacity + 4);
    }
    if (maxConcurrent < 15) {
      rec.suggestedMaxConcurrent = Math.min(maxConcurrent + 3, 20);
    }
    reasons.push(
      `p95 ${p95} ms exceeds SLO ${sloP95Ms} ms` +
      ` — raise capacity or concurrency to reduce latency`,
    );
  }

  // ── Heuristic 3: Queue pressure (no drops yet) ────────────────────────────
  // Queued > 20% of total means the rate limiter is the bottleneck.
  if (dropped === 0 && queued > 0) {
    const queueRate = queued / total;
    if (queueRate > 0.2) {
      if (rec.suggestedRefillRatePerSec === undefined) {
        rec.suggestedRefillRatePerSec = bump(refillRatePerSec * 1.5, refillRatePerSec + 1);
      }
      if (rec.suggestedCapacity === undefined) {
        rec.suggestedCapacity = bump(capacity * 1.25, capacity + 2);
      }
      reasons.push(
        `${(queueRate * 100).toFixed(0)}% of requests were queued` +
        ` — higher refill rate reduces wait time`,
      );
    }
  }

  // ── Heuristic 4: Under-utilisation ────────────────────────────────────────
  // No queuing, no drops, fast p95, large capacity → room to shrink.
  if (reasons.length === 0 && queued === 0 && dropped === 0 && capacity >= 8) {
    const qUtilFrac  = maxQueueLength > 0 ? queueDepth / maxQueueLength : 0;
    const isFastAgent = p95 !== null && p95 < 300;
    if (qUtilFrac < 0.1 && isFastAgent) {
      const suggested = Math.max(3, Math.floor(capacity * 0.7));
      if (suggested < capacity) {
        rec.suggestedCapacity = suggested;
        reasons.push(
          `Low utilisation with fast responses (p95 ${p95} ms)` +
          ` — capacity could drop from ${capacity} to ${suggested} to free resources`,
        );
      }
    }
  }

  // ── No issues found ────────────────────────────────────────────────────────
  if (reasons.length === 0) {
    return { agentId, rationale: 'Current config looks healthy — no changes recommended' };
  }

  return { ...rec, rationale: reasons.join('; ') } as AgentRecommendation;
}

/** Return the larger of two values, rounded up — ensures we always suggest more than current. */
function bump(a: number, b: number): number {
  return Math.ceil(Math.max(a, b));
}

/** Safe numeric extraction with fallback. */
function num(v: unknown, fallback = 0): number {
  const n = Number(v);
  return Number.isFinite(n) ? n : fallback;
}
