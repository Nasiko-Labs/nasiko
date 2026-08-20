import { useState, useCallback } from 'react'
import {
  LineChart, Line, AreaChart, Area, BarChart, Bar,
  XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, Legend
} from 'recharts'
import {
  Zap, Database, Shield, GitBranch, Activity,
  Trash2, RefreshCw, Send, Settings, AlertTriangle, CheckCircle
} from 'lucide-react'
import { useLiveStats, flushAllCache, flushAgentCache, updateRateLimit, sendTestRequest } from './hooks/useStats'

// ─── Helpers ──────────────────────────────────────────────────────────────────
const fmt = (n: number | undefined, d = 1) => (n ?? 0).toFixed(d)
const pct = (n: number | undefined) => `${fmt(n, 1)}%`
const ms = (n: number | undefined) => `${fmt(n, 0)}ms`

const AGENT_COLORS: Record<string, string> = {
  'agent-a': '#6c63ff',
  'agent-b': '#00d4aa',
  'agent-slow': '#f59e0b',
}
const agentColor = (id: string) => AGENT_COLORS[id] ?? '#8b8aa8'

// ─── Sub-components ───────────────────────────────────────────────────────────

function KpiCard({ label, value, sub, icon: Icon, accent = '#6c63ff' }: {
  label: string; value: string; sub?: string; icon: any; accent?: string
}) {
  return (
    <div className="kpi-card" style={{ '--card-accent': `${accent}12` } as any}>
      <div className="kpi-label">{label}</div>
      <div className="kpi-value" style={{ color: accent }}>{value}</div>
      {sub && <div className="kpi-sub">{sub}</div>}
      <div className="kpi-icon"><Icon size={40} color={accent} /></div>
    </div>
  )
}

function ProgressBar({ value, max, color }: { value: number; max: number; color: string }) {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0
  return (
    <div className="progress-wrap">
      <div className="progress-bar">
        <div className="progress-fill" style={{ width: `${pct}%`, background: color }} />
      </div>
      <span className="progress-label mono">{value}/{max}</span>
    </div>
  )
}

const CustomTooltip = ({ active, payload, label }: any) => {
  if (!active || !payload?.length) return null
  return (
    <div style={{
      background: '#1a1a2e', border: '1px solid #ffffff22',
      borderRadius: 8, padding: '8px 12px', fontSize: '0.75rem'
    }}>
      <div style={{ color: '#8b8aa8', marginBottom: 4 }}>{label}</div>
      {payload.map((p: any) => (
        <div key={p.name} style={{ color: p.color, display: 'flex', gap: 8 }}>
          <span>{p.name}:</span><span style={{ fontFamily: 'var(--mono)', fontWeight: 600 }}>{typeof p.value === 'number' ? p.value.toFixed(1) : p.value}</span>
        </div>
      ))}
    </div>
  )
}

// ─── Overview Tab ─────────────────────────────────────────────────────────────

function OverviewTab({ stats, history }: any) {
  const agents = Object.keys(stats?.cache?.per_agent ?? {})

  const chartData = history.map((s: any, i: number) => ({
    t: i,
    hitRate: s.cache?.hit_rate ?? 0,
    totalReq: s.proxy?.total_requests ?? 0,
    queueDepth: s.queues?.total_depth ?? 0,
    denied: s.rate_limiter?.total_denied ?? 0,
  }))

  return (
    <>
      {/* KPI Row */}
      <div className="kpi-grid">
        <KpiCard label="Cache Hit Rate" value={pct(stats?.cache?.hit_rate)} sub={`${stats?.cache?.hits ?? 0} hits / ${stats?.cache?.total ?? 0} total`} icon={Database} accent="#6c63ff" />
        <KpiCard label="Total Requests" value={(stats?.proxy?.total_requests ?? 0).toLocaleString()} sub={`${stats?.proxy?.total_errors ?? 0} errors`} icon={Activity} accent="#00d4aa" />
        <KpiCard label="Queue Depth" value={String(stats?.queues?.total_depth ?? 0)} sub="pending across all agents" icon={GitBranch} accent="#f59e0b" />
        <KpiCard label="Rate Limited" value={(stats?.rate_limiter?.total_denied ?? 0).toLocaleString()} sub={`${stats?.rate_limiter?.total_allowed ?? 0} allowed`} icon={Shield} accent="#ef4444" />
      </div>

      {/* Alerts */}
      {(stats?.cache?.hit_rate ?? 100) < 40 && (
        <div className="alert-banner alert-warn">
          <AlertTriangle size={14} /> Cache hit rate is below 40% — consider increasing TTL
        </div>
      )}
      {(stats?.queues?.total_depth ?? 0) > 30 && (
        <div className="alert-banner alert-danger">
          <AlertTriangle size={14} /> Queue depth is high ({stats.queues.total_depth}) — agents may be overwhelmed
        </div>
      )}
      {(stats?.cache?.hit_rate ?? 0) > 80 && (
        <div className="alert-banner alert-ok">
          <CheckCircle size={14} /> System healthy — cache hit rate above 80%
        </div>
      )}

      {/* Charts */}
      <div className="chart-grid">
        <div className="panel">
          <div className="panel-header">
            <div className="panel-title"><Database size={14} /> Cache Hit Rate <span className="panel-badge">LIVE</span></div>
          </div>
          <ResponsiveContainer width="100%" height={200}>
            <AreaChart data={chartData}>
              <defs>
                <linearGradient id="cacheGrad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="5%" stopColor="#6c63ff" stopOpacity={0.3} />
                  <stop offset="95%" stopColor="#6c63ff" stopOpacity={0} />
                </linearGradient>
              </defs>
              <CartesianGrid strokeDasharray="3 3" stroke="#ffffff08" />
              <XAxis dataKey="t" hide />
              <YAxis domain={[0, 100]} tick={{ fontSize: 10, fill: '#8b8aa8' }} width={30} />
              <Tooltip content={<CustomTooltip />} />
              <Area type="monotone" dataKey="hitRate" stroke="#6c63ff" fill="url(#cacheGrad)" name="Hit Rate %" strokeWidth={2} />
            </AreaChart>
          </ResponsiveContainer>
        </div>

        <div className="panel">
          <div className="panel-header">
            <div className="panel-title"><GitBranch size={14} /> Queue Depth <span className="panel-badge">LIVE</span></div>
          </div>
          <ResponsiveContainer width="100%" height={200}>
            <AreaChart data={chartData}>
              <defs>
                <linearGradient id="queueGrad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="5%" stopColor="#f59e0b" stopOpacity={0.3} />
                  <stop offset="95%" stopColor="#f59e0b" stopOpacity={0} />
                </linearGradient>
              </defs>
              <CartesianGrid strokeDasharray="3 3" stroke="#ffffff08" />
              <XAxis dataKey="t" hide />
              <YAxis tick={{ fontSize: 10, fill: '#8b8aa8' }} width={30} />
              <Tooltip content={<CustomTooltip />} />
              <Area type="monotone" dataKey="queueDepth" stroke="#f59e0b" fill="url(#queueGrad)" name="Queue Depth" strokeWidth={2} />
            </AreaChart>
          </ResponsiveContainer>
        </div>

        <div className="panel">
          <div className="panel-header">
            <div className="panel-title"><Shield size={14} /> Rate Limit Denials <span className="panel-badge">LIVE</span></div>
          </div>
          <ResponsiveContainer width="100%" height={200}>
            <BarChart data={chartData.slice(-20)}>
              <CartesianGrid strokeDasharray="3 3" stroke="#ffffff08" />
              <XAxis dataKey="t" hide />
              <YAxis tick={{ fontSize: 10, fill: '#8b8aa8' }} width={30} />
              <Tooltip content={<CustomTooltip />} />
              <Bar dataKey="denied" fill="#ef4444" name="Denied" radius={[3, 3, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        </div>

        <div className="panel">
          <div className="panel-header">
            <div className="panel-title"><Activity size={14} /> Per-Agent Cache Hits</div>
          </div>
          <table className="agent-table">
            <thead>
              <tr>
                <th>Agent</th><th>Hits</th><th>Misses</th><th>Hit Rate</th>
              </tr>
            </thead>
            <tbody>
              {agents.map(id => {
                const a = stats.cache.per_agent[id]
                return (
                  <tr key={id}>
                    <td><span className="agent-badge"><span className="agent-dot" style={{ background: agentColor(id) }} />{id}</span></td>
                    <td className="mono" style={{ color: '#6c63ff' }}>{a.hits}</td>
                    <td className="mono" style={{ color: '#8b8aa8' }}>{a.misses}</td>
                    <td>
                      <div className="progress-wrap">
                        <div className="progress-bar" style={{ maxWidth: 80 }}>
                          <div className="progress-fill" style={{ width: `${a.hit_rate}%`, background: agentColor(id) }} />
                        </div>
                        <span className="progress-label mono">{fmt(a.hit_rate)}%</span>
                      </div>
                    </td>
                  </tr>
                )
              })}
              {agents.length === 0 && (
                <tr><td colSpan={4} style={{ color: '#55546e', textAlign: 'center', padding: 24 }}>No data yet — send some requests!</td></tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
    </>
  )
}

// ─── Agents Tab ───────────────────────────────────────────────────────────────

function AgentsTab({ stats, history }: any) {
  const agents = Object.keys(stats?.proxy?.per_agent ?? {})

  const latencyHistory = history.map((s: any, i: number) => {
    const entry: any = { t: i }
    for (const [id, a] of Object.entries(s.proxy?.per_agent ?? {})) {
      entry[id] = (a as any).latency_avg_ms ?? 0
    }
    return entry
  })

  return (
    <>
      <div className="panel">
        <div className="panel-header">
          <div className="panel-title"><Activity size={14} /> Agent Latency (avg ms) <span className="panel-badge">LIVE</span></div>
        </div>
        <ResponsiveContainer width="100%" height={220}>
          <LineChart data={latencyHistory}>
            <CartesianGrid strokeDasharray="3 3" stroke="#ffffff08" />
            <XAxis dataKey="t" hide />
            <YAxis tick={{ fontSize: 10, fill: '#8b8aa8' }} width={40} unit="ms" />
            <Tooltip content={<CustomTooltip />} />
            <Legend wrapperStyle={{ fontSize: '0.75rem', color: '#8b8aa8' }} />
            {agents.map(id => (
              <Line key={id} type="monotone" dataKey={id} stroke={agentColor(id)} strokeWidth={2} dot={false} name={id} />
            ))}
          </LineChart>
        </ResponsiveContainer>
      </div>

      <div className="panel">
        <div className="panel-header">
          <div className="panel-title"><Zap size={14} /> Agent Performance Breakdown</div>
        </div>
        <table className="agent-table">
          <thead>
            <tr>
              <th>Agent</th><th>Requests</th><th>Errors</th><th>p50</th><th>p95</th><th>p99</th><th>Rate Limit</th><th>Queue Depth</th>
            </tr>
          </thead>
          <tbody>
            {agents.map(id => {
              const p = stats.proxy.per_agent[id]
              const rl = stats?.rate_limiter?.agents?.[id]
              const q = stats?.queues?.queues?.[id]
              return (
                <tr key={id}>
                  <td><span className="agent-badge"><span className="agent-dot" style={{ background: agentColor(id) }} />{id}</span></td>
                  <td className="mono">{p.total_requests}</td>
                  <td className="mono" style={{ color: p.errors > 0 ? '#ef4444' : '#22c55e' }}>{p.errors}</td>
                  <td className="mono" style={{ color: '#00d4aa' }}>{ms(p.latency_p50_ms)}</td>
                  <td className="mono">{ms(p.latency_p95_ms)}</td>
                  <td className="mono" style={{ color: p.latency_p99_ms > 3000 ? '#ef4444' : 'inherit' }}>{ms(p.latency_p99_ms)}</td>
                  <td>
                    <span style={{ fontSize: '0.75rem', fontFamily: 'var(--mono)' }}>
                      {rl?.rps ?? '–'} rps / {rl?.burst ?? '–'} burst
                    </span>
                  </td>
                  <td>
                    {q ? <ProgressBar value={q.current_depth} max={q.max_size} color={agentColor(id)} /> : '–'}
                  </td>
                </tr>
              )
            })}
            {agents.length === 0 && (
              <tr><td colSpan={8} style={{ color: '#55546e', textAlign: 'center', padding: 24 }}>No agent data yet</td></tr>
            )}
          </tbody>
        </table>
      </div>

      {/* Queue Bars */}
      <div className="panel">
        <div className="panel-header">
          <div className="panel-title"><GitBranch size={14} /> Queue Depths</div>
        </div>
        <div className="queue-list">
          {Object.entries(stats?.queues?.queues ?? {}).map(([id, q]: any) => (
            <div key={id}>
              <div className="queue-item-label">
                <span className="agent-badge"><span className="agent-dot" style={{ background: agentColor(id) }} />{id}</span>
                <span className="mono" style={{ fontSize: '0.75rem', color: '#8b8aa8' }}>
                  {q.total_enqueued} enqueued · {q.total_timed_out} timed out · {q.total_rejected} rejected
                </span>
              </div>
              <ProgressBar value={q.current_depth} max={q.max_size} color={agentColor(id)} />
            </div>
          ))}
          {Object.keys(stats?.queues?.queues ?? {}).length === 0 && (
            <div style={{ color: '#55546e', textAlign: 'center', padding: 16, fontSize: '0.85rem' }}>All queues empty</div>
          )}
        </div>
      </div>
    </>
  )
}

// ─── Admin Tab ────────────────────────────────────────────────────────────────

function AdminTab({ stats }: any) {
  const agents = Object.keys(stats?.rate_limiter?.agents ?? {})
  const [selectedAgent, setSelectedAgent] = useState(agents[0] ?? 'agent-a')
  const [rps, setRps] = useState('')
  const [burst, setBurst] = useState('')
  const [testQuery, setTestQuery] = useState('What is the capital of France?')
  const [testAgent, setTestAgent] = useState(agents[0] ?? 'agent-a')
  const [testResult, setTestResult] = useState<any>(null)
  const [loading, setLoading] = useState(false)
  const [toast, setToast] = useState('')

  const showToast = (msg: string) => { setToast(msg); setTimeout(() => setToast(''), 3000) }

  const handleFlushAll = async () => {
    await flushAllCache(); showToast('✓ All cache flushed')
  }
  const handleFlushAgent = async () => {
    await flushAgentCache(selectedAgent); showToast(`✓ Cache flushed for ${selectedAgent}`)
  }
  const handleUpdateRL = async () => {
    if (!rps && !burst) return
    await updateRateLimit(selectedAgent, parseFloat(rps) || 10, parseInt(burst) || 20)
    showToast(`✓ Rate limit updated for ${selectedAgent}`)
  }
  const handleTest = async (bypass: boolean) => {
    setLoading(true); setTestResult(null)
    try {
      const r = await sendTestRequest(testAgent, testQuery, bypass)
      setTestResult(r)
    } catch (e) {
      setTestResult({ error: String(e) })
    }
    setLoading(false)
  }

  return (
    <>
      {toast && <div className="alert-banner alert-ok" style={{ marginBottom: 8 }}><CheckCircle size={14} /> {toast}</div>}

      <div className="controls-grid">
        {/* Cache Controls */}
        <div className="control-card">
          <div className="control-title"><Database size={14} /> Cache Controls</div>
          <div className="input-label">Target Agent</div>
          <select className="input-field" value={selectedAgent} onChange={e => setSelectedAgent(e.target.value)}>
            {agents.map(id => <option key={id} value={id}>{id}</option>)}
            {agents.length === 0 && <option value="agent-a">agent-a</option>}
          </select>
          <div className="input-row" style={{ marginTop: 12 }}>
            <button className="btn btn-danger" onClick={handleFlushAgent}><Trash2 size={13} /> Flush Agent</button>
            <button className="btn btn-danger" onClick={handleFlushAll}><Trash2 size={13} /> Flush ALL</button>
          </div>
          <div style={{ marginTop: 12, padding: '10px', background: 'var(--surface2)', borderRadius: 8, fontSize: '0.75rem' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 4 }}>
              <span style={{ color: 'var(--muted)' }}>Hit Rate</span>
              <span className="mono" style={{ color: '#6c63ff' }}>{pct(stats?.cache?.hit_rate)}</span>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 4 }}>
              <span style={{ color: 'var(--muted)' }}>Backend</span>
              <span className="mono">{stats?.cache?.backend ?? '–'}</span>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--muted)' }}>LRU Size</span>
              <span className="mono">{stats?.cache?.lru_size ?? 0}</span>
            </div>
          </div>
        </div>

        {/* Rate Limit Controls */}
        <div className="control-card">
          <div className="control-title"><Shield size={14} /> Rate Limit Config</div>
          <div className="input-label">Agent</div>
          <select className="input-field" value={selectedAgent} onChange={e => setSelectedAgent(e.target.value)}>
            {agents.map(id => <option key={id} value={id}>{id}</option>)}
            {agents.length === 0 && <option value="agent-a">agent-a</option>}
          </select>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8, marginTop: 8 }}>
            <div>
              <div className="input-label">RPS (requests/sec)</div>
              <input className="input-field" placeholder={String(stats?.rate_limiter?.agents?.[selectedAgent]?.rps ?? 10)} value={rps} onChange={e => setRps(e.target.value)} type="number" min="0.1" step="0.5" />
            </div>
            <div>
              <div className="input-label">Burst Capacity</div>
              <input className="input-field" placeholder={String(stats?.rate_limiter?.agents?.[selectedAgent]?.burst ?? 20)} value={burst} onChange={e => setBurst(e.target.value)} type="number" min="1" />
            </div>
          </div>
          <button className="btn btn-primary" style={{ marginTop: 10, width: '100%' }} onClick={handleUpdateRL}>
            <RefreshCw size={13} /> Apply Changes
          </button>
        </div>

        {/* Test Request */}
        <div className="control-card" style={{ gridColumn: 'span 2' }}>
          <div className="control-title"><Send size={14} /> Test Request Sandbox</div>
          <div style={{ display: 'grid', gridTemplateColumns: '160px 1fr', gap: 8 }}>
            <div>
              <div className="input-label">Agent</div>
              <select className="input-field" value={testAgent} onChange={e => setTestAgent(e.target.value)}>
                {agents.map(id => <option key={id} value={id}>{id}</option>)}
                {agents.length === 0 && <>
                  <option value="agent-a">agent-a</option>
                  <option value="agent-b">agent-b</option>
                  <option value="agent-slow">agent-slow</option>
                </>}
              </select>
            </div>
            <div>
              <div className="input-label">Query Payload</div>
              <input className="input-field" value={testQuery} onChange={e => setTestQuery(e.target.value)} placeholder="Enter a query..." />
            </div>
          </div>
          <div className="input-row">
            <button className="btn btn-primary" onClick={() => handleTest(false)} disabled={loading}><Send size={13} /> Send (with cache)</button>
            <button className="btn btn-success" onClick={() => handleTest(true)} disabled={loading}><RefreshCw size={13} /> Send (bypass cache)</button>
            {loading && <span style={{ color: 'var(--muted)', fontSize: '0.8rem' }}>Waiting for agent...</span>}
          </div>
          {testResult && (
            <div style={{ marginTop: 12, background: 'var(--surface2)', borderRadius: 8, padding: 12, fontFamily: 'var(--mono)', fontSize: '0.75rem', whiteSpace: 'pre-wrap', maxHeight: 200, overflowY: 'auto' }}>
              {testResult.source && (
                <div style={{ marginBottom: 8, display: 'flex', gap: 12 }}>
                  <span style={{ color: testResult.source === 'cache' ? '#6c63ff' : testResult.source === 'queue' ? '#f59e0b' : '#00d4aa', fontWeight: 600 }}>
                    SOURCE: {testResult.source?.toUpperCase()}
                  </span>
                  <span style={{ color: 'var(--muted)' }}>Latency: {testResult.latency_ms?.toFixed(1)}ms</span>
                </div>
              )}
              {JSON.stringify(testResult.data ?? testResult, null, 2)}
            </div>
          )}
        </div>
      </div>
    </>
  )
}

// ─── Main App ─────────────────────────────────────────────────────────────────

export default function App() {
  const { stats, connected, history } = useLiveStats()
  const [tab, setTab] = useState<'overview' | 'agents' | 'admin'>('overview')

  const empty = !stats

  return (
    <div className="layout">
      {/* Topbar */}
      <header className="topbar">
        <div className="topbar-brand">
          <div className="logo-icon">
            <Zap size={18} color="#fff" />
          </div>
          <div>
            <h1>Resilient Agent Layer</h1>
            <span>Control Tower</span>
          </div>
        </div>
        <div className="topbar-right">
          <div className={`status-dot ${connected ? '' : 'status-dot-warn'}`}
            style={{ background: connected ? 'var(--success)' : 'var(--danger)', boxShadow: `0 0 8px ${connected ? 'var(--success)' : 'var(--danger)'}` }} />
          <span className="status-label">{connected ? 'Connected · Live' : 'Reconnecting...'}</span>
        </div>
      </header>

      {/* Main */}
      <main className="main-content">
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <div className="tabs">
            {(['overview', 'agents', 'admin'] as const).map(t => (
              <button key={t} className={`tab-btn ${tab === t ? 'active' : ''}`} onClick={() => setTab(t)}>
                {t.charAt(0).toUpperCase() + t.slice(1)}
              </button>
            ))}
          </div>
          {stats && (
            <span style={{ fontSize: '0.7rem', color: 'var(--dim)', fontFamily: 'var(--mono)' }}>
              Last update: {new Date(stats.timestamp * 1000).toLocaleTimeString()}
            </span>
          )}
        </div>

        {empty ? (
          <div style={{ display: 'grid', placeItems: 'center', flex: 1, color: 'var(--muted)', gap: 8, minHeight: 300 }}>
            <Activity size={40} color="#6c63ff" style={{ opacity: 0.5 }} />
            <div>Connecting to gateway...</div>
            <div style={{ fontSize: '0.75rem', color: 'var(--dim)' }}>Make sure the gateway is running on port 8000</div>
          </div>
        ) : (
          <>
            {tab === 'overview' && <OverviewTab stats={stats} history={history} />}
            {tab === 'agents' && <AgentsTab stats={stats} history={history} />}
            {tab === 'admin' && <AdminTab stats={stats} />}
          </>
        )}
      </main>

      <footer className="footer">
        Resilient Agent Request Layer · Nasiko Buildthon · Live data via SSE
      </footer>
    </div>
  )
}
