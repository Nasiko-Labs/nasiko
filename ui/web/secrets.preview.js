// Secrets page fixtures.
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

const gotoPage = async (page, query) => {
  const url = new URL(page.url());
  url.search = query;
  await page.goto(url.toString(), { waitUntil: "networkidle" });
};

export default {
  fetch: [
    ["GET /api/secrets", listSecrets],
    ["POST /api/secrets", { status_code: 201, message: "Secret created successfully", data: null }],
    [{ method: "DELETE", path: /^\/api\/secrets\/[^/]+$/ },
      { status_code: 200, message: "Secret deleted successfully", data: null }],
  ],
  scenarios: {
    // No secrets stored yet — the inline add row is the only affordance.
    empty: async (page) => {
      await gotoPage(page, "?empty=1");
      await page.waitForSelector("secrets-manager .sm-empty", { timeout: 5000 });
    },
    // Row-level delete uses an inline confirm, never window.confirm().
    "delete-confirm": async (page) => {
      await gotoPage(page, "");
      await page.waitForSelector("secrets-manager .sm-row", { timeout: 5000 });
      await page.click("secrets-manager [data-delete]");
      await page.waitForSelector("secrets-manager .sm-row.is-confirming", { timeout: 5000 });
      await page.mouse.move(0, 0); // park the cursor so no button shows a hover state
    },
  },
};
