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
    // Sections switch via the module tree nav (app-module-nav section rows).
    "models": async (page) => {
      await page.click('app-module-nav [data-section="models"]');
      await page.waitForSelector('[data-panel="models"].is-active');
      await page.waitForTimeout(300);
    },
    "registry": async (page) => {
      await page.click('app-module-nav [data-section="registry"]');
      await page.waitForSelector('[data-panel="registry"].is-active');
      await page.waitForTimeout(300);
    },
  },
};
