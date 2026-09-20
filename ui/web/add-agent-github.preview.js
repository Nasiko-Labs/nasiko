// Import from GitHub — repo list fixture (GET /api/auth/github/repos).
export default {
  fetch: [
    ["GET /api/auth/github/repos", [
      { full_name: "nasiko/coding-agent", description: "General-purpose coding assistant", language: "Python", updated_at: "2026-07-30T10:00:00Z", private: false },
      { full_name: "nasiko/research-agent", description: "Research and summarization agent", language: "TypeScript", updated_at: "2026-07-22T10:00:00Z", private: true },
      { full_name: "nasiko/docs-agent", description: "Documentation generator", language: "Rust", updated_at: "2026-06-14T10:00:00Z", private: false },
      { full_name: "acme/support-bot", description: "Customer support triage", language: "Python", updated_at: "2026-05-02T10:00:00Z", private: true },
    ]],
  ],
};
