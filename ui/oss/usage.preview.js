// Usage page fixtures
export default {
  window: {
    fetchUsageSummary: async () => ({
      total_requests: 1842,
      total_tokens_in: 482000,
      total_tokens_out: 215000,
      total_cost_usd: 12.47,
      active_agents: 3,
    }),
    fetchUsageHistory: async () => [
      { date: "2026-06-24", requests: 210, tokens_in: 58000, tokens_out: 28000, cost_usd: 1.52 },
      { date: "2026-06-25", requests: 245, requests: 245, tokens_in: 65000, tokens_out: 31000, cost_usd: 1.78 },
      { date: "2026-06-26", requests: 198, tokens_in: 52000, tokens_out: 24000, cost_usd: 1.35 },
      { date: "2026-06-27", requests: 312, tokens_in: 84000, tokens_out: 38000, cost_usd: 2.21 },
      { date: "2026-06-28", requests: 278, tokens_in: 73000, tokens_out: 34000, cost_usd: 1.95 },
      { date: "2026-06-29", requests: 290, tokens_in: 76000, tokens_out: 32000, cost_usd: 1.88 },
      { date: "2026-06-30", requests: 309, tokens_in: 74000, tokens_out: 28000, cost_usd: 1.78 },
    ],
    fetchUsageByAgent: async () => ({
      data: [
        { agent: "coding-agent", requests: 842, tokens_in: 220000, tokens_out: 98000, cost_usd: 5.62 },
        { agent: "research-agent", requests: 512, tokens_in: 145000, tokens_out: 62000, cost_usd: 3.55 },
        { agent: "qa-agent", requests: 310, tokens_in: 78000, tokens_out: 35000, cost_usd: 2.10 },
        { agent: "devops-agent", requests: 120, tokens_in: 28000, tokens_out: 14000, cost_usd: 0.82 },
        { agent: "docs-agent", requests: 58, tokens_in: 11000, tokens_out: 6000, cost_usd: 0.38 },
      ],
      total: 5,
    }),
  },
};
