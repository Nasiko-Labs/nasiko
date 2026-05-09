import { createHash } from 'crypto';

interface CacheEntry {
  value: unknown;
  expiresAt: number;
}

interface CacheStats {
  hits: number;
  misses: number;
  size: number;
}

let hits = 0;
let misses = 0;
const store = new Map<string, CacheEntry>();

export function buildKey(agentId: string, input: string, workflowId?: string): string {
  const payload = JSON.stringify({
    agentId,
    // Normalize so "Hello" and " hello " hit the same cache entry
    input: input.trim().toLowerCase(),
    workflowId: workflowId ?? null,
  });
  return createHash('sha256').update(payload).digest('hex');
}

export function get(key: string): unknown | null {
  const entry = store.get(key);
  if (!entry) {
    misses++;
    return null;
  }
  if (Date.now() > entry.expiresAt) {
    store.delete(key);
    misses++;
    return null;
  }
  hits++;
  return entry.value;
}

export function set(key: string, value: unknown, ttlMs: number): void {
  store.set(key, { value, expiresAt: Date.now() + ttlMs });
}

export function stats(): CacheStats {
  const now = Date.now();
  for (const [key, entry] of store) {
    if (now > entry.expiresAt) store.delete(key);
  }
  return { hits, misses, size: store.size };
}
