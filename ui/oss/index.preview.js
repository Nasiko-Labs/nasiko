// Orchestrator page fixtures
export default {
  fetch: [
    ["POST /api/a2a", { __stream: [
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"status","status":"thinking"}}\n', delay: 100 },
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"artifact","artifact":{"parts":[{"type":"text","text":"Here is an analysis of your request:\\n\\n1. The system is currently healthy\\n2. All agents are responding normally\\n3. No pending deployments detected"}]}}}\n', delay: 300 },
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"status","status":"completed"}}\n', delay: 100 },
    ]}],
    ["GET /api/containers", [
      { id: "coding-agent", name: "Coding Agent", image: "nasiko/coding-agent:latest", status: "running" },
      { id: "docs-agent", name: "Docs Agent", image: "nasiko/docs-agent:latest", status: "running" },
      { id: "nutrition-agent", name: "Nutrition", image: "nasiko/nutrition:latest", status: "running" },
      { id: "research-agent", name: "Research", image: "nasiko/research-agent:latest", status: "running" },
    ]],
  ],
  scenarios: {
    "sidebar-collapsed": async (page) => {
      await page.click("[data-sidebar-toggle]");
      await page.waitForTimeout(250);
    },
  },
};
