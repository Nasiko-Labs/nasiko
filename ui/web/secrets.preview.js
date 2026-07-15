// Secrets page fixtures
export default {
  window: {
    fetchSecrets: async () => ({
      data: [
        { key: "OPENAI_API_KEY", created_at: "2026-06-20T10:00:00Z" },
        { key: "GITHUB_TOKEN", created_at: "2026-06-15T08:00:00Z" },
        { key: "ANTHROPIC_API_KEY", created_at: "2026-06-22T12:00:00Z" },
        { key: "DATABASE_URL", created_at: "2026-06-18T16:00:00Z" },
      ],
      total: 4,
    }),
  },
};
