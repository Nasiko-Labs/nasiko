// Agents catalog page fixtures and scenarios
export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/agents/ }, {
      data: [
        { id: "a-001", name: "coding-agent", display_name: "Coding Agent", image: "nasiko/coding:latest", status: "running", replicas: 1, description: "General-purpose coding assistant for any language. Supports code generation, review, debugging, and refactoring.", tags: ["devops", "code", "debugging"] },
        { id: "a-002", name: "research-agent", display_name: "Research Agent", image: "nasiko/research:latest", status: "running", replicas: 2, description: "Research and summarization across documents and the web", tags: ["hr", "research", "analysis"] },
        { id: "a-003", name: "devops-agent", display_name: "DevOps Agent", image: "nasiko/devops:0.3.1", status: "stopped", replicas: 0, description: "Infrastructure automation, CI/CD pipelines, and deployments", tags: ["devops", "infrastructure", "kubernetes"] },
        { id: "a-004", name: "qa-agent", display_name: "QA Agent", image: "nasiko/qa:latest", status: "running", replicas: 1, description: "Test generation, quality checks, and coverage analysis", tags: ["devops", "testing", "quality"] },
        { id: "a-005", name: "docs-agent", display_name: "Docs Agent", image: "nasiko/docs:0.2.0", status: "error", replicas: 0, description: "Documentation generation from source code and specs", tags: ["legal", "documentation", "writing"] },
        { id: "a-006", name: "finance-bot", display_name: "Finance Bot", image: "nasiko/finance:1.0.0", status: "running", replicas: 1, description: "Expense tracking, invoice processing, budget analysis", tags: ["finance", "accounting", "reports"] },
      ],
      total: 6,
    }],
    ["GET /api/settings", { catalog_tabs: null }],
  ],
  scenarios: {
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
    },
    "search-filled": async (page) => {
      await page.waitForSelector(".card-name");
      await page.fill("#search-input", "devops");
      await page.waitForTimeout(200);
    },
    "filter-category": async (page) => {
      await page.waitForSelector(".card-name");
      await page.click('.type-tab[data-category="finance"]');
      await page.waitForTimeout(200);
    },
    "pinned-tabs": async (page) => {
      await page.evaluate(() => {
        window.fetchSettings = async () => ({ catalog_tabs: "finance, devops" });
        document.querySelector("agents-page").remove();
        document.body.appendChild(document.createElement("agents-page"));
      });
      await page.waitForSelector('.type-tab[data-category="finance"]');
    },
    "sidebar-collapsed": async (page) => {
      await page.waitForSelector(".card-name");
      await page.click("[data-sidebar-toggle]");
      await page.waitForTimeout(200);
    },
  },
};
