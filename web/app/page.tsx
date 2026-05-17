'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { motion } from 'framer-motion';
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { ChartTooltip } from '@/components/ChartTooltip';
import { StatCard } from '@/components/StatCard';
import { fetchAgentStats, fetchAgents, fetchSessions } from '@/lib/nasiko-client';
import {
  aggregateHourlyFromSessions,
  computeAgentRow,
  formatMs,
  startTime24hAgo,
} from '@/lib/metrics';
import type { AgentMetricsRow, HourlyBucket, ObsSession } from '@/lib/types';

const TOKEN_KEY = 'nasiko_metrics_token';

function IconBolt() {
  return (
    <svg className="h-6 w-6 text-white" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
    </svg>
  );
}

function IconCheck() {
  return (
    <svg className="h-6 w-6 text-white" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <path d="M22 11.08V12a10 10 0 11-5.93-9.14" />
      <polyline points="22 4 12 14.01 9 11.01" />
    </svg>
  );
}

function IconAlert() {
  return (
    <svg className="h-6 w-6 text-white" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <circle cx="12" cy="12" r="10" />
      <line x1="12" y1="8" x2="12" y2="12" />
      <line x1="12" y1="16" x2="12.01" y2="16" />
    </svg>
  );
}

function IconClock() {
  return (
    <svg className="h-6 w-6 text-white" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <circle cx="12" cy="12" r="10" />
      <polyline points="12 6 12 12 16 14" />
    </svg>
  );
}

export default function MetricsPage() {
  const [token, setToken] = useState('');
  const [tokenInput, setTokenInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [rows, setRows] = useState<AgentMetricsRow[]>([]);
  const [sessions, setSessions] = useState<ObsSession[]>([]);
  const [hourly, setHourly] = useState<HourlyBucket[]>([]);
  const [selectedAgent, setSelectedAgent] = useState<string | 'all'>('all');

  useEffect(() => {
    const saved = localStorage.getItem(TOKEN_KEY);
    if (saved) {
      setToken(saved);
      setTokenInput(saved);
    }
  }, []);

  const load = useCallback(async (authToken: string) => {
    setLoading(true);
    setError(null);
    const startTime = startTime24hAgo();
    try {
      const [agents, sessions] = await Promise.all([
        fetchAgents(authToken),
        fetchSessions(authToken, startTime),
      ]);

      const statsList = await Promise.all(
        agents.map((a) => fetchAgentStats(authToken, a.agent_id, startTime)),
      );

      const agentRows = agents.map((a, i) =>
        computeAgentRow(a.agent_id, a.name, sessions, statsList[i]),
      );

      setRows(agentRows);
      setSessions(sessions);
      setHourly(aggregateHourlyFromSessions(sessions));
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load metrics');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (token) void load(token);
  }, [token, load]);

  const agentHourly = useMemo(() => {
    if (selectedAgent === 'all') return hourly;
    return aggregateHourlyFromSessions(
      sessions.filter((s) => s.agent_id === selectedAgent),
    );
  }, [hourly, selectedAgent, sessions]);

  const totals = useMemo(() => {
    const success = rows.reduce((n, r) => n + r.successCount, 0);
    const errors = rows.reduce((n, r) => n + r.errorCount, 0);
    const avgMs =
      rows.length > 0
        ? Math.round(rows.reduce((n, r) => n + r.avgResponseMs, 0) / rows.length)
        : 0;
    const uptime =
      rows.length > 0
        ? Math.round(rows.reduce((n, r) => n + r.uptimePercent, 0) / rows.length)
        : 0;
    return { success, errors, avgMs, uptime, agents: rows.length };
  }, [rows]);

  function saveToken() {
    const t = tokenInput.trim();
    if (!t) return;
    localStorage.setItem(TOKEN_KEY, t);
    setToken(t);
  }

  return (
    <div className="mx-auto max-w-7xl px-4 py-8 sm:px-6">
      <motion.header
        initial={{ opacity: 0, y: -12 }}
        animate={{ opacity: 1, y: 0 }}
        className="mb-8"
      >
        <p className="text-xs font-semibold uppercase tracking-[0.2em]" style={{ color: '#21D4FD' }}>
          Nasiko Titan Builder Challenge
        </p>
        <h1 className="mt-2 text-3xl font-bold text-white md:text-4xl">
          Agent Performance Metrics
        </h1>
        <p className="mt-2 max-w-2xl text-sm" style={{ color: 'rgba(255,255,255,0.6)' }}>
          Per-agent response time, success/error counts, and uptime over the last 24 hours — powered by
          Nasiko observability APIs.
        </p>
      </motion.header>

      <div className="card-premium mb-6 flex flex-col gap-3 p-4 sm:flex-row sm:items-end">
        <label className="flex-1 text-sm">
          <span className="mb-1 block font-medium" style={{ color: 'rgba(255,255,255,0.55)' }}>
            API bearer token
          </span>
          <input
            type="password"
            value={tokenInput}
            onChange={(e) => setTokenInput(e.target.value)}
            placeholder="Paste token from nasiko login"
            className="w-full rounded-xl px-3 py-2.5 text-sm text-white outline-none"
            style={{
              background: 'rgba(255,255,255,0.04)',
              border: '1px solid rgba(255,255,255,0.1)',
            }}
          />
        </label>
        <button
          type="button"
          onClick={saveToken}
          className="rounded-xl px-5 py-2.5 text-sm font-semibold text-white"
          style={{
            background: 'linear-gradient(126.97deg, #0048ff 28.26%, #21D4FD 91.2%)',
          }}
        >
          Connect
        </button>
        <button
          type="button"
          disabled={!token || loading}
          onClick={() => void load(token)}
          className="rounded-xl px-5 py-2.5 text-sm font-medium text-white/80"
          style={{ border: '1px solid rgba(255,255,255,0.15)' }}
        >
          Refresh
        </button>
      </div>

      {error && (
        <div
          className="mb-6 rounded-xl px-4 py-3 text-sm"
          style={{
            background: 'rgba(245,87,68,0.1)',
            border: '1px solid rgba(245,87,68,0.3)',
            color: '#f5576c',
          }}
        >
          {error}
        </div>
      )}

      <div className="mb-6 grid gap-6 grid-cols-2 lg:grid-cols-4">
        <StatCard
          label="Agents"
          value={loading ? 0 : totals.agents}
          delay={0}
          icon={<IconBolt />}
        />
        <StatCard
          label="Avg uptime"
          value={loading ? 0 : totals.uptime}
          suffix="%"
          delay={0.05}
          icon={<IconClock />}
        />
        <StatCard
          label="Success (24h)"
          value={loading ? 0 : totals.success}
          delay={0.1}
          icon={<IconCheck />}
        />
        <StatCard
          label="Errors (24h)"
          value={loading ? 0 : totals.errors}
          delay={0.15}
          icon={<IconAlert />}
        />
      </div>

      <div className="mb-6 grid gap-6 lg:grid-cols-2">
        <motion.section
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.2 }}
          className="chart-panel"
        >
          <h2 className="mb-4 text-base font-bold text-white">Response time (24h)</h2>
          <ResponsiveContainer width="100%" height={260}>
            <AreaChart data={agentHourly} margin={{ top: 8, right: 8, left: -16, bottom: 0 }}>
              <defs>
                <linearGradient id="latGrad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#21D4FD" stopOpacity={0.85} />
                  <stop offset="100%" stopColor="#21D4FD" stopOpacity={0} />
                </linearGradient>
              </defs>
              <CartesianGrid strokeDasharray="4 4" stroke="rgba(255,255,255,0.07)" vertical={false} />
              <XAxis dataKey="label" tick={{ fill: 'rgba(255,255,255,0.55)', fontSize: 10 }} axisLine={false} tickLine={false} />
              <YAxis tick={{ fill: 'rgba(255,255,255,0.55)', fontSize: 10 }} axisLine={false} tickLine={false} width={40} />
              <Tooltip
                content={
                  <ChartTooltip accent="#21D4FD" formatter={(v) => formatMs(v)} />
                }
              />
              <Area type="monotone" dataKey="avgLatencyMs" name="Latency" stroke="#21D4FD" strokeWidth={2.5} fill="url(#latGrad)" />
            </AreaChart>
          </ResponsiveContainer>
        </motion.section>

        <motion.section
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.25 }}
          className="chart-panel"
        >
          <h2 className="mb-4 text-base font-bold text-white">Success vs error (24h)</h2>
          <ResponsiveContainer width="100%" height={260}>
            <BarChart data={agentHourly} margin={{ top: 8, right: 8, left: -16, bottom: 0 }}>
              <CartesianGrid strokeDasharray="4 4" stroke="rgba(255,255,255,0.07)" vertical={false} />
              <XAxis dataKey="label" tick={{ fill: 'rgba(255,255,255,0.55)', fontSize: 10 }} axisLine={false} tickLine={false} />
              <YAxis tick={{ fill: 'rgba(255,255,255,0.55)', fontSize: 10 }} axisLine={false} tickLine={false} width={32} allowDecimals={false} />
              <Tooltip content={<ChartTooltip accent="#01B574" unit="events" />} />
              <Legend wrapperStyle={{ color: 'rgba(255,255,255,0.7)', fontSize: 12 }} />
              <Bar dataKey="success" name="Success" fill="#01B574" radius={[4, 4, 0, 0]} />
              <Bar dataKey="error" name="Error" fill="#f5576c" radius={[4, 4, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        </motion.section>
      </div>

      <motion.section
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.3 }}
        className="card-premium overflow-hidden"
      >
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-white/10 px-6 py-4">
          <h2 className="text-base font-bold text-white">Per-agent breakdown</h2>
          <select
            value={selectedAgent}
            onChange={(e) => setSelectedAgent(e.target.value)}
            className="rounded-lg px-3 py-1.5 text-sm text-white outline-none"
            style={{ background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(255,255,255,0.1)' }}
          >
            <option value="all">All agents (charts)</option>
            {rows.map((r) => (
              <option key={r.agentId} value={r.agentId}>
                {r.name}
              </option>
            ))}
          </select>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full min-w-[640px] text-left text-sm">
            <thead>
              <tr style={{ color: 'rgba(255,255,255,0.5)' }}>
                <th className="px-6 py-3 font-medium">Agent</th>
                <th className="px-6 py-3 font-medium">Avg response</th>
                <th className="px-6 py-3 font-medium">Success</th>
                <th className="px-6 py-3 font-medium">Error</th>
                <th className="px-6 py-3 font-medium">Uptime</th>
                <th className="px-6 py-3 font-medium">Traces</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.agentId} className="border-t border-white/5 text-white/90">
                  <td className="px-6 py-3 font-medium">{r.name}</td>
                  <td className="px-6 py-3">{formatMs(r.avgResponseMs)}</td>
                  <td className="px-6 py-3" style={{ color: '#01B574' }}>
                    {r.successCount}
                  </td>
                  <td className="px-6 py-3" style={{ color: '#f5576c' }}>
                    {r.errorCount}
                  </td>
                  <td className="px-6 py-3">{r.uptimePercent}%</td>
                  <td className="px-6 py-3">{r.traceCount}</td>
                </tr>
              ))}
              {!loading && rows.length === 0 && (
                <tr>
                  <td colSpan={6} className="px-6 py-8 text-center text-sm leading-relaxed" style={{ color: 'rgba(255,255,255,0.55)' }}>
                    <p className="font-semibold text-white/80">No agents yet — that is why everything is zero.</p>
                    <p className="mt-2">
                      Upload one via the Nasiko UI ({' '}
                      <a href="http://localhost:9100/app/" className="underline" style={{ color: '#21D4FD' }}>
                        localhost:9100/app
                      </a>
                      ) or CLI:{' '}
                      <code className="text-white/60">nasiko agent upload-directory ./agents/callsense-agent --name callsense</code>
                    </p>
                    <p className="mt-2">Then run a few agent requests and click Refresh here.</p>
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </motion.section>

      <p className="mt-8 text-center text-xs" style={{ color: 'rgba(255,255,255,0.35)' }}>
        Official Nasiko Flutter UI ships as a Docker image; this React metrics app lives in{' '}
        <code className="text-white/50">nasiko/web</code> for Challenge 2.
      </p>
    </div>
  );
}
