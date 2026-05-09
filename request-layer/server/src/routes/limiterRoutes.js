const express = require("express");
const router = express.Router();
const RateLimiter = require("../limiter/rateLimiter");

/**
 * Rate limit configuration and status endpoints.
 */

// GET /api/limits — Get all agent rate limit configs
router.get("/", async (req, res) => {
  const redis = req.app.get("redis");
  const rateLimiter = new RateLimiter(redis);

  const configs = await rateLimiter.getAllConfigs();
  const stats = await rateLimiter.getAllStats();

  res.json({ configs, stats });
});

// GET /api/limits/:agent — Get rate limit config for specific agent
router.get("/:agent", async (req, res) => {
  const redis = req.app.get("redis");
  const rateLimiter = new RateLimiter(redis);
  const { agent } = req.params;

  const config = await rateLimiter.getAgentConfig(agent);
  const status = await rateLimiter.getBucketStatus(agent);
  const stats = await rateLimiter.getStats(agent);

  res.json({ agent, config, bucketStatus: status, stats });
});

// PUT /api/limits/:agent — Update rate limit config for specific agent
router.put("/:agent", async (req, res) => {
  const redis = req.app.get("redis");
  const io = req.app.get("io");
  const rateLimiter = new RateLimiter(redis);
  const { agent } = req.params;
  const { maxTokens, refillRate, maxQueueSize, maxWaitTime } = req.body;

  // Validate input
  const updates = {};
  if (maxTokens !== undefined) {
    if (typeof maxTokens !== "number" || maxTokens < 1 || maxTokens > 1000) {
      return res.status(400).json({ error: "maxTokens must be between 1 and 1000" });
    }
    updates.maxTokens = maxTokens;
  }
  if (refillRate !== undefined) {
    if (typeof refillRate !== "number" || refillRate < 0.1 || refillRate > 100) {
      return res.status(400).json({ error: "refillRate must be between 0.1 and 100" });
    }
    updates.refillRate = refillRate;
  }
  if (maxQueueSize !== undefined) {
    if (typeof maxQueueSize !== "number" || maxQueueSize < 0 || maxQueueSize > 500) {
      return res.status(400).json({ error: "maxQueueSize must be between 0 and 500" });
    }
    updates.maxQueueSize = maxQueueSize;
  }
  if (maxWaitTime !== undefined) {
    if (typeof maxWaitTime !== "number" || maxWaitTime < 1000 || maxWaitTime > 120000) {
      return res.status(400).json({ error: "maxWaitTime must be between 1000 and 120000 ms" });
    }
    updates.maxWaitTime = maxWaitTime;
  }

  if (Object.keys(updates).length === 0) {
    return res.status(400).json({ error: "No valid fields to update" });
  }

  const updated = await rateLimiter.setAgentConfig(agent, updates);

  io.emit("limits:updated", { agent, config: updated, timestamp: new Date().toISOString() });

  res.json({ message: `Rate limit updated for ${agent}`, config: updated });
});

// GET /api/limits/:agent/status — Current bucket status
router.get("/:agent/status", async (req, res) => {
  const redis = req.app.get("redis");
  const rateLimiter = new RateLimiter(redis);
  const status = await rateLimiter.getBucketStatus(req.params.agent);
  res.json(status);
});

module.exports = router;
