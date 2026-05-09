const config = require("../config");

/**
 * TokenBucketRateLimiter — Per-agent rate limiting using Redis-backed token buckets.
 *
 * Each agent gets its own bucket with configurable:
 *   - maxTokens (burst capacity)
 *   - refillRate (tokens per second)
 *
 * Uses a Lua script for atomic token operations (no race conditions).
 */
class RateLimiter {
  /**
   * @param {import("ioredis").Redis} redis
   */
  constructor(redis) {
    this.redis = redis;
    this.configKey = "ratelimit:config";

    // Lua script for atomic token bucket operation
    // Returns: [allowed (0|1), tokensRemaining, retryAfterMs]
    this.luaScript = `
      local key = KEYS[1]
      local maxTokens = tonumber(ARGV[1])
      local refillRate = tonumber(ARGV[2])
      local now = tonumber(ARGV[3])

      local bucket = redis.call('HMGET', key, 'tokens', 'lastRefill')
      local tokens = tonumber(bucket[1])
      local lastRefill = tonumber(bucket[2])

      -- Initialize bucket if new
      if tokens == nil then
        tokens = maxTokens
        lastRefill = now
      end

      -- Refill tokens based on elapsed time
      local elapsed = (now - lastRefill) / 1000
      local newTokens = elapsed * refillRate
      tokens = math.min(maxTokens, tokens + newTokens)

      -- Try to consume a token
      if tokens >= 1 then
        tokens = tokens - 1
        redis.call('HMSET', key, 'tokens', tokens, 'lastRefill', now)
        return {1, math.floor(tokens * 100) / 100, 0}
      else
        -- Calculate time until next token
        local deficit = 1 - tokens
        local retryAfterMs = math.ceil((deficit / refillRate) * 1000)
        redis.call('HMSET', key, 'tokens', tokens, 'lastRefill', now)
        return {0, 0, retryAfterMs}
      end
    `;
  }

  /**
   * Try to acquire a token for an agent.
   * @param {string} agentName
   * @returns {{ allowed: boolean, tokensRemaining: number, retryAfterMs: number }}
   */
  async tryAcquire(agentName) {
    const agentConfig = await this.getAgentConfig(agentName);
    const key = `ratelimit:bucket:${agentName}`;
    const now = Date.now();

    try {
      const result = await this.redis.eval(
        this.luaScript,
        1,
        key,
        agentConfig.maxTokens,
        agentConfig.refillRate,
        now
      );

      const allowed = result[0] === 1;
      const tokensRemaining = result[1];
      const retryAfterMs = result[2];

      // Track stats
      await this._recordAttempt(agentName, allowed);

      return { allowed, tokensRemaining, retryAfterMs };
    } catch (err) {
      console.error("RateLimiter error:", err.message);
      // Fail-open: allow request if rate limiter errors
      return { allowed: true, tokensRemaining: -1, retryAfterMs: 0 };
    }
  }

  /**
   * Get rate limit config for a specific agent.
   * Falls back to global defaults if no per-agent config exists.
   */
  async getAgentConfig(agentName) {
    try {
      const stored = await this.redis.hget(this.configKey, agentName);
      if (stored) {
        return JSON.parse(stored);
      }
    } catch {
      // Fall through to defaults
    }

    return {
      maxTokens: config.rateLimitMaxTokens,
      refillRate: config.rateLimitRefillRate,
      maxQueueSize: config.rateLimitMaxQueueSize,
      maxWaitTime: config.rateLimitMaxWaitTime,
    };
  }

  /**
   * Set rate limit config for a specific agent.
   */
  async setAgentConfig(agentName, newConfig) {
    const current = await this.getAgentConfig(agentName);
    const merged = { ...current, ...newConfig };

    await this.redis.hset(this.configKey, agentName, JSON.stringify(merged));

    // Reset the bucket so new config takes effect immediately
    await this.redis.del(`ratelimit:bucket:${agentName}`);

    return merged;
  }

  /**
   * Get all agent configs.
   */
  async getAllConfigs() {
    const configs = await this.redis.hgetall(this.configKey);
    const result = {};

    for (const [agent, configStr] of Object.entries(configs)) {
      result[agent] = JSON.parse(configStr);
    }

    return result;
  }

  /**
   * Get the current token bucket status for an agent.
   */
  async getBucketStatus(agentName) {
    const key = `ratelimit:bucket:${agentName}`;
    const bucket = await this.redis.hgetall(key);
    const agentConfig = await this.getAgentConfig(agentName);

    if (!bucket.tokens) {
      return {
        agent: agentName,
        tokens: agentConfig.maxTokens,
        maxTokens: agentConfig.maxTokens,
        refillRate: agentConfig.refillRate,
        status: "full",
      };
    }

    // Calculate current tokens with refill
    const now = Date.now();
    const elapsed = (now - parseFloat(bucket.lastRefill)) / 1000;
    const currentTokens = Math.min(
      agentConfig.maxTokens,
      parseFloat(bucket.tokens) + elapsed * agentConfig.refillRate
    );

    return {
      agent: agentName,
      tokens: Math.round(currentTokens * 100) / 100,
      maxTokens: agentConfig.maxTokens,
      refillRate: agentConfig.refillRate,
      status: currentTokens >= 1 ? "available" : "exhausted",
    };
  }

  /**
   * Get rate limiter stats for an agent.
   */
  async getStats(agentName) {
    const key = `ratelimit:stats:${agentName}`;
    const stats = await this.redis.hgetall(key);

    const allowed = parseInt(stats.allowed || "0", 10);
    const denied = parseInt(stats.denied || "0", 10);
    const total = allowed + denied;

    return {
      agent: agentName,
      allowed,
      denied,
      total,
      denyRate: total > 0 ? ((denied / total) * 100).toFixed(2) : "0.00",
    };
  }

  /**
   * Get stats for all agents.
   */
  async getAllStats() {
    // Scan for all stat keys
    const stats = {};
    let cursor = "0";

    do {
      const [nextCursor, keys] = await this.redis.scan(
        cursor, "MATCH", "ratelimit:stats:*", "COUNT", 100
      );
      cursor = nextCursor;

      for (const key of keys) {
        const agentName = key.replace("ratelimit:stats:", "");
        stats[agentName] = await this.getStats(agentName);
      }
    } while (cursor !== "0");

    return stats;
  }

  /**
   * Reset stats for an agent.
   */
  async resetStats(agentName) {
    await this.redis.del(`ratelimit:stats:${agentName}`);
  }

  // --- Private ---

  async _recordAttempt(agentName, allowed) {
    const key = `ratelimit:stats:${agentName}`;
    const field = allowed ? "allowed" : "denied";
    await this.redis.hincrby(key, field, 1);
  }
}

module.exports = RateLimiter;
