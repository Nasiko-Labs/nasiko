// Session detail fixtures — session/{id}, trace/{id}, span/{t}/{s}, chat transcript
// (shapes from oss/server/src/observability/service.rs + chat/models.rs; see /api/docs).

const SESSION_ID = "ses_18a5801d3353463ca39ebc216887f385";
const TRACE_ID = "8a880df26caf4c12a0e2d5f898f49420";

// Re-load the page with ?session_id=… when the harness opened it bare.
const withSession = async (page) => {
  if (page.url().includes("session_id=")) return;
  const u = new URL(page.url());
  u.searchParams.set("session_id", SESSION_ID);
  await page.goto(u.toString());
  await page.waitForSelector(".pane-title");
  await page.waitForTimeout(300);
};

const mkSpan = (spanId, name, kind, latency, tokens, model, children = []) => ({
  id: btoa(spanId),
  span_id: spanId,
  name,
  span_kind: kind,
  status_code: "OK",
  start_time: new Date(Date.now() - 40 * 60 * 1000).toISOString(),
  end_time: new Date(Date.now() - 40 * 60 * 1000 + latency).toISOString(),
  parent_id: null,
  latency_ms: latency,
  token_count_total: tokens,
  input_tokens: Math.round(tokens * 0.7),
  output_tokens: Math.round(tokens * 0.3),
  model,
  span_annotation_summaries: [],
  children,
});

const spanTree = mkSpan("span-root", "a2a.execute", "agent", 15620, 6705, null, [
  mkSpan("span-cc1", "ChatCompletion", "internal", 2110, 1411, "gpt-4o"),
  mkSpan("span-cc2", "ChatCompletion", "internal", 4320, 2350, "gpt-4o"),
  mkSpan("span-cc3", "ChatCompletion", "internal", 6950, 2944, "gpt-4o"),
]);

const spanDetail = (spanId, name) => ({
  data: {
    span: {
      id: btoa(spanId),
      span_id: spanId,
      trace: { id: btoa(TRACE_ID), trace_id: TRACE_ID },
      name,
      span_kind: name === "a2a.execute" ? "agent" : "internal",
      status_code: "OK",
      code: "OK",
      status_message: "",
      start_time: new Date(Date.now() - 40 * 60 * 1000).toISOString(),
      end_time: new Date(Date.now() - 40 * 60 * 1000 + 2111).toISOString(),
      parent_id: name === "a2a.execute" ? null : btoa("span-root"),
      latency_ms: 2111,
      token_count_total: 1411,
      cost_summary: { total: { cost: 0.0004 } },
      // The real wire shape, verified against the span-detail endpoint on a live
      // deployment (cp.nasiko.dev, 2026-08-10): the server resolves the message
      // content into `input.value`/`output.value` (a JSON *string* in the GenAI
      // semconv `parts[]` form), and `attributes` is RE-NESTED from the dotted
      // OTLP keys by unflatten_attrs (oss/server/src/observability/service.rs) —
      // `attributes.gen_ai.input.messages`, never a flat
      // `attributes["gen_ai.input.messages"]`. Two prior fixture shapes (nested
      // `llm.input_messages` arrays, then flat dotted keys) each matched a UI
      // build that worked in preview and broke against production. Keep this
      // mirroring the endpoint, not the UI.
      input: {
        value: JSON.stringify([
          { role: "system", parts: [{ type: "text", content: "You are the Nasiko orchestrator. Route user queries to the best agent." }] },
          { role: "user", parts: [{ type: "text", content: "Hello, what can you do?" }] },
        ]),
        mime_type: "json",
      },
      output: {
        value: JSON.stringify([
          {
            role: "assistant",
            parts: [
              { type: "text", content: "Hello! I'm an orchestrator that can help you with a variety of tasks by delegating to specialized agents:\n\n- Route coding questions to the coding agent\n- Answer research questions via the research agent\n- Manage deployments through the devops agent" },
            ],
          },
        ]),
        mime_type: "json",
      },
      attributes: {
        gen_ai: {
          operation: { name: "chat" },
          request: { model: "gpt-4o" },
          usage: { input_tokens: "987", output_tokens: "424" },
          input: {
            messages: JSON.stringify([
              { role: "system", parts: [{ type: "text", content: "You are the Nasiko orchestrator. Route user queries to the best agent." }] },
              { role: "user", parts: [{ type: "text", content: "Hello, what can you do?" }] },
            ]),
          },
          output: {
            messages: JSON.stringify([
              {
                role: "assistant",
                parts: [
                  { type: "text", content: "Hello! I'm an orchestrator that can help you with a variety of tasks by delegating to specialized agents:\n\n- Route coding questions to the coding agent\n- Answer research questions via the research agent\n- Manage deployments through the devops agent" },
                ],
              },
            ]),
          },
        },
      },
      events: [],
      span_annotations: [],
      span_annotation_summaries: [],
      document_retrieval_metrics: [],
      document_evaluations: [],
      project: { id: "orchestrator", annotation_configs: [] },
    },
  },
});

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/observability\/session\/ses_/ }, {
      data: {
        session: {
          id: btoa(SESSION_ID),
          session_id: SESSION_ID,
          num_traces: 1,
          token_usage: { total: 6705 },
          cost_summary: { total: { cost: 0.001 }, prompt: { cost: 0.0007 }, completion: { cost: 0.0003 } },
          latency_p50: 15620,
          latency_p99: 15620,
          traces: [{
            id: btoa(TRACE_ID),
            trace_id: TRACE_ID,
            cursor: "c1",
            root_span: {
              id: btoa("span-root"),
              span_id: "span-root",
              attributes: "{}",
              cumulative_token_count_total: 6705,
              latency_ms: 15620,
              start_time: new Date(Date.now() - 40 * 60 * 1000).toISOString(),
              span_annotations: [],
              span_annotation_summaries: [],
              project: { id: "orchestrator" },
              input: { value: "Hello, what can you do?", mime_type: "text/plain" },
              output: { value: "Hello! I'm an orchestrator...", mime_type: "text/plain" },
              trace: { id: btoa(TRACE_ID), cost_summary: { total: { cost: 0.001 } } },
            },
          }],
          pagination: { end_cursor: null, has_next_page: false },
        },
      },
    }],
    [{ method: "GET", path: /^\/api\/observability\/trace\// }, {
      data: {
        trace: {
          id: btoa(TRACE_ID),
          project_session_id: SESSION_ID,
          num_spans: 4,
          latency_ms: 15620,
          cost_summary: { total: { cost: 0.001 }, prompt: { cost: 0.0007 }, completion: { cost: 0.0003 } },
          root_spans: { edges: [{ span: { id: btoa("span-root"), span_id: "span-root", parent_id: null, status_code: "OK" } }] },
          spans: [spanTree],
          span_lookup: {},
        },
      },
    }],
    [{ method: "GET", path: /^\/api\/observability\/span\/[^/]+\/span-root$/ }, spanDetail("span-root", "a2a.execute")],
    // One static entry per span: function fixtures are serialized into the
    // page and lose module-scope closures (spanDetail would be undefined).
    [{ method: "GET", path: /^\/api\/observability\/span\/[^/]+\/span-cc1$/ }, spanDetail("span-cc1", "ChatCompletion")],
    [{ method: "GET", path: /^\/api\/observability\/span\/[^/]+\/span-cc2$/ }, spanDetail("span-cc2", "ChatCompletion")],
    [{ method: "GET", path: /^\/api\/observability\/span\/[^/]+\/span-cc3$/ }, spanDetail("span-cc3", "ChatCompletion")],
    [{ method: "GET", path: /^\/api\/chat\/sessions\/ses_/ }, {
      data: [
        { id: "m1", session_id: SESSION_ID, role: "user", content: "Hello, what can you do?", has_file_parts: false, timestamp: new Date(Date.now() - 40 * 60 * 1000).toISOString() },
        { id: "m2", session_id: SESSION_ID, role: "assistant", content: "Hello! I'm an orchestrator that can help you with a variety of tasks by delegating to specialized agents. Here's what I can do:", has_file_parts: false, timestamp: new Date(Date.now() - 39 * 60 * 1000).toISOString() },
      ],
    }],
  ],
  scenarios: {
    // The page reads session_id from the query; the harness loads the bare
    // URL (and never runs a scenario literally named "default"), so the
    // populated state is captured via this named scenario and every other
    // scenario first navigates with the id.
    "with-session": async (page) => { await withSession(page); },
    "chat-collapsed": async (page) => {
      await withSession(page);
      await page.waitForSelector(".pane-collapse");
      await page.click("#chat-pane .pane-collapse");
      await page.waitForTimeout(200);
    },
    "attributes-tab": async (page) => {
      await withSession(page);
      await page.waitForSelector(".tab-btn");
      await page.click('.tab-btn[data-tab="attributes"]');
      await page.waitForSelector(".raw-json");
      await page.waitForTimeout(300);
    },
    // A session the trace backend has nothing for: the span-detail pane folds
    // away and one empty state covers the width.
    "no-traces": async (page) => {
      await withSession(page);
      await page.evaluate(() => {
        window.fetchObservabilitySession = async () => ({
          data: { session: { session_id: "ses_empty", num_traces: 0, token_usage: { total: 0 }, cost_summary: { total: { cost: 0 } }, latency_p50: 0, latency_p99: 0, traces: [] } },
        });
        document.querySelector("observability-session-page").remove();
        document.body.appendChild(document.createElement("observability-session-page"));
      });
      await page.waitForSelector(".panes.traces-empty app-empty-state");
      await page.waitForTimeout(200);
    },
    // Neither traces nor a chat transcript — a single full-width empty state.
    "no-data": async (page) => {
      await withSession(page);
      await page.evaluate(() => {
        window.fetchObservabilitySession = async () => ({
          data: { session: { session_id: "ses_empty", num_traces: 0, token_usage: { total: 0 }, cost_summary: { total: { cost: 0 } }, latency_p50: 0, latency_p99: 0, traces: [] } },
        });
        window.fetchChatSession = async () => ({ data: [] });
        document.querySelector("observability-session-page").remove();
        document.body.appendChild(document.createElement("observability-session-page"));
      });
      await page.waitForSelector(".panes.traces-empty.chat-empty");
      await page.waitForTimeout(200);
    },
    // Long markdown reply: renders as markdown and clamps behind Show more.
    "long-chat": async (page) => {
      await withSession(page);
      await page.evaluate(() => {
        window.fetchChatSession = async () => ({
          data: [
            { id: "m1", role: "user", content: "Walk me through the deploy rollback procedure." },
            { id: "m2", role: "assistant", content: "## Rollback procedure\n\nRolling back is a **three step** operation:\n\n1. Freeze the build queue\n2. Re-point the service to the previous image\n3. Verify health, then unfreeze\n\n### Commands\n\n```bash\nnasiko ps --agent devops-agent\nnasiko deploy devops-agent --image devops-agent:0.3.0\nnasiko logs devops-agent --follow\n```\n\n### Verification checklist\n\n| Check | Expected |\n| --- | --- |\n| `/health` | `ok` |\n| Replicas | matches previous |\n| Error rate | back to baseline |\n\nIf the health check does not settle within five minutes, escalate to the on-call\nplatform engineer and leave the build queue frozen. Do *not* retry the rollback\nautomatically — a second image flip while the first is still converging will make\nthe deployment history ambiguous and complicate the incident review.\n\n> Note: rollbacks do not revert database migrations. Any migration applied by the\n> bad release must be reverted separately.\n" },
          ],
        });
        document.querySelector("observability-session-page").remove();
        document.body.appendChild(document.createElement("observability-session-page"));
      });
      await page.waitForSelector(".msg-more");
      await page.waitForTimeout(200);
    },
    "tool-span-selected": async (page) => {
      await withSession(page);
      await page.waitForSelector(".span-row");
      const rows = await page.$$(".span-row");
      if (rows[2]) await rows[2].click();
      await page.waitForTimeout(300);
    },
  },
};
