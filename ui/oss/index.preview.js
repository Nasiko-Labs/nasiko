// Orchestrator page fixtures
export default {
  fetch: [
    ["POST /api/a2a", { __stream: [
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"status","status":"thinking"}}\n', delay: 100 },
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"artifact","artifact":{"parts":[{"type":"text","text":"Here is an analysis of your request:\\n\\n1. The system is currently healthy\\n2. All agents are responding normally\\n3. No pending deployments detected"}]}}}\n', delay: 300 },
      { text: '{"jsonrpc":"2.0","id":"1","result":{"type":"status","status":"completed"}}\n', delay: 100 },
    ]}],
  ],
};
