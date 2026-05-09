export interface BucketConfig {
  capacity: number;
  refillRatePerSec: number;
  /** When true, minTokensReserved creates a floor that is never consumed. */
  critical: boolean;
  /** Execution is only allowed when tokens > this floor (i.e. >= floor + 1). */
  minTokensReserved: number;
}

export interface BucketStats {
  tokens: number;
  config: BucketConfig;
}

interface Bucket {
  tokens: number;
  lastRefill: number;
  config: BucketConfig;
}

// Sensible defaults — all new fields are zero/false so existing behaviour is unchanged.
const DEFAULT_CONFIG: BucketConfig = {
  capacity: 10, refillRatePerSec: 2, critical: false, minTokensReserved: 0,
};

const buckets = new Map<string, Bucket>();

function getOrCreate(agentId: string): Bucket {
  if (!buckets.has(agentId)) {
    buckets.set(agentId, {
      tokens: DEFAULT_CONFIG.capacity,
      lastRefill: Date.now(),
      config: { ...DEFAULT_CONFIG },
    });
  }
  return buckets.get(agentId)!;
}

// Drip tokens proportional to elapsed time — called on every read/write of the bucket.
function refill(bucket: Bucket): void {
  const now = Date.now();
  const elapsed = (now - bucket.lastRefill) / 1000;
  bucket.tokens = Math.min(
    bucket.config.capacity,
    bucket.tokens + elapsed * bucket.config.refillRatePerSec,
  );
  bucket.lastRefill = now;
}

export function allow(agentId: string): boolean {
  const bucket = getOrCreate(agentId);
  refill(bucket);
  // For critical agents: never consume below the reserved floor.
  // minTokensReserved = 0 (default) keeps the original `tokens >= 1` behaviour.
  if (bucket.tokens >= 1 + bucket.config.minTokensReserved) {
    bucket.tokens -= 1;
    return true;
  }
  return false;
}

export function getStats(agentId: string): BucketStats {
  const bucket = getOrCreate(agentId);
  refill(bucket);
  // Round to 2 decimal places so UI doesn't show 9.999999…
  return {
    tokens: Math.round(bucket.tokens * 100) / 100,
    config: { ...bucket.config },
  };
}

export function configure(agentId: string, patch: Partial<BucketConfig>): void {
  const bucket = getOrCreate(agentId);
  if (patch.capacity !== undefined)          bucket.config.capacity          = patch.capacity;
  if (patch.refillRatePerSec !== undefined)  bucket.config.refillRatePerSec  = patch.refillRatePerSec;
  if (patch.critical !== undefined)          bucket.config.critical          = patch.critical;
  if (patch.minTokensReserved !== undefined) bucket.config.minTokensReserved = patch.minTokensReserved;
  // Don't let current tokens exceed the new capacity
  bucket.tokens = Math.min(bucket.tokens, bucket.config.capacity);
}
