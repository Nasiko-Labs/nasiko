import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts'
import './App.css'
import {
  aggregateBuckets,
  buildFallbackSummary,
  formatLatency,
  formatNumber,
  formatPercent,
  formatTime,
  getTokenFromStorage,
  readSuperuserCredentials,
} from './metrics'
import type { AgentMetric, HourlyBucket, MetricsResponse } from './types'

type LoadState = 'idle' | 'loading' | 'ready' | 'error'

const API_PATH = '/api/v1/observability/agent-metrics?window_hours=24'
const LOGIN_PATH = '/auth/users/login'
const CREDENTIALS_PATH = `${import.meta.env.BASE_URL}superuser_credentials.json`

function formatChartTime(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) {
    return value
  }

  return date.toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
  })
}

function MetricChart({
  buckets,
  compact = false,
}: {
  buckets: HourlyBucket[]
  compact?: boolean
}) {
  const data = buckets.map((bucket) => ({
    ...bucket,
    label: formatChartTime(bucket.time),
  }))
  const height = compact ? 130 : 260

  return (
    <div className={compact ? 'metric-chart compact' : 'metric-chart'}>
      <ResponsiveContainer width="100%" height={height}>
        <LineChart
          data={data}
          margin={{ top: 12, right: compact ? 4 : 18, bottom: 8, left: 0 }}
        >
          <CartesianGrid stroke="rgba(177,255,229,0.12)" strokeDasharray="3 3" />
          <XAxis
            dataKey="label"
            minTickGap={compact ? 28 : 18}
            tick={{ fill: 'var(--muted)', fontSize: 11 }}
            tickLine={false}
          />
          <YAxis
            yAxisId="requests"
            allowDecimals={false}
            tick={{ fill: 'var(--muted)', fontSize: 11 }}
            tickLine={false}
            width={compact ? 24 : 40}
          />
          <YAxis
            yAxisId="latency"
            orientation="right"
            tick={{ fill: 'var(--muted)', fontSize: 11 }}
            tickFormatter={(value) => `${Math.round(value)}ms`}
            tickLine={false}
            width={compact ? 36 : 52}
          />
          <Tooltip
            contentStyle={{
              background: '#081411',
              border: '1px solid rgba(177,255,229,0.22)',
              borderRadius: 14,
              color: 'var(--text-h)',
            }}
            formatter={(value, name) => {
              const numericValue = Number(value)
              if (name === 'average_latency_ms') {
                return [formatLatency(numericValue), 'Avg latency']
              }

              return [formatNumber(numericValue), 'Requests']
            }}
            labelFormatter={(label) => `Hour ${label}`}
          />
          <Line
            yAxisId="requests"
            type="monotone"
            dataKey="requests"
            name="Requests"
            stroke="var(--accent)"
            strokeWidth={compact ? 2 : 3}
            dot={compact ? false : { r: 3 }}
            activeDot={{ r: 5 }}
          />
          <Line
            yAxisId="latency"
            type="monotone"
            dataKey="average_latency_ms"
            name="Avg latency"
            stroke="#8fb7ff"
            strokeWidth={compact ? 2 : 3}
            dot={false}
          />
        </LineChart>
      </ResponsiveContainer>
    </div>
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
  const [manualToken, setManualToken] = useState('')
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

  const signInWithGeneratedCredentials = useCallback(async () => {
    setState('loading')
    setError('')

    try {
      const credentialsResponse = await fetch(CREDENTIALS_PATH, {
        cache: 'no-store',
      })
      if (!credentialsResponse.ok) {
        throw new Error(
          'Generated superuser credentials are not available yet. Start the local stack and wait for orchestrator/superuser_credentials.json.',
        )
      }

      const credentials = readSuperuserCredentials(
        await credentialsResponse.json(),
      )
      if (!credentials) {
        throw new Error('Generated superuser credentials are incomplete.')
      }

      const loginResponse = await fetch(LOGIN_PATH, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          access_key: credentials.accessKey,
          access_secret: credentials.accessSecret,
        }),
      })

      if (!loginResponse.ok) {
        throw new Error(`Superuser sign-in returned ${loginResponse.status}`)
      }

      const loginPayload = (await loginResponse.json()) as {
        token?: string
        access_token?: string
      }
      const nextToken = loginPayload.token ?? loginPayload.access_token ?? ''
      if (!nextToken) {
        throw new Error('Superuser sign-in did not return a token.')
      }

      window.localStorage.setItem('nasiko_auth', JSON.stringify({ token: nextToken }))
      setToken(nextToken)
      await loadMetrics(nextToken)
    } catch (err) {
      setState('error')
      setError(err instanceof Error ? err.message : 'Unable to sign in')
    }
  }, [loadMetrics])

  useEffect(() => {
    if (hasAutoLoaded.current) {
      return
    }

    hasAutoLoaded.current = true
    const timeout = window.setTimeout(() => {
      if (token) {
        void loadMetrics(token)
      } else {
        void signInWithGeneratedCredentials()
      }
    }, 0)

    return () => window.clearTimeout(timeout)
  }, [loadMetrics, signInWithGeneratedCredentials, token])

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
          <label>Local superuser session</label>
          <div className="token-row">
            <button
              type="button"
              onClick={() => void signInWithGeneratedCredentials()}
            >
              {state === 'loading' ? 'Signing in…' : 'Sign in & refresh'}
            </button>
            {token && <span className="session-pill">Session ready</span>}
          </div>
          <span>
            Auto-loads <code>orchestrator/superuser_credentials.json</code>.
          </span>
        </div>
      </section>

      {state === 'error' && (
        <div className="notice error">
          <p>{error}</p>
          <div className="token-row" style={{ marginTop: 12 }}>
            <input
              type="password"
              placeholder="Paste bearer token manually"
              value={manualToken}
              onChange={(e) => setManualToken(e.target.value)}
            />
            <button
              type="button"
              disabled={!manualToken}
              onClick={() => {
                setToken(manualToken)
                void loadMetrics(manualToken)
              }}
            >
              Load
            </button>
          </div>
          <small style={{ color: 'var(--muted)', fontSize: 12 }}>
            Get a token from: <code>cat orchestrator/superuser_credentials.json</code> → sign in at <code>/auth/users/login</code>
          </small>
        </div>
      )}
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
          <h2>Requests and latency by hour</h2>
          <p>
            Recharts line chart with request volume and weighted average
            latency on separate axes.
          </p>
        </div>
        <MetricChart buckets={aggregateTrend} />
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
                <MetricChart buckets={agent.hourly} compact />
              </article>
            ))}
          </div>
        )}
      </section>
    </main>
  )
}

export default App
