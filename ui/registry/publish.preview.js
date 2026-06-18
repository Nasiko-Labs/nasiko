export default {
  fetch: [],
  window: {
    fetchNavigation: async () => [
      { title: "Artifacts", url: "/index.html" },
      { title: "Skills", url: "/skills.html" },
      { title: "Publish", url: "/publish.html" },
    ],
    publishArtifact: async (payload) => ({
      artifact: { ...payload, id: "new-id-001", status: "preview", oci_digest: null, size_bytes: null, created_at: "2026-05-30T10:00:00Z", updated_at: "2026-05-30T10:00:00Z" },
      upload_url: "http://localhost:3000/v2/nasiko/test/blobs/uploads/",
    }),
  },
};
