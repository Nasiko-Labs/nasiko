// In-process load pattern runner.
// Defines the default burst pattern and can execute it against a running server
// instance via loopback HTTP — no external script needed.

import * as http from 'http';

// ── Types ─────────────────────────────────────────────────────────────────────

export interface LoadStep {
  agentId: string;
  count: number;
  /** All requests in this step use this as the input (or as the base for unique inputs). */
  inputBase: string;
  /** When true, each request gets a unique input (`${inputBase} ${i}-${ts}`). Default: false. */
  uniqueInputs?: boolean;
  workflowId?: string;
  /** When false, requests are sent sequentially (useful for cache-priming). Default: true. */
  concurrent?: boolean;
}

export interface StepSummary {
  agentId: string;
  total: number;
  succeeded: number;   // executed, from_cache: false
  cached: number;      // from_cache: true
  queued: number;      // { queued: true } — accepted but deferred
  dropped: number;     // HTTP 429 — queue full
  errored: number;     // HTTP 5xx or network error
}

export interface ReplaySummary {
  patternName: string;
  durationMs: number;
  totalRequests: number;
  perAgent: StepSummary[];
}

// ── Default burst pattern ─────────────────────────────────────────────────────
//
// Mirrors scripts/loadTest.ts so operators can replay the same pattern from
// inside the dashboard without running an external script.

export const DEFAULT_BURST: LoadStep[] = [
  // Phase 1: same input, sequential — first request primes the cache,
  // the remaining 14 are served from memory (demonstrates caching).
  {
    agentId:     'agent_fast',
    count:       15,
    inputBase:   'replay: translate hello to French',
    uniqueInputs: false,
    concurrent:   false,
  },
  // Phase 2: unique inputs, concurrent burst — exercises the rate limiter
  // and queue (first 10 execute directly, rest enter the per-agent FIFO).
  {
    agentId:     'agent_slow',
    count:       20,
    inputBase:   'replay: review document',
    uniqueInputs: true,
    concurrent:   true,
  },
  // Phase 3: unique inputs, concurrent — generates controlled errors
  // (~30% of requests hit agent_flaky's random failure path).
  {
    agentId:     'agent_flaky',
    count:       15,
    inputBase:   'replay: flaky query',
    uniqueInputs: true,
    concurrent:   true,
  },
];

// ── HTTP helper ───────────────────────────────────────────────────────────────

interface RequestResult {
  status: number;
  data: Record<string, unknown>;
}

function sendRequest(
  baseUrl: string,
  body: { agent_id: string; input: string; workflow_id?: string },
): Promise<RequestResult> {
  return new Promise((resolve) => {
    const payload = JSON.stringify(body);
    const url = new URL('/request', baseUrl);

    const req = http.request(
      {
        hostname: url.hostname,
        port:     Number(url.port) || 80,
        path:     '/request',
        method:   'POST',
        headers: {
          'Content-Type':   'application/json',
          'Content-Length': Buffer.byteLength(payload),
        },
      },
      (res) => {
        let raw = '';
        res.on('data', (chunk: Buffer) => { raw += chunk.toString(); });
        res.on('end', () => {
          try {
            resolve({
              status: res.statusCode ?? 0,
              data:   JSON.parse(raw) as Record<string, unknown>,
            });
          } catch {
            resolve({ status: res.statusCode ?? 0, data: {} });
          }
        });
      },
    );
    // Network errors resolve with status 0 (never reject) to avoid unhandled rejections
    req.on('error', () => resolve({ status: 0, data: {} }));
    req.write(payload);
    req.end();
  });
}

// ── Result classifier ─────────────────────────────────────────────────────────

function classify(result: RequestResult, summary: StepSummary): void {
  summary.total++;
  if (result.status === 429)          summary.dropped++;
  else if (result.status >= 500 ||
           result.status === 0)        summary.errored++;
  else if (result.data.queued)         summary.queued++;
  else if (result.data.from_cache)     summary.cached++;
  else                                 summary.succeeded++;
}

// ── Pattern runner ────────────────────────────────────────────────────────────

export async function runPattern(
  baseUrl: string,
  steps: LoadStep[],
): Promise<ReplaySummary> {
  const start = Date.now();
  const summaries = new Map<string, StepSummary>();

  function get(agentId: string): StepSummary {
    if (!summaries.has(agentId)) {
      summaries.set(agentId, {
        agentId, total: 0, succeeded: 0, cached: 0, queued: 0, dropped: 0, errored: 0,
      });
    }
    return summaries.get(agentId)!;
  }

  for (const step of steps) {
    const summary = get(step.agentId);
    const ts = Date.now();

    const makeReq = (i: number): Promise<RequestResult> =>
      sendRequest(baseUrl, {
        agent_id:    step.agentId,
        input:       step.uniqueInputs
          ? `${step.inputBase} ${i}-${ts}`
          : step.inputBase,
        workflow_id: step.workflowId,
      });

    if (step.concurrent === false) {
      // Sequential — ensures cache is primed before the next request fires
      for (let i = 0; i < step.count; i++) {
        classify(await makeReq(i), summary);
      }
    } else {
      // Concurrent burst — all requests fire simultaneously
      const settled = await Promise.allSettled(
        Array.from({ length: step.count }, (_, i) => makeReq(i)),
      );
      for (const s of settled) {
        classify(s.status === 'fulfilled' ? s.value : { status: 0, data: {} }, summary);
      }
    }
  }

  const perAgent = [...summaries.values()];
  return {
    patternName:   'default',
    durationMs:    Date.now() - start,
    totalRequests: perAgent.reduce((n, s) => n + s.total, 0),
    perAgent,
  };
}
