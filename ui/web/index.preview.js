// Orchestrator page fixtures
const evt = (obj) => `data: ${JSON.stringify({ result: obj })}\n\n`;
const dataMsg = (data) => ({ status: { state: "TASK_STATE_WORKING", message: { parts: [{ data }] } } });

const answer =
  "Here's what I found after checking the deployment:\n\n" +
  "1. The system is currently **healthy**\n" +
  "2. All agents are responding normally\n" +
  "3. No pending deployments detected\n\n" +
  "| Agent | Status | Latency |\n| --- | --- | --- |\n| coding-agent | running | 42ms |\n| devops-agent | running | 51ms |\n";

const a2aStream = [
  { text: evt({ statusUpdate: { status: { state: "TASK_STATE_WORKING" } } }), delay: 50 },
  { text: evt({ statusUpdate: dataMsg({ type: "trace_meta", trace_id: "trace-preview-001" }) }), delay: 30 },
  { text: evt({ statusUpdate: dataMsg({ type: "thinking", content: "Deciding which agents can help..." }) }), delay: 150 },
  { text: evt({ statusUpdate: dataMsg({ type: "tool_call", agent: "coding-agent", message: "Check the failing deployment logs for service `api`", turn: 1 }) }), delay: 150 },
  { text: evt({ statusUpdate: dataMsg({ type: "sub_status", agent: "coding-agent", message: "Fetching container logs..." }) }), delay: 150 },
  { text: evt({ statusUpdate: dataMsg({ type: "sub_content", agent: "coding-agent", content: "Scanning last 200 log lines... found 3 OOMKilled events." }) }), delay: 150 },
  { text: evt({ statusUpdate: dataMsg({ type: "tool_result", agent: "coding-agent", result: "Scanning last 200 log lines... found 3 OOMKilled events.\nRoot cause: memory limit 128Mi too low for JVM workload.\nRecommend 512Mi.", success: true, turn: 1 }) }), delay: 150 },
  { text: evt({ statusUpdate: dataMsg({ type: "tool_call", agent: "devops-agent", message: "Verify cluster capacity for a 512Mi bump", turn: 2 }) }), delay: 150 },
  { text: evt({ statusUpdate: dataMsg({ type: "tool_result", agent: "devops-agent", result: "Cluster has 12Gi free; safe to raise the limit.", success: true, turn: 2 }) }), delay: 200 },
  { text: evt({ artifactUpdate: { artifact: { parts: [{ text: answer }] }, append: false } }), delay: 200 },
  { text: evt({ statusUpdate: { status: { state: "TASK_STATE_COMPLETED", message: { parts: [{ text: answer }] } } } }), delay: 50 },
];

// The workflows and executions views absorbed into this page (see index.html).
// MAF list endpoints answer {data:{data:[...],total},status_code,message}.
const wfStep = (i, agent, task) => ({
  step_id: `s-${agent}-${i}`, step_index: i, agent_id: `a-00${i + 1}`,
  agent_name: agent, agent_endpoint: `http://agents/${agent}`, task_description: task,
});

const mafWorkflows = [
  {
    id: "wf-001",
    name: "Social media content pipeline",
    description: "Generate social media content, review it, and publish approved posts every weekday.",
    maf_json: {
      description: "Generate, review and publish approved posts.",
      steps: [
        wfStep(0, "research-agent", "Fetch the latest campaign brief and brand tone guide"),
        wfStep(1, "content-writer", "Generate three caption variations from the brief"),
        wfStep(2, "review-agent", "Evaluate captions for tone, grammar and alignment"),
      ],
      output_generation: "Summarise which captions were approved and when they ship.",
    },
    status: "active", execution_count: 126,
    created_at: "2026-06-10T09:00:00Z", updated_at: "2026-08-05T09:00:00Z",
  },
  {
    id: "wf-002",
    name: "Agent onboarding pipeline",
    description: "Provision, smoke-test and register newly deployed agents.",
    maf_json: {
      description: "Provision, smoke-test and register a new agent.",
      steps: [
        wfStep(0, "devops-agent", "Provision the container and attach secrets"),
        wfStep(1, "qa-agent", "Run the smoke-test suite against the new endpoint"),
      ],
      output_generation: "Report whether the agent is registered and healthy.",
    },
    status: "active", execution_count: 12,
    created_at: "2026-07-01T09:00:00Z", updated_at: "2026-08-01T09:00:00Z",
  },
  {
    id: "wf-003",
    name: "Nightly cost rollup",
    description: "Aggregate per-agent token spend and post the daily summary.",
    maf_json: {
      description: "Roll up yesterday's spend.",
      steps: [wfStep(0, "finance-bot", "Aggregate token usage and cost by agent")],
      output_generation: "Post the rollup to the finance channel.",
    },
    status: "paused", execution_count: 43,
    created_at: "2026-05-02T09:00:00Z", updated_at: "2026-07-20T09:00:00Z",
  },
];

const mafExecutions = [
  { id: "ex-90", execution_number: 90, maf_id: "wf-001", status: "success", workflow_name: "Social media content pipeline", workflow_status: "active", created_at: "2026-08-07T10:00:00Z", completed_at: "2026-08-07T10:02:14Z" },
  { id: "ex-89", execution_number: 89, maf_id: "wf-001", status: "running", workflow_name: "Social media content pipeline", workflow_status: "active", created_at: "2026-08-07T09:30:00Z", completed_at: null },
  { id: "ex-88", execution_number: 88, maf_id: "wf-002", status: "failed", workflow_name: "Agent onboarding pipeline", workflow_status: "active", created_at: "2026-08-06T18:30:00Z", completed_at: "2026-08-06T18:31:02Z" },
  { id: "ex-87", execution_number: 87, maf_id: "wf-003", status: "success", workflow_name: "Nightly cost rollup", workflow_status: "paused", created_at: "2026-08-06T02:00:00Z", completed_at: "2026-08-06T02:00:48Z" },
];

const mafEnvelope = (rows) => ({ data: { data: rows, total: rows.length }, status_code: 200, message: "ok" });

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/maf\/workflows/ }, mafEnvelope(mafWorkflows)],
    [{ method: "GET", path: /^\/api\/maf\/executions/ }, mafEnvelope(mafExecutions)],
    ["POST /api/chat/sessions", { session_id: "s-preview-001", id: "s-preview-001" }],
    [{ method: "POST", path: /^\/api\/chat\/sessions\/.*\/messages$/ }, { ok: true }],
    ["POST /api/orchestrator/a2a", { __stream: a2aStream }],
    ["GET /api/agents?status=running&limit=6", [
      { id: "a1", name: "coding-agent", display_name: "Coding Agent", status: "running", description: "Writes, reviews and debugs code across languages." },
      { id: "a2", name: "docs-agent", display_name: "Docs Agent", status: "running", description: "Answers questions from your internal documentation." },
      { id: "a3", name: "nutrition-agent", display_name: "Nutrition Agent", status: "running", description: "Meal planning and nutrition breakdowns." },
      { id: "a4", name: "research-agent", display_name: "Research Agent", status: "running", description: "Deep research with cited web sources." },
    ]],
  ],
  scenarios: {
    // ── The module's views ──────────────────────────────────────────────────
    // Opened the way a shared link does (`?view=`), which also proves the shell
    // honours the param on load and not only on a nav click.
    "view-workflows": async (page) => {
      await page.goto(`${page.url().split("?")[0]}?view=workflows`);
      await page.waitForSelector("workflows-page .page-head");
      await page.waitForTimeout(400);
    },
    "view-executions": async (page) => {
      await page.goto(`${page.url().split("?")[0]}?view=executions`);
      await page.waitForSelector("executions-page .page-title");
      await page.waitForTimeout(400);
    },
    // Switching through the nested sidebar. Two things to check here: the
    // sidebar is identical to the default capture, and the orchestrate view's
    // clipped, height-capped box has given way to one that scrolls normally.
    "view-switch-click": async (page) => {
      await page.waitForSelector(".input-wrap");
      await page.click('app-module-nav [data-section="workflows"]');
      await page.waitForSelector("workflows-page .page-head");
      await page.waitForTimeout(400);
    },
    // Labeled sidebar after clicking the topbar rail toggle.
    "rail-expanded": async (page) => {
      await page.click("[data-rail-toggle]");
      await page.waitForTimeout(500);
    },
    "user-menu-open": async (page) => {
      await page.click("[data-user-toggle]");
      await page.waitForSelector(".user-dropdown.is-visible");
    },
    // Clicks the identity ROW (its centre lands on the name, not the avatar) —
    // fails if the trigger ever shrinks back to the 32px avatar.
    "rail-expanded-user-menu": async (page) => {
      await page.click("[data-rail-toggle]");
      await page.waitForTimeout(500);
      await page.click(".rail-identity");
      await page.waitForSelector(".user-dropdown.is-visible");
    },
    // The timeline's full vocabulary in one shot, driven directly through
    // onEvent() so it doesn't depend on stream timing: an agent whose tools
    // arrive as STRUCTURED data parts (nested rows with JSON input/output),
    // a second agent that only sends PROSE status (the seed-agent case, which
    // renders as an activity log), a still-running tool, and a policy block.
    "steps-vocabulary": async (page) => {
      await page.evaluate(async () => {
        // Mount inside the page's own content card — appending to <body> puts
        // the timeline on the ink shell, where main-text rows go invisible.
        const page_ = document.querySelector("orchestrator-page");
        page_.replaceChildren();
        const host = document.createElement("div");
        host.style.cssText = "max-width:860px;margin:0 auto";
        page_.appendChild(host);
        await import("/common/components/agent-steps.js");
        const steps = document.createElement("agent-steps");
        host.appendChild(steps);

        steps.onEvent({ type: "thinking", content: "The deploy is failing. I need the **container logs** first, then cluster capacity." });

        steps.onEvent({ type: "tool_call", agent: "devops-agent", turn: 1, message: "Check the failing deployment logs for service `api`" });
        steps.onEvent({ type: "tool_call_started", agent: "devops-agent", tool_name: "get_logs", id: "t1",
          arguments: { service: "api", lines: 200, since: "15m" } });
        steps.onEvent({ type: "tool_call_result", agent: "devops-agent", tool_name: "get_logs", id: "t1", duration_ms: 412,
          result: "found 3 OOMKilled events\nmemory limit 128Mi too low for JVM workload" });
        steps.onEvent({ type: "tool_call_started", agent: "devops-agent", tool_name: "web_search", id: "t2",
          arguments: { query: "JVM container OOMKilled 128Mi heap sizing" } });
        steps.onEvent({ type: "tool_call_result", agent: "devops-agent", tool_name: "web_search", id: "t2", duration_ms: 980,
          result: { results: 12, top: "Set -XX:MaxRAMPercentage=75 and raise the limit to 512Mi" } });
        steps.onEvent({ type: "tool_result", agent: "devops-agent", turn: 1, success: true, duration_ms: 2140,
          result: "Root cause: memory limit 128Mi too low for the JVM workload.\n\nRecommend **512Mi**." });

        // Prose-only agent: no structured parts, so the log is what it sent.
        steps.onEvent({ type: "tool_call", agent: "coding-agent", turn: 2, message: "Patch the deployment manifest to 512Mi" });
        steps.onEvent({ type: "sub_status", agent: "coding-agent", message: "reading k8s/api/deployment.yaml" });
        steps.onEvent({ type: "sub_status", agent: "coding-agent", message: "editing resources.limits.memory" });
        steps.onEvent({ type: "sub_content", agent: "coding-agent", content: "Updated `resources.limits.memory` to `512Mi`." });

        // Still running, plus a policy rejection.
        steps.onEvent({ type: "tool_call", agent: "qa-agent", turn: 3, message: "Re-run the deployment smoke tests" });
        steps.onEvent({ type: "policy_rejected", agent: "billing-agent", turn: 4, reason: "MaxFanOutExceeded: flow already fanned out to 4 agents (limit 4)" });
      });
      await page.waitForSelector("agent-steps .step--tool");
      await page.waitForTimeout(500);
    },
    // Nothing deployed: the whole composer column is the deploy empty state.
    "no-agents": async (page) => {
      await page.evaluate(() => {
        const real = window.fetch;
        window.fetch = (url, opts) =>
          String(url).includes("/api/agents")
            ? Promise.resolve(new Response("[]", { headers: { "content-type": "application/json" } }))
            : real(url, opts);
        document
          .querySelector("orchestrator-page")
          .replaceWith(document.createElement("orchestrator-page"));
      });
      await page.waitForSelector("app-empty-state .title");
    },
    // Mid-stream: tool-call steps expanded and running.
    "streaming-steps": async (page) => {
      await page.fill("#textarea", "Why is my deployment failing?");
      await page.click("#submitBtn");
      await page.waitForSelector("agent-steps .step", { timeout: 5000 });
      await page.waitForTimeout(600);
    },
    // Stream finished: steps collapsed into summary, markdown answer visible.
    "streamed-response": async (page) => {
      await page.fill("#textarea", "Why is my deployment failing?");
      await page.click("#submitBtn");
      // `.stream-content` is what #readStream renders the reply into. This waited
      // on `.response-content.is-visible`, a class no component has — the
      // scenario had been failing on a stale selector.
      await page.waitForSelector(".stream-content", { timeout: 8000 });
      await page.waitForTimeout(300);
    },
    // Completed response hovered: copy + trace toolbar visible.
    "response-hover-actions": async (page) => {
      await page.fill("#textarea", "Why is my deployment failing?");
      await page.click("#submitBtn");
      await page.waitForSelector(".response-content.is-visible", { timeout: 8000 });
      await page.hover(".response-content");
      await page.waitForTimeout(200);
    },
    // Steps summary re-expanded after completion, first call opened.
    "steps-expanded": async (page) => {
      await page.fill("#textarea", "Why is my deployment failing?");
      await page.click("#submitBtn");
      await page.waitForSelector("agent-steps.is-done", { timeout: 8000 });
      await page.click("agent-steps .steps-header");
      await page.click("agent-steps .step-summary");
      await page.waitForTimeout(200);
    },
  },
};
