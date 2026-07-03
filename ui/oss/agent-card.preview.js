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
  ],
  scenarios: {
    "settings-tab": async (page) => {
      await page.click('[data-tab="settings"]');
      await page.waitForSelector('[data-panel="settings"].is-active');
    },
  },
};
