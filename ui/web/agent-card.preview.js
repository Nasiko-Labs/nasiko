export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/agents\/[^/]+$/ }, {
      id: "a-001",
      name: "devops-cluster-lifecycle",
      display_name: "Devops cluster lifecycle agent",
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
    [{ method: "GET", path: /^\/api\/agents\/.*\/acl$/ }, {
      unrestricted: false,
      allowed: [
        "b-002-monitoring-agent",
        "c-003-logging-agent",
        "d-004-alerting-agent",
      ],
    }],
    [{ method: "GET", path: /^\/api\/agents\/.*\/visibility$/ }, {
      agent_id: "a-001",
      is_public: false,
      grants: [
        { id: "g-001", agent_id: "a-001", grant_type: "team", grantee_id: "team-platform", granted_by: null, created_at: "2026-06-15T10:00:00Z" },
        { id: "g-002", agent_id: "a-001", grant_type: "team", grantee_id: "team-infra", granted_by: null, created_at: "2026-06-16T10:00:00Z" },
        { id: "g-003", agent_id: "a-001", grant_type: "user", grantee_id: "user-alice", granted_by: null, created_at: "2026-06-17T10:00:00Z" },
        { id: "g-004", agent_id: "a-001", grant_type: "department", grantee_id: "dept-engineering", granted_by: null, created_at: "2026-06-18T10:00:00Z" },
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
    "overview": async (page) => {
      const url = new URL(page.url());
      url.searchParams.set('id', 'a-001');
      await page.goto(url.toString(), { waitUntil: 'networkidle' });
      await page.waitForSelector('.acp-page', { timeout: 5000 });
      await page.waitForSelector('.acp-acl-grid', { timeout: 5000 });
    },
    "access-control": async (page) => {
      const url = new URL(page.url());
      url.searchParams.set('id', 'a-001');
      await page.goto(url.toString(), { waitUntil: 'networkidle' });
      await page.waitForSelector('.acp-acl-grid', { timeout: 5000 });
      await page.evaluate(() => document.querySelector('#acp-acl').scrollIntoView());
    },
    "settings-tab": async (page) => {
      const url = new URL(page.url());
      url.searchParams.set('id', 'a-001');
      await page.goto(url.toString(), { waitUntil: 'networkidle' });
      await page.waitForSelector('[data-tab="settings"]', { timeout: 5000 });
      await page.click('[data-tab="settings"]');
      await page.waitForSelector('[data-panel="settings"].is-active');
    },
    "with-stats": async (page) => {
      const url = new URL(page.url());
      url.searchParams.set('id', 'a-001');
      await page.goto(url.toString(), { waitUntil: 'networkidle' });
      await page.waitForSelector('#acp-stats', { timeout: 5000 });
      await page.evaluate(() => {
        const el = document.querySelector('#acp-stats');
        if (!el) return;
        el.innerHTML = `
          <div class="acp-stat"><div class="acp-stat-label">Total executions</div><div class="acp-stat-value">312</div></div>
          <div class="acp-stat"><div class="acp-stat-label">Total cost</div><div class="acp-stat-value">$0.12</div></div>
          <div class="acp-stat"><div class="acp-stat-label">P50 latency</div><div class="acp-stat-value">980 ms</div></div>
          <div class="acp-stat"><div class="acp-stat-label">P99 latency</div><div class="acp-stat-value">3,200 ms</div></div>
        `;
      });
      await page.waitForSelector('.acp-stat-value');
    },
    "logs-tab": async (page) => {
      const url = new URL(page.url());
      url.searchParams.set('id', 'a-001');
      await page.goto(url.toString(), { waitUntil: 'networkidle' });
      await page.waitForSelector('[data-tab="logs"]', { timeout: 5000 });
      await page.click('[data-tab="logs"]');
      await page.waitForSelector('[data-panel="logs"].is-active');
      await page.waitForSelector('.acp-log-line', { timeout: 5000 });
    },
  },
};
