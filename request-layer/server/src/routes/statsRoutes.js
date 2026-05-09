const express = require("express");
const router = express.Router();
const StatsCollector = require("../metrics/statsCollector");

/**
 * Stats and monitoring endpoints.
 */

// GET /api/stats — Global runtime statistics
router.get("/", async (req, res) => {
  const redis = req.app.get("redis");
  const stats = new StatsCollector(redis);
  const global = await stats.getGlobalStats();
  res.json(global);
});

// GET /api/stats/agents — All per-agent stats
router.get("/agents", async (req, res) => {
  const redis = req.app.get("redis");
  const stats = new StatsCollector(redis);
  const agentStats = await stats.getAllAgentStats();
  res.json(agentStats);
});

// GET /api/stats/history — Time-series data
router.get("/history", async (req, res) => {
  const redis = req.app.get("redis");
  const stats = new StatsCollector(redis);
  const duration = parseInt(req.query.duration || "3600000", 10);
  const series = await stats.getTimeSeries(duration);
  res.json({ points: series.length, series });
});

// GET /api/stats/feed — Recent request log
router.get("/feed", async (req, res) => {
  const redis = req.app.get("redis");
  const stats = new StatsCollector(redis);
  const count = parseInt(req.query.count || "50", 10);
  const recent = await stats.getRecentRequests(count);
  res.json({ count: recent.length, requests: recent });
});

// GET /api/stats/:agent — Per-agent stats
router.get("/:agent", async (req, res) => {
  const redis = req.app.get("redis");
  const stats = new StatsCollector(redis);
  const agentStats = await stats.getAgentStats(req.params.agent);
  res.json(agentStats);
});

// POST /api/stats/reset — Reset all stats
router.post("/reset", async (req, res) => {
  const redis = req.app.get("redis");
  const stats = new StatsCollector(redis);
  await stats.reset();
  res.json({ message: "All stats reset" });
});

module.exports = router;
