// Flow detail page fixtures
export default {
  window: {
    fetchFlowDetail: async (flowId) => ({
      flow: {
        flow_id: flowId || "fl-a1b2c3d4e5f6",
        title: "Help me deploy the new auth service to production",
        root_agent_name: "devops-agent",
        status: "completed",
        total_invocations: 4,
        duration_ms: 12400,
        created_at: "2026-06-30T10:00:00Z",
      },
      steps: [
        { agent_name: "orchestrator", status: "completed", created_at: "2026-06-30T10:00:00Z", completed_at: "2026-06-30T10:00:01.200Z", latency_ms: 1200, input_summary: "Help me deploy the new auth service to production" },
        { agent_name: "devops-agent", status: "completed", created_at: "2026-06-30T10:00:01.200Z", completed_at: "2026-06-30T10:00:06.800Z", latency_ms: 5600, input_summary: "Deploy auth service with rolling update strategy" },
        { agent_name: "coding-agent", status: "completed", created_at: "2026-06-30T10:00:03.000Z", completed_at: "2026-06-30T10:00:08.200Z", latency_ms: 5200, input_summary: "Validate Dockerfile and k8s manifests" },
        { agent_name: "qa-agent", status: "completed", created_at: "2026-06-30T10:00:08.200Z", completed_at: "2026-06-30T10:00:12.400Z", latency_ms: 4200, input_summary: "Run integration tests against staging" },
      ],
    }),
  },
};
