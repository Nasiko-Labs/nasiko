const express = require("express");
const router = express.Router();

/**
 * Queue status endpoints.
 * The QueueManager instance is set on `app` during startup.
 */

// GET /api/queue/status — Queue depths for all agents
router.get("/status", (req, res) => {
  const queueManager = req.app.get("queueManager");
  if (!queueManager) {
    return res.json({ queues: {} });
  }
  res.json({ queues: queueManager.getStatus() });
});

// GET /api/queue/:agent — Queue details for specific agent
router.get("/:agent", async (req, res) => {
  const queueManager = req.app.get("queueManager");
  if (!queueManager) {
    return res.json({ waiting: 0, processed: 0, avgWaitTime: 0 });
  }

  const status = queueManager.getAgentStatus(req.params.agent);
  const events = await queueManager.getAgentEvents(req.params.agent);

  res.json({ agent: req.params.agent, ...status, events });
});

module.exports = router;
