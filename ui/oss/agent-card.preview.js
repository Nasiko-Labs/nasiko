export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/catalog\/agents/ }, {
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
    [{ method: "GET", path: /^\/api\/observe\/agents\/.*\/stats$/ }, {
      total_requests: 0,
      total_cost: 0.0,
      error_rate: 0,
      avg_latency_ms: 0,
      p50_latency_ms: 0,
      p95_latency_ms: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
      period_start: "2026-07-01T00:00:00Z",
      source: "tempo",
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
  },
};
