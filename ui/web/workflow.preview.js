// Workflow detail fixtures — review + live-run + history states.
// Envelope {data, status_code, message}; lists wrap {data:{data:[...],total}}.
const ago = (mins) => new Date(Date.now() - mins * 60_000).toISOString();
const env = (data) => ({ data, status_code: 200, message: "ok" });
const paged = (rows) => env({ data: rows, total: rows.length });

const STEP_DEFS = [
  ["s-1", "a-002", "Research Agent", "Fetch the latest campaign brief and brand tone guide"],
  ["s-2", "a-001", "Content Writer Agent", "Generate three caption variations using the campaign brief"],
  ["s-3", "a-004", "Review Writer Agent", "Review tone and grammar of the generated captions"],
  ["s-4", "a-005", "Compliance Checker", "Check every claim against brand and legal guidelines"],
  ["s-5", "a-006", "Publishing Agent", "Queue approved captions for publishing and record the schedule"],
];

const workflow = {
  id: "wf-001",
  name: "Social media content pipeline",
  description: "Generate social media content, review it, and publish approved posts every weekday.",
  maf_json: {
    description: "Generate social media content, review it, and publish approved posts every weekday.",
    steps: STEP_DEFS.map(([id, agentId, agentName, task], i) => ({
      step_id: id, step_index: i, agent_id: agentId, agent_name: agentName,
      agent_endpoint: `http://agents/${agentId}`, task_description: task,
    })),
    output_generation: "Summarise which captions were approved and when they are scheduled.",
  },
  status: "active",
  created_at: ago(60 * 24 * 21),
  updated_at: ago(60 * 24 * 2),
  execution_count: 164,
};

const neverRunWorkflow = {
  ...workflow,
  id: "wf-003",
  name: "Onboarding email sequence",
  description: "Send onboarding emails to new users based on signup data.",
  execution_count: 0,
};

const stepResult = ([id, agentId, agentName], i, overrides) => ({
  step_id: id, step_index: i, agent_id: agentId, agent_name: agentName,
  status: "pending", error: null, prompt_template: "", to_extract: "",
  prompt: `Using the campaign brief, ${STEP_DEFS[i][3].toLowerCase()}.`,
  extracted_info: null, tokens_used: 0, latency_ms: 0, context: null, obs_logs: null,
  ...overrides,
});

const runningExec = {
  id: "ex-164", execution_number: 164, maf_id: "wf-001", status: "running",
  attempt_count: 1, max_attempts: 3, tokens_used: 365,
  started_at: ago(2), completed_at: null, duration_ms: null, output: null, error: null,
  created_at: ago(2),
  step_results: [
    stepResult(STEP_DEFS[0], 0, { status: "success", extracted_info: "Pulled the August campaign brief and brand tone guide.\n\n- Extracted three product claims cleared by legal\n- Flagged one expired asset for replacement", tokens_used: 112, latency_ms: 1900 }),
    stepResult(STEP_DEFS[1], 1, { status: "running" }),
    stepResult(STEP_DEFS[2], 2), stepResult(STEP_DEFS[3], 3), stepResult(STEP_DEFS[4], 4),
  ],
};

const successExec = {
  id: "ex-163", execution_number: 163, maf_id: "wf-001", status: "success",
  attempt_count: 1, max_attempts: 3, tokens_used: 2140,
  started_at: ago(60 * 26), completed_at: ago(60 * 26 - 2), duration_ms: 118_000,
  output: "## Approved captions\n\n1. **Bright mornings** — scheduled for Monday 09:00\n2. **Weekend recharge** — scheduled for Wednesday 09:00\n\nOne caption was rejected for tone; see the review step for details.",
  error: null, created_at: ago(60 * 26),
  step_results: STEP_DEFS.map((d, i) => stepResult(d, i, {
    status: "success",
    extracted_info: `Completed: ${STEP_DEFS[i][3]}.`,
    tokens_used: 300 + i * 60,
    latency_ms: 2100 + i * 900,
  })),
};

const failedExec = {
  id: "ex-162", execution_number: 162, maf_id: "wf-001", status: "failed",
  attempt_count: 3, max_attempts: 3, tokens_used: 188,
  started_at: ago(60 * 50), completed_at: ago(60 * 50 - 1), duration_ms: 31_000,
  output: null,
  error: "step 2 (Content Writer Agent) failed after 3 attempts: ETIMEDOUT after 30000ms",
  created_at: ago(60 * 50),
  step_results: [
    stepResult(STEP_DEFS[0], 0, { status: "success", extracted_info: "Pulled the August campaign brief and brand tone guide.", tokens_used: 112, latency_ms: 1900 }),
    stepResult(STEP_DEFS[1], 1, { status: "failed", error: "ETIMEDOUT after 30000ms waiting for upstream response", latency_ms: 30_000 }),
    stepResult(STEP_DEFS[2], 2), stepResult(STEP_DEFS[3], 3), stepResult(STEP_DEFS[4], 4),
  ],
};

const executionRows = [runningExec, successExec, failedExec].map((e) => ({
  ...e, step_results: null,
}));

const goto = async (page, search) => {
  const u = new URL(page.url());
  u.search = search;
  await page.goto(u.toString());
};

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/maf\/workflow\/wf-003\/executions/ }, paged([])],
    [{ method: "GET", path: /^\/api\/maf\/workflow\/wf-001\/executions/ }, paged(executionRows)],
    [{ method: "GET", path: /^\/api\/maf\/workflow\/wf-003(\?|$)/ }, env(neverRunWorkflow)],
    [{ method: "GET", path: /^\/api\/maf\/workflow\/wf-001(\?|$)/ }, env(workflow)],
    [{ method: "GET", path: /^\/api\/maf\/execution\/ex-164/ }, env(runningExec)],
    [{ method: "GET", path: /^\/api\/maf\/execution\/ex-163/ }, env(successExec)],
    [{ method: "GET", path: /^\/api\/maf\/execution\/ex-162/ }, env(failedExec)],
    [{ method: "GET", path: /^\/api\/agents/ }, {
      data: [
        { id: "a-001", display_name: "Content Writer Agent" },
        { id: "a-002", display_name: "Research Agent" },
        { id: "a-004", display_name: "Review Writer Agent" },
        { id: "a-005", display_name: "Compliance Checker" },
        { id: "a-006", display_name: "Publishing Agent" },
      ],
      total: 5,
    }],
  ],
  scenarios: {
    review: async (page) => {
      await goto(page, "?id=wf-001");
      await page.waitForSelector("wf-step-editor .step-card");
      await page.waitForTimeout(300);
    },
    "active-run": async (page) => {
      await goto(page, "?id=wf-001&exec=ex-164");
      await page.waitForSelector("wf-run-steps .step");
      await page.waitForTimeout(400);
    },
    history: async (page) => {
      await goto(page, "?id=wf-001&exec=ex-163");
      await page.waitForSelector("#run-output:not([hidden])");
      await page.waitForTimeout(300);
    },
    "failed-run": async (page) => {
      await goto(page, "?id=wf-001&exec=ex-162");
      await page.waitForSelector("#run-error:not([hidden])");
      await page.waitForTimeout(300);
    },
    "never-run": async (page) => {
      await goto(page, "?id=wf-003");
      await page.waitForSelector(".exec-empty");
      await page.waitForTimeout(300);
    },
  },
};
