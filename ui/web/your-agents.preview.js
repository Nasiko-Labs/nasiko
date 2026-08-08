// Your Agents page fixtures and scenarios
export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/agents/ }, {
      data: [
        { id: "a-001", tags: ["devops","code","debugging"], description: "General-purpose coding assistant for any language. Supports code generation, review, and refactoring.", name: "coding-agent", display_name: "Coding Agent", image: "nasiko/coding:latest", status: "running", version: "1.2.0" },
        { id: "a-002", tags: ["research","analysis"], description: "Research and summarization across documents and the web.", name: "research-agent", display_name: "Research Agent", image: "nasiko/research:0.4.0", status: "running", version: "0.4.0" },
        { id: "a-003", tags: ["devops","kubernetes"], description: "Infrastructure automation, CI/CD pipelines, and deployments.", name: "devops-agent", display_name: "DevOps Agent", image: "nasiko/devops:0.3.1", status: "stopped", version: "0.3.1" },
        { id: "a-004", tags: ["testing","quality"], description: "Test generation, quality checks, and coverage analysis.", name: "qa-agent", display_name: "QA Agent", image: "nasiko/qa:latest", status: "running", version: "1.0.0" },
        { id: "a-005", tags: ["documentation"], name: "docs-agent", display_name: "Docs Agent", image: "nasiko/docs:0.2.0", status: "error", version: "0.2.0" },
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
      await page.waitForTimeout(400);
    },
    "filter-running": async (page) => {
      await page.waitForSelector(".agent-card-name");
      await page.click('.type-tab[data-status="running"]');
      await page.waitForTimeout(200);
    },
    "filter-failed": async (page) => {
      await page.waitForSelector(".agent-card-name");
      await page.click('.type-tab[data-status="failed"]');
      await page.waitForTimeout(200);
    },
    "sort-status": async (page) => {
      await page.waitForSelector(".agent-card-name");
      await page.selectOption("#sort-select", "status");
      await page.waitForTimeout(200);
    },
  },
};
