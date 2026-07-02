// Session trace page fixtures
const TRACE_DATA = {
  trace_id: "8a880df26caf4c12a0e2d5f898f49420",
  started_at: "2026-07-01T08:52:00Z",
  ended_at: "2026-07-01T08:52:41Z",
  duration_ms: 41000,
  spans: [
    {
      span_id: "a1b2c3d4e5f60001",
      parent_span_id: null,
      name: "a2a.server.request",
      started_at: "2026-07-01T08:52:00Z",
      ended_at: "2026-07-01T08:52:41Z",
      duration_ms: 41000,
      service_name: "devops-agent",
      attributes: { "http.method": "POST", "http.url": "/jsonrpc", "http.status_code": 200 },
    },
    {
      span_id: "a1b2c3d4e5f60002",
      parent_span_id: "a1b2c3d4e5f60001",
      name: "ChatCompletion",
      started_at: "2026-07-01T08:52:00.500Z",
      ended_at: "2026-07-01T08:52:01.700Z",
      duration_ms: 1200,
      service_name: "devops-agent",
      attributes: {
        "gen_ai.operation.name": "chat",
        "gen_ai.request.model": "gemini-2.5-flash",
        "gen_ai.usage.input_tokens": 2074,
        "gen_ai.usage.output_tokens": 312,
      },
    },
    {
      span_id: "a1b2c3d4e5f60003",
      parent_span_id: "a1b2c3d4e5f60001",
      name: "tool.get_topology",
      started_at: "2026-07-01T08:52:01.700Z",
      ended_at: "2026-07-01T08:52:03.200Z",
      duration_ms: 1500,
      service_name: "devops-agent",
      attributes: { "tool.name": "get_topology", "tool.status": "success" },
    },
    {
      span_id: "a1b2c3d4e5f60004",
      parent_span_id: "a1b2c3d4e5f60001",
      name: "ChatCompletion",
      started_at: "2026-07-01T08:52:03.200Z",
      ended_at: "2026-07-01T08:52:08.000Z",
      duration_ms: 4800,
      service_name: "devops-agent",
      attributes: {
        "gen_ai.operation.name": "chat",
        "gen_ai.request.model": "gemini-2.5-flash",
        "gen_ai.usage.input_tokens": 8420,
        "gen_ai.usage.output_tokens": 1890,
      },
    },
    {
      span_id: "a1b2c3d4e5f60005",
      parent_span_id: "a1b2c3d4e5f60001",
      name: "tool.get_device_metrics",
      started_at: "2026-07-01T08:52:08.000Z",
      ended_at: "2026-07-01T08:52:09.500Z",
      duration_ms: 1500,
      service_name: "devops-agent",
      attributes: { "tool.name": "get_device_metrics", "tool.status": "success" },
    },
    {
      span_id: "a1b2c3d4e5f60006",
      parent_span_id: "a1b2c3d4e5f60001",
      name: "ChatCompletion",
      started_at: "2026-07-01T08:52:09.500Z",
      ended_at: "2026-07-01T08:52:41Z",
      duration_ms: 31500,
      service_name: "devops-agent",
      attributes: {
        "gen_ai.operation.name": "chat",
        "gen_ai.request.model": "gemini-2.5-flash",
        "gen_ai.usage.input_tokens": 12800,
        "gen_ai.usage.output_tokens": 4210,
        "otel.status_code": "OK",
      },
    },
  ],
};

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/observe\/traces\// }, TRACE_DATA],
  ],
  window: {
    fetchTraceDetail: async () => TRACE_DATA,
  },
};
