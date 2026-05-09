export interface GlobalMetrics {
  totalRequests: number;
  cacheHits: number;
  cacheMisses: number;
  workflowDedupeHits: number;
}

// Shape returned in snapshots (latencies is a copy, p95 already computed)
export interface PerAgentMetricsSnapshot {
  total: number;
  executed: number;
  queued: number;
  dropped: number;
  errorCount: number;
  workflowDedupeHits: number;
  latencies: number[];
  p95LatencyMs: number | null;
}

export interface MetricsSnapshot {
  global: GlobalMetrics;
  agents: Record<string, PerAgentMetricsSnapshot>;
}

// Internal shape keeps a mutable latency ring buffer
interface PerAgentMetricsInternal {
  total: number;
  executed: number;
  queued: number;
  dropped: number;
  errorCount: number;
  workflowDedupeHits: number;
  latencies: number[]; // rolling last-100
}

const globalMetrics: GlobalMetrics = {
  totalRequests: 0, cacheHits: 0, cacheMisses: 0, workflowDedupeHits: 0,
};
const agentMetrics = new Map<string, PerAgentMetricsInternal>();

function getOrCreate(agentId: string): PerAgentMetricsInternal {
  if (!agentMetrics.has(agentId)) {
    agentMetrics.set(agentId, {
      total: 0, executed: 0, queued: 0, dropped: 0,
      errorCount: 0, workflowDedupeHits: 0, latencies: [],
    });
  }
  return agentMetrics.get(agentId)!;
}

function calcP95(latencies: number[]): number | null {
  if (latencies.length === 0) return null;
  const sorted = [...latencies].sort((a, b) => a - b);
  const idx = Math.ceil(sorted.length * 0.95) - 1;
  return sorted[Math.max(0, idx)];
}

export function recordRequest(agentId: string): void {
  globalMetrics.totalRequests++;
  getOrCreate(agentId).total++;
}

export function recordCacheHit(): void {
  globalMetrics.cacheHits++;
}

export function recordCacheMiss(): void {
  globalMetrics.cacheMisses++;
}

export function recordExecuted(agentId: string, latencyMs: number): void {
  const m = getOrCreate(agentId);
  m.executed++;
  m.latencies.push(latencyMs);
  if (m.latencies.length > 100) m.latencies.shift();
}

export function recordQueued(agentId: string): void {
  getOrCreate(agentId).queued++;
}

export function recordDropped(agentId: string): void {
  getOrCreate(agentId).dropped++;
}

export function recordError(agentId: string): void {
  getOrCreate(agentId).errorCount++;
}

export function recordWorkflowDedupe(agentId: string): void {
  globalMetrics.workflowDedupeHits++;
  getOrCreate(agentId).workflowDedupeHits++;
}

export function snapshot(): MetricsSnapshot {
  const agents: Record<string, PerAgentMetricsSnapshot> = {};
  for (const [id, m] of agentMetrics) {
    const latencies = [...m.latencies];
    agents[id] = { ...m, latencies, p95LatencyMs: calcP95(latencies) };
  }
  return { global: { ...globalMetrics }, agents };
}
