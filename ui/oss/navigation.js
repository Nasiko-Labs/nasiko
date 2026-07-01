import { fetchApi } from '/common/services/api.js';

window.fetchNavigation = async () => [
  { title: "Orchestrator", url: "/index.html" },
  { title: "Agents", url: "/agents.html" },
  { title: "Your Agents", url: "/your-agents.html" },
  { title: "Add Agent", url: "/add-agent.html" },
  { title: "Sessions", url: "/sessions.html" },
  { title: "Flows", url: "/flows.html" },
  { title: "Builds", url: "/builds.html" },
  { title: "Usage", url: "/usage.html" },
  { title: "Secrets", url: "/secrets.html" },
  { title: "Settings", url: "/settings.html" },
];

window.fetchAgents = async (query, page, limit) => {
  const params = new URLSearchParams({ q: query || '', page, limit });
  return fetchApi(`/catalog/agents?${params}`);
};
