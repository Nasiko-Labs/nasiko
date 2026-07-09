// Orchestrator page fixtures
export default {
  fetch: [
    ["POST /api/orchestrator/a2a", { __stream: [
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"status","status":"thinking"}}\n', delay: 100 },
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"artifact","artifact":{"parts":[{"type":"text","text":"Here is an analysis of your request:\\n\\n1. The system is currently healthy\\n2. All agents are responding normally\\n3. No pending deployments detected"}]}}}\n', delay: 300 },
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"status","status":"completed"}}\n', delay: 100 },
    ]}],
    ["GET /api/agents?status=running&limit=6", [
      { id: "a1", name: "coding-agent", display_name: "Coding Agent", status: "running" },
      { id: "a2", name: "docs-agent", display_name: "Docs Agent", status: "running" },
      { id: "a3", name: "nutrition-agent", display_name: "Nutrition Agent", status: "running" },
      { id: "a4", name: "research-agent", display_name: "Research Agent", status: "running" },
    ]],
  ],
  scenarios: {
    "sidebar-collapsed": async (page) => {
      await page.click("[data-sidebar-toggle]");
      await page.waitForTimeout(250);
    },
    "user-menu-open": async (page) => {
      await page.click("[data-user-toggle]");
      await page.waitForSelector(".user-dropdown.is-visible");
    },
  },
};
