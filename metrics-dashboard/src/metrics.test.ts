import { describe, expect, it } from 'vitest'
import { aggregateBuckets, formatLatency } from './metrics'
import type { AgentMetric } from './types'

function agent(agent_id: string, hourly: AgentMetric['hourly']): AgentMetric {
  return {
    agent_id,
    agent_name: agent_id,
    description: '',
    status: 'active',
    requests: 0,
    success_count: 0,
    error_count: 0,
    uptime_percentage: 100,
    average_latency_ms: 0,
    p50_latency_ms: null,
    p99_latency_ms: null,
    last_activity_at: null,
    hourly,
    error: null,
  }
}

describe('metrics helpers', () => {
  it('aggregates hourly request buckets across agents', () => {
    const buckets = aggregateBuckets([
      agent('translator', [
        {
          time: '2026-05-17T00:00:00.000Z',
          requests: 2,
          success_count: 2,
          error_count: 0,
          average_latency_ms: 100,
        },
      ]),
      agent('summarizer', [
        {
          time: '2026-05-17T00:00:00.000Z',
          requests: 1,
          success_count: 0,
          error_count: 1,
          average_latency_ms: 400,
        },
      ]),
    ])

    expect(buckets[0]).toEqual({
      time: '2026-05-17T00:00:00.000Z',
      requests: 3,
      success_count: 2,
      error_count: 1,
      average_latency_ms: 200,
    })
  })

  it('formats latency in milliseconds and seconds', () => {
    expect(formatLatency(950)).toBe('950 ms')
    expect(formatLatency(1250)).toBe('1.25 s')
  })
})
