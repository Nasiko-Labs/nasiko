import express, { Request, Response, NextFunction } from 'express';
import { randomUUID } from 'crypto';
import path from 'path';
import dotenv from 'dotenv';
import * as cache from './cache';
import * as rateLimiter from './rateLimiter';
import * as queue from './queue';
import * as agents from './agents';
import * as metrics from './metrics';
import * as concurrency from './concurrency';
import * as slo from './slo';
import * as recommendations from './recommendations';
import * as loadPatterns from './loadPatterns';

dotenv.config();

const app = express();
const PORT = process.env.PORT ?? 4000;

app.use(express.json());
// Serve public/ (dashboard.html, dashboard.js) — path works for both ts-node-dev and compiled dist/
app.use(express.static(path.join(__dirname, '../public')));

// ─── Request types ────────────────────────────────────────────────────────────

interface RequestBody {
  agent_id: string;
  input: string;
  user_id?: string;
  workflow_id?: string;
}

interface AgentConfigBody {
  agent_id: string;
  // Rate limiter
  capacity?: number;
  refillRatePerSec?: number;
  critical?: boolean;
  minTokensReserved?: number;
  // Queue
  maxQueueLength?: number;
  // Concurrency guard
  maxConcurrent?: number;
  // SLO
  sloP95Ms?: number;
  sloLabel?: string;
}

// ─── POST /request ─────────────────────────────────────────────────────────────
//
// Flow:
//   1. Validate payload
//   2. Check cache → return cached result if hit
//   3. concurrency.canExecute() AND rateLimiter.allow() → execute directly
//        execute: startExecution → executeAgent → finishExecution (finally)
//        → cache result, return { from_cache: false, ...result }
//   4. Either guard blocked → try to enqueue → return { queued, request_id }
//      Queue full → 429 overloaded

app.post('/request', async (req: Request, res: Response, next: NextFunction): Promise<void> => {
  try {
    const { agent_id, input, user_id, workflow_id } = req.body as RequestBody;

    if (!agent_id || typeof agent_id !== 'string') {
      res.status(400).json({ error: 'agent_id is required and must be a string' });
      return;
    }
    if (!input || typeof input !== 'string') {
      res.status(400).json({ error: 'input is required and must be a string' });
      return;
    }

    metrics.recordRequest(agent_id);

    // ── Cache check ───────────────────────────────────────────────────────────
    const cacheKey = cache.buildKey(agent_id, input, workflow_id);
    const cached = cache.get(cacheKey);
    if (cached !== null) {
      metrics.recordCacheHit();
      // When a workflow_id is present this is a workflow-scoped idempotency hit.
      const isWorkflowDedupe = typeof workflow_id === 'string' && workflow_id.length > 0;
      if (isWorkflowDedupe) metrics.recordWorkflowDedupe(agent_id);
      res.json({
        from_cache: true,
        ...(isWorkflowDedupe ? { deduped_within_workflow: true, workflow_id } : {}),
        ...(cached as Record<string, unknown>),
      });
      return;
    }
    metrics.recordCacheMiss();

    // ── Both guards clear: execute directly ──────────────────────────────────
    // Concurrency check first (no token consumed); rate-limiter second (consumes one token).
    if (concurrency.canExecute(agent_id) && rateLimiter.allow(agent_id)) {
      concurrency.startExecution(agent_id);
      const start = Date.now();
      try {
        const result = await agents.executeAgent(agent_id, {
          input,
          userId: user_id,
          workflowId: workflow_id,
        });
        const latency = Date.now() - start;
        metrics.recordExecuted(agent_id, latency);
        cache.set(cacheKey, result, 60_000);
        res.json({ from_cache: false, ...result });
      } catch (err) {
        metrics.recordError(agent_id);
        next(err);
      } finally {
        concurrency.finishExecution(agent_id); // always release the slot
      }
      return;
    }

    // ── Rate-limited: try to enqueue ──────────────────────────────────────────
    const requestId = randomUUID();
    const enqueueResult = queue.enqueue(agent_id, {
      requestId,
      agentId: agent_id,
      input,
      userId: user_id,
      workflowId: workflow_id,
      // Callbacks are fulfilled by the background worker; useful for future
      // long-poll or result-polling endpoints.
      resolve: () => {},
      reject: () => {},
    });

    if (enqueueResult.enqueued) {
      metrics.recordQueued(agent_id);
      res.json({ queued: true, request_id: requestId });
      return;
    }

    // ── Queue full: drop the request ──────────────────────────────────────────
    metrics.recordDropped(agent_id);
    res.status(429).json({
      error: 'overloaded',
      message: `Agent "${agent_id}" is rate-limited and its queue is full. Retry later.`,
      agent_id,
    });
  } catch (err) {
    next(err);
  }
});

// ─── GET /ops/health ──────────────────────────────────────────────────────────

app.get('/ops/health', (_req: Request, res: Response): void => {
  res.json({ status: 'ok', service: 'buildathon-resilient-request-layer' });
});

// ─── GET /ops/cache/stats ─────────────────────────────────────────────────────

app.get('/ops/cache/stats', (_req: Request, res: Response): void => {
  res.json(cache.stats());
});

// ─── Shared: build enriched per-agent snapshot ────────────────────────────────
// Used by both /ops/agents/stats and /ops/agents/recommendations.

function buildEnrichedStats(): { global: object; agents: Record<string, object> } {
  const snap = metrics.snapshot();
  const enriched: Record<string, object> = {};
  for (const id of Object.keys(snap.agents)) {
    const sloSnap = slo.computeSnapshot(id, snap.agents[id].p95LatencyMs);
    enriched[id] = {
      ...snap.agents[id],
      ...sloSnap,
      rateLimiter:  rateLimiter.getStats(id),
      queue:        queue.getQueueStats(id),
      concurrency:  concurrency.getStats(id),
    };
  }
  return { global: { ...snap.global }, agents: enriched };
}

// ─── GET /ops/agents/stats ────────────────────────────────────────────────────

app.get('/ops/agents/stats', (_req: Request, res: Response): void => {
  const { global, agents } = buildEnrichedStats();
  res.json({ global, agents });
});

// ─── GET /ops/agents/recommendations ─────────────────────────────────────────
// Read-only advisory endpoint. Returns suggested config changes per agent based
// on observed metrics. Never mutates live config.

app.get('/ops/agents/recommendations', (_req: Request, res: Response): void => {
  const { agents } = buildEnrichedStats();
  const recMap = recommendations.computeRecommendations(agents);
  res.json({ agents: Object.values(recMap) });
});

// ─── POST /ops/agents/config ──────────────────────────────────────────────────
// Allows changing rate-limiter capacity/refillRatePerSec and queue depth at runtime.
// Body: { agent_id, capacity?, refillRatePerSec?, maxQueueLength? }

app.post('/ops/agents/config', (req: Request, res: Response): void => {
  const {
    agent_id, capacity, refillRatePerSec, critical, minTokensReserved,
    maxQueueLength, maxConcurrent, sloP95Ms, sloLabel,
  } = req.body as AgentConfigBody;

  if (!agent_id || typeof agent_id !== 'string') {
    res.status(400).json({ error: 'agent_id is required' });
    return;
  }

  // Rate-limiter patch (all fields optional)
  const rlPatch: Partial<rateLimiter.BucketConfig> = {};
  if (capacity          !== undefined) rlPatch.capacity          = capacity;
  if (refillRatePerSec  !== undefined) rlPatch.refillRatePerSec  = refillRatePerSec;
  if (critical          !== undefined) rlPatch.critical          = critical;
  if (minTokensReserved !== undefined) rlPatch.minTokensReserved = minTokensReserved;
  if (Object.keys(rlPatch).length > 0) rateLimiter.configure(agent_id, rlPatch);

  if (maxQueueLength !== undefined) queue.configureQueue(agent_id, maxQueueLength);
  if (maxConcurrent  !== undefined) concurrency.configure(agent_id, maxConcurrent);

  const sloPatch: import('./slo').SloConfig = {};
  if (sloP95Ms !== undefined) sloPatch.sloP95Ms = sloP95Ms;
  if (sloLabel  !== undefined) sloPatch.sloLabel  = sloLabel;
  if (Object.keys(sloPatch).length > 0) slo.configure(agent_id, sloPatch);

  // Recompute SLO status from current p95 for the response
  const p95Now = metrics.snapshot().agents[agent_id]?.p95LatencyMs ?? null;

  res.json({
    agent_id,
    rateLimiter:  rateLimiter.getStats(agent_id),
    queue:        queue.getQueueStats(agent_id),
    concurrency:  concurrency.getStats(agent_id),
    slo:          slo.computeSnapshot(agent_id, p95Now),
  });
});

// ─── POST /ops/load/replay ────────────────────────────────────────────────────
// Fires the default burst pattern against this server's own /request endpoint
// and returns a summary of what happened. Read this before/after config changes
// to validate tuning. No auth required — local/demo use only.

app.post('/ops/load/replay', async (_req: Request, res: Response, next: NextFunction): Promise<void> => {
  try {
    const baseUrl = `http://127.0.0.1:${PORT}`;
    const summary = await loadPatterns.runPattern(baseUrl, loadPatterns.DEFAULT_BURST);
    const { global: globalMetrics, agents: agentStats } = buildEnrichedStats();

    res.json({
      patternName:   summary.patternName,
      durationMs:    summary.durationMs,
      totalRequests: summary.totalRequests,
      replaySummary: summary.perAgent,
      cacheStats:    cache.stats(),
      globalMetrics,
      agentStats,
    });
  } catch (err) {
    next(err);
  }
});

// ─── GET /ops/dashboard ───────────────────────────────────────────────────────

app.get('/ops/dashboard', (_req: Request, res: Response): void => {
  res.sendFile(path.join(__dirname, '../public/dashboard.html'));
});

// ─── Error handler ────────────────────────────────────────────────────────────

app.use((err: unknown, _req: Request, res: Response, _next: NextFunction): void => {
  const message = err instanceof Error ? err.message : 'Internal server error';
  res.status(500).json({ error: message });
});

// ─── Boot ─────────────────────────────────────────────────────────────────────

app.listen(PORT, () => {
  console.log(`Resilient Request Layer listening on port ${PORT}`);
});

export default app;
