import { allow } from './rateLimiter';
import { executeAgent } from './agents';
import * as metrics from './metrics';
import * as concurrency from './concurrency';

export interface QueuedRequest {
  requestId: string;
  agentId: string;
  input: string;
  userId?: string;
  workflowId?: string;
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
}

interface QueueState {
  items: QueuedRequest[];
  maxQueueLength: number;
}

const DEFAULT_MAX_QUEUE_LENGTH = 50;
const queues = new Map<string, QueueState>();

function getOrCreate(agentId: string): QueueState {
  if (!queues.has(agentId)) {
    queues.set(agentId, { items: [], maxQueueLength: DEFAULT_MAX_QUEUE_LENGTH });
  }
  return queues.get(agentId)!;
}

export function enqueue(
  agentId: string,
  req: QueuedRequest,
): { enqueued: boolean; requestId?: string } {
  const q = getOrCreate(agentId);
  if (q.items.length >= q.maxQueueLength) {
    return { enqueued: false };
  }
  q.items.push(req);
  return { enqueued: true, requestId: req.requestId };
}

export function getQueueStats(agentId: string): { depth: number; maxQueueLength: number } {
  const q = getOrCreate(agentId);
  return { depth: q.items.length, maxQueueLength: q.maxQueueLength };
}

export function configureQueue(agentId: string, maxQueueLength: number): void {
  getOrCreate(agentId).maxQueueLength = maxQueueLength;
}

// Background worker — runs every 50 ms.
// Both the rate limiter AND the concurrency guard must allow before dequeueing.
setInterval(() => {
  for (const [agentId, q] of queues) {
    if (q.items.length === 0) continue;
    if (!concurrency.canExecute(agentId)) continue; // concurrency ceiling hit
    if (!allow(agentId)) continue;                  // token bucket empty

    const item = q.items.shift()!;
    concurrency.startExecution(agentId);
    const start = Date.now();

    executeAgent(item.agentId, {
      input: item.input,
      userId: item.userId,
      workflowId: item.workflowId,
    })
      .then((result) => {
        concurrency.finishExecution(agentId);
        metrics.recordExecuted(agentId, Date.now() - start);
        item.resolve(result);
      })
      .catch((err: unknown) => {
        concurrency.finishExecution(agentId);
        metrics.recordError(agentId);
        item.reject(err);
      });
  }
}, 50);
