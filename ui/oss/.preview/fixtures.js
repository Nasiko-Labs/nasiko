// App-wide fixtures — navigation, auth, and shared data functions.
export default {
  fetch: [
    ["GET /api/me", { user_id: "u-001", username: "admin", is_superuser: true, role: "admin" }],
    [{ method: "GET", path: /^\/api\/catalog\/agents/ }, {
      data: [
        { id: "a-001", name: "coding-agent", display_name: "Coding Agent", image: "nasiko/coding:latest", status: "running", replicas: 1, description: "General-purpose coding assistant for any language", tags: ["devops", "code", "debugging"] },
        { id: "a-002", name: "research-agent", display_name: "Research Agent", image: "nasiko/research:latest", status: "running", replicas: 2, description: "Research and summarization across documents and the web", tags: ["hr", "research", "analysis"] },
        { id: "a-003", name: "devops-agent", display_name: "DevOps Agent", image: "nasiko/devops:0.3.1", status: "stopped", replicas: 0, description: "Infrastructure automation, CI/CD pipelines, and deployments", tags: ["devops", "infrastructure", "kubernetes"] },
        { id: "a-004", name: "qa-agent", display_name: "QA Agent", image: "nasiko/qa:latest", status: "running", replicas: 1, description: "Test generation, quality checks, and coverage analysis", tags: ["devops", "testing", "quality"] },
        { id: "a-005", name: "docs-agent", display_name: "Docs Agent", image: "nasiko/docs:0.2.0", status: "error", replicas: 0, description: "Documentation generation from source code and specs", tags: ["legal", "documentation", "writing"] },
        { id: "a-006", name: "finance-bot", display_name: "Finance Bot", image: "nasiko/finance:1.0.0", status: "running", replicas: 1, description: "Expense tracking, invoice processing, budget analysis", tags: ["finance", "accounting", "reports"] },
      ],
      total: 6,
    }],
  ],
  window: {
    fetchNavigation: async () => [
      { title: "Orchestrator", url: "/index.html", icon: "send" },
      { title: "Agents", url: "/agents.html", icon: "layers" },
      { title: "Your Agents", url: "/your-agents.html", icon: "user" },
      { title: "Add Agent", url: "/add-agent.html", icon: "plus" },
      { title: "Sessions", url: "/sessions.html", icon: "clock" },
      { title: "Flows", url: "/flows.html", icon: "cornerUpRight" },
      { title: "Builds", url: "/builds.html", icon: "cube" },
      { title: "Usage", url: "/usage.html", icon: "code" },
      { title: "Secrets", url: "/secrets.html", icon: "key" },
      { title: "Settings", url: "/settings.html", icon: "settings" },
    ],
  },
};
