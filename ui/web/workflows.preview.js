// Workflows library fixtures — GET /api/maf/workflows + /api/maf/executions
// (envelope {data:{data:[...],total},status_code,message}).
const ago = (mins) => new Date(Date.now() - mins * 60_000).toISOString();

const step = (i, agentId, agentName, task) => ({
  step_id: `s-${agentId}-${i}`,
  step_index: i,
  agent_id: agentId,
  agent_name: agentName,
  agent_endpoint: `http://agents/${agentName}`,
  task_description: task,
});

const workflows = [
  {
    id: "wf-001",
    name: "Social media content pipeline",
    description: "Generate social media content, review it, and publish approved posts every weekday.",
    maf_json: {
      description: "Generate social media content, review it, and publish approved posts every weekday.",
      steps: [
        step(0, "a-002", "research-agent", "Fetch the latest campaign brief and brand tone guide"),
        step(1, "a-001", "content-writer", "Generate three caption variations using the campaign brief"),
        step(2, "a-004", "review-agent", "Evaluate captions for tone, grammar, and campaign alignment"),
        step(3, "a-006", "publishing-agent", "Queue approved captions for publishing and record the schedule"),
      ],
      output_generation: "Summarise which captions were approved and when they are scheduled.",
    },
    status: "active",
    created_at: ago(60 * 24 * 21),
    updated_at: ago(60 * 24 * 2),
    execution_count: 126,
  },
  {
    id: "wf-002",
    name: "Lead enrichment pipeline",
    description: "Enrich incoming leads with company data and assign a priority score.",
    maf_json: {
      description: "Enrich incoming leads with company data and assign a priority score.",
      steps: [
        step(0, "a-002", "research-agent", "Look up company data for each incoming lead"),
        step(1, "a-005", "scoring-agent", "Assign a priority score from firmographics and intent"),
        step(2, "a-003", "crm-agent", "Write the enriched lead and score back to the CRM"),
      ],
    },
    status: "active",
    created_at: ago(60 * 24 * 12),
    updated_at: ago(60 * 24 * 5),
    execution_count: 48,
  },
  {
    id: "wf-003",
    name: "Onboarding email sequence",
    description: "Send onboarding emails to new users based on signup data.",
    maf_json: {
      description: "Send onboarding emails to new users based on signup data.",
      steps: [
        step(0, "a-001", "content-writer", "Draft a personalised welcome email from the signup profile"),
        step(1, "a-006", "publishing-agent", "Schedule the email sequence over the first week"),
      ],
    },
    status: "active",
    created_at: ago(60 * 24 * 3),
    updated_at: ago(60 * 24 * 3),
    execution_count: 0,
  },
];

const executions = [
  {
    id: "ex-201", execution_number: 126, maf_id: "wf-001", status: "success",
    tokens_used: 2140, started_at: ago(125), completed_at: ago(122), duration_ms: 138_000,
    step_results: null, error: null, created_at: ago(125),
    workflow_name: "Social media content pipeline", workflow_status: "active",
  },
  {
    id: "ex-105", execution_number: 48, maf_id: "wf-002", status: "failed",
    tokens_used: 188, started_at: ago(20), completed_at: ago(19), duration_ms: 31_000,
    step_results: null, error: "step 2 failed: ETIMEDOUT after 30000ms", created_at: ago(20),
    workflow_name: "Lead enrichment pipeline", workflow_status: "active",
  },
];

const paged = (rows) => ({ data: { data: rows, total: rows.length }, status_code: 200, message: "ok" });

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/maf\/workflows/ }, paged(workflows)],
    [{ method: "GET", path: /^\/api\/maf\/executions/ }, paged(executions)],
  ],
  scenarios: {
    empty: async (page) => {
      await page.evaluate(() => {
        window.fetchWorkflows = async () => [];
        window.fetchAllExecutions = async () => [];
        document.querySelector("workflows-page").remove();
        document.body.appendChild(document.createElement("workflows-page"));
      });
      await page.waitForSelector(".wf-empty");
      await page.waitForTimeout(300);
    },
  },
};
