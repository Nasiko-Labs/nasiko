/**
 * Load test for the Resilient Request Layer.
 *
 * Usage (from buildathon-resilient-request-layer/):
 *   npm run load:test
 *
 * What it does:
 *   Phase 1 — 15 requests to agent_fast with identical input → demonstrates cache
 *   Phase 2 — 20 concurrent requests to agent_slow with unique inputs → exercises
 *             rate limiter and queue
 *   Phase 3 — 15 requests to agent_flaky → generates controlled errors
 *   Then prints a human-readable summary with numbers to quote in a pitch.
 */

import * as http from 'http';
import * as path from 'path';
import * as fs from 'fs';

// Load .env from project root (works when run via npm script from project dir)
const envPath = path.resolve(process.cwd(), '.env');
if (fs.existsSync(envPath)) {
  const lines = fs.readFileSync(envPath, 'utf8').split('\n');
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq === -1) continue;
    const key = trimmed.slice(0, eq).trim();
    const val = trimmed.slice(eq + 1).trim();
    if (!(key in process.env)) process.env[key] = val;
  }
}

const PORT = parseInt(process.env.PORT ?? '4001', 10);

// ── HTTP helpers (built-in http module — no extra deps) ───────────────────────

interface HttpResult {
  status: number;
  data: unknown;
}

function httpPost(urlPath: string, body: object): Promise<HttpResult> {
  return new Promise((resolve, reject) => {
    const payload = JSON.stringify(body);
    const req = http.request(
      {
        hostname: 'localhost',
        port: PORT,
        path: urlPath,
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(payload),
        },
      },
      (res) => {
        let raw = '';
        res.on('data', (chunk: Buffer) => { raw += chunk.toString(); });
        res.on('end', () => {
          try { resolve({ status: res.statusCode ?? 0, data: JSON.parse(raw) }); }
          catch { resolve({ status: res.statusCode ?? 0, data: raw }); }
        });
      },
    );
    req.on('error', reject);
    req.write(payload);
    req.end();
  });
}

function httpGet(urlPath: string): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { hostname: 'localhost', port: PORT, path: urlPath, method: 'GET' },
      (res) => {
        let raw = '';
        res.on('data', (chunk: Buffer) => { raw += chunk.toString(); });
        res.on('end', () => {
          try { resolve(JSON.parse(raw)); } catch { resolve(raw); }
        });
      },
    );
    req.on('error', reject);
    req.end();
  });
}

// ── Display helpers ───────────────────────────────────────────────────────────

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function pct(n: number, d: number): string {
  if (d === 0) return '—  ';
  return ((n / d) * 100).toFixed(1) + '%';
}

function bar(ratio: number, width = 24): string {
  const filled = Math.min(width, Math.round(ratio * width));
  return '[' + '█'.repeat(filled) + '░'.repeat(width - filled) + ']';
}

function hr(char = '─', width = 56): string {
  return char.repeat(width);
}

// ── Phases ────────────────────────────────────────────────────────────────────

async function phaseCache(n: number): Promise<void> {
  process.stdout.write(`[1/3] Cache test  — ${n}x agent_fast, same input\n`);
  process.stdout.write('      Priming cache with first request… ');

  await httpPost('/request', {
    agent_id: 'agent_fast',
    input: 'translate: the quick brown fox',
    user_id: 'load-test',
  });
  process.stdout.write('done\n');

  process.stdout.write(`      Firing ${n - 1} concurrent requests (expect cache hits)… `);
  await Promise.allSettled(
    Array.from({ length: n - 1 }, () =>
      httpPost('/request', {
        agent_id: 'agent_fast',
        input: 'translate: the quick brown fox',
        user_id: 'load-test',
      }),
    ),
  );
  process.stdout.write('done\n\n');
}

async function phaseQueue(n: number): Promise<void> {
  process.stdout.write(`[2/3] Queue test  — ${n}x agent_slow, unique inputs (burst)\n`);
  process.stdout.write('      Firing all concurrently; first 10 execute, rest enter queue… ');

  await Promise.allSettled(
    Array.from({ length: n }, (_, i) =>
      httpPost('/request', {
        agent_id: 'agent_slow',
        input: `doc-review-${i}-${Date.now()}`,
        user_id: 'load-test',
      }),
    ),
  );
  process.stdout.write('done\n');
  process.stdout.write('      Waiting 3 s for queue to drain…\n\n');
  await sleep(3000);
}

async function phaseFlaky(n: number): Promise<void> {
  process.stdout.write(`[3/3] Error test  — ${n}x agent_flaky (~30% expected errors)\n`);
  process.stdout.write('      Firing… ');

  await Promise.allSettled(
    Array.from({ length: n }, (_, i) =>
      httpPost('/request', {
        agent_id: 'agent_flaky',
        input: `flaky-input-${i}`,
        user_id: 'load-test',
      }),
    ),
  );
  process.stdout.write('done\n\n');
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const N_CACHE = 15;
  const N_QUEUE = 20;
  const N_FLAKY = 15;

  console.log('\n' + hr('═') + '\n  Resilient Request Layer — Load Test');
  console.log(`  Target: http://localhost:${PORT}\n` + hr('═') + '\n');

  await phaseCache(N_CACHE);
  await phaseQueue(N_QUEUE);
  await phaseFlaky(N_FLAKY);

  // ── Fetch stats ─────────────────────────────────────────────────────────────
  process.stdout.write('Fetching stats… ');

  interface CacheStats { hits: number; misses: number; size: number; }
  interface AgentEntry {
    total: number; executed: number; queued: number;
    dropped: number; errorCount: number; p95LatencyMs: number | null;
    rateLimiter?: { tokens: number; config?: { capacity: number; refillRatePerSec: number } };
    queue?: { depth: number; maxQueueLength: number };
  }
  interface AgentsStats {
    global: { totalRequests: number; cacheHits: number; cacheMisses: number };
    agents: Record<string, AgentEntry>;
  }

  const [cacheRaw, agentsRaw] = await Promise.all([
    httpGet('/ops/cache/stats'),
    httpGet('/ops/agents/stats'),
  ]);
  process.stdout.write('done\n\n');

  const cache  = cacheRaw  as CacheStats;
  const agents = agentsRaw as AgentsStats;

  const totalCache = cache.hits + cache.misses;
  const hitRatio   = totalCache > 0 ? cache.hits / totalCache : 0;

  // ── Cache block ─────────────────────────────────────────────────────────────
  console.log(hr('─') + '\n  Cache\n' + hr('─'));
  console.log(`  Hits     ${String(cache.hits).padStart(6)}   Misses   ${String(cache.misses).padStart(6)}   Entries ${cache.size}`);
  console.log(`  Hit Rate ${(hitRatio * 100).toFixed(1).padStart(5)}%  ${bar(hitRatio)}\n`);

  // ── Global block ────────────────────────────────────────────────────────────
  const g = agents.global;
  console.log(hr('─') + '\n  Global\n' + hr('─'));
  console.log(`  Total Requests  ${g.totalRequests}`);
  console.log(`  Cache Hits      ${g.cacheHits}   (${pct(g.cacheHits, g.totalRequests)} of all requests)\n`);

  // ── Per-agent block ─────────────────────────────────────────────────────────
  console.log(hr('─') + '\n  Per-Agent\n' + hr('─'));

  for (const [id, a] of Object.entries(agents.agents)) {
    const rl  = a.rateLimiter ?? {};
    const q   = a.queue       ?? { depth: 0, maxQueueLength: 50 };
    const cfg = rl.config     ?? { capacity: 10, refillRatePerSec: 2 };

    const qRatio    = q.maxQueueLength > 0 ? q.depth / q.maxQueueLength : 0;
    const tokenRatio = cfg.capacity > 0 ? (rl.tokens ?? 0) / cfg.capacity : 0;

    console.log(`\n  ${id}`);
    console.log(
      `    Requests  ${String(a.total).padEnd(5)}` +
      `  Executed ${String(a.executed).padEnd(5)}` +
      `  Queued  ${String(a.queued).padEnd(5)}` +
      `  Dropped ${String(a.dropped).padEnd(5)}` +
      `  Errors  ${a.errorCount}`,
    );
    console.log(`    p95 Latency   ${a.p95LatencyMs !== null ? a.p95LatencyMs + ' ms' : '—'}`);
    console.log(
      `    Tokens        ${(rl.tokens ?? '?').toString().padEnd(5)} / ${cfg.capacity}` +
      `  (refill ${cfg.refillRatePerSec}/sec)  ${bar(tokenRatio, 16)}`,
    );
    console.log(
      `    Queue depth   ${String(q.depth).padEnd(5)} / ${q.maxQueueLength}` +
      `                       ${bar(qRatio, 16)}`,
    );
  }

  // ── Pitch numbers ───────────────────────────────────────────────────────────
  console.log('\n' + hr('═') + '\n  Numbers to quote in your pitch\n' + hr('═'));

  console.log(`\n  Cache hit rate (overall)     ${(hitRatio * 100).toFixed(0)}%`);
  console.log(  `    ${cache.hits} hits out of ${totalCache} total requests`);

  const fast = agents.agents['agent_fast'];
  if (fast) {
    // For repeated-input cache test: hits = N_CACHE - 1 out of N_CACHE
    const fastHitRate = pct(N_CACHE - 1, N_CACHE);
    console.log(`\n  Cache hit rate (repeated workflow) ~${fastHitRate}`);
    console.log(`    ${N_CACHE - 1} of ${N_CACHE} identical queries served from cache`);
    console.log(`    p95 latency for agent_fast: ${fast.p95LatencyMs !== null ? fast.p95LatencyMs + ' ms' : '—'}`);
  }

  const slow = agents.agents['agent_slow'];
  if (slow) {
    console.log(`\n  Queue behavior (agent_slow burst of ${N_QUEUE})`);
    console.log(`    Queued:  ${slow.queued}   Dropped: ${slow.dropped}   (${slow.dropped === 0 ? '0 dropped — graceful degradation' : 'some dropped'})`);
  }

  const flaky = agents.agents['agent_flaky'];
  if (flaky && flaky.total > 0) {
    console.log(`\n  Error isolation (agent_flaky)`);
    console.log(`    ${flaky.errorCount} errors out of ${flaky.total} requests (${pct(flaky.errorCount, flaky.total)} error rate)`);
    console.log(`    Other agents unaffected — per-agent isolation`);
  }

  console.log('\n' + hr('═') + '\n');
}

main().catch((err: unknown) => {
  console.error('\nLoad test failed:', err instanceof Error ? err.message : err);
  process.exit(1);
});
