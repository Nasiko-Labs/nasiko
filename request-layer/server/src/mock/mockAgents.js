/**
 * MockAgents — Built-in simulated agents for demo/testing.
 *
 * Each mock agent simulates realistic processing delays and returns
 * structured responses. Used when agent name starts with "mock-".
 */
const mockAgents = {
  "mock-translator": {
    name: "mock-translator",
    description: "Multi-language translation agent",
    capabilities: ["translation", "language-detection"],
    handler: async (query) => {
      // Simulate processing delay (800ms - 2s)
      const delay = 800 + Math.random() * 1200;
      await sleep(delay);

      return {
        agent: "mock-translator",
        result: `[Translated] ${query}`,
        language_detected: "English",
        target_language: "French",
        confidence: (0.85 + Math.random() * 0.15).toFixed(3),
        processing_time_ms: Math.round(delay),
      };
    },
  },

  "mock-summarizer": {
    name: "mock-summarizer",
    description: "Text summarization agent",
    capabilities: ["summarization", "key-extraction"],
    handler: async (query) => {
      // Simulate heavier processing (1s - 3s)
      const delay = 1000 + Math.random() * 2000;
      await sleep(delay);

      const words = query.split(" ");
      const summary = words.slice(0, Math.max(5, Math.floor(words.length / 3))).join(" ") + "...";

      return {
        agent: "mock-summarizer",
        result: summary,
        original_length: query.length,
        summary_length: summary.length,
        compression_ratio: (summary.length / query.length).toFixed(2),
        processing_time_ms: Math.round(delay),
      };
    },
  },

  "mock-analyzer": {
    name: "mock-analyzer",
    description: "Sentiment and content analysis agent",
    capabilities: ["sentiment-analysis", "entity-extraction"],
    handler: async (query) => {
      // Simulate fast processing (500ms - 1.5s)
      const delay = 500 + Math.random() * 1000;
      await sleep(delay);

      const sentiments = ["positive", "neutral", "negative"];
      const sentiment = sentiments[Math.floor(Math.random() * sentiments.length)];

      return {
        agent: "mock-analyzer",
        result: `Analysis complete for input text.`,
        sentiment,
        confidence: (0.7 + Math.random() * 0.3).toFixed(3),
        word_count: query.split(" ").length,
        entities: ["text", "analysis"],
        processing_time_ms: Math.round(delay),
      };
    },
  },
};

/**
 * Check if an agent name is a mock agent.
 */
function isMockAgent(agentName) {
  return agentName in mockAgents;
}

/**
 * Get all mock agent definitions (for listing).
 */
function listMockAgents() {
  return Object.values(mockAgents).map((a) => ({
    name: a.name,
    description: a.description,
    capabilities: a.capabilities,
    type: "mock",
    status: "active",
  }));
}

/**
 * Process a query through a mock agent.
 */
async function processMockAgent(agentName, query) {
  const agent = mockAgents[agentName];
  if (!agent) {
    throw new Error(`Unknown mock agent: ${agentName}`);
  }
  return agent.handler(query);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

module.exports = { isMockAgent, listMockAgents, processMockAgent };
