export type HourlyBucket = {
  time: string
  requests: number
  success_count: number
  error_count: number
  average_latency_ms: number
}

export type AgentMetric = {
  agent_id: string
  agent_name: string
  description: string
  status: 'active' | 'idle' | 'unavailable' | string
  requests: number
  success_count: number
  error_count: number
  uptime_percentage: number
  average_latency_ms: number
  p50_latency_ms: number | null
  p99_latency_ms: number | null
  last_activity_at: string | null
  hourly: HourlyBucket[]
  error: string | null
}

export type MetricsSummary = {
  total_agents: number
  active_agents: number
  total_requests: number
  success_count: number
  error_count: number
  error_rate: number
  average_latency_ms: number
}

export type MetricsResponse = {
  data: {
    window: {
      hours: number
      start_time: string
      end_time: string
    }
    summary: MetricsSummary
    agents: AgentMetric[]
    generated_at: string
    user_id: string
  }
}
