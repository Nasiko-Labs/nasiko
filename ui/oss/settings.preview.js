// Settings page fixtures
export default {
  fetch: [
    ["GET /api/settings", {
      instance_name: "nasiko-dev",
      default_model: "claude-sonnet-4-6",
      max_tokens: 8192,
      registry_url: "https://registry.nasiko.dev",
    }],
    ["PUT /api/settings", { ok: true }],
  ],
};
