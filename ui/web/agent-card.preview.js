const baseAgent = {
  id: "a-001",
  name: "devops-cluster-lifecycle",
  display_name: "Devops cluster lifecycle agent",
  owner_id: "u-owner-1",
  can_manage: true,
  image: "nasiko/devops-cluster:0.1.0",
  status: "running",
  replicas: 1,
  version: "0.1.0",
  protocol_version: "0.2.9",
  transport: "JSONRPC",
  default_io: "application/json, text/plain",
  description: "Manages the full lifecycle of virtual clusters (vClusters) on SkyCommand. Handles creation, scaling, health scoring, and decommissioning of Kubernetes clusters for tenants.",
  tags: ["Cluster", "Provisioning", "Vcluster", "Kubernetes", "DevOps", "Infrastructure", "Scaling", "Health", "Tenant", "Lifecycle", "GPU"],
  provider: "Nasiko",
  project_url: "",
  docs_url: "",
  capabilities: {
    streaming: false,
    push_notifications: false,
    state_transition_history: true,
  },
  skills: [
    {
      name: "Cluster Provisioning",
      description: "Create new vClusters with specified resource blueprints, networking policies, and storage configurations.",
      sample_query: "Create a new vCluster with 4 GPU nodes for team-ml",
    },
    {
      name: "Cluster Scaling",
      description: "Scale cluster resources up or down based on workload demands with pre-flight safety checks.",
      sample_query: "Scale gpu-cluster-prod to 8 GPU nodes",
    },
    {
      name: "Cluster Readiness Scoring",
      description: "Compute a readiness score (0-100) for clusters based on node health, networking, storage, and GPU accessibility.",
      sample_query: "What is the readiness score for gpu-cluster-prod?",
    },
  ],
};

const mcpEnvelope = (data) => ({ data, status_code: 200, message: "OK" });

// Sets ?id= and loads the page. All owner scenarios use a-001 (can_manage
// true); the viewer scenario uses a-viewer (can_manage false).
const gotoAgent = async (page, id) => {
  const url = new URL(page.url());
  url.searchParams.set('id', id);
  await page.goto(url.toString(), { waitUntil: 'networkidle' });
  await page.waitForSelector('.acp-page', { timeout: 5000 });
};

export default {
  fetch: [
    // Owner-scoped container usage → GET /api/observability/agent/{ref}/resources.
    // Registered as a fetch fixture, not a window one: navigation.js loads after
    // the fixtures and would clobber a window.fetchAgentResourceStats override.
    [{ method: "GET", path: /^\/api\/observability\/agent\/[^/]+\/resources$/ }, {
      data: {
        agent_id: "a-001",
        agent_name: "devops-cluster-lifecycle",
        usage: {
          name: "nasiko-agent-a-001",
          display_name: "devops-cluster-lifecycle",
          group: "agent_runtime",
          state: "running",
          cpu_percent: 42.6,
          mem_used_bytes: 281018368,
          mem_limit_bytes: 3972844748,
          net_rx_bytes: 23068672,
          net_tx_bytes: 14680064,
          block_read_bytes: 0,
          block_write_bytes: 0,
        },
        collected_at: "2026-08-09T11:20:00Z",
      },
    }],
    [{ method: "GET", path: /^\/api\/me$/ }, {
      sub: "u-owner-1", username: "akhil", is_superuser: false,
    }],
    // Viewer variant — same card, no management rights → no gated tabs.
    [{ method: "GET", path: /^\/api\/agents\/a-viewer$/ }, {
      status_code: 200,
      message: "Agent retrieved successfully",
      data: { ...baseAgent, id: "a-viewer", owner_id: "u-someone-else", can_manage: false },
    }],
    [{ method: "GET", path: /^\/api\/agents\/[^/]+$/ }, {
      // SingleResponse envelope — matches GET /api/agents/{id} (see /api/docs)
      status_code: 200,
      message: "Agent retrieved successfully",
      data: baseAgent,
    }],
    // Shape mirrors GET /api/observability/agent/{id}/stats (`nasiko observe stats`).
    [{ method: "GET", path: /^\/api\/observability\/agent\/.*\/stats/ }, {
      data: {
        project: {
          id: "devops-agent",
          trace_count: 128,
          latency_ms_p50: 412,
          latency_ms_p99: 2380,
          cost_summary: {
            total: { cost: 1.8642 },
            prompt: { cost: 0.7121 },
            completion: { cost: 1.1521 },
          },
          span_annotation_names: [],
          document_evaluation_names: [],
        },
      },
    }],
    // GET /api/agents/{id}/secrets → plain [SecretListEntry] (see /api/docs)
    [{ method: "GET", path: /^\/api\/agents\/[^/]+\/secrets$/ }, [
      { name: "OPENAI_API_KEY", updated_at: null },
      { name: "SKYCOMMAND_TOKEN", updated_at: null },
    ]],
    [{ method: "POST", path: /^\/api\/agents\/[^/]+\/secrets$/ }, {}],
    [{ method: "DELETE", path: /^\/api\/agents\/[^/]+\/secrets\/[^/]+$/ }, {}],

    // ── Access & security (EE-shaped: teams/departments answer with arrays) ──
    [{ method: "GET", path: /^\/api\/agents\/[^/]+\/visibility$/ }, {
      agent_id: "a-001",
      is_public: false,
      grants: [
        { id: "g-001", agent_id: "a-001", grant_type: "team", grantee_id: "team-platform", granted_by: null, created_at: "2026-06-15T10:00:00Z" },
        { id: "g-002", agent_id: "a-001", grant_type: "team", grantee_id: "team-infra", granted_by: null, created_at: "2026-06-16T10:00:00Z" },
        { id: "g-003", agent_id: "a-001", grant_type: "user", grantee_id: "u-alice", granted_by: null, created_at: "2026-06-17T10:00:00Z" },
        { id: "g-004", agent_id: "a-001", grant_type: "department", grantee_id: "dept-engineering", granted_by: null, created_at: "2026-06-18T10:00:00Z" },
        { id: "g-005", agent_id: "a-001", grant_type: "agent", grantee_id: "b-002", granted_by: null, created_at: "2026-06-19T10:00:00Z" },
      ],
    }],
    [{ method: "GET", path: /^\/api\/agents\/[^/]+\/users$/ }, [
      { id: "u-owner-1", username: "akhil", email: "akhil@nasiko.com", role: "admin" },
      { id: "u-alice", username: "alice", email: "alice@nasiko.com", role: "team_member" },
      { id: "u-bob", username: "bob", email: "bob@nasiko.com", role: "team_member" },
      { id: "u-carol", username: "carol", email: "carol@nasiko.com", role: "team_lead" },
    ]],
    [{ method: "GET", path: /^\/api\/agents\/[^/]+\/teams$/ }, [
      { id: "team-platform", name: "Platform", department_id: "dept-engineering", description: null, members_count: 8 },
      { id: "team-infra", name: "Infrastructure", department_id: "dept-engineering", description: null, members_count: 5 },
    ]],
    [{ method: "GET", path: /^\/api\/agents\/[^/]+\/departments$/ }, [
      { id: "dept-engineering", name: "Engineering", description: null, members_count: 24, teams_count: 4 },
    ]],
    [{ method: "GET", path: /^\/api\/agents\/[^/]+\/grants\/agents$/ }, [
      { target_agent_id: "b-002", target_name: "monitoring-agent" },
      { target_agent_id: "c-003", target_name: "logging-agent" },
    ]],
    [{ method: "POST", path: /^\/api\/agents\/[^/]+\/grants\// }, {}],
    [{ method: "DELETE", path: /^\/api\/agents\/[^/]+\/grants\// }, {}],
    [{ method: "PUT", path: /^\/api\/agents\/[^/]+\/owner$/ }, {}],
    [{ method: "GET", path: /^\/api\/search\/users/ }, {
      data: [
        { id: "u-dave", username: "dave", display_name: "Dave N", email: "dave@nasiko.com", role: "team_member", score: 9 },
        { id: "u-dana", username: "dana", display_name: "Dana K", email: "dana@nasiko.com", role: "team_member", score: 7 },
      ],
      query: "da", total_matches: 2, showing: 2,
    }],
    [{ method: "GET", path: /^\/api\/search\/agents/ }, {
      agents: [
        { ...baseAgent, id: "d-004", name: "alerting-agent", display_name: "Alerting agent", score: 8 },
      ],
      total: 1, max_score: 8,
    }],

    // ── Configure: MCP connectors + tools + rules ──
    [{ method: "GET", path: /^\/api\/mcp\/agents\/[^/]+\/connectors$/ }, mcpEnvelope({
      connectors: [
        { connector_id: "mc-github", provider_type: "composio", name: "github", display_name: "GitHub", description: "Repos, issues, and pull requests.", logo_url: "", enabled: true, connected: true },
        { connector_id: "mc-slack", provider_type: "composio", name: "slack", display_name: "Slack", description: "Send messages and read channels.", logo_url: "", enabled: false, connected: true },
        { connector_id: "mc-search", provider_type: "custom", name: "web-search", display_name: "Web Search", description: "Company-hosted search MCP server.", logo_url: "", enabled: true, connected: true },
      ],
    })],
    [{ method: "GET", path: /^\/api\/mcp\/agents\/[^/]+\/connectors\/mc-github\/tools$/ }, mcpEnvelope({
      tools: [
        { name: "create_issue", description: "Open a new issue in a repository.", stance: "allow", last_synced_at: null },
        { name: "merge_pull_request", description: "Merge an open pull request.", stance: "deny", last_synced_at: null },
        { name: "list_repos", description: "List repositories visible to the connected account.", stance: "allow", last_synced_at: null },
      ],
    })],
    [{ method: "GET", path: /^\/api\/mcp\/agents\/[^/]+\/connectors\/mc-slack\/tools$/ }, mcpEnvelope({
      tools: [
        { name: "send_message", description: "Post a message to a channel.", stance: "allow", last_synced_at: null },
        { name: "read_channel", description: "Read recent messages from a channel.", stance: "allow", last_synced_at: null },
      ],
    })],
    [{ method: "GET", path: /^\/api\/mcp\/agents\/[^/]+\/connectors\/mc-search\/tools$/ }, mcpEnvelope({
      tools: [
        { name: "web_search", description: "Search the public web.", stance: "allow", last_synced_at: null },
      ],
    })],
    [{ method: "GET", path: /^\/api\/mcp\/agents\/[^/]+\/tools$/ }, mcpEnvelope({
      rules: [
        { connector_id: "mc-github", tool_pattern: "merge_pull_request", stance: "deny" },
      ],
    })],
    [{ method: "PUT", path: /^\/api\/mcp\/agents\// }, mcpEnvelope({})],

    // ── Configure: LLM router (agent-llm-config self-fetches these) ──
    [{ method: "GET", path: /^\/api\/agents\/[^/]+\/llm-config$/ }, {
      inbound_format: "openai",
      llm_config: {
        provider: "openai",
        model: "gpt-4o-mini",
        temperature: 0.2,
        max_tokens: null,
        fallback_models: ["anthropic/claude-haiku-4-5"],
        api_key_secret_name: "OPENAI_API_KEY",
      },
    }],
    // GET /api/secrets answers with the ApiResponse envelope (see
    // oss/server/src/secrets/routes.rs) — the fixture must match the wire.
    [{ method: "GET", path: /^\/api\/secrets$/ }, {
      status_code: 200,
      message: "Secrets retrieved successfully",
      data: [
        { id: "sec-1", name: "OPENAI_API_KEY", created_at: "2026-06-01T10:00:00Z", updated_at: "2026-07-02T09:12:00Z" },
        { id: "sec-2", name: "SKYCOMMAND_TOKEN", created_at: "2026-06-14T08:30:00Z", updated_at: "2026-06-14T08:30:00Z" },
      ],
    }],

    [{ method: "GET", path: /^\/api\/observability\/agents\/.*\/logs/ }, [
      { timestamp: "2026-07-03T10:00:01Z", level: "info", message: "Agent container started successfully", source: "container" },
      { timestamp: "2026-07-03T10:00:01Z", level: "info", message: "\u001b[2m2026-07-03T10:00:01.766680Z\u001b[0m \u001b[32m INFO\u001b[0m \u001b[2mnasiko_devops_agent\u001b[0m\u001b[2m:\u001b[0m DevOps Engineer listening on 0.0.0.0:8000", source: "container" },
      { timestamp: "2026-07-03T10:00:02Z", level: "warn", message: "\u001b[2m2026-07-03T10:00:02.100121Z\u001b[0m \u001b[33m WARN\u001b[0m \u001b[2mnasiko_devops_agent::llm\u001b[0m\u001b[2m:\u001b[0m \u001b[1mOPENAI_BASE_URL not set\u001b[0m, falling back to default", source: "container" },
      { timestamp: "2026-07-03T10:00:02Z", level: "info", message: "Listening on 0.0.0.0:8080", source: "container" },
      { timestamp: "2026-07-03T10:00:03Z", level: "debug", message: "Loading model weights from /models/nomic-embed-text", source: "container" },
      { timestamp: "2026-07-03T10:00:05Z", level: "info", message: "Model loaded in 2.1s, ready to serve requests", source: "container" },
      { timestamp: "2026-07-03T10:00:12Z", level: "info", message: "POST /a2a 200 - 312ms", source: "container" },
      { timestamp: "2026-07-03T10:00:14Z", level: "info", message: "POST /a2a 200 - 289ms", source: "container" },
      { timestamp: "2026-07-03T10:00:18Z", level: "warn", message: "Request latency exceeds threshold: 1200ms > 1000ms", source: "container" },
      { timestamp: "2026-07-03T10:00:20Z", level: "info", message: "POST /a2a 200 - 445ms", source: "container" },
      { timestamp: "2026-07-03T10:00:25Z", level: "error", message: "Connection to upstream LLM timed out after 30s", source: "container" },
      { timestamp: "2026-07-03T10:00:26Z", level: "info", message: "Retrying LLM request (attempt 2/3)", source: "container" },
      { timestamp: "2026-07-03T10:00:28Z", level: "info", message: "POST /a2a 200 - 2100ms (retry succeeded)", source: "container" },
      { timestamp: "2026-07-03T10:00:30Z", level: "debug", message: "GC pause: 12ms, heap: 128MB/256MB", source: "container" },
      { timestamp: "2026-07-03T10:00:35Z", level: "info", message: "POST /a2a 200 - 310ms", source: "container" },
      { timestamp: "2026-07-03T10:00:40Z", level: "info", message: "Health check passed: all subsystems OK", source: "container" },
      { timestamp: "2026-07-03T10:00:45Z", level: "info", message: "POST /a2a 200 - 278ms", source: "container" },
    ]],
  ],
  scenarios: {
    // Container CPU / memory / network — sits below Quick performance, so scroll
    // it into view or the capture stops at the fold.
    "resource-usage": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.waitForSelector('#acp-resources .acp-stat-value', { timeout: 5000 });
      await page.$eval('#acp-resources', (el) => el.scrollIntoView({ block: 'center' }));
      await page.waitForTimeout(300);
    },
    "overview": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.waitForSelector('.acp-stat-value', { timeout: 5000 });
    },
    "viewer": async (page) => {
      // Non-manager: only Overview + Logs tabs, no topbar actions.
      await gotoAgent(page, 'a-viewer');
      await page.waitForSelector('.acp-stat-value', { timeout: 5000 });
    },
    "json-view": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.click('.acp-json-toggle');
      await page.waitForSelector('#acp-json-view:not([hidden])');
    },
    "owner-access": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.click('[data-tab="access"]');
      await page.waitForSelector('.acp-table', { timeout: 5000 });
    },
    "owner-access-grant-modal": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.click('[data-tab="access"]');
      await page.waitForSelector('#acp-grant-open', { timeout: 5000 });
      await page.click('#acp-grant-open');
      await page.waitForSelector('#acp-grant-modal dialog[open]', { timeout: 5000 });
      await page.fill('#acp-grant-query', 'da');
      await page.waitForSelector('.acp-picker-option', { timeout: 5000 });
    },
    "owner-configure": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.click('[data-tab="configure"]');
      await page.waitForSelector('.acp-mcp-card', { timeout: 5000 });
      // Expand the first (enabled) and second (disabled) connectors.
      const toggles = await page.$$('.acp-mcp-toggle-open');
      await toggles[0].click();
      await page.waitForSelector('.acp-mcp-tool', { timeout: 5000 });
      const toggles2 = await page.$$('.acp-mcp-toggle-open');
      await toggles2[1].click();
      await page.waitForSelector('.acp-mcp-tool.is-dim', { timeout: 5000 });
    },
    "owner-settings": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.click('[data-tab="settings"]');
      await page.waitForSelector('[data-panel="settings"].is-active');
      await page.waitForSelector('secrets-manager .sm-row', { timeout: 5000 });
    },
    // Secrets live far down the Settings panel — scroll them into view so the
    // shared <secrets-manager> is actually visible in a shot.
    "owner-settings-secrets": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.click('[data-tab="settings"]');
      await page.waitForSelector('secrets-manager .sm-row', { timeout: 5000 });
      await page.$eval('secrets-manager', (el) => el.scrollIntoView({ block: 'center' }));
      await page.waitForTimeout(300);
    },
    "logs-tab": async (page) => {
      await gotoAgent(page, 'a-001');
      await page.click('[data-tab="logs"]');
      await page.waitForSelector('[data-panel="logs"].is-active');
      await page.waitForSelector('.acp-log-line', { timeout: 5000 });
    },
  },
};
