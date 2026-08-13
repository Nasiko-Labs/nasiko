// Agents module fixtures and scenarios. One page, four views (hub, your-agents,
// import, builds) switched by the nested sidebar — so this file has to stub
// every endpoint any of them calls, and each view gets a `view-*` scenario.
export default {
  fetch: [
    // your-agents view: upload provenance per card, and the deploy dialog's
    // secret picker.
    ["GET /api/agents/my-uploads", { data: [
      { agent_name: "research-agent", upload_info: { upload_type: "github", status_message: null } },
      { agent_name: "docs-agent", upload_info: { upload_type: "zip", status_message: "Building and deploying..." } },
    ] }],
    ["GET /api/secrets", [
      { name: "OPENAI_API_KEY" },
      { name: "GITHUB_TOKEN" },
      { name: "DATABASE_URL" },
    ]],
    // import view: GitHub connect state for the two import routes.
    ["GET /api/github/status", { connected: false }],
    [{ method: "GET", path: /^\/api\/agents/ }, {
      data: [
        { id: "a-001", version: "1.2.0", name: "coding-agent", display_name: "Coding Agent", image: "nasiko/coding:latest", status: "running", replicas: 1, description: "General-purpose coding assistant for any language. Supports code generation, review, debugging, and refactoring.", tags: ["devops", "code", "debugging"] },
        { id: "a-002", version: "1.1.0", name: "research-agent", display_name: "Research Agent", image: "nasiko/research:latest", status: "running", replicas: 2, description: "Research and summarization across documents and the web", tags: ["hr", "research", "analysis"] },
        { id: "a-003", version: "0.3.1", name: "devops-agent", display_name: "DevOps Agent", image: "nasiko/devops:0.3.1", status: "stopped", replicas: 0, description: "Infrastructure automation, CI/CD pipelines, and deployments", tags: ["devops", "infrastructure", "kubernetes"] },
        { id: "a-004", version: "2.0.1", name: "qa-agent", display_name: "QA Agent", image: "nasiko/qa:latest", status: "running", replicas: 1, description: "Test generation, quality checks, and coverage analysis", tags: ["devops", "testing", "quality"] },
        { id: "a-005", version: "0.2.0", name: "docs-agent", display_name: "Docs Agent", image: "nasiko/docs:0.2.0", status: "error", replicas: 0, description: "Documentation generation from source code and specs", tags: ["legal", "documentation", "writing"] },
        { id: "a-006", version: "1.0.0", name: "finance-bot", display_name: "Finance Bot", image: "nasiko/finance:1.0.0", status: "running", replicas: 1, description: "Expense tracking, invoice processing, budget analysis", tags: ["finance", "accounting", "reports"] },
      ],
      total: 6,
    }],
    ["GET /api/settings", { catalog_tabs: null }],
    [{ method: "GET", path: /^\/api\/mcp\/connectors/ }, { data: { created_by_you: [
      { id: "c1", name: "github", display_name: "GitHub", url: "https://api.githubcopilot.com/mcp" },
      { id: "c2", name: "slack", display_name: "Slack", url: "https://mcp.slack.example.com/mcp" },
    ], shared_with_you: [], total: 2 } }],
    [{ method: "GET", path: /^\/api\/chat\/sessions/ }, { data: [
      { session_id: "cs-1", agent_id: "a-001", agent_name: "Coding Agent", last_message: "Fixed the DNS resolution issue in container networking", message_count: 12, updated_at: "2026-07-03T20:40:00Z" },
      { session_id: "cs-2", agent_id: "a-002", agent_name: "Research Agent", last_message: "Here's the summary of Kubernetes operator patterns", message_count: 8, updated_at: "2026-07-03T16:15:00Z" },
    ], total: 2 }],
    // ⌘F palette sources — MAF list envelope is {data:{data:[...],total}}.
    [{ method: "GET", path: /^\/api\/maf\/workflows/ }, { data: { data: [
      { id: "wf-101", name: "Agent onboarding pipeline", description: "Provision, smoke-test, and register newly deployed agents.", status: "active", execution_count: 12, created_at: "2026-07-01T09:00:00Z", updated_at: "2026-08-01T09:00:00Z" },
      { id: "wf-102", name: "Social media content pipeline", description: "Generate, review, and publish approved posts every weekday.", status: "active", execution_count: 126, created_at: "2026-06-10T09:00:00Z", updated_at: "2026-08-05T09:00:00Z" },
    ], total: 2 }, status_code: 200, message: "ok" }],
    [{ method: "GET", path: /^\/api\/maf\/executions/ }, { data: { data: [
      { id: "ex-90", execution_number: 90, maf_id: "wf-101", status: "success", workflow_name: "Agent onboarding pipeline", workflow_status: "active", created_at: "2026-08-07T10:00:00Z" },
      { id: "ex-89", execution_number: 89, maf_id: "wf-101", status: "failed", workflow_name: "Agent onboarding pipeline", workflow_status: "active", created_at: "2026-08-06T18:30:00Z" },
    ], total: 2 }, status_code: 200, message: "ok" }],
    [{ method: "GET", path: /^\/api\/mcp\/composio\/toolkits/ }, { data: { toolkits: [
      { connector_id: "tk-001", name: "github", display_name: "GitHub", description: "Repository automation for coding agents.", auth_flow: "oauth", tool_count: 24, is_connected: true, logo_url: null },
      { connector_id: "tk-002", name: "gmail", display_name: "Gmail", description: "Send and search email from agent workflows.", auth_flow: "oauth", tool_count: 18, is_connected: false, logo_url: null },
    ] } }],
    [{ method: "GET", path: /^\/api\/builds/ }, { data: [
      { id: "b-a1b2c3d4-e5f6-7890-abcd-ef1234567890", agent_id: "a-001", version_tag: "1.2.0", image_reference: "nasiko/coding-agent:1.2.0", status: "success", created_at: "2026-08-05T14:00:00Z" },
      { id: "b-f1e2d3c4-b5a6-7890-dcba-fe0987654321", agent_id: "a-002", version_tag: "0.4.0", image_reference: "nasiko/research-agent:0.4.0", status: "building", created_at: "2026-08-07T15:10:00Z" },
    ], total: 2 }],
    // EE-only user search (feature-detected; OSS deployments 404 here).
    [{ method: "GET", path: /^\/api\/search\/users/ }, { data: [
      { id: "u-1", username: "agent-ops", display_name: "Agent Ops", email: "agent-ops@nasiko.dev", role: "team_lead" },
    ] }],
  ],
  scenarios: {
    // ── The module's views ──────────────────────────────────────────────────
    // Each opens the way a shared link does (`?view=`), which also proves the
    // shell honours the param on load rather than only on a nav click.
    "view-your-agents": async (page) => {
      await page.goto(`${page.url().split("?")[0]}?view=your-agents`);
      await page.waitForSelector(".agent-card-name");
      await page.waitForTimeout(300);
    },
    "view-import": async (page) => {
      await page.goto(`${page.url().split("?")[0]}?view=import`);
      await page.waitForSelector("add-agent-page .page-icon");
      await page.waitForTimeout(300);
    },
    "view-builds": async (page) => {
      await page.goto(`${page.url().split("?")[0]}?view=builds`);
      await page.waitForSelector("builds-page .page-head");
      await page.waitForTimeout(400);
    },
    // Switching by clicking the nested sidebar: the sidebar itself must not
    // change between this and the default capture.
    "view-switch-click": async (page) => {
      await page.waitForSelector(".card-name");
      await page.click('app-module-nav [data-section="builds"]');
      await page.waitForSelector("builds-page .page-head");
      await page.waitForTimeout(400);
    },
    // ⌘F global palette anchored under the topbar search field: one query
    // that hits Agents, Workflows, Executions, Chats, Toolkits, Builds,
    // Users, and Pages at once.
    "search-palette": async (page) => {
      await page.waitForSelector(".card-name");
      await page.evaluate(() => document.querySelector("app-nav-search")?.open());
      await page.waitForSelector("[data-nav-dialog][open]");
      await page.fill("[data-nav-input]", "agent");
      await page.waitForTimeout(600);
    },
    // Same state scrolled to the bottom sections (Toolkits/Builds/Users/Pages).
    "search-palette-scrolled": async (page) => {
      await page.waitForSelector(".card-name");
      await page.evaluate(() => document.querySelector("app-nav-search")?.open());
      await page.waitForSelector("[data-nav-dialog][open]");
      await page.fill("[data-nav-input]", "agent");
      await page.waitForTimeout(600);
      await page.evaluate(() => {
        const list = document.querySelector("[data-nav-results]");
        list.scrollTop = list.scrollHeight;
      });
      await page.waitForTimeout(200);
    },

    "mobile-menu": async (page) => { await page.evaluate(() => document.querySelector("[data-mobile-menu]")?.click()); await new Promise(r => setTimeout(r, 400)); },
    // ⌘F global palette: grouped Pages + Agents results (connector/chat
    // sources aren't stubbed on this page and drop out gracefully).
    "search-open": async (page) => {
      await page.waitForSelector(".card-name");
      await page.click("[data-search-trigger]");
      await page.waitForSelector("[data-nav-dialog][open]");
      await page.fill("[data-nav-input]", "agent");
      await page.waitForTimeout(400);
    },
    // Avatar menu with the Light/Dark/System theme switch (System default).
    "user-menu": async (page) => {
      await page.waitForSelector(".card-name");
      await page.click("[data-user-toggle]");
      await page.waitForSelector("[data-user-dropdown].is-visible");
      await page.waitForTimeout(200);
    },
    // Dark pinned from the avatar menu — must override an emulated light OS.
    "theme-dark-pinned": async (page) => {
      await page.waitForSelector(".card-name");
      await page.click("[data-user-toggle]");
      await page.click('[data-theme-choice="dark"]');
      await page.waitForTimeout(300);
    },
    empty: async (page) => {
      await page.evaluate(() => {
        window.fetchAgents = async () => ({ data: [], total: 0 });
      });
      await page.evaluate(() => {
        document.querySelector("agents-page").remove();
        const el = document.createElement("agents-page");
        document.body.appendChild(el);
      });
      await page.waitForSelector("app-empty-state");
      await page.waitForTimeout(400);
    },
    "search-filled": async (page) => {
      await page.waitForSelector(".card-name");
      await page.fill("#search-input", "devops");
      await page.waitForTimeout(200);
    },
    "filter-category": async (page) => {
      await page.waitForSelector(".card-name");
      await page.click('.type-tab[data-category="devops"]');
      await page.waitForTimeout(400);
    },
    "pinned-tabs": async (page) => {
      await page.evaluate(() => {
        window.fetchSettings = async () => ({ catalog_tabs: "finance, devops" });
        document.querySelector("agents-page").remove();
        document.body.appendChild(document.createElement("agents-page"));
      });
      await page.waitForSelector('.type-tab[data-category="finance"]');
      await page.waitForTimeout(400);
    },
    "rail-expanded": async (page) => {
      await page.waitForSelector(".card-name");
      await page.click("[data-rail-toggle]");
      await page.waitForTimeout(500);
    },
  },
};
