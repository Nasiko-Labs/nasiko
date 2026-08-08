// All-executions fixtures — GET /api/maf/executions rows include
// workflow_name/workflow_status and snapshotted step_results.
const ago = (mins) => new Date(Date.now() - mins * 60_000).toISOString();
const paged = (rows) => ({ data: { data: rows, total: rows.length }, status_code: 200, message: "ok" });

const stepResult = (i, agentName, task, overrides) => ({
  step_id: `s-${i + 1}`, step_index: i, agent_id: `a-00${i + 1}`, agent_name: agentName,
  status: "pending", error: null, prompt_template: "", to_extract: "",
  prompt: `Using the campaign brief, ${task.toLowerCase()}.`,
  extracted_info: null, tokens_used: 0, latency_ms: 0, context: null, obs_logs: null,
  ...overrides,
});

const pipelineSteps = (states) => [
  ["Research Agent", "Fetch the latest campaign brief"],
  ["Content Writer Agent", "Generate three caption variations"],
  ["Review Writer Agent", "Review tone and grammar"],
  ["Compliance Checker", "Check brand compliance"],
  ["Publishing Agent", "Queue approved posts"],
].map(([agent, task], i) => stepResult(i, agent, task, states[i] || {}));

const executions = [
  {
    id: "ex-164", execution_number: 164, maf_id: "wf-001", status: "running",
    attempt_count: 1, max_attempts: 3, tokens_used: 365,
    started_at: ago(2), completed_at: null, duration_ms: null, output: null, error: null,
    created_at: ago(2), workflow_name: "Social media content pipeline", workflow_status: "active",
    step_results: pipelineSteps([
      { status: "success", extracted_info: "Pulled the August campaign brief and brand tone guide.", tokens_used: 112, latency_ms: 1900 },
      { status: "running" },
    ]),
  },
  {
    id: "ex-163", execution_number: 163, maf_id: "wf-001", status: "success",
    attempt_count: 1, max_attempts: 3, tokens_used: 2140,
    started_at: ago(60 * 26), completed_at: ago(60 * 26 - 2), duration_ms: 118_000,
    output: "Approved two captions and scheduled them for Monday and Wednesday.",
    error: null, created_at: ago(60 * 26),
    workflow_name: "Social media content pipeline", workflow_status: "active",
    step_results: pipelineSteps([0, 1, 2, 3, 4].map((i) => ({
      status: "success", extracted_info: `Step ${i + 1} completed.`, tokens_used: 280 + i * 60, latency_ms: 2000 + i * 700,
    }))),
  },
  {
    id: "ex-162", execution_number: 162, maf_id: "wf-001", status: "failed",
    attempt_count: 3, max_attempts: 3, tokens_used: 188,
    started_at: ago(60 * 50), completed_at: ago(60 * 50 - 1), duration_ms: 31_000,
    output: null, error: "step 2 (Content Writer Agent) failed after 3 attempts: ETIMEDOUT after 30000ms",
    created_at: ago(60 * 50), workflow_name: "Social media content pipeline", workflow_status: "active",
    step_results: pipelineSteps([
      { status: "success", extracted_info: "Pulled the August campaign brief.", tokens_used: 112, latency_ms: 1900 },
      { status: "failed", error: "ETIMEDOUT after 30000ms waiting for upstream response", latency_ms: 30_000 },
    ]),
  },
  {
    id: "ex-101", execution_number: 12, maf_id: "wf-old", status: "failed",
    attempt_count: 1, max_attempts: 3, tokens_used: 204,
    started_at: ago(60 * 24 * 6), completed_at: ago(60 * 24 * 6 - 4), duration_ms: 240_000,
    output: null, error: "workflow definition invalid: agent no longer exists",
    created_at: ago(60 * 24 * 6), workflow_name: null, workflow_status: "deleted",
    step_results: pipelineSteps([{ status: "failed", error: "agent no longer exists" }]).slice(0, 3),
  },
];

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/maf\/executions/ }, paged(executions)],
  ],
  scenarios: {
    history: async (page) => {
      await page.waitForSelector(".run-card");
      await page.click('[data-tab="history"]');
      await page.waitForSelector(".seg-ctrl");
      // Expand the failed run to show its timeline + error.
      await page.click('[data-toggle="ex-162"]');
      await page.waitForTimeout(400);
    },
    empty: async (page) => {
      await page.evaluate(() => {
        window.fetchAllExecutions = async () => [];
        document.querySelector("executions-page").remove();
        document.body.appendChild(document.createElement("executions-page"));
      });
      await page.waitForSelector(".runs-empty");
      await page.waitForTimeout(300);
    },
    "no-active": async (page) => {
      await page.evaluate(() => {
        const finished = {
          id: "ex-9", execution_number: 9, maf_id: "wf-001", status: "success",
          attempt_count: 1, max_attempts: 3, tokens_used: 900,
          started_at: new Date(Date.now() - 3600e3).toISOString(),
          completed_at: new Date(Date.now() - 3500e3).toISOString(),
          duration_ms: 100_000, output: "done", error: null,
          created_at: new Date(Date.now() - 3600e3).toISOString(),
          workflow_name: "Social media content pipeline", workflow_status: "active",
          step_results: [],
        };
        window.fetchAllExecutions = async () => [finished];
        document.querySelector("executions-page").remove();
        document.body.appendChild(document.createElement("executions-page"));
      });
      await page.waitForSelector(".runs-empty");
      await page.waitForTimeout(300);
    },
  },
};
