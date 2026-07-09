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
  { title: "Secrets", url: "/secrets.html", icon: "lock" },
  { title: "Settings", url: "/settings.html", icon: "settings" },
];

window.fetchAgents = async (query, page, limit) => {
  const params = new URLSearchParams({ q: query || '', page, limit });
  const agents = await fetchApi(`/agents?${params}`);
  return { data: Array.isArray(agents) ? agents : agents.data || [], total: agents.total || agents.length };
};

window.fetchSessions = async (_query, _page, limit) => {
  const params = new URLSearchParams({ limit });
  return fetchApi(`/chat/sessions?${params}`);
};

window.fetchContainers = async (query, page, limit) => {
  const params = new URLSearchParams({ limit, offset: ((page || 1) - 1) * limit });
  if (query) params.set('q', query);
  const body = await fetchApi(`/agents?${params}`);
  const data = Array.isArray(body) ? body : (body.data || []);
  return { data, total: body.total || data.length };
};

window.fetchFlows = async (query, page, limit) => {
  const params = new URLSearchParams({ q: query || '', page, limit });
  return fetchApi(`/flows?${params}`);
};

window.fetchFlowDetail = async (flowId) => {
  return fetchApi(`/flows/${flowId}`);
};

window.fetchTraceDetail = async (traceId) => {
  // Server route: /api/observability/trace/{id}, response envelope {data:{trace,spans}}.
  // Normalize to the flat {spans, duration_ms} shape session-trace-page renders.
  const resp = await fetchApi(`/observability/trace/${traceId}`);
  const d = resp.data || resp;
  return {
    ...d,
    spans: d.spans || [],
    duration_ms: d.duration_ms ?? d.trace?.latency_ms ?? 0,
  };
};

window.fetchUsageSummary = async () => {
  return fetchApi('/usage/summary');
};

window.fetchUsageHistory = async (days = 7) => {
  return fetchApi(`/usage/history?days=${days}`);
};

window.fetchUsageByAgent = async (query, page, limit) => {
  const params = new URLSearchParams({ q: query || '', limit, offset: ((page || 1) - 1) * limit });
  return fetchApi(`/usage/by-agent?${params}`);
};

window.fetchUsageByModel = async (query, page, limit) => {
  const params = new URLSearchParams({ q: query || '', limit, offset: ((page || 1) - 1) * limit });
  return fetchApi(`/usage/by-model?${params}`);
};

window.fetchBuilds = async (query, page, limit) => {
  const params = new URLSearchParams({ limit, offset: ((page || 1) - 1) * limit });
  if (query) params.set('q', query);
  return fetchApi(`/builds?${params}`);
};

window.fetchSettings = async () => {
  return fetchApi('/settings');
};

window.saveSettings = async (settings) => {
  return fetchApi('/settings', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(settings),
  });
};
