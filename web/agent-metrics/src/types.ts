export interface HourlyBucket {
  hour: string;
  requests: number;
  errors: number;
  avg_latency_ms: number;
}

export interface AgentMetrics {
  agent_id: string;
  name: string;
  deployment_status: string;
  has_observability: boolean;
  avg_response_time_ms: number;
  success_count: number;
  error_count: number;
  total_requests: number;
  uptime_percent: number;
  hourly: HourlyBucket[];
}

export interface MetricsResponse {
  data: {
    period_hours: number;
    start_time: string;
    agents: AgentMetrics[];
  };
}

export interface LoginResponse {
  access_token?: string;
  token?: string;
}
