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

export default {
  fetch: [
    ["POST /api/chat/sessions", { session_id: "s-preview-001", id: "s-preview-001" }],
    [{ method: "POST", path: /^\/api\/chat\/sessions\/.*\/messages$/ }, { ok: true }],
    ["POST /api/orchestrator/a2a", { __stream: a2aStream }],
    ["GET /api/agents?status=running&limit=6", [
      { id: "a1", name: "coding-agent", display_name: "Coding Agent", status: "running" },
      { id: "a2", name: "docs-agent", display_name: "Docs Agent", status: "running" },
      { id: "a3", name: "nutrition-agent", display_name: "Nutrition Agent", status: "running" },
      { id: "a4", name: "research-agent", display_name: "Research Agent", status: "running" },
    ]],
  ],
  scenarios: {
    "sidebar-collapsed": async (page) => {
      await page.click("[data-sidebar-toggle]");
      await page.waitForTimeout(250);
    },
    "user-menu-open": async (page) => {
      await page.click("[data-user-toggle]");
      await page.waitForSelector(".user-dropdown.is-visible");
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
      await page.waitForSelector(".response-content.is-visible", { timeout: 8000 });
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
      await page.waitForSelector(".response-content.is-visible", { timeout: 8000 });
      await page.click("agent-steps .steps-header");
      await page.click("agent-steps .step-summary");
      await page.waitForTimeout(200);
    },
    // Icon-only sidebar rail after clicking the collapse toggle.
    "sidebar-collapsed": async (page) => {
      await page.click("[data-sidebar-toggle]");
      await page.waitForTimeout(400);
    },
  },
};
