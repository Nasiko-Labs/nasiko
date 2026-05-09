// Per-agent concurrency guard.
// Tracks in-flight executions and enforces a per-agent ceiling so one noisy
// agent cannot monopolise the Node.js event-loop or downstream resources.

export interface ConcurrencyStats {
  inFlight: number;
  maxConcurrent: number;
}

interface ConcurrencyState {
  inFlight: number;
  maxConcurrent: number;
}

const DEFAULT_MAX_CONCURRENT = 10;
const states = new Map<string, ConcurrencyState>();

function getOrCreate(agentId: string): ConcurrencyState {
  if (!states.has(agentId)) {
    states.set(agentId, { inFlight: 0, maxConcurrent: DEFAULT_MAX_CONCURRENT });
  }
  return states.get(agentId)!;
}

/** Returns true only if this agent has a free execution slot. Does NOT consume the slot. */
export function canExecute(agentId: string): boolean {
  return getOrCreate(agentId).inFlight < getOrCreate(agentId).maxConcurrent;
}

/** Must be called immediately before executeAgent to claim a slot. */
export function startExecution(agentId: string): void {
  getOrCreate(agentId).inFlight++;
}

/** Must be called in both success and error paths (use finally) to release the slot. */
export function finishExecution(agentId: string): void {
  const state = getOrCreate(agentId);
  if (state.inFlight > 0) state.inFlight--;
}

export function getStats(agentId: string): ConcurrencyStats {
  const s = getOrCreate(agentId);
  return { inFlight: s.inFlight, maxConcurrent: s.maxConcurrent };
}

export function configure(agentId: string, maxConcurrent: number): void {
  getOrCreate(agentId).maxConcurrent = maxConcurrent;
}
