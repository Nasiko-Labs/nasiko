// Build detail page fixtures — GET /api/builds/{id} returns a BuildRecord
// (oss/server/src/build/routes.rs).
const build = {
  id: "b-a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  agent_id: "a-001",
  github_url: "https://github.com/nasiko/coding-agent",
  commit_hash: "9f8e7d6c5b4a3f2e1d0c",
  version_tag: "1.2.0",
  image_reference: "nasiko/coding:1.2.0",
  status: "success",
  logs_url: "https://logs.example.com/builds/b-a1b2c3d4",
  created_at: "2026-06-30T14:00:00Z",
  updated_at: "2026-06-30T14:02:22Z",
};

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/builds\/[^/]+$/ }, build],
  ],
};
