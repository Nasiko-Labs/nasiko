const express = require("express");
const router = express.Router();
const CacheManager = require("../cache/cacheManager");

/**
 * Cache management endpoints.
 */

// GET /api/cache/stats — Cache hit/miss statistics
router.get("/stats", async (req, res) => {
  const redis = req.app.get("redis");
  const cacheManager = new CacheManager(redis);
  const stats = await cacheManager.getStats();
  res.json(stats);
});

// GET /api/cache/entries — List all cached entries (metadata only)
router.get("/entries", async (req, res) => {
  const redis = req.app.get("redis");
  const cacheManager = new CacheManager(redis);
  const entries = await cacheManager.listEntries();
  res.json({ count: entries.length, entries });
});

// DELETE /api/cache/flush — Flush entire cache
router.delete("/flush", async (req, res) => {
  const redis = req.app.get("redis");
  const io = req.app.get("io");
  const cacheManager = new CacheManager(redis);
  const count = await cacheManager.flush();

  io.emit("cache:flushed", { count, timestamp: new Date().toISOString() });

  res.json({ message: "Cache flushed", entriesRemoved: count });
});

// DELETE /api/cache/agent/:name — Invalidate cache for specific agent
router.delete("/agent/:name", async (req, res) => {
  const redis = req.app.get("redis");
  const io = req.app.get("io");
  const cacheManager = new CacheManager(redis);
  const count = await cacheManager.invalidateAgent(req.params.name);

  io.emit("cache:invalidated", {
    agent: req.params.name,
    count,
    timestamp: new Date().toISOString(),
  });

  res.json({
    message: `Cache invalidated for agent: ${req.params.name}`,
    entriesRemoved: count,
  });
});

// POST /api/cache/reset-stats — Reset cache statistics
router.post("/reset-stats", async (req, res) => {
  const redis = req.app.get("redis");
  const cacheManager = new CacheManager(redis);
  await cacheManager.resetStats();
  res.json({ message: "Cache stats reset" });
});

module.exports = router;
