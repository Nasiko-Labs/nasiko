import { fetchApi } from '/common/services/api.js';

window.fetchNavigation = async () => [
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
];

window.fetchAgents = async (query, page, limit) => {
  const params = new URLSearchParams({ q: query || '', page, limit });
  const agents = await fetchApi(`/catalog/agents?${params}`);
  return { data: Array.isArray(agents) ? agents : agents.data || [], total: agents.total || agents.length };
};
