const config = require("../config");

/**
 * CacheManager — Redis-backed response cache with per-agent tracking.
 *
 * Stores agent responses keyed by content hash.
 * Tracks hit/miss stats for monitoring.
 */
class CacheManager {
  /**
   * @param {import("ioredis").Redis} redis - ioredis instance
   */
  constructor(redis) {
    this.redis = redis;
    this.statsKey = "cache:stats";
  }

  /**
   * Get a cached response.
   * @param {string} cacheKey - The cache key
   * @returns {object|null} Cached response data or null
   */
  async get(cacheKey) {
    try {
      const data = await this.redis.get(cacheKey);
      if (data) {
        await this._incrementStat("hits");
        return JSON.parse(data);
      }
      await this._incrementStat("misses");
      return null;
    } catch (err) {
      console.error("Cache GET error:", err.message);
      await this._incrementStat("misses");
      return null;
    }
  }

  /**
   * Store a response in cache.
   * @param {string} cacheKey - The cache key
   * @param {object} responseData - The response to cache
   * @param {number} [ttl] - TTL in seconds (default from config)
   */
  async set(cacheKey, responseData, ttl = config.cacheDefaultTTL) {
    try {
      const payload = JSON.stringify({
        data: responseData,
        cachedAt: new Date().toISOString(),
        ttl,
      });

      await this.redis.setex(cacheKey, ttl, payload);
      await this._incrementStat("sets");

      // Track the key in a set for listing/counting
      await this.redis.sadd("cache:keys", cacheKey);
    } catch (err) {
      console.error("Cache SET error:", err.message);
    }
  }

  /**
   * Invalidate a specific cache entry.
   * @param {string} cacheKey
   * @returns {boolean} Whether the key existed
   */
  async invalidate(cacheKey) {
    try {
      const result = await this.redis.del(cacheKey);
      await this.redis.srem("cache:keys", cacheKey);
      await this._incrementStat("invalidations");
      return result > 0;
    } catch (err) {
      console.error("Cache INVALIDATE error:", err.message);
      return false;
    }
  }

  /**
   * Invalidate all cache entries for a specific agent.
   * @param {string} agentName
   * @returns {number} Number of entries invalidated
   */
  async invalidateAgent(agentName) {
    try {
      const pattern = `cache:${agentName.toLowerCase()}:*`;
      let count = 0;
      let cursor = "0";

      do {
        const [nextCursor, keys] = await this.redis.scan(cursor, "MATCH", pattern, "COUNT", 100);
        cursor = nextCursor;

        if (keys.length > 0) {
          await this.redis.del(...keys);
          for (const key of keys) {
            await this.redis.srem("cache:keys", key);
          }
          count += keys.length;
        }
      } while (cursor !== "0");

      return count;
    } catch (err) {
      console.error("Cache INVALIDATE_AGENT error:", err.message);
      return 0;
    }
  }

  /**
   * Flush the entire cache.
   * @returns {number} Number of entries flushed
   */
  async flush() {
    try {
      const keys = await this.redis.smembers("cache:keys");
      let count = 0;

      if (keys.length > 0) {
        count = await this.redis.del(...keys);
      }

      // Clean up tracking
      await this.redis.del("cache:keys");
      await this._incrementStat("flushes");

      return count;
    } catch (err) {
      console.error("Cache FLUSH error:", err.message);
      return 0;
    }
  }

  /**
   * Get cache statistics.
   * @returns {object} Cache stats
   */
  async getStats() {
    try {
      const stats = await this.redis.hgetall(this.statsKey);
      const totalEntries = await this.redis.scard("cache:keys");

      const hits = parseInt(stats.hits || "0", 10);
      const misses = parseInt(stats.misses || "0", 10);
      const total = hits + misses;

      return {
        hits,
        misses,
        sets: parseInt(stats.sets || "0", 10),
        invalidations: parseInt(stats.invalidations || "0", 10),
        flushes: parseInt(stats.flushes || "0", 10),
        hitRate: total > 0 ? ((hits / total) * 100).toFixed(2) : "0.00",
        totalEntries,
      };
    } catch (err) {
      console.error("Cache STATS error:", err.message);
      return { hits: 0, misses: 0, hitRate: "0.00", totalEntries: 0 };
    }
  }

  /**
   * List cached entries (metadata only, not full responses).
   * @returns {Array} List of cache entry metadata
   */
  async listEntries() {
    try {
      const keys = await this.redis.smembers("cache:keys");
      const entries = [];

      for (const key of keys) {
        const ttl = await this.redis.ttl(key);
        const parts = key.split(":");
        entries.push({
          key,
          agent: parts[1] || "unknown",
          hash: parts[2] || "",
          ttlRemaining: ttl,
        });
      }

      return entries;
    } catch (err) {
      console.error("Cache LIST error:", err.message);
      return [];
    }
  }

  /**
   * Reset stats counters.
   */
  async resetStats() {
    await this.redis.del(this.statsKey);
  }

  // --- Private helpers ---

  async _incrementStat(field) {
    try {
      await this.redis.hincrby(this.statsKey, field, 1);
    } catch {
      // Silently fail — stats are non-critical
    }
  }
}

module.exports = CacheManager;
