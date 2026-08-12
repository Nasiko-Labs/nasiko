import { apiFetch, fetchApi } from '/common/services/api.js';

window.fetchNavigation = async () => [
  // rail: true → shown as a rail module icon; everything else is reachable
  // through the module tree navs and the ⌘F nav search.
  { title: "Orchestrator", url: "/index.html", icon: "brain", rail: true },
  // On the rail: without it the only route to the workflow list was to open
  // "Create workflow" and back out of it.
  { title: "Workflows", url: "/workflows.html", icon: "workflow", rail: true },
  { title: "Executions", url: "/executions.html", icon: "play" },
  { title: "Agents", url: "/agents.html", icon: "bot", rail: true },
  { title: "Sessions", url: "/sessions.html", icon: "activity", rail: true },
  { title: "MCP gateway", url: "/mcp.html", icon: "server", rail: true },
  { title: "LLM router", url: "/llm-router.html", icon: "route", rail: true },
  { title: "TokenOps", url: "/tokenops.html", icon: "banknote", rail: true },
  { title: "Your Agents", url: "/your-agents.html", icon: "user" },
  { title: "Add Agent", url: "/add-agent.html", icon: "plus" },
  { title: "Set up CLI", url: "/setup-cli.html", icon: "terminal" },
  { title: "Flows", url: "/flows.html", icon: "cornerUpRight" },
  { title: "Builds", url: "/builds.html", icon: "cube" },
  { title: "Secrets", url: "/secrets.html", icon: "lock" },
  { title: "Settings", url: "/settings.html", icon: "settings", rail: true },
];

// In-card module tree navs (app-module-nav). Items are either page links
// ({label, url}) or in-page sections ({label, section} → the page handles
// the `module-nav-select` event). Only real pages/features appear here.
const MODULE_NAVS = {
  orchestrator: {
    title: 'Orchestrator', icon: 'brain',
    groups: [
      { label: 'Session', items: [
        { label: 'Orchestrate a task', url: '/index.html' },
      ]},
      { label: 'Workflows', items: [
        { label: 'All workflows', url: '/workflows.html' },
        { label: 'Executions', url: '/executions.html' },
      ]},
    ],
  },
  mcp: {
    title: 'MCP gateway', icon: 'server',
    groups: [
      // Scope rows filter the unified catalog grid; ownership scopes apply
      // to custom MCP servers only (toolkits are platform-registered).
      { label: 'MCP servers', items: [
        { label: 'All', section: 'all' },
        { label: 'Created by you', section: 'created-by-you' },
        { label: 'Shared with me', section: 'shared-with-me' },
        { label: 'My uploads', section: 'uploads' },
      ]},
      { label: 'Toolkits', items: [
        { label: 'All toolkits', section: 'toolkits' },
      ]},
      { label: 'Access', items: [
        { label: 'Agent access', section: 'agent-access' },
      ]},
    ],
  },
  agents: {
    title: 'Agent registry', icon: 'bot',
    groups: [
      { label: 'Agent sources', items: [
        { label: 'Agent hub', url: '/agents.html' },
        { label: 'Your agents', url: '/your-agents.html' },
        { label: 'Import agent', url: '/add-agent.html' },
      ]},
      { label: 'Builds', items: [
        { label: 'All builds', url: '/builds.html' },
      ]},
    ],
  },
  observability: {
    title: 'Observability', icon: 'activity',
    groups: [
      { label: 'Home', items: [
        { label: 'Execution history', url: '/sessions.html' },
        { label: 'Live flows', url: '/flows.html' },
        { label: 'Resources', url: '/resources.html' },
      ]},
    ],
  },
  settings: {
    title: 'Settings', icon: 'settings',
    groups: [
      { label: 'Workspace', items: [
        { label: 'General', section: 'general' },
        { label: 'Flow limits', section: 'limits' },
        { label: 'Registry', section: 'registry' },
      ]},
      { label: 'Security', items: [
        { label: 'Single sign-on', section: 'sso' },
        { label: 'Secrets', url: '/secrets.html' },
      ]},
    ],
  },
};

window.fetchModuleNav = async (module) => {
  const nav = MODULE_NAVS[module];
  if (!nav) return null;
  // Observability used to append a dynamic "Recent activity" group listing the
  // five newest sessions. Dropped: on sessions.html — the only page it appeared
  // on — it restated the first five rows of the table beside it, and the table
  // is filterable, sortable and complete. Truncated duplicates of the primary
  // content are noise, and it cost an extra API call per page load.
  return { ...nav, groups: [...nav.groups] };
};

window.fetchAgents = async (query, page, limit) => {
  const params = new URLSearchParams({ q: query || '', page, limit });
  const agents = await fetchApi(`/agents?${params}`);
  return { data: Array.isArray(agents) ? agents : agents.data || [], total: agents.total || agents.length };
};

// `/chat/sessions` is keyset-paginated: pass the `next_cursor` from the previous
// response to get the following page. Returns {data, has_more, next_cursor}.
window.fetchSessions = async (_query, limit = 25, cursor = null) => {
  const params = new URLSearchParams({ limit });
  if (cursor) params.set('cursor', cursor);
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

// ── MAF workflows — /api/maf/* (oss/server/src/maf.rs) ───────────────────────
// Every response uses the {data, status_code, message} envelope; list
// endpoints additionally wrap the rows as data:{data:[...], total} (total is
// the page length, not the true total).
const mafRows = (body) =>
  (Array.isArray(body?.data) ? body.data : body?.data?.data) || [];

window.fetchWorkflows = async (limit = 100, offset = 0) => {
  return mafRows(await fetchApi(`/maf/workflows?limit=${limit}&offset=${offset}`));
};

window.fetchWorkflow = async (id) => {
  return (await fetchApi(`/maf/workflow/${encodeURIComponent(id)}`)).data;
};

window.createWorkflow = async (body) => {
  return (await fetchApi('/maf/workflows', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })).data;
};

window.updateWorkflow = async (id, body) => {
  return (await fetchApi(`/maf/workflow/${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })).data;
};

window.deleteWorkflow = async (id) => {
  return fetchApi(`/maf/workflow/${encodeURIComponent(id)}`, { method: 'DELETE' });
};

window.runWorkflow = async (id) => {
  // 202 Accepted → data: {execution_id, execution_number, execution_count}
  return (await fetchApi(`/maf/workflow/${encodeURIComponent(id)}/run`, { method: 'POST' })).data;
};

window.fetchExecution = async (id) => {
  return (await fetchApi(`/maf/execution/${encodeURIComponent(id)}`)).data;
};

window.fetchWorkflowExecutions = async (id, limit = 50, offset = 0) => {
  return mafRows(await fetchApi(
    `/maf/workflow/${encodeURIComponent(id)}/executions?limit=${limit}&offset=${offset}`,
  ));
};

window.fetchAllExecutions = async (limit = 100, offset = 0) => {
  return mafRows(await fetchApi(`/maf/executions?limit=${limit}&offset=${offset}`));
};

// The create page branches on the failure mode (503 = no LLM key configured,
// 400 = user has no agents, 422 = planner failure), so surface the status.
window.generateWorkflow = async (description) => {
  const res = await apiFetch('/maf/generate', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ description }),
  });
  const body = await res.json().catch(() => null);
  if (!res.ok) {
    const err = new Error(body?.message || `HTTP ${res.status}`);
    err.status = res.status;
    throw err;
  }
  return body?.data;
};

window.fetchTraceDetail = async (traceId) => {
  // Server route: GET /api/observability/trace/{id} (same as `nasiko observe trace`).
  // Envelope {data:{trace}}; trace.spans is a nested tree (children embedded).
  const resp = await fetchApi(`/observability/trace/${traceId}`);
  return resp.data?.trace ?? resp.trace ?? resp;
};

// Observability — execution history + per-session traces (see /api/docs)
// Paged: every row costs the server one trace-store lookup, so asking for the
// whole history is what made Execution history slow to appear.
window.fetchObservabilitySessions = async (limit = 25, offset = 0) => {
  const params = new URLSearchParams({ limit, offset });
  return fetchApi(`/observability/session/list?${params}`);
};

window.fetchObservabilitySession = async (sessionId) => {
  return fetchApi(`/observability/session/${encodeURIComponent(sessionId)}`);
};

// Resource usage — host + per-container CPU/memory/IO (admin-only endpoint).
window.fetchResourceStats = async () => {
  return fetchApi('/observability/resources');
};

// Owner-scoped: usage for a single agent. Accepts a UUID or an agent name.
window.fetchAgentResourceStats = async (agentRef) => {
  return fetchApi(`/observability/agent/${encodeURIComponent(agentRef)}/resources`);
};

window.fetchObservabilityTrace = async (traceId) => {
  const resp = await fetchApi(`/observability/trace/${encodeURIComponent(traceId)}`);
  return resp.data?.trace ?? resp.trace ?? resp;
};

window.fetchSpanDetail = async (traceId, spanId) => {
  return fetchApi(`/observability/span/${encodeURIComponent(traceId)}/${encodeURIComponent(spanId)}`);
};

window.fetchChatSession = async (sessionId) => {
  return fetchApi(`/chat/sessions/${encodeURIComponent(sessionId)}`);
};

// LLM router — routing configs + provider/model catalog (see /api/docs)
window.fetchLlmConfigs = async () => {
  return fetchApi('/llm-configs');
};

window.createLlmConfig = async (body) => {
  return fetchApi('/llm-configs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
};

window.deleteLlmConfig = async (id) => {
  return fetchApi(`/llm-configs/${encodeURIComponent(id)}`, { method: 'DELETE' });
};

window.setDefaultLlmConfig = async (id) => {
  return fetchApi(`/llm-configs/${encodeURIComponent(id)}/default`, { method: 'POST' });
};

window.clearDefaultLlmConfig = async (id) => {
  return fetchApi(`/llm-configs/${encodeURIComponent(id)}/default`, { method: 'DELETE' });
};

window.fetchLlmProviders = async () => {
  return fetchApi('/llm-router/providers');
};

window.fetchSecretsList = async () => fetchApi('/secrets');

window.fetchUsageSummary = async () => {
  return fetchApi('/usage/summary');
};

// TokenOps dashboard — GET /api/observability/finops/dashboard
window.fetchTokenopsDashboard = async (startTime, endTime) => {
  const q = new URLSearchParams();
  if (startTime) q.set('start_time', startTime);
  // if (endTime) q.set('end_time', endTime);
  const params = q.size ? `?${q}` : '';
  return fetchApi(`/observability/finops/dashboard${params}`);
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

// User directory search for the ⌘F palette (GET /api/search/users, an OSS
// route — org-scoped on EE). A 404 hides the palette's Users section.
window.fetchUserSearch = async (query) => {
  const params = new URLSearchParams({ q: query || '' });
  return fetchApi(`/search/users?${params}`);
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

// ── MCP gateway — connectors, uploads, credentials, per-agent access ─────────
// Envelope {data, status_code, message}; see /api/docs (tag "mcp").
window.fetchMcpConnectors = async () => {
  return fetchApi('/mcp/connectors');
};

window.registerMcpConnector = async (body) => {
  return fetchApi('/mcp/connectors', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
};

window.probeMcpConnector = async (url) => {
  return fetchApi('/mcp/connectors/probe', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ url }),
  });
};

window.updateMcpConnector = async (connectorId, body) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
};

window.deleteMcpConnector = async (connectorId) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}`, { method: 'DELETE' });
};

window.uploadMcpServerZip = async (formData) => {
  // Multipart fields: name, version_tag, env (JSON string), file.
  return fetchApi('/mcp/connectors/upload', { method: 'POST', body: formData });
};

window.uploadMcpServerGithub = async (body) => {
  return fetchApi('/mcp/connectors/upload-github', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
};

window.fetchMcpMyUploads = async () => {
  return fetchApi('/mcp/connectors/my-uploads');
};

window.fetchMcpBuildStatus = async (connectorId) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}/build-status`);
};

window.fetchMcpBuildLogs = async (connectorId, tail = 200) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}/build-logs?tail=${tail}`);
};

window.fetchMcpCredentialStatus = async (connectorId) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}/credential/status`);
};

window.setMcpCredential = async (connectorId, value) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}/credential`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ value }),
  });
};

window.deleteMcpCredential = async (connectorId) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}/credential`, { method: 'DELETE' });
};

window.authorizeMcpOauth = async (connectorId) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}/oauth/authorize`, { method: 'POST' });
};

window.fetchMcpOauthStatus = async (connectorId) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}/oauth/status`);
};

window.revokeMcpOauthToken = async (connectorId) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(connectorId)}/oauth/token`, { method: 'DELETE' });
};

// Toolkits — platform Composio connectables the caller can connect to.
window.fetchMcpToolkits = async () => {
  return fetchApi('/mcp/composio/toolkits');
};

// body: {connector_id} (+ optional credentials: {value} for api_key flows).
// data.status: connected | initiated (oauth_url) | oauth_required (authorization_url).
window.connectMcpService = async (body) => {
  return fetchApi('/mcp/connect', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
};

window.fetchMcpConnections = async () => {
  return fetchApi('/mcp/connections');
};

window.disconnectMcpConnection = async (connectorId) => {
  return fetchApi(`/mcp/connections/${encodeURIComponent(connectorId)}`, { method: 'DELETE' });
};

window.fetchAgentMcpConnectors = async (agentId) => {
  return fetchApi(`/mcp/agents/${encodeURIComponent(agentId)}/connectors`);
};

window.setAgentMcpConnectorAccess = async (agentId, connectorId, enabled) => {
  return fetchApi(
    `/mcp/agents/${encodeURIComponent(agentId)}/connectors/${encodeURIComponent(connectorId)}`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ enabled }),
    },
  );
};

window.fetchAgentMcpConnectorTools = async (agentId, connectorId) => {
  return fetchApi(
    `/mcp/agents/${encodeURIComponent(agentId)}/connectors/${encodeURIComponent(connectorId)}/tools`,
  );
};

window.fetchAgentMcpToolRules = async (agentId) => {
  return fetchApi(`/mcp/agents/${encodeURIComponent(agentId)}/tools`);
};

window.saveAgentMcpToolRules = async (agentId, rules) => {
  return fetchApi(`/mcp/agents/${encodeURIComponent(agentId)}/tools`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ rules }),
  });
};
