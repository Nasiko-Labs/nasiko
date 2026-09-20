// Settings module fixtures and scenarios. One page, two views (settings —
// itself four sections — and secrets) switched by the nested sidebar, so this
// file has to stub every endpoint either of them calls.
//
// Settings field names mirror the server's `Settings` struct
// (oss/server/src/settings.rs) exactly — a fixture key the server doesn't have
// would make the preview look like it works while the real page saves nothing.
//
// GET /api/secrets answers the ApiResponse envelope the server returns
// (oss/server/src/secrets/routes.rs::list_secrets → { status_code, message,
// data: [SecretEntry] }). `?empty=1` on the page URL flips it to the empty
// state so both can be shot from one page.
//
// NOTE: fixture functions are serialized with toString() and eval'd inside the
// page, so they must be self-contained — no references to module scope.
const listSecrets = () => ({
  status_code: 200,
  message: "Secrets retrieved successfully",
  data: location.search.includes("empty") ? [] : [
    { id: "s-1", name: "ANTHROPIC_API_KEY", created_at: "2026-06-22T12:00:00Z", updated_at: "2026-08-06T12:00:00Z" },
    { id: "s-2", name: "DATABASE_URL", created_at: "2026-06-18T16:00:00Z", updated_at: "2026-07-30T16:00:00Z" },
    { id: "s-3", name: "GITHUB_TOKEN", created_at: "2026-06-15T08:00:00Z", updated_at: "2026-08-01T08:00:00Z" },
    { id: "s-4", name: "OPENAI_API_KEY", created_at: "2026-06-20T10:00:00Z", updated_at: "2026-08-07T10:00:00Z" },
  ],
});

// Scenario helper: land on the page the way a shared link does. The query is
// the whole point of most of these, so it replaces (not appends to) the
// existing one.
const gotoView = async (page, query) => {
  const url = new URL(page.url());
  url.search = query;
  await page.goto(url.toString(), { waitUntil: "networkidle" });
};

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
    // secrets view.
    ["GET /api/secrets", listSecrets],
    ["POST /api/secrets", { status_code: 201, message: "Secret created successfully", data: null }],
    [{ method: "DELETE", path: /^\/api\/secrets\/[^/]+$/ },
      { status_code: 200, message: "Secret deleted successfully", data: null }],
  ],
  scenarios: {
    // ── The module's views ──────────────────────────────────────────────────
    // Opened the way a shared link does, which also proves the shell honours
    // the param on load rather than only on a nav click.
    "view-secrets": async (page) => {
      await gotoView(page, "?view=secrets");
      await page.waitForSelector("secrets-page .page-head");
      await page.waitForSelector("secrets-manager .sm-row", { timeout: 5000 });
      await page.waitForTimeout(300);
    },
    // The second level of the same param: `limits` is a section *inside* the
    // settings view, not a view key. The shell must fall back to its default
    // view (this page) rather than showing nothing, and settings-page must open
    // on that section with the sidebar row highlighted to match.
    "view-limits-deep-link": async (page) => {
      await gotoView(page, "?view=limits");
      await page.waitForSelector('[data-panel="limits"].is-active');
      await page.waitForSelector('app-module-nav [data-section="limits"].is-active');
      await page.waitForTimeout(300);
    },
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
    // Both levels driven by clicks, in the order that used to be impossible:
    // a section row → a sibling view → back to a *different* section row. The
    // sidebar must not change between the three, and the settings view has to
    // come back showing Flow limits.
    "view-switch-click": async (page) => {
      await page.waitForSelector('[data-panel="general"].is-active');
      await page.click('app-module-nav [data-section="secrets"]');
      await page.waitForSelector("secrets-manager .sm-row", { timeout: 5000 });
      await page.click('app-module-nav [data-section="limits"]');
      await page.waitForSelector('[data-panel="limits"].is-active');
      await page.waitForTimeout(300);
    },
    // ── The secrets view's own states ───────────────────────────────────────
    // No secrets stored yet — the inline add row is the only affordance.
    "secrets-empty": async (page) => {
      await gotoView(page, "?view=secrets&empty=1");
      await page.waitForSelector("secrets-manager .sm-empty", { timeout: 5000 });
    },
    // Row-level delete uses an inline confirm, never window.confirm().
    "secrets-delete-confirm": async (page) => {
      await gotoView(page, "?view=secrets");
      await page.waitForSelector("secrets-manager .sm-row", { timeout: 5000 });
      await page.click("secrets-manager [data-delete]");
      await page.waitForSelector("secrets-manager .sm-row.is-confirming", { timeout: 5000 });
      await page.mouse.move(0, 0); // park the cursor so no button shows a hover state
    },
  },
};
