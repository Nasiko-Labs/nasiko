const axios = require("axios");
const config = require("../config");
const { isMockAgent, processMockAgent } = require("../mock/mockAgents");

/**
 * AgentProxy — Forwards requests to real Nasiko agents (via Kong) or mock agents.
 */
class AgentProxy {
  constructor() {
    this.kongUrl = config.kongGatewayUrl;
    this.timeout = 60000; // 60s timeout for agent requests
  }

  /**
   * Forward a request to the appropriate agent.
   * @param {string} agentName - Target agent name
   * @param {string} query - User query
   * @param {object} [options={}] - Additional options (token, params)
   * @returns {object} Agent response with timing metadata
   */
  async forward(agentName, query, options = {}) {
    const startTime = Date.now();

    try {
      let response;

      if (isMockAgent(agentName)) {
        // Route to built-in mock agent
        response = await processMockAgent(agentName, query);
      } else {
        // Route to real Nasiko agent via Kong Gateway
        response = await this._forwardToKong(agentName, query, options);
      }

      const latency = Date.now() - startTime;

      return {
        success: true,
        agent: agentName,
        response,
        latency,
        source: isMockAgent(agentName) ? "mock" : "kong",
        timestamp: new Date().toISOString(),
      };
    } catch (err) {
      const latency = Date.now() - startTime;
      return {
        success: false,
        agent: agentName,
        error: err.message,
        latency,
        source: isMockAgent(agentName) ? "mock" : "kong",
        timestamp: new Date().toISOString(),
      };
    }
  }

  /**
   * Forward request to a real Nasiko agent via Kong Gateway.
   */
  async _forwardToKong(agentName, query, options = {}) {
    const url = `${this.kongUrl}/agents/${agentName}/`;

    const headers = { "Content-Type": "application/json" };
    if (options.token) {
      headers["Authorization"] = `Bearer ${options.token}`;
    }

    const payload = {
      jsonrpc: "2.0",
      method: "message/send",
      id: Date.now().toString(),
      params: {
        message: {
          role: "user",
          parts: [{ kind: "text", text: query }],
        },
      },
    };

    const res = await axios.post(url, payload, {
      headers,
      timeout: this.timeout,
    });

    return res.data;
  }

  /**
   * Check health of a specific agent.
   */
  async checkHealth(agentName) {
    if (isMockAgent(agentName)) {
      return { agent: agentName, status: "healthy", type: "mock" };
    }

    try {
      const url = `${this.kongUrl}/agents/${agentName}/health`;
      const res = await axios.get(url, { timeout: 5000 });
      return {
        agent: agentName,
        status: res.status === 200 ? "healthy" : "unhealthy",
        type: "real",
        statusCode: res.status,
      };
    } catch (err) {
      return {
        agent: agentName,
        status: "unhealthy",
        type: "real",
        error: err.message,
      };
    }
  }
}

module.exports = AgentProxy;
