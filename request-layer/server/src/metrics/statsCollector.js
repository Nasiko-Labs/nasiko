/**
 * StatsCollector — Aggregates runtime metrics from cache, rate limiter, and queue.
 *
 * Provides a unified view of system health and performance for the dashboard.
 * Stores time-series data in Redis for historical charts.
 */
class StatsCollector {
  /**
   * @param {import("ioredis").Redis} redis
   */
  constructor(redis) {
    this.redis = redis;
    this.startTime = Date.now();
    this.requestLog = "stats:request_log";
    this.timeSeriesKey = "stats:timeseries";
  }

  /**
   * Record a completed request for stats tracking.
   */
  async recordRequest(data) {
    const {
      agent,
      cached = false,
      latency = 0,
      queued = false,
      rateLimited = false,
      success = true,
    } = data;

    const timestamp = Date.now();

    // Increment global counters
    await this.redis.hincrby("stats:global", "totalRequests", 1);
    if (cached) await this.redis.hincrby("stats:global", "cacheHits", 1);
    if (queued) await this.redis.hincrby("stats:global", "queuedRequests", 1);
    if (rateLimited) await this.redis.hincrby("stats:global", "rateLimitedRequests", 1);
    if (!success) await this.redis.hincrby("stats:global", "failedRequests", 1);

    // Increment per-agent counters
    const agentKey = `stats:agent:${agent}`;
    await this.redis.hincrby(agentKey, "totalRequests", 1);
    if (cached) await this.redis.hincrby(agentKey, "cacheHits", 1);
    if (queued) await this.redis.hincrby(agentKey, "queuedRequests", 1);
    if (!success) await this.redis.hincrby(agentKey, "failedRequests", 1);

    // Store latency for percentile calculations
    await this.redis.zadd(`stats:latency:${agent}`, timestamp, `${latency}:${timestamp}`);

    // Store in time-series (bucketed by 10-second windows)
    const bucket = Math.floor(timestamp / 10000) * 10000;
    const tsKey = `stats:ts:${bucket}`;
    await this.redis.hincrby(tsKey, "requests", 1);
    if (cached) await this.redis.hincrby(tsKey, "cacheHits", 1);
    if (queued) await this.redis.hincrby(tsKey, "queued", 1);
    await this.redis.hincrby(tsKey, "totalLatency", latency);
    await this.redis.expire(tsKey, 3600); // Keep for 1 hour

    // Add to recent request log (keep last 100)
    const logEntry = JSON.stringify({
      agent,
      cached,
      latency,
      queued,
      success,
      timestamp: new Date(timestamp).toISOString(),
    });
    await this.redis.lpush(this.requestLog, logEntry);
    await this.redis.ltrim(this.requestLog, 0, 99);
  }

  /**
   * Get global runtime statistics.
   */
  async getGlobalStats() {
    const raw = await this.redis.hgetall("stats:global");

    const totalRequests = parseInt(raw.totalRequests || "0", 10);
    const cacheHits = parseInt(raw.cacheHits || "0", 10);
    const queuedRequests = parseInt(raw.queuedRequests || "0", 10);
    const rateLimitedRequests = parseInt(raw.rateLimitedRequests || "0", 10);
    const failedRequests = parseInt(raw.failedRequests || "0", 10);

    return {
      totalRequests,
      cacheHits,
      cacheMisses: totalRequests - cacheHits,
      cacheHitRate: totalRequests > 0
        ? ((cacheHits / totalRequests) * 100).toFixed(2)
        : "0.00",
      queuedRequests,
      rateLimitedRequests,
      failedRequests,
      successRate: totalRequests > 0
        ? (((totalRequests - failedRequests) / totalRequests) * 100).toFixed(2)
        : "100.00",
      uptime: Math.floor((Date.now() - this.startTime) / 1000),
    };
  }

  /**
   * Get per-agent statistics.
   */
  async getAgentStats(agentName) {
    const raw = await this.redis.hgetall(`stats:agent:${agentName}`);

    const totalRequests = parseInt(raw.totalRequests || "0", 10);
    const cacheHits = parseInt(raw.cacheHits || "0", 10);
    const queuedRequests = parseInt(raw.queuedRequests || "0", 10);
    const failedRequests = parseInt(raw.failedRequests || "0", 10);

    // Get average latency from recent samples
    const latencies = await this._getLatencies(agentName);

    return {
      agent: agentName,
      totalRequests,
      cacheHits,
      cacheHitRate: totalRequests > 0
        ? ((cacheHits / totalRequests) * 100).toFixed(2)
        : "0.00",
      queuedRequests,
      failedRequests,
      avgLatency: latencies.avg,
      p95Latency: latencies.p95,
      minLatency: latencies.min,
      maxLatency: latencies.max,
    };
  }

  /**
   * Get all per-agent stats.
   */
  async getAllAgentStats() {
    const agents = {};
    let cursor = "0";

    do {
      const [nextCursor, keys] = await this.redis.scan(
        cursor, "MATCH", "stats:agent:*", "COUNT", 100
      );
      cursor = nextCursor;

      for (const key of keys) {
        const agentName = key.replace("stats:agent:", "");
        agents[agentName] = await this.getAgentStats(agentName);
      }
    } while (cursor !== "0");

    return agents;
  }

  /**
   * Get time-series data (last 1 hour, bucketed by 10 seconds).
   */
  async getTimeSeries(durationMs = 3600000) {
    const now = Date.now();
    const start = now - durationMs;
    const series = [];

    for (let t = Math.floor(start / 10000) * 10000; t <= now; t += 10000) {
      const tsKey = `stats:ts:${t}`;
      const data = await this.redis.hgetall(tsKey);

      if (Object.keys(data).length > 0) {
        const requests = parseInt(data.requests || "0", 10);
        const totalLatency = parseInt(data.totalLatency || "0", 10);

        series.push({
          timestamp: new Date(t).toISOString(),
          time: t,
          requests,
          cacheHits: parseInt(data.cacheHits || "0", 10),
          queued: parseInt(data.queued || "0", 10),
          avgLatency: requests > 0 ? Math.round(totalLatency / requests) : 0,
        });
      }
    }

    return series;
  }

  /**
   * Get recent request log.
   */
  async getRecentRequests(count = 50) {
    const raw = await this.redis.lrange(this.requestLog, 0, count - 1);
    return raw.map((entry) => JSON.parse(entry));
  }

  /**
   * Reset all stats.
   */
  async reset() {
    // Delete global stats
    await this.redis.del("stats:global");

    // Delete agent stats
    let cursor = "0";
    do {
      const [nextCursor, keys] = await this.redis.scan(
        cursor, "MATCH", "stats:*", "COUNT", 100
      );
      cursor = nextCursor;
      if (keys.length > 0) await this.redis.del(...keys);
    } while (cursor !== "0");
  }

  // --- Private ---

  async _getLatencies(agentName) {
    const key = `stats:latency:${agentName}`;
    const entries = await this.redis.zrange(key, -100, -1); // Last 100

    if (entries.length === 0) {
      return { avg: 0, p95: 0, min: 0, max: 0 };
    }

    const values = entries.map((e) => parseInt(e.split(":")[0], 10));
    values.sort((a, b) => a - b);

    const sum = values.reduce((a, b) => a + b, 0);
    const p95Index = Math.floor(values.length * 0.95);

    return {
      avg: Math.round(sum / values.length),
      p95: values[p95Index] || values[values.length - 1],
      min: values[0],
      max: values[values.length - 1],
    };
  }
}

module.exports = StatsCollector;
