// Settings page fixtures
export default {
  window: {
    fetchSettings: async () => ({
      cluster_name: "nasiko-dev",
      scheduler_mode: "local",
      otel_enabled: false,
      otel_collector_endpoint: "http://otel-collector.nasiko-infra:4318",
      seed_agents: "nasiko/coding:latest nasiko/research:latest",
      max_containers: 20,
      default_replicas: 1,
    }),
  },
};
