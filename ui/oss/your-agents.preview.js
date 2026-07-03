// Your Agents page fixtures and scenarios
export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/catalog\/agents/ }, {
      data: [
        { id: "a-001", name: "coding-agent", display_name: "Coding Agent", image: "nasiko/coding:latest", status: "running", version: "1.2.0" },
        { id: "a-002", name: "research-agent", display_name: "Research Agent", image: "nasiko/research:0.4.0", status: "running", version: "0.4.0" },
        { id: "a-003", name: "devops-agent", display_name: "DevOps Agent", image: "nasiko/devops:0.3.1", status: "stopped", version: "0.3.1" },
        { id: "a-004", name: "qa-agent", display_name: "QA Agent", image: "nasiko/qa:latest", status: "running", version: "1.0.0" },
        { id: "a-005", name: "docs-agent", display_name: "Docs Agent", image: "nasiko/docs:0.2.0", status: "error", version: "0.2.0" },
      ],
      total: 5,
    }],
    ["GET /api/secrets", [
      { name: "OPENAI_API_KEY" },
      { name: "GITHUB_TOKEN" },
      { name: "DATABASE_URL" },
    ]],
  ],
  scenarios: {
    empty: async (page) => {
      await page.evaluate(() => {
        window.fetchContainers = async () => ({ data: [], total: 0 });
      });
      await page.evaluate(() => {
        document.querySelector("your-agents-page").remove();
        const el = document.createElement("your-agents-page");
        document.body.appendChild(el);
      });
      await page.waitForSelector("app-empty-state");
    },
    "filter-running": async (page) => {
      await page.waitForSelector(".agent-card-name");
      await page.click('.stat-chip[data-filter="running"]');
      await page.waitForTimeout(200);
    },
    "filter-error": async (page) => {
      await page.waitForSelector(".agent-card-name");
      await page.click('.stat-chip[data-filter="error"]');
      await page.waitForTimeout(200);
    },
    "sort-status": async (page) => {
      await page.waitForSelector(".agent-card-name");
      await page.selectOption("#sort-select", "status");
      await page.waitForTimeout(200);
    },
  },
};
