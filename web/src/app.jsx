const { useEffect, useMemo, useRef, useState } = React;

const demoAgents = window.NASIKO_DEMO_AGENT_METRICS || window.NASIKO_AGENT_METRICS || [];
const LIVE_AGENT_COLORS = ["#157a6e", "#5c5ff0", "#c47a14", "#cc4052", "#2563eb", "#7c3aed"];

function numberFormat(value) {
  return new Intl.NumberFormat().format(value);
}

function msFormat(value) {
  return `${numberFormat(Math.round(value))} ms`;
}

function percentFormat(value) {
  return `${Number(value).toFixed(value % 1 === 0 ? 0 : 2)}%`;
}

function average(values) {
  return values.reduce((total, value) => total + value, 0) / Math.max(1, values.length);
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function parseStoredToken(rawValue) {
  if (!rawValue) return "";

  try {
    const parsed = JSON.parse(rawValue);
    if (typeof parsed === "string") return parsed;
    return parsed.token || parsed.access_token || parsed.jwt_token || parsed.jwt || "";
  } catch (_error) {
    return rawValue;
  }
}

function getAuthHeader() {
  const tokenKeys = [
    "nasiko_token",
    "nasiko_jwt",
    "jwt_token",
    "auth_token",
    "authToken",
    "access_token",
    "token",
  ];

  for (const storage of [window.localStorage, window.sessionStorage]) {
    for (const key of tokenKeys) {
      const token = parseStoredToken(storage.getItem(key));
      if (token) {
        return token.toLowerCase().startsWith("bearer ") ? token : `Bearer ${token}`;
      }
    }
  }

  return "";
}

function getApiBaseCandidates() {
  const configuredBase =
    window.NASIKO_METRICS_CONFIG?.apiBaseUrl ||
    window.localStorage.getItem("nasiko_api_base_url") ||
    "";
  const originBase = `${window.location.origin}/api/v1`;
  const candidates = [
    configuredBase,
    originBase,
    "http://localhost:9100/api/v1",
    "http://127.0.0.1:9100/api/v1",
    "http://localhost:8000/api/v1",
    "http://127.0.0.1:8000/api/v1",
  ];

  return [...new Set(candidates.map((candidate) => candidate.replace(/\/$/, "")).filter(Boolean))];
}

function readNumber(...values) {
  for (const value of values) {
    const number = Number(value);
    if (Number.isFinite(number)) return number;
  }
  return 0;
}

function prettifyAgentName(agentId) {
  return String(agentId || "agent")
    .replace(/^agent[-_]/, "")
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

function sessionHasError(session) {
  const textParts = [];
  const annotations = session.session_annotations || [];
  const summaries = session.session_annotation_summaries || [];

  annotations.forEach((annotation) => {
    textParts.push(annotation.label, annotation.name);
    if (Number(annotation.score) <= 0) textParts.push("error");
  });

  summaries.forEach((summary) => {
    textParts.push(summary.name);
    (summary.label_fractions || []).forEach((fraction) => textParts.push(fraction.label));
  });

  const text = textParts.filter(Boolean).join(" ").toLowerCase();
  return /error|failed|failure|exception|timeout|critical/.test(text);
}

function getSessionErrorCount(session, traces) {
  const summaries = session.session_annotation_summaries || [];
  let errorFraction = 0;

  summaries.forEach((summary) => {
    (summary.label_fractions || []).forEach((fraction) => {
      const label = String(fraction.label || "").toLowerCase();
      if (/error|failed|failure|exception|timeout|critical/.test(label)) {
        errorFraction += Number(fraction.fraction) || 0;
      }
    });
  });

  if (errorFraction > 0) return Math.max(1, Math.round(traces * Math.min(1, errorFraction)));
  return sessionHasError(session) ? Math.max(1, Math.round(traces * 0.25)) : 0;
}

function buildEmptyHourlyBuckets() {
  const now = new Date();
  const start = new Date(now);
  start.setMinutes(0, 0, 0);
  start.setHours(start.getHours() - 23);

  return Array.from({ length: 24 }, (_, index) => {
    const timestamp = new Date(start);
    timestamp.setHours(start.getHours() + index);
    return {
      timestamp,
      hour: timestamp.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
      responseMsTotal: 0,
      p95ResponseMsTotal: 0,
      latencyWeight: 0,
      successCount: 0,
      errorCount: 0,
      uptime: 100,
      saturation: 0,
    };
  });
}

function summarizeLiveAgent(agent, hourly) {
  const maxTraffic = Math.max(...hourly.map((point) => point.successCount + point.errorCount), 1);
  const normalizedHourly = hourly.map((point) => {
    const requestCount = point.successCount + point.errorCount;
    const responseMs = point.latencyWeight
      ? Math.round(point.responseMsTotal / point.latencyWeight)
      : agent.avgResponseMs || 0;
    const p95ResponseMs = point.latencyWeight
      ? Math.round(point.p95ResponseMsTotal / point.latencyWeight)
      : Math.round(responseMs * 1.25);
    const uptime = requestCount
      ? Number((((point.successCount / requestCount) * 100)).toFixed(2))
      : 100;

    return {
      hour: point.hour,
      responseMs,
      p95ResponseMs,
      successCount: point.successCount,
      errorCount: point.errorCount,
      uptime,
      saturation: Math.round((requestCount / maxTraffic) * 100),
    };
  });

  const totals = normalizedHourly.reduce(
    (acc, point) => {
      acc.responseMs += point.responseMs * Math.max(1, point.successCount + point.errorCount);
      acc.p95ResponseMs += point.p95ResponseMs * Math.max(1, point.successCount + point.errorCount);
      acc.weight += Math.max(1, point.successCount + point.errorCount);
      acc.successCount += point.successCount;
      acc.errorCount += point.errorCount;
      acc.saturation += point.saturation;
      return acc;
    },
    { responseMs: 0, p95ResponseMs: 0, weight: 0, successCount: 0, errorCount: 0, saturation: 0 }
  );
  const totalRequests = totals.successCount + totals.errorCount;
  const avgResponseMs = Math.round(totals.responseMs / Math.max(1, totals.weight));
  const errorRate = Number(((totals.errorCount / Math.max(1, totalRequests)) * 100).toFixed(2));
  const uptime = Number((((totals.successCount / Math.max(1, totalRequests)) * 100)).toFixed(2));
  const recentResponse = average(normalizedHourly.slice(-6).map((point) => point.responseMs));
  const previousResponse = average(normalizedHourly.slice(-12, -6).map((point) => point.responseMs));

  return {
    ...agent,
    avgResponseMs,
    p95ResponseMs: Math.round(totals.p95ResponseMs / Math.max(1, totals.weight)),
    successCount: totals.successCount,
    errorCount: totals.errorCount,
    totalRequests,
    errorRate,
    uptime: totalRequests ? uptime : 100,
    saturation: Math.round(totals.saturation / normalizedHourly.length),
    responseTrendMs: Math.round(recentResponse - previousResponse),
    reliabilityScore: clamp(Math.round((totalRequests ? uptime : 100) - errorRate * 1.8 - avgResponseMs / 1200), 0, 100),
    hourly: normalizedHourly,
  };
}

function transformSessionsToAgents(sessions) {
  const grouped = new Map();
  const knownAgents = new Map(demoAgents.map((agent) => [agent.id, agent]));

  sessions.forEach((session, index) => {
    const agentId = session.agent_id || session.project_name || session.project_id || "unknown-agent";
    const knownAgent = knownAgents.get(agentId);
    const group =
      grouped.get(agentId) ||
      {
        id: agentId,
        name: knownAgent?.name || prettifyAgentName(agentId),
        lane: knownAgent?.lane || "Live",
        mission: knownAgent?.mission || "Observed from Nasiko traces",
        owner: knownAgent?.owner || "Nasiko",
        region: knownAgent?.region || "live",
        version: knownAgent?.version || "live",
        color: knownAgent?.color || LIVE_AGENT_COLORS[grouped.size % LIVE_AGENT_COLORS.length],
        hourly: buildEmptyHourlyBuckets(),
        avgResponseMs: 0,
      };

    const traces = Math.max(1, Math.round(readNumber(session.num_traces, session.trace_count, 1)));
    const errorCount = Math.min(traces, getSessionErrorCount(session, traces));
    const successCount = Math.max(0, traces - errorCount);
    const responseMs = Math.round(readNumber(session.trace_latency_ms_p50, session.latency_p50, session.latency_ms_p50, 0));
    const p95ResponseMs = Math.round(readNumber(session.trace_latency_ms_p99, session.latency_ms_p99, responseMs * 1.25));
    const startTime = new Date(session.start_time || session.created_at || Date.now());
    const bucketIndex = group.hourly.findIndex((bucket) => {
      const nextHour = new Date(bucket.timestamp);
      nextHour.setHours(bucket.timestamp.getHours() + 1);
      return startTime >= bucket.timestamp && startTime < nextHour;
    });
    const bucket = group.hourly[bucketIndex >= 0 ? bucketIndex : group.hourly.length - 1];
    const latency = responseMs || 0;
    const p95 = p95ResponseMs || Math.round(latency * 1.25);

    bucket.successCount += successCount;
    bucket.errorCount += errorCount;
    bucket.responseMsTotal += latency * traces;
    bucket.p95ResponseMsTotal += p95 * traces;
    bucket.latencyWeight += traces;

    grouped.set(agentId, group);
  });

  return [...grouped.values()]
    .map((agent) => summarizeLiveAgent(agent, agent.hourly))
    .filter((agent) => agent.totalRequests > 0);
}

async function fetchJsonWithTimeout(url, options = {}, timeoutMs = 6000) {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), timeoutMs);

  try {
    const response = await fetch(url, { ...options, signal: controller.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return await response.json();
  } finally {
    window.clearTimeout(timeout);
  }
}

async function loadLiveTelemetry() {
  const authHeader = getAuthHeader();
  if (!authHeader) {
    return { mode: "demo", agents: demoAgents, reason: "No auth token found for live observability API." };
  }

  const startTime = new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString();
  const headers = { Accept: "application/json", Authorization: authHeader };

  for (const apiBase of getApiBaseCandidates()) {
    try {
      const url = `${apiBase}/observability/session/list?start_time=${encodeURIComponent(startTime)}`;
      const payload = await fetchJsonWithTimeout(url, { headers, credentials: "include" });
      const sessions = payload?.data?.sessions || [];
      const liveAgents = Array.isArray(sessions) ? transformSessionsToAgents(sessions) : [];

      if (liveAgents.length > 0) {
        return {
          mode: "live",
          agents: liveAgents,
          reason: `Loaded ${sessions.length} sessions from ${apiBase}.`,
        };
      }
    } catch (_error) {
      // Try the next likely Nasiko API base and keep the dashboard usable.
    }
  }

  return { mode: "demo", agents: demoAgents, reason: "Live API unavailable or returned no sessions." };
}

function getReliability(agent) {
  const errorRate = agent.errorRate ?? (agent.errorCount / Math.max(1, agent.totalRequests)) * 100;
  if (agent.uptime < 98.5 || errorRate >= 6) return { className: "risk", label: "Risk" };
  if (agent.uptime < 99 || errorRate >= 3) return { className: "watch", label: "Watching" };
  return { className: "good", label: "Healthy" };
}

function trendCopy(value) {
  if (value < -8) return `${Math.abs(value)} ms faster`;
  if (value > 8) return `${value} ms slower`;
  return "Stable latency";
}

function aggregateHourly(selectedAgents) {
  if (!selectedAgents.length) return [];

  return selectedAgents[0].hourly.map((point, index) => {
    const row = selectedAgents.reduce(
      (acc, agent) => {
        const hour = agent.hourly[index];
        acc.responseMs += hour.responseMs;
        acc.p95ResponseMs += hour.p95ResponseMs;
        acc.successCount += hour.successCount;
        acc.errorCount += hour.errorCount;
        acc.uptime += hour.uptime;
        acc.saturation += hour.saturation;
        return acc;
      },
      {
        hour: point.hour,
        responseMs: 0,
        p95ResponseMs: 0,
        successCount: 0,
        errorCount: 0,
        uptime: 0,
        saturation: 0,
      }
    );

    return {
      ...row,
      responseMs: Math.round(row.responseMs / selectedAgents.length),
      p95ResponseMs: Math.round(row.p95ResponseMs / selectedAgents.length),
      uptime: Number((row.uptime / selectedAgents.length).toFixed(2)),
      saturation: Math.round(row.saturation / selectedAgents.length),
    };
  });
}

function aggregateSummary(selectedAgents, hourly) {
  if (!selectedAgents.length || !hourly.length) {
    return {
      avgResponseMs: 0,
      p95ResponseMs: 0,
      successCount: 0,
      errorCount: 0,
      totalRequests: 0,
      errorRate: 0,
      uptime: 0,
      saturation: 0,
      reliabilityScore: 0,
      activeAgents: 0,
      responseTrendMs: 0,
      hottestHour: { hour: "N/A", successCount: 0, errorCount: 0 },
    };
  }

  const totals = selectedAgents.reduce(
    (acc, agent) => {
      acc.avgResponseMs += agent.avgResponseMs;
      acc.p95ResponseMs += agent.p95ResponseMs;
      acc.successCount += agent.successCount;
      acc.errorCount += agent.errorCount;
      acc.totalRequests += agent.totalRequests;
      acc.uptime += agent.uptime;
      acc.saturation += agent.saturation;
      acc.reliabilityScore += agent.reliabilityScore;
      return acc;
    },
    {
      avgResponseMs: 0,
      p95ResponseMs: 0,
      successCount: 0,
      errorCount: 0,
      totalRequests: 0,
      uptime: 0,
      saturation: 0,
      reliabilityScore: 0,
    }
  );

  const hottestHour = hourly.reduce((best, point) => {
    const pointTotal = point.successCount + point.errorCount;
    const bestTotal = best.successCount + best.errorCount;
    return pointTotal > bestTotal ? point : best;
  }, hourly[0]);
  const responseTrendMs = Math.round(
    average(hourly.slice(-6).map((point) => point.responseMs)) -
      average(hourly.slice(-12, -6).map((point) => point.responseMs))
  );

  return {
    avgResponseMs: Math.round(totals.avgResponseMs / selectedAgents.length),
    p95ResponseMs: Math.round(totals.p95ResponseMs / selectedAgents.length),
    successCount: totals.successCount,
    errorCount: totals.errorCount,
    totalRequests: totals.totalRequests,
    errorRate: Number(((totals.errorCount / Math.max(1, totals.totalRequests)) * 100).toFixed(2)),
    uptime: Number((totals.uptime / selectedAgents.length).toFixed(2)),
    saturation: Math.round(totals.saturation / selectedAgents.length),
    reliabilityScore: Math.round(totals.reliabilityScore / selectedAgents.length),
    activeAgents: selectedAgents.length,
    responseTrendMs,
    hottestHour,
  };
}

function cssVar(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function ChartCanvas({ config, className }) {
  const canvasRef = useRef(null);
  const chartRef = useRef(null);

  useEffect(() => {
    if (!canvasRef.current) return undefined;

    if (chartRef.current) {
      chartRef.current.destroy();
    }

    chartRef.current = new Chart(canvasRef.current, config);

    return () => {
      if (chartRef.current) {
        chartRef.current.destroy();
      }
    };
  }, [config]);

  return (
    <div className={className}>
      <canvas ref={canvasRef} />
    </div>
  );
}

function TrendPill({ value, positiveGood = false }) {
  const isGood = positiveGood ? value >= 0 : value <= 0;
  const isFlat = Math.abs(value) <= 8;
  return <span className={`trend-pill ${isFlat ? "flat" : isGood ? "good" : "bad"}`}>{trendCopy(value)}</span>;
}

function StatTile({ label, value, detail, tone, children }) {
  return (
    <section className={`stat-tile ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
      {children}
    </section>
  );
}

function ProgressBar({ value, color }) {
  return (
    <span className="progress-track" aria-hidden="true">
      <span className="progress-fill" style={{ width: `${Math.min(100, value)}%`, background: color }} />
    </span>
  );
}

function SparkBars({ points, color }) {
  const values = points.map((point) => point.responseMs);
  const min = Math.min(...values);
  const max = Math.max(...values);

  return (
    <span className="spark-bars" aria-hidden="true">
      {values.map((value, index) => {
        const height = 24 + ((value - min) / Math.max(1, max - min)) * 56;
        return <span key={`${value}-${index}`} style={{ height: `${height}%`, background: color }} />;
      })}
    </span>
  );
}

function FleetHero({ summary, activeLabel, telemetry }) {
  const responseTone = summary.responseTrendMs <= 0 ? "good" : "bad";
  const telemetryLabel = telemetry.mode === "live" ? "Live telemetry" : "Demo telemetry";

  return (
    <section className="command-panel">
      <div className="command-copy">
        <span className="eyebrow">Nasiko observability</span>
        <h1>Agent Performance Metrics</h1>
        <p>Last 24 hours across response latency, request outcomes, uptime, and fleet pressure.</p>
      </div>

      <div className="command-score">
        <div className="score-ring" aria-label={`Reliability score ${percentFormat(summary.reliabilityScore)}`}>
          <svg className="score-ring-chart" viewBox="0 0 120 120" aria-hidden="true" focusable="false">
            <circle className="score-ring-track" cx="60" cy="60" r="48" pathLength="100" />
            <circle
              className="score-ring-value"
              cx="60"
              cy="60"
              r="48"
              pathLength="100"
              style={{ "--score-offset": 100 - summary.reliabilityScore }}
            />
          </svg>
          <span className="score-ring-copy">
            <strong>{summary.reliabilityScore}<small>%</small></strong>
            <span>Score</span>
          </span>
        </div>
        <div className="score-copy">
          <span className={`freshness ${telemetry.mode}`}>
            <span className="pulse" />
            {telemetryLabel}
          </span>
          <strong>{activeLabel}</strong>
          <small>{summary.activeAgents} agent view</small>
        </div>
      </div>

      <div className="command-brief">
        <span>
          <small>P95 latency</small>
          <strong>{msFormat(summary.p95ResponseMs)}</strong>
        </span>
        <span>
          <small>Error rate</small>
          <strong>{percentFormat(summary.errorRate)}</strong>
        </span>
        <span>
          <small>Capacity</small>
          <strong>{summary.saturation}%</strong>
        </span>
        <span className={`brief-trend ${responseTone}`}>
          <small>Latency movement</small>
          <strong>{trendCopy(summary.responseTrendMs)}</strong>
        </span>
      </div>
    </section>
  );
}

function ExecutiveReadout({ summary, activeAgent, agents }) {
  const fastestAgent = [...agents].sort((a, b) => a.avgResponseMs - b.avgResponseMs)[0];
  const trafficLeader = [...agents].sort((a, b) => b.totalRequests - a.totalRequests)[0];
  const attentionAgent =
    activeAgent || [...agents].sort((a, b) => a.reliabilityScore - b.reliabilityScore)[0];
  const posture = summary.reliabilityScore >= 93 ? "Inside SLO" : "Needs watch";

  const items = [
    {
      tone: "green",
      label: "Fleet posture",
      value: posture,
      detail: `${percentFormat(summary.uptime)} uptime across ${summary.activeAgents} agents`,
    },
    {
      tone: "blue",
      label: "Fastest agent",
      value: fastestAgent.name,
      detail: `${msFormat(fastestAgent.avgResponseMs)} average response`,
    },
    {
      tone: "amber",
      label: "Attention point",
      value: attentionAgent.name,
      detail: `${msFormat(attentionAgent.p95ResponseMs)} P95 - ${percentFormat(attentionAgent.errorRate)} errors`,
    },
    {
      tone: "violet",
      label: "Traffic leader",
      value: trafficLeader.name,
      detail: `${numberFormat(trafficLeader.totalRequests)} requests completed`,
    },
  ];

  return (
    <section className="readout-grid" aria-label="Executive telemetry readout">
      {items.map((item) => (
        <article className={`readout-card ${item.tone}`} key={item.label}>
          <span>{item.label}</span>
          <strong>{item.value}</strong>
          <small>{item.detail}</small>
        </article>
      ))}
    </section>
  );
}

function AgentCard({ agent, isActive, onSelect }) {
  const reliability = getReliability(agent);

  return (
    <button
      className={`agent-card ${isActive ? "active" : ""}`}
      onClick={() => onSelect(agent.id)}
      type="button"
      style={{ "--agent-color": agent.color }}
    >
      <span className="agent-card-header">
        <span>
          <strong>{agent.name}</strong>
          <small>{agent.mission}</small>
        </span>
        <span className={`status-pill ${reliability.className}`}>{reliability.label}</span>
      </span>

      <SparkBars points={agent.hourly} color={agent.color} />

      <span className="agent-card-metrics">
        <span>
          <small>Response</small>
          <strong>{msFormat(agent.avgResponseMs)}</strong>
        </span>
        <span>
          <small>Success</small>
          <strong>{numberFormat(agent.successCount)}</strong>
        </span>
        <span>
          <small>Errors</small>
          <strong>{numberFormat(agent.errorCount)}</strong>
        </span>
        <span>
          <small>Uptime</small>
          <strong>{percentFormat(agent.uptime)}</strong>
        </span>
      </span>

      <span className="agent-card-footer">
        <span>{agent.owner}</span>
        <span>{agent.region}</span>
        <span>{agent.version}</span>
      </span>
    </button>
  );
}

function ResponseChart({ selectedAgents, hourly }) {
  const config = useMemo(() => {
    const grid = cssVar("--grid-line");
    const text = cssVar("--muted-text");

    return {
      type: "line",
      data: {
        labels: hourly.map((point) => point.hour),
        datasets: selectedAgents.map((agent) => ({
          label: agent.name,
          data: agent.hourly.map((point) => point.responseMs),
          borderColor: agent.color,
          backgroundColor: `${agent.color}22`,
          tension: 0.42,
          borderWidth: 3,
          pointRadius: 0,
          pointHoverRadius: 5,
          fill: false,
        })),
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        interaction: { mode: "index", intersect: false },
        plugins: {
          legend: { labels: { color: text, boxWidth: 10, boxHeight: 10, usePointStyle: true } },
          tooltip: { callbacks: { label: (context) => `${context.dataset.label}: ${context.raw} ms` } },
        },
        scales: {
          x: { grid: { display: false }, ticks: { color: text, maxRotation: 0, autoSkip: true, maxTicksLimit: 8 } },
          y: { grid: { color: grid }, ticks: { color: text, callback: (value) => `${value} ms` } },
        },
      },
    };
  }, [selectedAgents, hourly]);

  return <ChartCanvas config={config} className="chart-shell tall" />;
}

function TrafficChart({ hourly }) {
  const config = useMemo(() => {
    const grid = cssVar("--grid-line");
    const text = cssVar("--muted-text");

    return {
      type: "bar",
      data: {
        labels: hourly.map((point) => point.hour),
        datasets: [
          {
            label: "Success",
            data: hourly.map((point) => point.successCount),
            backgroundColor: "#157a6e",
            borderRadius: 5,
            stack: "requests",
          },
          {
            label: "Errors",
            data: hourly.map((point) => point.errorCount),
            backgroundColor: "#cc4052",
            borderRadius: 5,
            stack: "requests",
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: { labels: { color: text, boxWidth: 10, boxHeight: 10, usePointStyle: true } },
        },
        scales: {
          x: { stacked: true, grid: { display: false }, ticks: { color: text, maxRotation: 0, autoSkip: true, maxTicksLimit: 8 } },
          y: { stacked: true, grid: { color: grid }, ticks: { color: text, precision: 0 } },
        },
      },
    };
  }, [hourly]);

  return <ChartCanvas config={config} className="chart-shell" />;
}

function UptimeChart({ hourly }) {
  const config = useMemo(() => {
    const grid = cssVar("--grid-line");
    const text = cssVar("--muted-text");

    return {
      type: "line",
      data: {
        labels: hourly.map((point) => point.hour),
        datasets: [
          {
            label: "Uptime",
            data: hourly.map((point) => point.uptime),
            borderColor: "#c47a14",
            backgroundColor: "#c47a1424",
            fill: true,
            tension: 0.38,
            borderWidth: 3,
            pointRadius: 0,
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: { display: false },
          tooltip: { callbacks: { label: (context) => `Uptime: ${context.raw}%` } },
        },
        scales: {
          x: { grid: { display: false }, ticks: { color: text, maxRotation: 0, autoSkip: true, maxTicksLimit: 6 } },
          y: {
            min: 96,
            max: 100,
            grid: { color: grid },
            ticks: { color: text, callback: (value) => `${value}%` },
          },
        },
      },
    };
  }, [hourly]);

  return <ChartCanvas config={config} className="chart-shell" />;
}

function PressureChart({ hourly }) {
  const config = useMemo(() => {
    const grid = cssVar("--grid-line");
    const text = cssVar("--muted-text");

    return {
      type: "line",
      data: {
        labels: hourly.map((point) => point.hour),
        datasets: [
          {
            label: "Saturation",
            data: hourly.map((point) => point.saturation),
            borderColor: "#2563eb",
            backgroundColor: "#2563eb20",
            fill: true,
            tension: 0.36,
            borderWidth: 2,
            pointRadius: 0,
          },
          {
            label: "P95 response",
            data: hourly.map((point) => Math.round(point.p95ResponseMs / 20)),
            borderColor: "#7c3aed",
            backgroundColor: "#7c3aed1f",
            fill: false,
            tension: 0.36,
            borderWidth: 2,
            pointRadius: 0,
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: { labels: { color: text, boxWidth: 10, boxHeight: 10, usePointStyle: true } },
          tooltip: {
            callbacks: {
              label: (context) =>
                context.dataset.label === "P95 response"
                  ? `P95 response: ${context.raw * 20} ms`
                  : `Saturation: ${context.raw}%`,
            },
          },
        },
        scales: {
          x: { grid: { display: false }, ticks: { color: text, maxRotation: 0, autoSkip: true, maxTicksLimit: 6 } },
          y: { min: 0, max: 100, grid: { color: grid }, ticks: { color: text, callback: (value) => `${value}` } },
        },
      },
    };
  }, [hourly]);

  return <ChartCanvas config={config} className="chart-shell" />;
}

function InsightPanel({ summary, activeAgent, agents }) {
  const watchedAgents = [...agents].sort((a, b) => a.reliabilityScore - b.reliabilityScore).slice(0, 3);
  const bestAgent = [...agents].sort((a, b) => a.avgResponseMs - b.avgResponseMs)[0];
  const focusedAgent = activeAgent || watchedAgents[0];

  return (
    <section className="panel insight-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Operations pulse</span>
          <h2>Judge-ready signals</h2>
        </div>
        <span className="range-pill">24h</span>
      </div>

      <div className="signal-stack">
        <div className="signal-card primary">
          <span>Highest attention</span>
          <strong>{focusedAgent.name}</strong>
          <small>{percentFormat(focusedAgent.errorRate)} error rate - {msFormat(focusedAgent.p95ResponseMs)} P95</small>
          <ProgressBar value={focusedAgent.saturation} color={focusedAgent.color} />
        </div>
        <div className="signal-card">
          <span>Fastest performer</span>
          <strong>{bestAgent.name}</strong>
          <small>{msFormat(bestAgent.avgResponseMs)} average response</small>
        </div>
        <div className="signal-card">
          <span>Peak traffic hour</span>
          <strong>{summary.hottestHour.hour}</strong>
          <small>{numberFormat(summary.hottestHour.successCount + summary.hottestHour.errorCount)} requests</small>
        </div>
      </div>

      <div className="watch-list">
        {watchedAgents.map((agent) => {
          const reliability = getReliability(agent);
          return (
            <span key={agent.id}>
              <span className="watch-name">
                <span className="color-dot" style={{ backgroundColor: agent.color }} />
                {agent.name}
              </span>
              <strong>{agent.reliabilityScore}</strong>
              <small className={reliability.className}>{reliability.label}</small>
            </span>
          );
        })}
      </div>
    </section>
  );
}

function HeatmapPanel({ agents }) {
  return (
    <section className="panel heatmap-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Latency heatmap</span>
          <h2>Agent pressure by hour</h2>
        </div>
      </div>
      <div className="heatmap">
        {agents.map((agent) => (
          <div className="heatmap-row" key={agent.id}>
            <span className="heatmap-label">{agent.name}</span>
            <span className="heatmap-cells">
              {agent.hourly.map((point, index) => {
                const intensity = Math.min(1, Math.max(0.1, point.responseMs / agent.p95ResponseMs));
                return (
                  <span
                    key={`${agent.id}-${point.hour}-${index}`}
                    title={`${agent.name} ${point.hour}: ${point.responseMs} ms`}
                    style={{
                      backgroundColor: agent.color,
                      opacity: 0.24 + intensity * 0.66,
                    }}
                  />
                );
              })}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}

function AgentTable({ agents, selectedAgentId, onSelect }) {
  return (
    <section className="panel table-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Per-agent stats</span>
          <h2>Current 24-hour rollup</h2>
        </div>
      </div>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Agent</th>
              <th>Avg response</th>
              <th>P95</th>
              <th>Success</th>
              <th>Errors</th>
              <th>Uptime</th>
              <th>Capacity</th>
            </tr>
          </thead>
          <tbody>
            {agents.map((agent) => (
              <tr
                key={agent.id}
                className={selectedAgentId === agent.id ? "selected" : ""}
                onClick={() => onSelect(agent.id)}
              >
                <td>
                  <span className="agent-name">
                    <span className="color-dot" style={{ backgroundColor: agent.color }} />
                    <span>
                      <strong>{agent.name}</strong>
                      <small>{agent.lane} - {agent.owner}</small>
                    </span>
                  </span>
                </td>
                <td>{msFormat(agent.avgResponseMs)}</td>
                <td>{msFormat(agent.p95ResponseMs)}</td>
                <td>{numberFormat(agent.successCount)}</td>
                <td>{numberFormat(agent.errorCount)}</td>
                <td>{percentFormat(agent.uptime)}</td>
                <td>{agent.saturation}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function App() {
  const [selectedAgentId, setSelectedAgentId] = useState("all");
  const [telemetry, setTelemetry] = useState({
    mode: "demo",
    agents: demoAgents,
    reason: "Using bundled demo telemetry while checking for live data.",
  });

  useEffect(() => {
    let isActive = true;

    loadLiveTelemetry().then((result) => {
      if (isActive) setTelemetry(result);
    });

    return () => {
      isActive = false;
    };
  }, []);

  const agents = telemetry.agents.length ? telemetry.agents : demoAgents;

  useEffect(() => {
    if (selectedAgentId !== "all" && !agents.some((agent) => agent.id === selectedAgentId)) {
      setSelectedAgentId("all");
    }
  }, [agents, selectedAgentId]);

  const selectedAgents = useMemo(() => {
    if (selectedAgentId === "all") return agents;
    const matchingAgents = agents.filter((agent) => agent.id === selectedAgentId);
    return matchingAgents.length ? matchingAgents : agents;
  }, [agents, selectedAgentId]);

  const activeAgent =
    selectedAgentId === "all"
      ? null
      : agents.find((agent) => agent.id === selectedAgentId) || null;
  const hourly = useMemo(() => aggregateHourly(selectedAgents), [selectedAgents]);
  const summary = useMemo(() => aggregateSummary(selectedAgents, hourly), [selectedAgents, hourly]);
  const activeLabel = activeAgent ? activeAgent.name : "All agents";

  return (
    <main className="page-shell">
      <FleetHero summary={summary} activeLabel={activeLabel} telemetry={telemetry} />

      <section className="toolbar" aria-label="Agent filter">
        <button
          className={selectedAgentId === "all" ? "selected" : ""}
          onClick={() => setSelectedAgentId("all")}
          type="button"
        >
          All agents
        </button>
        {agents.map((agent) => (
          <button
            key={agent.id}
            className={selectedAgentId === agent.id ? "selected" : ""}
            onClick={() => setSelectedAgentId(agent.id)}
            type="button"
          >
            {agent.name}
          </button>
        ))}
      </section>

      <section className="stats-grid">
        <StatTile label="Avg response" value={msFormat(summary.avgResponseMs)} detail={activeLabel} tone="latency">
          <TrendPill value={summary.responseTrendMs} />
        </StatTile>
        <StatTile label="Success count" value={numberFormat(summary.successCount)} detail="Completed requests" tone="success" />
        <StatTile label="Error count" value={numberFormat(summary.errorCount)} detail={`${percentFormat(summary.errorRate)} error rate`} tone="error" />
        <StatTile label="Uptime" value={percentFormat(summary.uptime)} detail={`${summary.activeAgents} active agents`} tone="uptime">
          <ProgressBar value={summary.uptime} color="#c47a14" />
        </StatTile>
      </section>

      <p className={`telemetry-note ${telemetry.mode}`}>{telemetry.reason}</p>

      <ExecutiveReadout summary={summary} activeAgent={activeAgent} agents={agents} />

      <section className="agent-grid">
        {agents.map((agent) => (
          <AgentCard
            key={agent.id}
            agent={agent}
            isActive={selectedAgentId === agent.id}
            onSelect={setSelectedAgentId}
          />
        ))}
      </section>

      <section className="dashboard-grid">
        <section className="panel response-panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">Latency trend</span>
              <h2>Average response time</h2>
            </div>
            <span className="range-pill">24h</span>
          </div>
          <ResponseChart selectedAgents={selectedAgents} hourly={hourly} />
        </section>

        <InsightPanel summary={summary} activeAgent={activeAgent} agents={agents} />

        <section className="panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">Outcomes</span>
              <h2>Success vs errors</h2>
            </div>
          </div>
          <TrafficChart hourly={hourly} />
        </section>

        <section className="panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">Availability</span>
              <h2>Uptime percentage</h2>
            </div>
          </div>
          <UptimeChart hourly={hourly} />
        </section>

        <section className="panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">Capacity guardrail</span>
              <h2>Saturation and P95 pressure</h2>
            </div>
          </div>
          <PressureChart hourly={hourly} />
        </section>
      </section>

      <HeatmapPanel agents={agents} />
      <AgentTable agents={agents} selectedAgentId={selectedAgentId} onSelect={setSelectedAgentId} />
    </main>
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(<App />);
