// Orchestrator page fixtures
export default {
  fetch: [
    ["POST /api/a2a", { __stream: [
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"status","status":"thinking"}}\n', delay: 100 },
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"artifact","artifact":{"parts":[{"type":"text","text":"Here is an analysis of your request:\\n\\n1. The system is currently healthy\\n2. All agents are responding normally\\n3. No pending deployments detected"}]}}}\n', delay: 300 },
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"status","status":"completed"}}\n', delay: 100 },
    ]}],
    ["GET /api/containers", [
      { container_id: "coding-agent", state: "running", replicas_live: 1, endpoint: "http://coding-agent:8000" },
      { container_id: "docs-agent", state: "running", replicas_live: 1, endpoint: "http://docs-agent:8000" },
      { container_id: "nutrition-agent", state: "running", replicas_live: 1, endpoint: "http://nutrition:8000" },
      { container_id: "research-agent", state: "running", replicas_live: 1, endpoint: "http://research-agent:8000" },
      { container_id: "devops-agent", state: "stopped", replicas_live: 0, endpoint: null },
    ]],
  ],
  scenarios: {
    "sidebar-collapsed": async (page) => {
      await page.click("[data-sidebar-toggle]");
      await page.waitForTimeout(250);
    },
  },
};
