import { apiFetch, fetchApi } from '/common/services/api.js';

window.fetchNavigation = async () => [
  // rail: true → shown as a rail module icon; everything else is reachable
  // through the module tree navs and the ⌘F nav search.
  // module → which module tree (MODULE_NAVS below) a page belongs to. The rail
  // item carrying the same key stays selected while any of its children is
  // open, so a child page never leaves the rail with nothing highlighted.
  { title: "Orchestrator", url: "/index.html", icon: "brain", rail: true, module: "orchestrator" },
  // Not on the rail: workflows are the Orchestrator module's second group, and
  // a second rail icon into the same tree read as a separate module. Views of
  // that module page, not pages of their own — ⌘F and the rail reach them
  // through `?view=`, which index.html reads once on load.
  { title: "Workflows", url: "/index.html?view=workflows", icon: "workflow", module: "orchestrator" },
  { title: "Executions", url: "/index.html?view=executions", icon: "play", module: "orchestrator" },
  { title: "Agents", url: "/agents.html", icon: "bot", rail: true, module: "agents" },
  { title: "Sessions", url: "/sessions.html", icon: "activity", rail: true, module: "observability" },
  { title: "MCP gateway", url: "/mcp.html", icon: "server", rail: true, module: "mcp" },
  { title: "LLM router", url: "/llm-router.html", icon: "route", rail: true },
  { title: "TokenOps", url: "/tokenops.html", icon: "banknote", rail: true },
  // Views of the Agents module page, not pages of their own — ⌘F and the rail
  // reach them through `?view=`, which agents.html reads once on load.
  { title: "Your Agents", url: "/agents.html?view=your-agents", icon: "user", module: "agents" },
  { title: "Add Agent", url: "/agents.html?view=import", icon: "plus", module: "agents" },
  { title: "Set up CLI", url: "/setup-cli.html", icon: "terminal" },
  // Views of the Observability module page, not pages of their own — ⌘F and the
  // rail reach them through `?view=`, which sessions.html reads once on load.
  { title: "Flows", url: "/sessions.html?view=flows", icon: "cornerUpRight", module: "observability" },
  // In the Observability module tree but missing here, so ⌘F couldn't find it
  // and the rail lost its selection on the page.
  { title: "Resources", url: "/sessions.html?view=resources", icon: "activity", module: "observability" },
  { title: "Builds", url: "/agents.html?view=builds", icon: "cube", module: "agents" },
  // A view of the Settings module page, not a page of its own.
  { title: "Secrets", url: "/settings.html?view=secrets", icon: "lock", module: "settings" },
  { title: "Settings", url: "/settings.html", icon: "settings", rail: true, module: "settings" },
];

// In-card module tree navs (app-module-nav). Items are either page links
// ({label, url}) or in-page sections ({label, section} → the page handles
// the `module-nav-select` event). Only real pages/features appear here.
const MODULE_NAVS = {
  // Section keys must match the `data-view` keys in web/index.html — the whole
  // module lives in that one document, and these rows switch views in place.
  orchestrator: {
    title: 'Orchestrator', icon: 'brain',
    groups: [
      { label: 'Session', items: [
        { label: 'Orchestrate a task', section: 'orchestrate' },
      ]},
      { label: 'Workflows', items: [
        { label: 'All workflows', section: 'workflows' },
        { label: 'Executions', section: 'executions' },
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
        { label: 'My servers', section: 'my-servers' },
        { label: 'Shared with me', section: 'shared-with-me' },
      ]},
      { label: 'Toolkits', items: [
        { label: 'All toolkits', section: 'toolkits' },
      ]},
    ],
  },
  // Section keys must match the `data-view` keys in web/agents.html — the whole
  // module lives in that one document, and these rows switch views in place.
  agents: {
    title: 'Agent registry', icon: 'bot',
    groups: [
      { label: 'Agent sources', items: [
        { label: 'Agent hub', section: 'hub' },
        { label: 'Your agents', section: 'your-agents' },
        { label: 'Import agent', section: 'import' },
      ]},
      { label: 'Builds', items: [
        { label: 'All builds', section: 'builds' },
      ]},
    ],
  },
  // Section keys must match the `data-view` keys in web/sessions.html — the
  // whole module lives in that one document, and these rows switch views in
  // place.
  observability: {
    title: 'Observability', icon: 'activity',
    groups: [
      { label: 'Home', items: [
        { label: 'Execution history', section: 'history' },
        { label: 'Live flows', section: 'flows' },
        { label: 'Resources', section: 'resources' },
      ]},
    ],
  },
  // Section keys must match the `data-view` keys in web/settings.html — the
  // whole module lives in that one document, and these rows switch views in
  // place. The four workspace sections are the exception by design: they are
  // sections *within* the `settings` view, handled by settings-page.js, which is
  // why they name no view of their own (see the comment in settings.html).
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
        { label: 'Secrets', section: 'secrets' },
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

window.updateLlmConfig = async (id, body) => {
  return fetchApi(`/llm-configs/${encodeURIComponent(id)}`, {
    method: 'PATCH',
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

window.fetchMcpConnectorDetail = async (id) => {
  return fetchApi(`/mcp/connectors/${encodeURIComponent(id)}`);
};
