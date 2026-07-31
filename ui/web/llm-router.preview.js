// LLM router fixtures — /api/llm-configs, /api/llm-router/providers, /api/secrets
// (shapes from oss/server/src/llm_configs.rs + llm_router/providers.rs; see /api/docs).

const providers = [
  { provider: "anthropic", models: ["claude-opus-4-1", "claude-sonnet-4-5", "claude-haiku-4-5", "claude-sonnet-4-6", "claude-3-7-sonnet", "claude-3-5-haiku", "claude-3-opus", "claude-3-sonnet"] },
  { provider: "deepseek", models: ["deepseek-chat", "deepseek-reasoner", "deepseek-coder", "deepseek-v3"] },
  { provider: "gemini", models: ["gemini-2.5-pro", "gemini-2.5-flash", "gemini-2.0-flash", "gemini-1.5-pro"] },
  { provider: "groq", models: ["llama-3.3-70b", "llama-3.1-8b", "mixtral-8x7b"] },
  { provider: "openai", models: ["gpt-5.2", "gpt-5-mini", "gpt-4o", "gpt-4o-mini", "gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano", "o3", "o3-mini", "o4-mini", "gpt-4-turbo", "gpt-3.5-turbo", "text-embedding-3-small", "text-embedding-3-large"] },
].map(({ provider, models }) => ({
  provider,
  models: models.map((model, i) => ({
    model,
    input_price_per_1m: 1 + i * 0.5,
    output_price_per_1m: 3 + i * 1.5,
    cache_creation_price_per_1m: null,
    cache_read_price_per_1m: null,
    currency: "USD",
    notes: null,
    effective_from: "2026-01-01T00:00:00Z",
    effective_until: null,
  })),
}));

export default {
  fetch: [
    ["GET /api/llm-configs", {
      data: [],
      status_code: 200,
      message: "LLM configs retrieved successfully",
    }],
    ["GET /api/llm-router/providers", {
      data: providers,
      status_code: 200,
      message: "Providers retrieved successfully",
    }],
    ["GET /api/secrets", [
      { id: "s-001", name: "OPENAI_API_KEY", created_at: "2026-06-02T10:00:00Z", updated_at: "2026-06-02T10:00:00Z" },
      { id: "s-002", name: "ANTHROPIC_API_KEY", created_at: "2026-06-14T10:00:00Z", updated_at: "2026-07-01T10:00:00Z" },
      { id: "s-003", name: "GEMINI_API_KEY", created_at: "2026-07-10T10:00:00Z", updated_at: "2026-07-10T10:00:00Z" },
    ]],
    ["POST /api/llm-configs", {
      data: { id: "cfg-new" },
      status_code: 201,
      message: "LLM config created successfully",
    }],
  ],
  scenarios: {
    "configure-form": async (page) => {
      await page.waitForSelector('[data-action="new-config"]');
      await page.click('.provider-card[data-provider="openai"]');
      await page.waitForSelector("#config-form");
    },
    "configure-new-secret": async (page) => {
      await page.waitForSelector('[data-action="new-config"]');
      await page.click('.provider-card[data-provider="anthropic"]');
      await page.waitForSelector("#config-form");
      await page.click("#secret-new");
      await page.waitForTimeout(200);
    },
  },
};
