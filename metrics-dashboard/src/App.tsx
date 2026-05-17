import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import './App.css'
import {
  aggregateBuckets,
  buildFallbackSummary,
  formatLatency,
  formatNumber,
  formatPercent,
  formatTime,
  getTokenFromStorage,
} from './metrics'
import type { AgentMetric, HourlyBucket, MetricsResponse } from './types'

type LoadState = 'idle' | 'loading' | 'ready' | 'error'

const API_PATH = '/api/v1/observability/agent-metrics?window_hours=24'

function Sparkline({ buckets }: { buckets: HourlyBucket[] }) {
  const max = Math.max(...buckets.map((bucket) => bucket.requests), 1)
  const width = 220
  const height = 72
  const step = buckets.length > 1 ? width / (buckets.length - 1) : width
  const points = buckets
    .map((bucket, index) => {
      const x = index * step
      const y = height - (bucket.requests / max) * (height - 10) - 5
      return `${x},${y}`
    })
    .join(' ')

  return (
    <svg className="sparkline" viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
      <polyline points={points} />
      {buckets.map((bucket, index) => (
        <circle
          key={`${bucket.time}-${index}`}
          cx={index * step}
          cy={height - (bucket.requests / max) * (height - 10) - 5}
          r={bucket.requests > 0 ? 3 : 1.8}
        />
      ))}
    </svg>
  )
}

function HealthRing({ value }: { value: number }) {
  const clamped = Math.max(0, Math.min(100, value))
  const style = {
    background: `conic-gradient(var(--good) ${clamped * 3.6}deg, var(--ring) 0deg)`,
  }

  return (
    <div className="health-ring" style={style}>
      <span>{Math.round(clamped)}</span>
    </div>
  )
}

function StatusPill({ status }: { status: AgentMetric['status'] }) {
  return <span className={`status-pill status-${status}`}>{status}</span>
}

function App() {
  const [token, setToken] = useState(() => getTokenFromStorage())
  const [state, setState] = useState<LoadState>('idle')
  const [error, setError] = useState('')
  const [metrics, setMetrics] = useState<MetricsResponse['data'] | null>(null)
  const hasAutoLoaded = useRef(false)

  const loadMetrics = useCallback(async (nextToken: string) => {
    if (!nextToken) {
      setState('error')
      setError('Paste a Nasiko bearer token to load authenticated metrics.')
      return
    }

    setState('loading')
    setError('')

    try {
      const response = await fetch(API_PATH, {
        headers: {
          Authorization: nextToken.startsWith('Bearer ')
            ? nextToken
            : `Bearer ${nextToken}`,
        },
      })

      if (!response.ok) {
        throw new Error(`API returned ${response.status}`)
      }

      const payload = (await response.json()) as MetricsResponse
      setMetrics(payload.data)
      setState('ready')
    } catch (err) {
      setState('error')
      setError(err instanceof Error ? err.message : 'Unable to load metrics')
    }
  }, [])

  useEffect(() => {
    if (!token || hasAutoLoaded.current) {
      return
    }

    hasAutoLoaded.current = true
    const timeout = window.setTimeout(() => {
      void loadMetrics(token)
    }, 0)

    return () => window.clearTimeout(timeout)
  }, [loadMetrics, token])

  const agents = useMemo(() => metrics?.agents ?? [], [metrics])
  const summary = metrics?.summary ?? buildFallbackSummary(agents)
  const aggregateTrend = useMemo(() => aggregateBuckets(agents), [agents])
  const errorAgents = agents.filter((agent) => agent.error)

  return (
    <main className="dashboard-shell">
      <section className="hero-panel">
        <div>
          <p className="eyebrow">Nasiko Titan Console</p>
          <h1>Agent Performance Metrics</h1>
          <p className="lede">
            Last-24-hour response health across every agent you can access,
            powered by Phoenix traces and the Nasiko registry.
          </p>
        </div>
        <div className="auth-card">
          <label htmlFor="token">Bearer token</label>
          <div className="token-row">
            <input
              id="token"
              value={token}
              type="password"
              placeholder="Paste token from /auth/users/login"
              onChange={(event) => setToken(event.target.value)}
            />
            <button type="button" onClick={() => void loadMetrics(token)}>
              {state === 'loading' ? 'Loading' : 'Refresh'}
            </button>
          </div>
          <span>
            Opens at <code>/metrics</code> and calls <code>{API_PATH}</code>.
          </span>
        </div>
      </section>

      {state === 'error' && <div className="notice error">{error}</div>}
      {errorAgents.length > 0 && (
        <div className="notice warning">
          {errorAgents.length} agent{errorAgents.length > 1 ? 's' : ''} could
          not be read from Phoenix. Zero-state rows are preserved.
        </div>
      )}

      <section className="summary-grid">
        <article>
          <span>Total agents</span>
          <strong>{formatNumber(summary.total_agents)}</strong>
          <small>{formatNumber(summary.active_agents)} active now</small>
        </article>
        <article>
          <span>Requests</span>
          <strong>{formatNumber(summary.total_requests)}</strong>
          <small>{formatNumber(summary.success_count)} successful</small>
        </article>
        <article>
          <span>Error rate</span>
          <strong>{formatPercent(summary.error_rate)}</strong>
          <small>{formatNumber(summary.error_count)} failed traces</small>
        </article>
        <article>
          <span>Average latency</span>
          <strong>{formatLatency(summary.average_latency_ms)}</strong>
          <small>Weighted by request volume</small>
        </article>
      </section>

      <section className="trend-panel">
        <div>
          <p className="eyebrow">24-hour traffic</p>
          <h2>Aggregate request pulse</h2>
        </div>
        <Sparkline buckets={aggregateTrend} />
      </section>

      <section className="agent-table-card">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Per-agent breakdown</p>
            <h2>Live performance board</h2>
          </div>
          <span>{metrics ? `Generated ${formatTime(metrics.generated_at)}` : 'Awaiting data'}</span>
        </div>

        {agents.length === 0 ? (
          <div className="empty-state">
            <h3>No agent metrics yet</h3>
            <p>
              Deploy the translator agent, start a session, then refresh this
              dashboard after Phoenix receives traces.
            </p>
          </div>
        ) : (
          <div className="agent-table">
            {agents.map((agent) => (
              <article key={agent.agent_id} className="agent-row">
                <div className="agent-identity">
                  <HealthRing value={agent.uptime_percentage} />
                  <div>
                    <div className="agent-title">
                      <h3>{agent.agent_name}</h3>
                      <StatusPill status={agent.status} />
                    </div>
                    <p>{agent.description || agent.agent_id}</p>
                    {agent.error && <small>{agent.error}</small>}
                  </div>
                </div>
                <div className="metric-stack">
                  <span>Avg response</span>
                  <strong>{formatLatency(agent.average_latency_ms)}</strong>
                  <small>P99 {formatLatency(agent.p99_latency_ms)}</small>
                </div>
                <div className="metric-stack">
                  <span>Success / error</span>
                  <strong>
                    {formatNumber(agent.success_count)} /{' '}
                    {formatNumber(agent.error_count)}
                  </strong>
                  <small>{formatNumber(agent.requests)} total</small>
                </div>
                <div className="metric-stack">
                  <span>Last activity</span>
                  <strong>{formatTime(agent.last_activity_at)}</strong>
                  <small>{formatPercent(agent.uptime_percentage)} uptime</small>
                </div>
                <Sparkline buckets={agent.hourly} />
              </article>
            ))}
          </div>
        )}
      </section>
    </main>
  )
}

export default App
