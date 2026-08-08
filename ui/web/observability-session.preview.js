// Session detail fixtures — session/{id}, trace/{id}, span/{t}/{s}, chat transcript
// (shapes from oss/server/src/observability/service.rs + chat/models.rs; see /api/docs).

const SESSION_ID = "ses_18a5801d3353463ca39ebc216887f385";
const TRACE_ID = "8a880df26caf4c12a0e2d5f898f49420";

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
      input: { value: JSON.stringify({ messages: [{ role: "user", content: "Hello, what can you do?" }] }), mime_type: "application/json" },
      output: { value: JSON.stringify({ messages: [{ role: "assistant", content: "Hello! I'm an orchestrator that can help you with a variety of tasks by delegating to specialized agents." }] }), mime_type: "application/json" },
      attributes: {
        llm: {
          model_name: "gpt-4o",
          input_messages: [
            { message: { role: "system", content: "You are the Nasiko orchestrator. Route user queries to the best agent." } },
            { message: { role: "user", content: "Hello, what can you do?" } },
          ],
          output_messages: [
            { message: { role: "assistant", content: "Hello! I'm an orchestrator that can help you with a variety of tasks by delegating to specialized agents. Here's what I can do:\n\n- Route coding questions to the coding agent\n- Answer research questions via the research agent\n- Manage deployments through the devops agent" } },
          ],
          invocation_parameters: '{"temperature":0.2,"max_tokens":2048}',
          token_count: { prompt: 987, completion: 424, total: 1411 },
        },
        openinference: { span: { kind: "LLM" } },
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
    [{ method: "GET", path: /^\/api\/observability\/span\/[^/]+\/span-cc\d$/ }, (req) => {
      const spanId = new URL(req.url).pathname.split("/").pop();
      return spanDetail(spanId, "ChatCompletion");
    }],
    [{ method: "GET", path: /^\/api\/chat\/sessions\/ses_/ }, {
      data: [
        { id: "m1", session_id: SESSION_ID, role: "user", content: "Hello, what can you do?", has_file_parts: false, timestamp: new Date(Date.now() - 40 * 60 * 1000).toISOString() },
        { id: "m2", session_id: SESSION_ID, role: "assistant", content: "Hello! I'm an orchestrator that can help you with a variety of tasks by delegating to specialized agents. Here's what I can do:", has_file_parts: false, timestamp: new Date(Date.now() - 39 * 60 * 1000).toISOString() },
      ],
    }],
  ],
  scenarios: {
    "chat-collapsed": async (page) => {
      await page.waitForSelector(".pane-collapse");
      await page.click("#chat-pane .pane-collapse");
      await page.waitForTimeout(200);
    },
    "attributes-tab": async (page) => {
      await page.waitForSelector(".tab-btn");
      await page.click('.tab-btn[data-tab="attributes"]');
      await page.waitForSelector(".raw-json");
    },
    "tool-span-selected": async (page) => {
      await page.waitForSelector(".span-row");
      const rows = await page.$$(".span-row");
      if (rows[2]) await rows[2].click();
      await page.waitForTimeout(300);
    },
  },
};
