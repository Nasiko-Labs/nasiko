import type { AgentMetric, HourlyBucket, MetricsSummary } from './types'

export type SuperuserCredentials = {
  accessKey: string
  accessSecret: string
}

export function formatNumber(value: number): string {
  return new Intl.NumberFormat('en-US').format(value)
}

export function formatLatency(value: number | null): string {
  if (value === null || Number.isNaN(value)) {
    return '—'
  }

  if (value === 0) {
    return '0 ms'
  }

  if (value >= 1000) {
    return `${(value / 1000).toFixed(value >= 10000 ? 1 : 2)} s`
  }

  return `${Math.round(value)} ms`
}

function tokenFromJson(value: unknown): string {
  if (typeof value === 'string') {
    return value
  }

  if (!value || typeof value !== 'object') {
    return ''
  }

  const record = value as Record<string, unknown>
  for (const key of ['token', 'access_token', 'authToken', 'nasiko_token']) {
    const token = record[key]
    if (typeof token === 'string' && token.length > 0) {
      return token
    }
  }

  for (const child of Object.values(record)) {
    const token = tokenFromJson(child)
    if (token) {
      return token
    }
  }

  return ''
}

export function formatPercent(value: number): string {
  return `${value.toFixed(value % 1 === 0 ? 0 : 1)}%`
}

export function formatTime(value: string | null): string {
  if (!value) {
    return 'No activity'
  }

  const date = new Date(value)
  if (Number.isNaN(date.getTime())) {
    return value
  }

  return date.toLocaleString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    month: 'short',
    day: 'numeric',
  })
}

export function getTokenFromStorage(): string {
  const keys = [
    'nasiko_auth',
    'nasiko_token',
    'token',
    'access_token',
    'authToken',
  ]

  for (const storage of [window.localStorage, window.sessionStorage]) {
    for (const key of keys) {
      const value = storage.getItem(key)
      if (!value) {
        continue
      }

      try {
        const token = tokenFromJson(JSON.parse(value))
        if (token) {
          return token
        }
      } catch {
        return value.replace(/^"|"$/g, '')
      }
    }

    for (const key of Object.keys(storage)) {
      if (!key.toLowerCase().includes('token')) {
        continue
      }
      const value = storage.getItem(key)
      if (value && value.length > 20) {
        return value.replace(/^"|"$/g, '')
      }
    }
  }

  return ''
}

export function readSuperuserCredentials(
  value: unknown,
): SuperuserCredentials | null {
  if (!value || typeof value !== 'object') {
    return null
  }

  const record = value as Record<string, unknown>
  const accessKey = record.access_key ?? record.accessKey
  const accessSecret = record.access_secret ?? record.accessSecret

  if (typeof accessKey !== 'string' || typeof accessSecret !== 'string') {
    return null
  }

  if (!accessKey || !accessSecret) {
    return null
  }

  return { accessKey, accessSecret }
}

export function aggregateBuckets(agents: AgentMetric[]): HourlyBucket[] {
  const first = agents[0]?.hourly ?? []

  return first.map((bucket, index) => {
    const agentBuckets = agents
      .map((agent) => agent.hourly[index])
      .filter(Boolean)
    const requests = agentBuckets.reduce((sum, item) => sum + item.requests, 0)
    const success_count = agentBuckets.reduce(
      (sum, item) => sum + item.success_count,
      0,
    )
    const error_count = agentBuckets.reduce(
      (sum, item) => sum + item.error_count,
      0,
    )
    const latencyTotal = agentBuckets.reduce(
      (sum, item) => sum + item.average_latency_ms * item.requests,
      0,
    )

    return {
      time: bucket.time,
      requests,
      success_count,
      error_count,
      average_latency_ms: requests ? latencyTotal / requests : 0,
    }
  })
}

export function buildFallbackSummary(agents: AgentMetric[]): MetricsSummary {
  const total_requests = agents.reduce((sum, agent) => sum + agent.requests, 0)
  const success_count = agents.reduce((sum, agent) => sum + agent.success_count, 0)
  const error_count = agents.reduce((sum, agent) => sum + agent.error_count, 0)
  const active_agents = agents.filter((agent) => agent.status === 'active').length
  const latencyTotal = agents.reduce(
    (sum, agent) => sum + agent.average_latency_ms * agent.requests,
    0,
  )

  return {
    total_agents: agents.length,
    active_agents,
    total_requests,
    success_count,
    error_count,
    error_rate: total_requests ? (error_count / total_requests) * 100 : 0,
    average_latency_ms: total_requests ? latencyTotal / total_requests : 0,
  }
}
