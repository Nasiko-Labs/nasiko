// Builds page fixtures
export default {
  window: {
    fetchBuilds: async () => ({
      data: [
        { build_id: "b-a1b2c3d4-e5f6-7890-abcd-ef1234567890", agent_name: "coding-agent", image: "nasiko/coding:latest", status: "success", progress: 100, duration_s: 142, started_at: "2026-06-30T14:00:00Z" },
        { build_id: "b-f1e2d3c4-b5a6-7890-dcba-fe0987654321", agent_name: "research-agent", image: "nasiko/research:0.4.0", status: "building", progress: 67, duration_s: null, started_at: "2026-06-30T15:10:00Z" },
        { build_id: "b-11223344-5566-7788-99aa-bbccddeeff00", agent_name: "devops-agent", image: "nasiko/devops:0.3.2", status: "failed", progress: 34, duration_s: 58, started_at: "2026-06-30T12:30:00Z" },
        { build_id: "b-aabbccdd-eeff-1122-3344-556677889900", agent_name: "qa-agent", image: "nasiko/qa:latest", status: "queued", progress: 0, duration_s: null, started_at: null },
        { build_id: "b-99887766-5544-3322-1100-ffeeddccbbaa", agent_name: "docs-agent", image: "nasiko/docs:0.2.1", status: "success", progress: 100, duration_s: 89, started_at: "2026-06-29T20:00:00Z" },
      ],
      total: 5,
    }),
  },
  sse: [
    ["GET /api/builds/events", []],
  ],
};
