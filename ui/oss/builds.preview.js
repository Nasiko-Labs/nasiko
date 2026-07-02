// Builds page fixtures — matches backend BuildRecord model
export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/build\/builds/ }, {
      data: [
        { id: "b-a1b2c3d4-e5f6-7890-abcd-ef1234567890", agent_id: "a-001", version_tag: "1.2.0", image_reference: "nasiko/coding:1.2.0", status: "success", github_url: "https://github.com/nasiko/coding-agent", created_at: "2026-06-30T14:00:00Z", updated_at: "2026-06-30T14:02:22Z" },
        { id: "b-f1e2d3c4-b5a6-7890-dcba-fe0987654321", agent_id: "a-002", version_tag: "0.4.0", image_reference: "nasiko/research:0.4.0", status: "building", github_url: "https://github.com/nasiko/research-agent", created_at: "2026-06-30T15:10:00Z", updated_at: "2026-06-30T15:10:00Z" },
        { id: "b-11223344-5566-7788-99aa-bbccddeeff00", agent_id: "a-003", version_tag: "0.3.2", image_reference: "nasiko/devops:0.3.2", status: "failed", github_url: null, created_at: "2026-06-30T12:30:00Z", updated_at: "2026-06-30T12:31:58Z" },
        { id: "b-aabbccdd-eeff-1122-3344-556677889900", agent_id: "a-004", version_tag: "1.0.1", image_reference: "nasiko/qa:1.0.1", status: "queued", github_url: null, created_at: "2026-06-30T16:00:00Z", updated_at: "2026-06-30T16:00:00Z" },
        { id: "b-99887766-5544-3322-1100-ffeeddccbbaa", agent_id: "a-005", version_tag: "0.2.1", image_reference: "nasiko/docs:0.2.1", status: "success", github_url: "https://github.com/nasiko/docs-agent", created_at: "2026-06-29T20:00:00Z", updated_at: "2026-06-29T20:01:29Z" },
      ],
      total: 5,
    }],
  ],
};
