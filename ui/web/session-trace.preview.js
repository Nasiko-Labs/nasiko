// Session trace page fixtures.
//
// session-trace.html is now a redirect stub: it reads `project_session_id`
// off the trace and hands off to /observability-session.html. Only that field
// is load-bearing here; the span tree below is retained because the shape
// still documents GET /api/observability/trace/{trace_id}. The session id
// matches observability-session.preview.js so the redirect URL is realistic
// (the harness does not carry fixtures across a page navigation, so the
// landing page is covered by its own scenarios, not by this one).
//
// Shape mirrors GET /api/observability/trace/{trace_id} — the same endpoint
// `nasiko observe trace` consumes: envelope {data:{trace}}, spans as a nested
// tree with children embedded in each SpanNode (server: oss/server/src/observability/service.rs).
const span = (over) => ({
  id: over.span_id,
  span_kind: "INTERNAL",
  status_code: "OK",
  end_time: null,
  parent_id: null,
  token_count_total: (over.input_tokens || 0) + (over.output_tokens || 0),
  input_tokens: 0,
  output_tokens: 0,
  model: null,
  span_annotation_summaries: [],
  children: [],
  ...over,
});

const TRACE = {
  id: "8a880df26caf4c12a0e2d5f898f49420",
  project_session_id: "ses_18a5801d3353463ca39ebc216887f385",
  num_spans: 6,
  latency_ms: 41000,
  cost_summary: {
    total: { cost: 0.1662 },
    prompt: { cost: 0.0699 },
    completion: { cost: 0.0963 },
  },
  root_spans: { edges: [{ node: { span_id: "a1b2c3d4e5f60001" } }] },
  spans: [
    span({
      span_id: "a1b2c3d4e5f60001",
      name: "a2a.server.request",
      span_kind: "SERVER",
      start_time: "2026-07-01T08:52:00Z",
      end_time: "2026-07-01T08:52:41Z",
      latency_ms: 41000,
      children: [
        span({
          span_id: "a1b2c3d4e5f60002",
          name: "ChatCompletion",
          span_kind: "LLM",
          parent_id: "a1b2c3d4e5f60001",
          start_time: "2026-07-01T08:52:00.500Z",
          latency_ms: 1200,
          model: "gemini-2.5-flash",
          input_tokens: 2074,
          output_tokens: 312,
        }),
        span({
          span_id: "a1b2c3d4e5f60003",
          name: "tool.get_topology",
          span_kind: "TOOL",
          parent_id: "a1b2c3d4e5f60001",
          start_time: "2026-07-01T08:52:01.700Z",
          latency_ms: 1500,
        }),
        span({
          span_id: "a1b2c3d4e5f60004",
          name: "ChatCompletion",
          span_kind: "LLM",
          parent_id: "a1b2c3d4e5f60001",
          start_time: "2026-07-01T08:52:03.200Z",
          latency_ms: 4800,
          model: "gemini-2.5-flash",
          input_tokens: 8420,
          output_tokens: 1890,
        }),
        span({
          span_id: "a1b2c3d4e5f60005",
          name: "tool.get_device_metrics",
          span_kind: "TOOL",
          parent_id: "a1b2c3d4e5f60001",
          start_time: "2026-07-01T08:52:08.000Z",
          latency_ms: 1500,
        }),
        span({
          span_id: "a1b2c3d4e5f60006",
          name: "ChatCompletion",
          span_kind: "LLM",
          parent_id: "a1b2c3d4e5f60001",
          start_time: "2026-07-01T08:52:09.500Z",
          latency_ms: 31500,
          model: "gemini-2.5-flash",
          input_tokens: 12800,
          output_tokens: 4210,
        }),
      ],
    }),
  ],
  span_lookup: {},
};

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/observability\/trace\// }, { data: { trace: TRACE } }],
  ],
};
