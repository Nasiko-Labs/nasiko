// Settings page fixtures.
// Field names mirror the server's `Settings` struct (oss/server/src/settings.rs)
// exactly — a fixture key the server doesn't have would make the preview look
// like it works while the real page saves nothing.
export default {
  fetch: [
    ["GET /api/settings", {
      router_model: "gpt-4o",
      default_provider: "openai",
      max_flow_depth: 5,
      max_flow_fan_out: 20,
      max_flow_tokens: 100000,
      flow_timeout_secs: 120,
      registry_url: "https://registry.nasiko.dev",
      oidc_issuer_url: "https://login.microsoftonline.com/contoso/v2.0",
      oidc_client_id: "8f14e45f-ceea-467a-9c1f-3a2b7d9e0011",
      oidc_redirect_uri: "https://cp.nasiko.dev/auth/callback",
      oidc_scopes: "openid profile email",
      oidc_provider_label: "Microsoft",
      oidc_client_secret_configured: true,
      catalog_tabs: "devops, finance, support",
    }],
    ["PUT /api/settings", { ok: true }],
  ],
  scenarios: {
    // Sections switch via the module tree nav (app-module-nav section rows).
    "limits": async (page) => {
      await page.click('app-module-nav [data-section="limits"]');
      await page.waitForSelector('[data-panel="limits"].is-active');
      await page.waitForTimeout(300);
    },
    "registry": async (page) => {
      await page.click('app-module-nav [data-section="registry"]');
      await page.waitForSelector('[data-panel="registry"].is-active');
      await page.waitForTimeout(300);
    },
    "sso": async (page) => {
      await page.click('app-module-nav [data-section="sso"]');
      await page.waitForSelector('[data-panel="sso"].is-active');
      await page.waitForTimeout(300);
    },
  },
};
