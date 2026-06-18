export default {
  fetch: [
    [
      "GET /v1/search",
      {
        data: [
          { id: "1", owner: "nasiko", name: "openai-agent", version: "1.0.0", artifact_type: "agent", status: "stable", description: "Research agent using OpenAI Agents SDK with Wikipedia search", framework: "openai", tags: ["openai", "research", "a2a"], license: "MIT" },
          { id: "2", owner: "nasiko", name: "claude-sdk-agent", version: "1.0.0", artifact_type: "agent", status: "stable", description: "Research agent using Anthropic Claude SDK with streaming", framework: "claude-sdk", tags: ["claude", "anthropic", "streaming"], license: "MIT" },
          { id: "3", owner: "nasiko", name: "langgraph-agent", version: "1.0.0", artifact_type: "agent", status: "preview", description: "Currency conversion agent using LangGraph with streaming", framework: "langgraph", tags: ["langgraph", "streaming", "a2a"], license: "MIT" },
          { id: "4", owner: "nasiko", name: "web-search-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search the web using DuckDuckGo", framework: "openai", tags: ["web", "search", "duckduckgo"], license: "MIT" },
          { id: "5", owner: "nasiko", name: "wikipedia-search-claude-sdk", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search Wikipedia for a summary of any topic", framework: "claude-sdk", tags: ["wikipedia", "search", "research"], license: "MIT" },
          { id: "6", owner: "nasiko", name: "a2a-go-agent", version: "1.0.0", artifact_type: "agent", status: "preview", description: "Go A2A SDK reference agent", framework: "a2a-go", tags: ["go", "a2a"], license: "MIT" },
          { id: "7", owner: "nasiko", name: "http-request-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Make HTTP requests to any URL", framework: "openai", tags: ["http", "api", "rest"], license: "MIT" },
          { id: "8", owner: "community", name: "slack-notifier", version: "0.2.0", artifact_type: "tool", status: "preview", description: "Send notifications to Slack channels", framework: "openai", tags: ["slack", "notifications"], license: "Apache-2.0" },
        ],
        total: 8,
      },
    ],
  ],
  window: {
    fetchNavigation: async () => [
      { title: "Artifacts", url: "/index.html" },
      { title: "Skills", url: "/skills.html" },
      { title: "Publish", url: "/publish.html" },
    ],
    fetchArtifacts: async (query, page, limit) => ({
      data: [
        { id: "1", owner: "nasiko", name: "openai-agent", version: "1.0.0", artifact_type: "agent", status: "stable", description: "Research agent using OpenAI Agents SDK with Wikipedia search", framework: "openai", tags: ["openai", "research", "a2a"], license: "MIT" },
        { id: "2", owner: "nasiko", name: "claude-sdk-agent", version: "1.0.0", artifact_type: "agent", status: "stable", description: "Research agent using Anthropic Claude SDK with streaming", framework: "claude-sdk", tags: ["claude", "anthropic", "streaming"], license: "MIT" },
        { id: "3", owner: "nasiko", name: "langgraph-agent", version: "1.0.0", artifact_type: "agent", status: "preview", description: "Currency conversion agent using LangGraph with streaming", framework: "langgraph", tags: ["langgraph", "streaming", "a2a"], license: "MIT" },
        { id: "4", owner: "nasiko", name: "web-search-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search the web using DuckDuckGo", framework: "openai", tags: ["web", "search", "duckduckgo"], license: "MIT" },
        { id: "5", owner: "nasiko", name: "wikipedia-search-claude-sdk", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search Wikipedia for a summary of any topic", framework: "claude-sdk", tags: ["wikipedia", "search", "research"], license: "MIT" },
        { id: "6", owner: "nasiko", name: "a2a-go-agent", version: "1.0.0", artifact_type: "agent", status: "preview", description: "Go A2A SDK reference agent", framework: "a2a-go", tags: ["go", "a2a"], license: "MIT" },
        { id: "7", owner: "nasiko", name: "http-request-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Make HTTP requests to any URL", framework: "openai", tags: ["http", "api", "rest"], license: "MIT" },
        { id: "8", owner: "community", name: "slack-notifier", version: "0.2.0", artifact_type: "tool", status: "preview", description: "Send notifications to Slack channels", framework: "openai", tags: ["slack", "notifications"], license: "Apache-2.0" },
      ],
      total: 8,
    }),
  },
};
