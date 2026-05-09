const express = require("express");
const router = express.Router();
const CacheManager = require("../cache/cacheManager");
const RequestHasher = require("../cache/hasher");
const RateLimiter = require("../limiter/rateLimiter");
const QueueManager = require("../queue/queueManager");
const AgentProxy = require("../proxy/agentProxy");

const agentProxy = new AgentProxy();

/**
 * POST /api/process
 *
 * Main entry point for the request layer.
 * Flow: Check Cache → Rate Limit → Queue (if needed) → Forward to Agent → Cache Response
 *
 * Body: { agent: string, query: string, params?: object, ttl?: number }
 */
router.post("/process", async (req, res) => {
  const { agent, query, params = {}, ttl } = req.body;

  // Validate input
  if (!agent || !query) {
    return res.status(400).json({
      error: "Missing required fields: 'agent' and 'query'",
    });
  }

  const redis = req.app.get("redis");
  const io = req.app.get("io");
  const queueManager = req.app.get("queueManager");
  const statsCollector = req.app.get("statsCollector");
  const cacheManager = new CacheManager(redis);
  const rateLimiter = new RateLimiter(redis);
  const startTime = Date.now();

  // Step 1: Generate cache key
  const cacheKey = RequestHasher.computeKey(agent, query, params);

  // Step 2: Check cache
  const cached = await cacheManager.get(cacheKey);
  if (cached) {
    const latency = Date.now() - startTime;

    const result = {
      success: true,
      cached: true,
      cacheKey,
      agent,
      response: cached.data,
      latency,
      originalLatency: cached.data?.latency || null,
      cachedAt: cached.cachedAt,
      rateLimited: false,
      queued: false,
      timestamp: new Date().toISOString(),
    };

    // Emit real-time event
    io.emit("request:completed", {
      agent,
      query: query.substring(0, 100),
      cached: true,
      latency,
      rateLimited: false,
      queued: false,
      timestamp: result.timestamp,
    });

    // Record stats
    if (statsCollector) {
      await statsCollector.recordRequest({ agent, cached: true, latency, success: true });
    }

    return res.json(result);
  }

  // Step 3: Rate limit check
  let wasQueued = false;
  let queueWaitTime = 0;
  const rateLimitResult = await rateLimiter.tryAcquire(agent);

  if (!rateLimitResult.allowed) {
    // Step 3b: Try to queue the request
    if (queueManager) {
      io.emit("request:queued", {
        agent,
        query: query.substring(0, 100),
        timestamp: new Date().toISOString(),
      });

      const queueResult = await queueManager.enqueueAndWait(agent);

      if (!queueResult.acquired) {
        // Queue full or timeout — reject
        const latency = Date.now() - startTime;

        io.emit("request:rejected", {
          agent,
          reason: queueResult.reason,
          latency,
          timestamp: new Date().toISOString(),
        });

        return res.status(429).json({
          error: "Rate limited",
          reason: queueResult.reason,
          retryAfterMs: rateLimitResult.retryAfterMs,
          queuePosition: queueResult.queuePosition,
          waitTime: queueResult.waitTime,
          estimatedWait: queueResult.estimatedWait,
        });
      }

      // Successfully dequeued — got a token
      wasQueued = true;
      queueWaitTime = queueResult.waitTime;
    } else {
      // No queue manager — reject immediately
      return res.status(429).json({
        error: "Rate limited",
        retryAfterMs: rateLimitResult.retryAfterMs,
        tokensRemaining: rateLimitResult.tokensRemaining,
      });
    }
  }

  // Step 4: Forward to agent
  const agentResult = await agentProxy.forward(agent, query, {
    token: req.headers.authorization?.replace("Bearer ", ""),
    params,
  });

  const totalLatency = Date.now() - startTime;

  // Step 5: Cache successful responses
  if (agentResult.success) {
    await cacheManager.set(cacheKey, agentResult, ttl);
  }

  const result = {
    success: agentResult.success,
    cached: false,
    cacheKey,
    agent,
    response: agentResult.success ? agentResult.response : undefined,
    error: agentResult.error || undefined,
    latency: totalLatency,
    agentLatency: agentResult.latency,
    source: agentResult.source,
    rateLimited: wasQueued,
    queued: wasQueued,
    queueWaitTime: wasQueued ? queueWaitTime : undefined,
    timestamp: new Date().toISOString(),
  };

  // Emit real-time event
  io.emit("request:completed", {
    agent,
    query: query.substring(0, 100),
    cached: false,
    latency: totalLatency,
    success: agentResult.success,
    source: agentResult.source,
    rateLimited: wasQueued,
    queued: wasQueued,
    queueWaitTime: wasQueued ? queueWaitTime : undefined,
    timestamp: result.timestamp,
  });

  // Record stats
  if (statsCollector) {
    await statsCollector.recordRequest({
      agent,
      cached: false,
      latency: totalLatency,
      queued: wasQueued,
      rateLimited: wasQueued,
      success: agentResult.success,
    });
  }

  return res.json(result);
});

module.exports = router;
