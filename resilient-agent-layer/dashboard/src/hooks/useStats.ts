import { useState, useEffect, useRef, useCallback } from 'react'

const GATEWAY = import.meta.env.VITE_GATEWAY_URL || ''

export interface LiveStats {
  timestamp: number
  cache: {
    hits: number
    misses: number
    total: number
    hit_rate: number
    backend: string
    lru_size: number
    per_agent: Record<string, { hits: number; misses: number; total: number; hit_rate: number }>
  }
  rate_limiter: {
    backend: string
    total_allowed: number
    total_denied: number
    agents: Record<string, { rps: number; burst: number; allowed: number; denied: number }>
  }
  queues: {
    total_depth: number
    queues: Record<string, {
      agent_id: string
      current_depth: number
      max_size: number
      timeout_ms: number
      total_enqueued: number
      total_dequeued: number
      total_timed_out: number
      total_rejected: number
    }>
  }
  proxy: {
    total_requests: number
    total_errors: number
    per_agent: Record<string, {
      total_requests: number
      errors: number
      cache_bypasses: number
      latency_p50_ms: number
      latency_p95_ms: number
      latency_p99_ms: number
      latency_avg_ms: number
    }>
  }
}

export function useLiveStats() {
  const [stats, setStats] = useState<LiveStats | null>(null)
  const [connected, setConnected] = useState(false)
  const [history, setHistory] = useState<LiveStats[]>([])
  const esRef = useRef<EventSource | null>(null)

  useEffect(() => {
    const connect = () => {
      const es = new EventSource(`${GATEWAY}/admin/stream`)
      esRef.current = es

      es.onopen = () => setConnected(true)
      es.onmessage = (e) => {
        try {
          const data: LiveStats = JSON.parse(e.data)
          setStats(data)
          setHistory(prev => {
            const next = [...prev, data]
            return next.slice(-60) // keep 60s of history
          })
        } catch {}
      }
      es.onerror = () => {
        setConnected(false)
        es.close()
        setTimeout(connect, 3000) // reconnect
      }
    }
    connect()
    return () => { esRef.current?.close() }
  }, [])

  return { stats, connected, history }
}

export async function flushAllCache() {
  await fetch(`${GATEWAY}/admin/cache`, { method: 'DELETE' })
}

export async function flushAgentCache(agentId: string) {
  await fetch(`${GATEWAY}/admin/cache/${agentId}`, { method: 'DELETE' })
}

export async function updateRateLimit(agentId: string, rps: number, burst: number) {
  await fetch(`${GATEWAY}/admin/rate-limits/${agentId}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ rps, burst }),
  })
}

export async function sendTestRequest(agentId: string, query: string, bypass = false) {
  const res = await fetch(`${GATEWAY}/invoke`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ agent_id: agentId, payload: { query }, bypass_cache: bypass }),
  })
  return res.json()
}
