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
  scenarios: {
    "models": async (page) => {
      await page.click('[data-tab="models"]');
      await page.waitForSelector('[data-panel="models"].is-active');
      await page.waitForSelector('.side-nav-item.is-active[data-tab="models"]');
    },
    "registry": async (page) => {
      await page.click('[data-tab="registry"]');
      await page.waitForSelector('[data-panel="registry"].is-active');
    },
  },
};
