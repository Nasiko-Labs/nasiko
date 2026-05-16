const { useEffect, useMemo, useRef, useState } = React;

const agents = window.NASIKO_AGENT_METRICS;

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
  return agents[0].hourly.map((point, index) => {
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

function FleetHero({ summary, activeLabel }) {
  const responseTone = summary.responseTrendMs <= 0 ? "good" : "bad";

  return (
    <section className="command-panel">
      <div className="command-copy">
        <span className="eyebrow">Nasiko observability</span>
        <h1>Agent Performance Metrics</h1>
        <p>Last 24 hours across response latency, request outcomes, uptime, and fleet pressure.</p>
      </div>

      <div className="command-score" style={{ "--score": `${summary.reliabilityScore}%` }}>
        <div className="score-ring">
          <strong>{summary.reliabilityScore}</strong>
          <span>Score</span>
        </div>
        <div className="score-copy">
          <span className="freshness">
            <span className="pulse" />
            Demo telemetry
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

function ExecutiveReadout({ summary, activeAgent }) {
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

function InsightPanel({ summary, activeAgent }) {
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

function HeatmapPanel() {
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

function AgentTable({ selectedAgentId, onSelect }) {
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

  const selectedAgents = useMemo(() => {
    if (selectedAgentId === "all") return agents;
    return agents.filter((agent) => agent.id === selectedAgentId);
  }, [selectedAgentId]);

  const activeAgent = selectedAgentId === "all" ? null : selectedAgents[0];
  const hourly = useMemo(() => aggregateHourly(selectedAgents), [selectedAgents]);
  const summary = useMemo(() => aggregateSummary(selectedAgents, hourly), [selectedAgents, hourly]);
  const activeLabel = activeAgent ? activeAgent.name : "All agents";

  return (
    <main className="page-shell">
      <FleetHero summary={summary} activeLabel={activeLabel} />

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

      <ExecutiveReadout summary={summary} activeAgent={activeAgent} />

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

        <InsightPanel summary={summary} activeAgent={activeAgent} />

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

      <HeatmapPanel />
      <AgentTable selectedAgentId={selectedAgentId} onSelect={setSelectedAgentId} />
    </main>
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(<App />);
