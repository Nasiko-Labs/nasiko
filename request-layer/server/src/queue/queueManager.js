const config = require("../config");

/**
 * QueueManager — Lightweight per-agent overflow queue.
 *
 * When rate limiting blocks a request, it enters the queue and waits
 * for a token to become available. If the wait exceeds maxWaitTime
 * or the queue is full, the request is rejected with 429.
 *
 * Uses in-memory queues with Redis for stats persistence.
 * This is intentionally simple for hackathon feasibility — no BullMQ workers needed.
 */
class QueueManager {
  /**
   * @param {import("ioredis").Redis} redis
   * @param {import("../limiter/rateLimiter")} rateLimiter
   */
  constructor(redis, rateLimiter) {
    this.redis = redis;
    this.rateLimiter = rateLimiter;

    // In-memory per-agent queues: { agentName: { waiting: number, processed: number } }
    this.queues = {};
  }

  /**
   * Enqueue a request and wait for a rate limit token.
   *
   * @param {string} agentName
   * @param {object} [options={}]
   * @returns {{ acquired: boolean, waitTime: number, queuePosition: number, rejected: boolean }}
   */
  async enqueueAndWait(agentName, options = {}) {
    const agentConfig = await this.rateLimiter.getAgentConfig(agentName);
    const maxQueueSize = options.maxQueueSize || agentConfig.maxQueueSize || config.rateLimitMaxQueueSize;
    const maxWaitTime = options.maxWaitTime || agentConfig.maxWaitTime || config.rateLimitMaxWaitTime;

    // Initialize queue tracking for this agent
    if (!this.queues[agentName]) {
      this.queues[agentName] = { waiting: 0, processed: 0, totalWaitTime: 0 };
    }

    const queue = this.queues[agentName];

    // Check if queue is full
    if (queue.waiting >= maxQueueSize) {
      await this._recordQueueEvent(agentName, "rejected");
      return {
        acquired: false,
        waitTime: 0,
        queuePosition: -1,
        rejected: true,
        reason: "queue_full",
        estimatedWait: this._estimateWait(agentName),
      };
    }

    // Enter queue
    queue.waiting++;
    const position = queue.waiting;
    const enterTime = Date.now();
    await this._recordQueueEvent(agentName, "enqueued");

    // Poll for token availability
    const pollInterval = 250; // Check every 250ms
    let elapsed = 0;

    while (elapsed < maxWaitTime) {
      const result = await this.rateLimiter.tryAcquire(agentName);

      if (result.allowed) {
        // Token acquired!
        queue.waiting--;
        queue.processed++;
        const waitTime = Date.now() - enterTime;
        queue.totalWaitTime += waitTime;
        await this._recordQueueEvent(agentName, "dequeued");

        return {
          acquired: true,
          waitTime,
          queuePosition: position,
          rejected: false,
        };
      }

      // Wait before polling again
      await this._sleep(Math.min(pollInterval, result.retryAfterMs || pollInterval));
      elapsed = Date.now() - enterTime;
    }

    // Timeout — request waited too long
    queue.waiting--;
    await this._recordQueueEvent(agentName, "timeout");

    return {
      acquired: false,
      waitTime: Date.now() - enterTime,
      queuePosition: position,
      rejected: true,
      reason: "timeout",
    };
  }

  /**
   * Get queue status for all agents.
   */
  getStatus() {
    const status = {};
    for (const [agent, queue] of Object.entries(this.queues)) {
      status[agent] = {
        waiting: queue.waiting,
        processed: queue.processed,
        avgWaitTime: queue.processed > 0
          ? Math.round(queue.totalWaitTime / queue.processed)
          : 0,
      };
    }
    return status;
  }

  /**
   * Get queue status for a specific agent.
   */
  getAgentStatus(agentName) {
    const queue = this.queues[agentName];
    if (!queue) {
      return { waiting: 0, processed: 0, avgWaitTime: 0 };
    }
    return {
      waiting: queue.waiting,
      processed: queue.processed,
      avgWaitTime: queue.processed > 0
        ? Math.round(queue.totalWaitTime / queue.processed)
        : 0,
    };
  }

  /**
   * Get queue events/stats from Redis (persistent).
   */
  async getAgentEvents(agentName) {
    const key = `queue:stats:${agentName}`;
    const stats = await this.redis.hgetall(key);
    return {
      agent: agentName,
      enqueued: parseInt(stats.enqueued || "0", 10),
      dequeued: parseInt(stats.dequeued || "0", 10),
      rejected: parseInt(stats.rejected || "0", 10),
      timeout: parseInt(stats.timeout || "0", 10),
    };
  }

  /**
   * Estimate wait time based on recent history.
   */
  _estimateWait(agentName) {
    const queue = this.queues[agentName];
    if (!queue || queue.processed === 0) return "unknown";
    return Math.round(queue.totalWaitTime / queue.processed) + "ms (avg)";
  }

  async _recordQueueEvent(agentName, event) {
    const key = `queue:stats:${agentName}`;
    await this.redis.hincrby(key, event, 1);
  }

  _sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }
}

module.exports = QueueManager;
