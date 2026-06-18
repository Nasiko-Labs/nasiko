export default {
  fetch: [
    [
      "GET /v1/search",
      {
        data: [
          { id: "1", owner: "nasiko", name: "web-search-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search the web using DuckDuckGo. No API key required.", framework: "openai", tags: ["web", "search", "duckduckgo"], license: "MIT" },
          { id: "2", owner: "nasiko", name: "web-search-claude-sdk", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search the web using DuckDuckGo. No API key required.", framework: "claude-sdk", tags: ["web", "search", "duckduckgo"], license: "MIT" },
          { id: "3", owner: "nasiko", name: "wikipedia-search-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search Wikipedia for a summary of any topic or concept", framework: "openai", tags: ["wikipedia", "search", "research"], license: "MIT" },
          { id: "4", owner: "nasiko", name: "wikipedia-search-claude-sdk", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search Wikipedia for a summary of any topic or concept", framework: "claude-sdk", tags: ["wikipedia", "search", "research"], license: "MIT" },
          { id: "5", owner: "nasiko", name: "http-request-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Make HTTP requests (GET, POST, PUT, DELETE) to any URL", framework: "openai", tags: ["http", "api", "rest"], license: "MIT" },
          { id: "6", owner: "nasiko", name: "arxiv-search-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search arxiv.org for academic research papers", framework: "openai", tags: ["arxiv", "research", "academic"], license: "MIT" },
          { id: "7", owner: "nasiko", name: "tmdb-search-openai", version: "1.0.0", artifact_type: "skill", status: "preview", description: "Search movies and TV shows via TMDB API", framework: "openai", tags: ["tmdb", "movies", "entertainment"], license: "MIT" },
        ],
        total: 7,
      },
    ],
  ],
  window: {
    fetchNavigation: async () => [
      { title: "Artifacts", url: "/index.html" },
      { title: "Skills", url: "/skills.html" },
      { title: "Publish", url: "/publish.html" },
    ],
    fetchSkillArtifacts: async (query, page, limit) => ({
      data: [
        { id: "1", owner: "nasiko", name: "web-search-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search the web using DuckDuckGo. No API key required.", framework: "openai", tags: ["web", "search", "duckduckgo"], license: "MIT" },
        { id: "2", owner: "nasiko", name: "web-search-claude-sdk", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search the web using DuckDuckGo. No API key required.", framework: "claude-sdk", tags: ["web", "search", "duckduckgo"], license: "MIT" },
        { id: "3", owner: "nasiko", name: "wikipedia-search-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search Wikipedia for a summary of any topic or concept", framework: "openai", tags: ["wikipedia", "search", "research"], license: "MIT" },
        { id: "4", owner: "nasiko", name: "wikipedia-search-claude-sdk", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search Wikipedia for a summary of any topic or concept", framework: "claude-sdk", tags: ["wikipedia", "search", "research"], license: "MIT" },
        { id: "5", owner: "nasiko", name: "http-request-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Make HTTP requests (GET, POST, PUT, DELETE) to any URL", framework: "openai", tags: ["http", "api", "rest"], license: "MIT" },
        { id: "6", owner: "nasiko", name: "arxiv-search-openai", version: "1.0.0", artifact_type: "skill", status: "stable", description: "Search arxiv.org for academic research papers", framework: "openai", tags: ["arxiv", "research", "academic"], license: "MIT" },
        { id: "7", owner: "nasiko", name: "tmdb-search-openai", version: "1.0.0", artifact_type: "skill", status: "preview", description: "Search movies and TV shows via TMDB API", framework: "openai", tags: ["tmdb", "movies", "entertainment"], license: "MIT" },
      ],
      total: 7,
    }),
  },
};
