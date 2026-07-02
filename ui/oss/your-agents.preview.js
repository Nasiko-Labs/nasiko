// Your Agents page fixtures — matches catalog Agent model
export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/catalog\/agents/ }, {
      data: [
        { id: "a-001", name: "coding-agent", display_name: "Coding Agent", image: "nasiko/coding:latest", status: "running", version: "1.2.0" },
        { id: "a-002", name: "research-agent", display_name: "Research Agent", image: "nasiko/research:latest", status: "running", version: "0.4.0" },
        { id: "a-003", name: "devops-agent", display_name: "DevOps Agent", image: "nasiko/devops:0.3.1", status: "stopped", version: "0.3.1" },
        { id: "a-004", name: "qa-agent", display_name: "QA Agent", image: "nasiko/qa:latest", status: "running", version: "1.0.0" },
        { id: "a-005", name: "docs-agent", display_name: "Docs Agent", image: "nasiko/docs:0.2.0", status: "error", version: "0.2.0" },
      ],
      total: 5,
    }],
  ],
};
