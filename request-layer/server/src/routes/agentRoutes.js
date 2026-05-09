const express = require("express");
const router = express.Router();
const axios = require("axios");
const config = require("../config");
const AgentProxy = require("../proxy/agentProxy");
const { listMockAgents, isMockAgent } = require("../mock/mockAgents");

const agentProxy = new AgentProxy();

/**
 * Agent listing, health check, and discovery endpoints.
 */

// GET /api/agents — List all known agents (mock + discovered real agents)
router.get("/", async (req, res) => {
  const agents = [];

  // Add mock agents
  agents.push(...listMockAgents());

  // Try to discover real agents from Nasiko backend
  try {
    const response = await axios.get(`${config.nasikoBackendUrl}/registries`, {
      timeout: 5000,
    });

    if (response.data && Array.isArray(response.data)) {
      for (const agent of response.data) {
        agents.push({
          name: agent.name || agent.agent_name,
          description: agent.description || "",
          capabilities: agent.capabilities || [],
          type: "real",
          status: agent.status || "unknown",
          url: agent.url || agent.agent_url || null,
        });
      }
    }
  } catch (err) {
    // Real agents unavailable — that's fine, we still have mock agents
    console.log("Could not fetch real agents from Nasiko backend:", err.message);
  }

  res.json({
    total: agents.length,
    mockAgents: agents.filter((a) => a.type === "mock").length,
    realAgents: agents.filter((a) => a.type === "real").length,
    agents,
  });
});

// GET /api/agents/:name/health — Check health of a specific agent
router.get("/:name/health", async (req, res) => {
  const { name } = req.params;
  const health = await agentProxy.checkHealth(name);
  const statusCode = health.status === "healthy" ? 200 : 503;
  res.status(statusCode).json(health);
});

// POST /api/agents/health-all — Check health of all known agents
router.post("/health-all", async (req, res) => {
  const results = [];

  // Check mock agents
  const mockAgents = listMockAgents();
  for (const agent of mockAgents) {
    results.push({
      name: agent.name,
      type: "mock",
      status: "healthy",
    });
  }

  // Check real agents if any names provided
  const { agents: agentNames = [] } = req.body;
  for (const name of agentNames) {
    if (!isMockAgent(name)) {
      const health = await agentProxy.checkHealth(name);
      results.push({
        name,
        type: "real",
        status: health.status,
        error: health.error || undefined,
      });
    }
  }

  const healthy = results.filter((r) => r.status === "healthy").length;
  const unhealthy = results.filter((r) => r.status !== "healthy").length;

  res.json({ total: results.length, healthy, unhealthy, agents: results });
});

module.exports = router;
