"use strict";

const {
  useEffect,
  useMemo,
  useRef,
  useState
} = React;
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
  const errorRate = agent.errorRate ?? agent.errorCount / Math.max(1, agent.totalRequests) * 100;
  if (agent.uptime < 98.5 || errorRate >= 6) return {
    className: "risk",
    label: "Risk"
  };
  if (agent.uptime < 99 || errorRate >= 3) return {
    className: "watch",
    label: "Watching"
  };
  return {
    className: "good",
    label: "Healthy"
  };
}
function trendCopy(value) {
  if (value < -8) return `${Math.abs(value)} ms faster`;
  if (value > 8) return `${value} ms slower`;
  return "Stable latency";
}
function aggregateHourly(selectedAgents) {
  return agents[0].hourly.map((point, index) => {
    const row = selectedAgents.reduce((acc, agent) => {
      const hour = agent.hourly[index];
      acc.responseMs += hour.responseMs;
      acc.p95ResponseMs += hour.p95ResponseMs;
      acc.successCount += hour.successCount;
      acc.errorCount += hour.errorCount;
      acc.uptime += hour.uptime;
      acc.saturation += hour.saturation;
      return acc;
    }, {
      hour: point.hour,
      responseMs: 0,
      p95ResponseMs: 0,
      successCount: 0,
      errorCount: 0,
      uptime: 0,
      saturation: 0
    });
    return {
      ...row,
      responseMs: Math.round(row.responseMs / selectedAgents.length),
      p95ResponseMs: Math.round(row.p95ResponseMs / selectedAgents.length),
      uptime: Number((row.uptime / selectedAgents.length).toFixed(2)),
      saturation: Math.round(row.saturation / selectedAgents.length)
    };
  });
}
function aggregateSummary(selectedAgents, hourly) {
  const totals = selectedAgents.reduce((acc, agent) => {
    acc.avgResponseMs += agent.avgResponseMs;
    acc.p95ResponseMs += agent.p95ResponseMs;
    acc.successCount += agent.successCount;
    acc.errorCount += agent.errorCount;
    acc.totalRequests += agent.totalRequests;
    acc.uptime += agent.uptime;
    acc.saturation += agent.saturation;
    acc.reliabilityScore += agent.reliabilityScore;
    return acc;
  }, {
    avgResponseMs: 0,
    p95ResponseMs: 0,
    successCount: 0,
    errorCount: 0,
    totalRequests: 0,
    uptime: 0,
    saturation: 0,
    reliabilityScore: 0
  });
  const hottestHour = hourly.reduce((best, point) => {
    const pointTotal = point.successCount + point.errorCount;
    const bestTotal = best.successCount + best.errorCount;
    return pointTotal > bestTotal ? point : best;
  }, hourly[0]);
  const responseTrendMs = Math.round(average(hourly.slice(-6).map(point => point.responseMs)) - average(hourly.slice(-12, -6).map(point => point.responseMs)));
  return {
    avgResponseMs: Math.round(totals.avgResponseMs / selectedAgents.length),
    p95ResponseMs: Math.round(totals.p95ResponseMs / selectedAgents.length),
    successCount: totals.successCount,
    errorCount: totals.errorCount,
    totalRequests: totals.totalRequests,
    errorRate: Number((totals.errorCount / Math.max(1, totals.totalRequests) * 100).toFixed(2)),
    uptime: Number((totals.uptime / selectedAgents.length).toFixed(2)),
    saturation: Math.round(totals.saturation / selectedAgents.length),
    reliabilityScore: Math.round(totals.reliabilityScore / selectedAgents.length),
    activeAgents: selectedAgents.length,
    responseTrendMs,
    hottestHour
  };
}
function cssVar(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}
function ChartCanvas(_ref) {
  let {
    config,
    className
  } = _ref;
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
  return React.createElement("div", {
    className: className
  }, React.createElement("canvas", {
    ref: canvasRef
  }));
}
function TrendPill(_ref2) {
  let {
    value,
    positiveGood = false
  } = _ref2;
  const isGood = positiveGood ? value >= 0 : value <= 0;
  const isFlat = Math.abs(value) <= 8;
  return React.createElement("span", {
    className: `trend-pill ${isFlat ? "flat" : isGood ? "good" : "bad"}`
  }, trendCopy(value));
}
function StatTile(_ref3) {
  let {
    label,
    value,
    detail,
    tone,
    children
  } = _ref3;
  return React.createElement("section", {
    className: `stat-tile ${tone}`
  }, React.createElement("span", null, label), React.createElement("strong", null, value), React.createElement("small", null, detail), children);
}
function ProgressBar(_ref4) {
  let {
    value,
    color
  } = _ref4;
  return React.createElement("span", {
    className: "progress-track",
    "aria-hidden": "true"
  }, React.createElement("span", {
    className: "progress-fill",
    style: {
      width: `${Math.min(100, value)}%`,
      background: color
    }
  }));
}
function SparkBars(_ref5) {
  let {
    points,
    color
  } = _ref5;
  const values = points.map(point => point.responseMs);
  const min = Math.min(...values);
  const max = Math.max(...values);
  return React.createElement("span", {
    className: "spark-bars",
    "aria-hidden": "true"
  }, values.map((value, index) => {
    const height = 24 + (value - min) / Math.max(1, max - min) * 56;
    return React.createElement("span", {
      key: `${value}-${index}`,
      style: {
        height: `${height}%`,
        background: color
      }
    });
  }));
}
function FleetHero(_ref6) {
  let {
    summary,
    activeLabel
  } = _ref6;
  const responseTone = summary.responseTrendMs <= 0 ? "good" : "bad";
  return React.createElement("section", {
    className: "command-panel"
  }, React.createElement("div", {
    className: "command-copy"
  }, React.createElement("span", {
    className: "eyebrow"
  }, "Nasiko observability"), React.createElement("h1", null, "Agent Performance Metrics"), React.createElement("p", null, "Last 24 hours across response latency, request outcomes, uptime, and fleet pressure.")), React.createElement("div", {
    className: "command-score",
    style: {
      "--score": `${summary.reliabilityScore}%`
    }
  }, React.createElement("div", {
    className: "score-ring"
  }, React.createElement("strong", null, summary.reliabilityScore), React.createElement("span", null, "Score")), React.createElement("div", {
    className: "score-copy"
  }, React.createElement("span", {
    className: "freshness"
  }, React.createElement("span", {
    className: "pulse"
  }), "Demo telemetry"), React.createElement("strong", null, activeLabel), React.createElement("small", null, summary.activeAgents, " agent view"))), React.createElement("div", {
    className: "command-brief"
  }, React.createElement("span", null, React.createElement("small", null, "P95 latency"), React.createElement("strong", null, msFormat(summary.p95ResponseMs))), React.createElement("span", null, React.createElement("small", null, "Error rate"), React.createElement("strong", null, percentFormat(summary.errorRate))), React.createElement("span", null, React.createElement("small", null, "Capacity"), React.createElement("strong", null, summary.saturation, "%")), React.createElement("span", {
    className: `brief-trend ${responseTone}`
  }, React.createElement("small", null, "Latency movement"), React.createElement("strong", null, trendCopy(summary.responseTrendMs)))));
}
function ExecutiveReadout(_ref7) {
  let {
    summary,
    activeAgent
  } = _ref7;
  const fastestAgent = [...agents].sort((a, b) => a.avgResponseMs - b.avgResponseMs)[0];
  const trafficLeader = [...agents].sort((a, b) => b.totalRequests - a.totalRequests)[0];
  const attentionAgent = activeAgent || [...agents].sort((a, b) => a.reliabilityScore - b.reliabilityScore)[0];
  const posture = summary.reliabilityScore >= 93 ? "Inside SLO" : "Needs watch";
  const items = [{
    tone: "green",
    label: "Fleet posture",
    value: posture,
    detail: `${percentFormat(summary.uptime)} uptime across ${summary.activeAgents} agents`
  }, {
    tone: "blue",
    label: "Fastest agent",
    value: fastestAgent.name,
    detail: `${msFormat(fastestAgent.avgResponseMs)} average response`
  }, {
    tone: "amber",
    label: "Attention point",
    value: attentionAgent.name,
    detail: `${msFormat(attentionAgent.p95ResponseMs)} P95 - ${percentFormat(attentionAgent.errorRate)} errors`
  }, {
    tone: "violet",
    label: "Traffic leader",
    value: trafficLeader.name,
    detail: `${numberFormat(trafficLeader.totalRequests)} requests completed`
  }];
  return React.createElement("section", {
    className: "readout-grid",
    "aria-label": "Executive telemetry readout"
  }, items.map(item => React.createElement("article", {
    className: `readout-card ${item.tone}`,
    key: item.label
  }, React.createElement("span", null, item.label), React.createElement("strong", null, item.value), React.createElement("small", null, item.detail))));
}
function AgentCard(_ref8) {
  let {
    agent,
    isActive,
    onSelect
  } = _ref8;
  const reliability = getReliability(agent);
  return React.createElement("button", {
    className: `agent-card ${isActive ? "active" : ""}`,
    onClick: () => onSelect(agent.id),
    type: "button",
    style: {
      "--agent-color": agent.color
    }
  }, React.createElement("span", {
    className: "agent-card-header"
  }, React.createElement("span", null, React.createElement("strong", null, agent.name), React.createElement("small", null, agent.mission)), React.createElement("span", {
    className: `status-pill ${reliability.className}`
  }, reliability.label)), React.createElement(SparkBars, {
    points: agent.hourly,
    color: agent.color
  }), React.createElement("span", {
    className: "agent-card-metrics"
  }, React.createElement("span", null, React.createElement("small", null, "Response"), React.createElement("strong", null, msFormat(agent.avgResponseMs))), React.createElement("span", null, React.createElement("small", null, "Success"), React.createElement("strong", null, numberFormat(agent.successCount))), React.createElement("span", null, React.createElement("small", null, "Errors"), React.createElement("strong", null, numberFormat(agent.errorCount))), React.createElement("span", null, React.createElement("small", null, "Uptime"), React.createElement("strong", null, percentFormat(agent.uptime)))), React.createElement("span", {
    className: "agent-card-footer"
  }, React.createElement("span", null, agent.owner), React.createElement("span", null, agent.region), React.createElement("span", null, agent.version)));
}
function ResponseChart(_ref9) {
  let {
    selectedAgents,
    hourly
  } = _ref9;
  const config = useMemo(() => {
    const grid = cssVar("--grid-line");
    const text = cssVar("--muted-text");
    return {
      type: "line",
      data: {
        labels: hourly.map(point => point.hour),
        datasets: selectedAgents.map(agent => ({
          label: agent.name,
          data: agent.hourly.map(point => point.responseMs),
          borderColor: agent.color,
          backgroundColor: `${agent.color}22`,
          tension: 0.42,
          borderWidth: 3,
          pointRadius: 0,
          pointHoverRadius: 5,
          fill: false
        }))
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        interaction: {
          mode: "index",
          intersect: false
        },
        plugins: {
          legend: {
            labels: {
              color: text,
              boxWidth: 10,
              boxHeight: 10,
              usePointStyle: true
            }
          },
          tooltip: {
            callbacks: {
              label: context => `${context.dataset.label}: ${context.raw} ms`
            }
          }
        },
        scales: {
          x: {
            grid: {
              display: false
            },
            ticks: {
              color: text,
              maxRotation: 0,
              autoSkip: true,
              maxTicksLimit: 8
            }
          },
          y: {
            grid: {
              color: grid
            },
            ticks: {
              color: text,
              callback: value => `${value} ms`
            }
          }
        }
      }
    };
  }, [selectedAgents, hourly]);
  return React.createElement(ChartCanvas, {
    config: config,
    className: "chart-shell tall"
  });
}
function TrafficChart(_ref10) {
  let {
    hourly
  } = _ref10;
  const config = useMemo(() => {
    const grid = cssVar("--grid-line");
    const text = cssVar("--muted-text");
    return {
      type: "bar",
      data: {
        labels: hourly.map(point => point.hour),
        datasets: [{
          label: "Success",
          data: hourly.map(point => point.successCount),
          backgroundColor: "#157a6e",
          borderRadius: 5,
          stack: "requests"
        }, {
          label: "Errors",
          data: hourly.map(point => point.errorCount),
          backgroundColor: "#cc4052",
          borderRadius: 5,
          stack: "requests"
        }]
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: {
            labels: {
              color: text,
              boxWidth: 10,
              boxHeight: 10,
              usePointStyle: true
            }
          }
        },
        scales: {
          x: {
            stacked: true,
            grid: {
              display: false
            },
            ticks: {
              color: text,
              maxRotation: 0,
              autoSkip: true,
              maxTicksLimit: 8
            }
          },
          y: {
            stacked: true,
            grid: {
              color: grid
            },
            ticks: {
              color: text,
              precision: 0
            }
          }
        }
      }
    };
  }, [hourly]);
  return React.createElement(ChartCanvas, {
    config: config,
    className: "chart-shell"
  });
}
function UptimeChart(_ref11) {
  let {
    hourly
  } = _ref11;
  const config = useMemo(() => {
    const grid = cssVar("--grid-line");
    const text = cssVar("--muted-text");
    return {
      type: "line",
      data: {
        labels: hourly.map(point => point.hour),
        datasets: [{
          label: "Uptime",
          data: hourly.map(point => point.uptime),
          borderColor: "#c47a14",
          backgroundColor: "#c47a1424",
          fill: true,
          tension: 0.38,
          borderWidth: 3,
          pointRadius: 0
        }]
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: {
            display: false
          },
          tooltip: {
            callbacks: {
              label: context => `Uptime: ${context.raw}%`
            }
          }
        },
        scales: {
          x: {
            grid: {
              display: false
            },
            ticks: {
              color: text,
              maxRotation: 0,
              autoSkip: true,
              maxTicksLimit: 6
            }
          },
          y: {
            min: 96,
            max: 100,
            grid: {
              color: grid
            },
            ticks: {
              color: text,
              callback: value => `${value}%`
            }
          }
        }
      }
    };
  }, [hourly]);
  return React.createElement(ChartCanvas, {
    config: config,
    className: "chart-shell"
  });
}
function PressureChart(_ref12) {
  let {
    hourly
  } = _ref12;
  const config = useMemo(() => {
    const grid = cssVar("--grid-line");
    const text = cssVar("--muted-text");
    return {
      type: "line",
      data: {
        labels: hourly.map(point => point.hour),
        datasets: [{
          label: "Saturation",
          data: hourly.map(point => point.saturation),
          borderColor: "#2563eb",
          backgroundColor: "#2563eb20",
          fill: true,
          tension: 0.36,
          borderWidth: 2,
          pointRadius: 0
        }, {
          label: "P95 response",
          data: hourly.map(point => Math.round(point.p95ResponseMs / 20)),
          borderColor: "#7c3aed",
          backgroundColor: "#7c3aed1f",
          fill: false,
          tension: 0.36,
          borderWidth: 2,
          pointRadius: 0
        }]
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: {
            labels: {
              color: text,
              boxWidth: 10,
              boxHeight: 10,
              usePointStyle: true
            }
          },
          tooltip: {
            callbacks: {
              label: context => context.dataset.label === "P95 response" ? `P95 response: ${context.raw * 20} ms` : `Saturation: ${context.raw}%`
            }
          }
        },
        scales: {
          x: {
            grid: {
              display: false
            },
            ticks: {
              color: text,
              maxRotation: 0,
              autoSkip: true,
              maxTicksLimit: 6
            }
          },
          y: {
            min: 0,
            max: 100,
            grid: {
              color: grid
            },
            ticks: {
              color: text,
              callback: value => `${value}`
            }
          }
        }
      }
    };
  }, [hourly]);
  return React.createElement(ChartCanvas, {
    config: config,
    className: "chart-shell"
  });
}
function InsightPanel(_ref13) {
  let {
    summary,
    activeAgent
  } = _ref13;
  const watchedAgents = [...agents].sort((a, b) => a.reliabilityScore - b.reliabilityScore).slice(0, 3);
  const bestAgent = [...agents].sort((a, b) => a.avgResponseMs - b.avgResponseMs)[0];
  const focusedAgent = activeAgent || watchedAgents[0];
  return React.createElement("section", {
    className: "panel insight-panel"
  }, React.createElement("div", {
    className: "panel-heading"
  }, React.createElement("div", null, React.createElement("span", {
    className: "eyebrow"
  }, "Operations pulse"), React.createElement("h2", null, "Judge-ready signals")), React.createElement("span", {
    className: "range-pill"
  }, "24h")), React.createElement("div", {
    className: "signal-stack"
  }, React.createElement("div", {
    className: "signal-card primary"
  }, React.createElement("span", null, "Highest attention"), React.createElement("strong", null, focusedAgent.name), React.createElement("small", null, percentFormat(focusedAgent.errorRate), " error rate - ", msFormat(focusedAgent.p95ResponseMs), " P95"), React.createElement(ProgressBar, {
    value: focusedAgent.saturation,
    color: focusedAgent.color
  })), React.createElement("div", {
    className: "signal-card"
  }, React.createElement("span", null, "Fastest performer"), React.createElement("strong", null, bestAgent.name), React.createElement("small", null, msFormat(bestAgent.avgResponseMs), " average response")), React.createElement("div", {
    className: "signal-card"
  }, React.createElement("span", null, "Peak traffic hour"), React.createElement("strong", null, summary.hottestHour.hour), React.createElement("small", null, numberFormat(summary.hottestHour.successCount + summary.hottestHour.errorCount), " requests"))), React.createElement("div", {
    className: "watch-list"
  }, watchedAgents.map(agent => {
    const reliability = getReliability(agent);
    return React.createElement("span", {
      key: agent.id
    }, React.createElement("span", {
      className: "watch-name"
    }, React.createElement("span", {
      className: "color-dot",
      style: {
        backgroundColor: agent.color
      }
    }), agent.name), React.createElement("strong", null, agent.reliabilityScore), React.createElement("small", {
      className: reliability.className
    }, reliability.label));
  })));
}
function HeatmapPanel() {
  return React.createElement("section", {
    className: "panel heatmap-panel"
  }, React.createElement("div", {
    className: "panel-heading"
  }, React.createElement("div", null, React.createElement("span", {
    className: "eyebrow"
  }, "Latency heatmap"), React.createElement("h2", null, "Agent pressure by hour"))), React.createElement("div", {
    className: "heatmap"
  }, agents.map(agent => React.createElement("div", {
    className: "heatmap-row",
    key: agent.id
  }, React.createElement("span", {
    className: "heatmap-label"
  }, agent.name), React.createElement("span", {
    className: "heatmap-cells"
  }, agent.hourly.map((point, index) => {
    const intensity = Math.min(1, Math.max(0.1, point.responseMs / agent.p95ResponseMs));
    return React.createElement("span", {
      key: `${agent.id}-${point.hour}-${index}`,
      title: `${agent.name} ${point.hour}: ${point.responseMs} ms`,
      style: {
        backgroundColor: agent.color,
        opacity: 0.24 + intensity * 0.66
      }
    });
  }))))));
}
function AgentTable(_ref14) {
  let {
    selectedAgentId,
    onSelect
  } = _ref14;
  return React.createElement("section", {
    className: "panel table-panel"
  }, React.createElement("div", {
    className: "panel-heading"
  }, React.createElement("div", null, React.createElement("span", {
    className: "eyebrow"
  }, "Per-agent stats"), React.createElement("h2", null, "Current 24-hour rollup"))), React.createElement("div", {
    className: "table-wrap"
  }, React.createElement("table", null, React.createElement("thead", null, React.createElement("tr", null, React.createElement("th", null, "Agent"), React.createElement("th", null, "Avg response"), React.createElement("th", null, "P95"), React.createElement("th", null, "Success"), React.createElement("th", null, "Errors"), React.createElement("th", null, "Uptime"), React.createElement("th", null, "Capacity"))), React.createElement("tbody", null, agents.map(agent => React.createElement("tr", {
    key: agent.id,
    className: selectedAgentId === agent.id ? "selected" : "",
    onClick: () => onSelect(agent.id)
  }, React.createElement("td", null, React.createElement("span", {
    className: "agent-name"
  }, React.createElement("span", {
    className: "color-dot",
    style: {
      backgroundColor: agent.color
    }
  }), React.createElement("span", null, React.createElement("strong", null, agent.name), React.createElement("small", null, agent.lane, " - ", agent.owner)))), React.createElement("td", null, msFormat(agent.avgResponseMs)), React.createElement("td", null, msFormat(agent.p95ResponseMs)), React.createElement("td", null, numberFormat(agent.successCount)), React.createElement("td", null, numberFormat(agent.errorCount)), React.createElement("td", null, percentFormat(agent.uptime)), React.createElement("td", null, agent.saturation, "%")))))));
}
function App() {
  const [selectedAgentId, setSelectedAgentId] = useState("all");
  const selectedAgents = useMemo(() => {
    if (selectedAgentId === "all") return agents;
    return agents.filter(agent => agent.id === selectedAgentId);
  }, [selectedAgentId]);
  const activeAgent = selectedAgentId === "all" ? null : selectedAgents[0];
  const hourly = useMemo(() => aggregateHourly(selectedAgents), [selectedAgents]);
  const summary = useMemo(() => aggregateSummary(selectedAgents, hourly), [selectedAgents, hourly]);
  const activeLabel = activeAgent ? activeAgent.name : "All agents";
  return React.createElement("main", {
    className: "page-shell"
  }, React.createElement(FleetHero, {
    summary: summary,
    activeLabel: activeLabel
  }), React.createElement("section", {
    className: "toolbar",
    "aria-label": "Agent filter"
  }, React.createElement("button", {
    className: selectedAgentId === "all" ? "selected" : "",
    onClick: () => setSelectedAgentId("all"),
    type: "button"
  }, "All agents"), agents.map(agent => React.createElement("button", {
    key: agent.id,
    className: selectedAgentId === agent.id ? "selected" : "",
    onClick: () => setSelectedAgentId(agent.id),
    type: "button"
  }, agent.name))), React.createElement("section", {
    className: "stats-grid"
  }, React.createElement(StatTile, {
    label: "Avg response",
    value: msFormat(summary.avgResponseMs),
    detail: activeLabel,
    tone: "latency"
  }, React.createElement(TrendPill, {
    value: summary.responseTrendMs
  })), React.createElement(StatTile, {
    label: "Success count",
    value: numberFormat(summary.successCount),
    detail: "Completed requests",
    tone: "success"
  }), React.createElement(StatTile, {
    label: "Error count",
    value: numberFormat(summary.errorCount),
    detail: `${percentFormat(summary.errorRate)} error rate`,
    tone: "error"
  }), React.createElement(StatTile, {
    label: "Uptime",
    value: percentFormat(summary.uptime),
    detail: `${summary.activeAgents} active agents`,
    tone: "uptime"
  }, React.createElement(ProgressBar, {
    value: summary.uptime,
    color: "#c47a14"
  }))), React.createElement(ExecutiveReadout, {
    summary: summary,
    activeAgent: activeAgent
  }), React.createElement("section", {
    className: "agent-grid"
  }, agents.map(agent => React.createElement(AgentCard, {
    key: agent.id,
    agent: agent,
    isActive: selectedAgentId === agent.id,
    onSelect: setSelectedAgentId
  }))), React.createElement("section", {
    className: "dashboard-grid"
  }, React.createElement("section", {
    className: "panel response-panel"
  }, React.createElement("div", {
    className: "panel-heading"
  }, React.createElement("div", null, React.createElement("span", {
    className: "eyebrow"
  }, "Latency trend"), React.createElement("h2", null, "Average response time")), React.createElement("span", {
    className: "range-pill"
  }, "24h")), React.createElement(ResponseChart, {
    selectedAgents: selectedAgents,
    hourly: hourly
  })), React.createElement(InsightPanel, {
    summary: summary,
    activeAgent: activeAgent
  }), React.createElement("section", {
    className: "panel"
  }, React.createElement("div", {
    className: "panel-heading"
  }, React.createElement("div", null, React.createElement("span", {
    className: "eyebrow"
  }, "Outcomes"), React.createElement("h2", null, "Success vs errors"))), React.createElement(TrafficChart, {
    hourly: hourly
  })), React.createElement("section", {
    className: "panel"
  }, React.createElement("div", {
    className: "panel-heading"
  }, React.createElement("div", null, React.createElement("span", {
    className: "eyebrow"
  }, "Availability"), React.createElement("h2", null, "Uptime percentage"))), React.createElement(UptimeChart, {
    hourly: hourly
  })), React.createElement("section", {
    className: "panel"
  }, React.createElement("div", {
    className: "panel-heading"
  }, React.createElement("div", null, React.createElement("span", {
    className: "eyebrow"
  }, "Capacity guardrail"), React.createElement("h2", null, "Saturation and P95 pressure"))), React.createElement(PressureChart, {
    hourly: hourly
  }))), React.createElement(HeatmapPanel, null), React.createElement(AgentTable, {
    selectedAgentId: selectedAgentId,
    onSelect: setSelectedAgentId
  }));
}
ReactDOM.createRoot(document.getElementById("root")).render(React.createElement(App, null));
