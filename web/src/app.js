const {
  useEffect,
  useMemo,
  useRef,
  useState
} = React;
const demoAgents = window.NASIKO_DEMO_AGENT_METRICS || window.NASIKO_AGENT_METRICS || [];
const LIVE_AGENT_COLORS = ["#157a6e", "#5c5ff0", "#c47a14", "#cc4052", "#2563eb", "#7c3aed"];
const DEFAULT_CURSOR_AGENT_ENDPOINT = "http://127.0.0.1:8787/metrics-agent";
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
  const tokenKeys = ["nasiko_token", "nasiko_jwt", "jwt_token", "auth_token", "authToken", "access_token", "token"];
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
  const configuredBase = window.NASIKO_METRICS_CONFIG?.apiBaseUrl || window.localStorage.getItem("nasiko_api_base_url") || "";
  const originBase = `${window.location.origin}/api/v1`;
  const candidates = [configuredBase, originBase, "http://localhost:9100/api/v1", "http://127.0.0.1:9100/api/v1", "http://localhost:8000/api/v1", "http://127.0.0.1:8000/api/v1"];
  return [...new Set(candidates.map(candidate => candidate.replace(/\/$/, "")).filter(Boolean))];
}
function readNumber(...values) {
  for (const value of values) {
    const number = Number(value);
    if (Number.isFinite(number)) return number;
  }
  return 0;
}
function prettifyAgentName(agentId) {
  return String(agentId || "agent").replace(/^agent[-_]/, "").split(/[-_\s]+/).filter(Boolean).map(word => word.charAt(0).toUpperCase() + word.slice(1)).join(" ");
}
function sessionHasError(session) {
  const textParts = [];
  const annotations = session.session_annotations || [];
  const summaries = session.session_annotation_summaries || [];
  annotations.forEach(annotation => {
    textParts.push(annotation.label, annotation.name);
    if (Number(annotation.score) <= 0) textParts.push("error");
  });
  summaries.forEach(summary => {
    textParts.push(summary.name);
    (summary.label_fractions || []).forEach(fraction => textParts.push(fraction.label));
  });
  const text = textParts.filter(Boolean).join(" ").toLowerCase();
  return /error|failed|failure|exception|timeout|critical/.test(text);
}
function getSessionErrorCount(session, traces) {
  const summaries = session.session_annotation_summaries || [];
  let errorFraction = 0;
  summaries.forEach(summary => {
    (summary.label_fractions || []).forEach(fraction => {
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
  return Array.from({
    length: 24
  }, (_, index) => {
    const timestamp = new Date(start);
    timestamp.setHours(start.getHours() + index);
    return {
      timestamp,
      hour: timestamp.toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit"
      }),
      responseMsTotal: 0,
      p95ResponseMsTotal: 0,
      latencyWeight: 0,
      successCount: 0,
      errorCount: 0,
      uptime: 100,
      saturation: 0
    };
  });
}
function summarizeLiveAgent(agent, hourly) {
  const maxTraffic = Math.max(...hourly.map(point => point.successCount + point.errorCount), 1);
  const normalizedHourly = hourly.map(point => {
    const requestCount = point.successCount + point.errorCount;
    const responseMs = point.latencyWeight ? Math.round(point.responseMsTotal / point.latencyWeight) : agent.avgResponseMs || 0;
    const p95ResponseMs = point.latencyWeight ? Math.round(point.p95ResponseMsTotal / point.latencyWeight) : Math.round(responseMs * 1.25);
    const uptime = requestCount ? Number((point.successCount / requestCount * 100).toFixed(2)) : 100;
    return {
      hour: point.hour,
      responseMs,
      p95ResponseMs,
      successCount: point.successCount,
      errorCount: point.errorCount,
      uptime,
      saturation: Math.round(requestCount / maxTraffic * 100)
    };
  });
  const totals = normalizedHourly.reduce((acc, point) => {
    acc.responseMs += point.responseMs * Math.max(1, point.successCount + point.errorCount);
    acc.p95ResponseMs += point.p95ResponseMs * Math.max(1, point.successCount + point.errorCount);
    acc.weight += Math.max(1, point.successCount + point.errorCount);
    acc.successCount += point.successCount;
    acc.errorCount += point.errorCount;
    acc.saturation += point.saturation;
    return acc;
  }, {
    responseMs: 0,
    p95ResponseMs: 0,
    weight: 0,
    successCount: 0,
    errorCount: 0,
    saturation: 0
  });
  const totalRequests = totals.successCount + totals.errorCount;
  const avgResponseMs = Math.round(totals.responseMs / Math.max(1, totals.weight));
  const errorRate = Number((totals.errorCount / Math.max(1, totalRequests) * 100).toFixed(2));
  const uptime = Number((totals.successCount / Math.max(1, totalRequests) * 100).toFixed(2));
  const recentResponse = average(normalizedHourly.slice(-6).map(point => point.responseMs));
  const previousResponse = average(normalizedHourly.slice(-12, -6).map(point => point.responseMs));
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
    hourly: normalizedHourly
  };
}
function transformSessionsToAgents(sessions) {
  const grouped = new Map();
  const knownAgents = new Map(demoAgents.map(agent => [agent.id, agent]));
  sessions.forEach((session, index) => {
    const agentId = session.agent_id || session.project_name || session.project_id || "unknown-agent";
    const knownAgent = knownAgents.get(agentId);
    const group = grouped.get(agentId) || {
      id: agentId,
      name: knownAgent?.name || prettifyAgentName(agentId),
      lane: knownAgent?.lane || "Live",
      mission: knownAgent?.mission || "Observed from Nasiko traces",
      owner: knownAgent?.owner || "Nasiko",
      region: knownAgent?.region || "live",
      version: knownAgent?.version || "live",
      color: knownAgent?.color || LIVE_AGENT_COLORS[grouped.size % LIVE_AGENT_COLORS.length],
      hourly: buildEmptyHourlyBuckets(),
      avgResponseMs: 0
    };
    const traces = Math.max(1, Math.round(readNumber(session.num_traces, session.trace_count, 1)));
    const errorCount = Math.min(traces, getSessionErrorCount(session, traces));
    const successCount = Math.max(0, traces - errorCount);
    const responseMs = Math.round(readNumber(session.trace_latency_ms_p50, session.latency_p50, session.latency_ms_p50, 0));
    const p95ResponseMs = Math.round(readNumber(session.trace_latency_ms_p99, session.latency_ms_p99, responseMs * 1.25));
    const startTime = new Date(session.start_time || session.created_at || Date.now());
    const bucketIndex = group.hourly.findIndex(bucket => {
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
  return [...grouped.values()].map(agent => summarizeLiveAgent(agent, agent.hourly)).filter(agent => agent.totalRequests > 0);
}
async function fetchJsonWithTimeout(url, options = {}, timeoutMs = 6000) {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, {
      ...options,
      signal: controller.signal
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return await response.json();
  } finally {
    window.clearTimeout(timeout);
  }
}
function getCursorAgentEndpoint() {
  return (window.NASIKO_METRICS_CONFIG?.cursorAgentEndpoint || window.localStorage.getItem("nasiko_cursor_agent_endpoint") || DEFAULT_CURSOR_AGENT_ENDPOINT).trim();
}
function getCursorAgentHealthEndpoint(endpoint) {
  try {
    const url = new URL(endpoint);
    url.pathname = "/health";
    url.search = "";
    return url.toString();
  } catch (_error) {
    return "";
  }
}
async function checkCursorAgentBridge(endpoint) {
  const healthEndpoint = getCursorAgentHealthEndpoint(endpoint);
  if (!healthEndpoint) return false;
  try {
    const health = await fetchJsonWithTimeout(healthEndpoint, {
      method: "GET"
    }, 1800);
    return health?.status === "ready";
  } catch (_error) {
    return false;
  }
}
async function askCursorAgent(question, context, endpoint = getCursorAgentEndpoint()) {
  if (!endpoint) return null;
  const authHeader = getAuthHeader();
  const headers = {
    Accept: "application/json",
    "Content-Type": "application/json"
  };
  if (authHeader) headers.Authorization = authHeader;
  const payload = await fetchJsonWithTimeout(endpoint, {
    method: "POST",
    headers,
    body: JSON.stringify({
      question,
      context,
      source: "nasiko-challenge-2-metrics"
    })
  }, 10000);
  return payload?.answer || payload?.message || payload?.response || payload?.text || payload?.data?.answer || payload?.choices?.[0]?.message?.content || null;
}
async function loadLiveTelemetry() {
  const authHeader = getAuthHeader();
  if (!authHeader) {
    return {
      mode: "demo",
      agents: demoAgents,
      reason: "No auth token found for live observability API."
    };
  }
  const startTime = new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString();
  const headers = {
    Accept: "application/json",
    Authorization: authHeader
  };
  for (const apiBase of getApiBaseCandidates()) {
    try {
      const url = `${apiBase}/observability/session/list?start_time=${encodeURIComponent(startTime)}`;
      const payload = await fetchJsonWithTimeout(url, {
        headers,
        credentials: "include"
      });
      const sessions = payload?.data?.sessions || [];
      const liveAgents = Array.isArray(sessions) ? transformSessionsToAgents(sessions) : [];
      if (liveAgents.length > 0) {
        return {
          mode: "live",
          agents: liveAgents,
          reason: `Loaded ${sessions.length} sessions from ${apiBase}.`
        };
      }
    } catch (_error) {
      // Try the next likely Nasiko API base and keep the dashboard usable.
    }
  }
  return {
    mode: "demo",
    agents: demoAgents,
    reason: "Live API unavailable or returned no sessions."
  };
}
function agentErrorRate(agent) {
  return agent.errorRate ?? Number((agent.errorCount / Math.max(1, agent.totalRequests) * 100).toFixed(2));
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
  if (!selectedAgents.length) return [];
  return selectedAgents[0].hourly.map((point, index) => {
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
      hottestHour: {
        hour: "N/A",
        successCount: 0,
        errorCount: 0
      }
    };
  }
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
function describeAgent(agent) {
  if (!agent) return "No agent is selected.";
  return `${agent.name}: ${msFormat(agent.avgResponseMs)} average response, ${msFormat(agent.p95ResponseMs)} P95, ${numberFormat(agent.successCount)} successes, ${numberFormat(agent.errorCount)} errors, ${percentFormat(agent.uptime)} uptime, and ${agent.saturation}% capacity.`;
}
function findMentionedAgent(question, agents) {
  const normalizedQuestion = question.toLowerCase();
  return agents.find(agent => {
    const name = agent.name.toLowerCase();
    const id = agent.id.toLowerCase();
    return normalizedQuestion.includes(name) || normalizedQuestion.includes(id);
  });
}
function buildMetricsChatContext(summary, activeAgent, activeLabel, agents, telemetry) {
  const fastestAgent = [...agents].sort((a, b) => a.avgResponseMs - b.avgResponseMs)[0];
  const slowestAgent = [...agents].sort((a, b) => b.avgResponseMs - a.avgResponseMs)[0];
  const mostErrorsAgent = [...agents].sort((a, b) => b.errorCount - a.errorCount)[0];
  const lowestUptimeAgent = [...agents].sort((a, b) => a.uptime - b.uptime)[0];
  const highestPressureAgent = [...agents].sort((a, b) => b.saturation - a.saturation)[0];
  const attentionAgent = [...agents].sort((a, b) => a.reliabilityScore - b.reliabilityScore)[0];
  return {
    telemetryMode: telemetry.mode,
    telemetryReason: telemetry.reason,
    activeLabel,
    summary,
    activeAgent,
    fastestAgent,
    slowestAgent,
    mostErrorsAgent,
    lowestUptimeAgent,
    highestPressureAgent,
    attentionAgent,
    agents: agents.map(agent => ({
      id: agent.id,
      name: agent.name,
      lane: agent.lane,
      mission: agent.mission,
      avgResponseMs: agent.avgResponseMs,
      p95ResponseMs: agent.p95ResponseMs,
      successCount: agent.successCount,
      errorCount: agent.errorCount,
      errorRate: agentErrorRate(agent),
      uptime: agent.uptime,
      saturation: agent.saturation,
      reliabilityScore: agent.reliabilityScore,
      responseTrendMs: agent.responseTrendMs
    }))
  };
}
function createLocalMetricsAnswer(question, context) {
  const q = question.toLowerCase();
  const mentionedAgent = findMentionedAgent(question, context.agents);
  const summaryLine = `${context.activeLabel} currently shows ${msFormat(context.summary.avgResponseMs)} average response, ${numberFormat(context.summary.successCount)} successes, ${numberFormat(context.summary.errorCount)} errors, ${percentFormat(context.summary.uptime)} uptime, and ${percentFormat(context.summary.errorRate)} error rate.`;
  if (/^(hi|hello|hey|yo|sup|good morning|good afternoon|good evening)\b/.test(q)) {
    return "Hi, I am the Nasiko assistant for this dashboard. You can ask me about Nasiko, the current agent metrics, uptime, errors, latency, or what to highlight in your demo.";
  }
  if (/thank|thanks|appreciate/.test(q)) {
    return "You are welcome. I can keep helping with the Nasiko dashboard, Challenge 2 talking points, or any quick explanation you need.";
  }
  if (/who are you|what are you|your role|representative/.test(q)) {
    return "I am acting as Nasiko's dashboard representative here. I explain the platform in simple terms and answer questions using the current Challenge 2 metrics shown on this page.";
  }
  if (mentionedAgent) {
    return `${describeAgent(mentionedAgent)} ${getReliability(mentionedAgent).label} status is based on uptime and error rate.`;
  }
  if (/nasiko|platform|project/.test(q)) {
    return "Nasiko is an AI agent platform for building, deploying, routing, and observing agents. In this Challenge 2 page, Nasiko observability is shown through response time, success count, error count, uptime, capacity, and 24-hour charts.";
  }
  if (/live|demo|telemetry|data|backend|api/.test(q)) {
    return `This dashboard is using ${context.telemetryMode === "live" ? "Live telemetry" : "Demo telemetry"}. ${context.telemetryReason} The chatbot answers from the same metrics currently shown on the page.`;
  }
  if (/best|fastest|quickest/.test(q)) {
    return `${context.fastestAgent.name} is the fastest agent right now with ${msFormat(context.fastestAgent.avgResponseMs)} average response and ${percentFormat(context.fastestAgent.uptime)} uptime.`;
  }
  if (/slow|latency|response|p95/.test(q)) {
    return `${context.slowestAgent.name} has the highest average response at ${msFormat(context.slowestAgent.avgResponseMs)}. For the selected view, P95 latency is ${msFormat(context.summary.p95ResponseMs)} and latency movement is ${trendCopy(context.summary.responseTrendMs)}.`;
  }
  if (/error|fail|failure|risk/.test(q)) {
    return `${context.mostErrorsAgent.name} has the most errors with ${numberFormat(context.mostErrorsAgent.errorCount)} errors and ${percentFormat(agentErrorRate(context.mostErrorsAgent))} error rate. The selected view has ${numberFormat(context.summary.errorCount)} total errors.`;
  }
  if (/success|request|traffic|volume/.test(q)) {
    const trafficLeader = [...context.agents].sort((a, b) => b.successCount - a.successCount)[0];
    return `${trafficLeader.name} leads successful requests with ${numberFormat(trafficLeader.successCount)} successes. The selected view has ${numberFormat(context.summary.successCount)} successful requests in the last 24 hours.`;
  }
  if (/uptime|availability|available/.test(q)) {
    return `${context.lowestUptimeAgent.name} has the lowest uptime at ${percentFormat(context.lowestUptimeAgent.uptime)}. The selected view uptime is ${percentFormat(context.summary.uptime)} across ${context.summary.activeAgents} active agent${context.summary.activeAgents === 1 ? "" : "s"}.`;
  }
  if (/capacity|pressure|load|saturation/.test(q)) {
    return `${context.highestPressureAgent.name} is carrying the highest pressure at ${context.highestPressureAgent.saturation}% capacity. The selected view is at ${context.summary.saturation}% capacity.`;
  }
  if (/attention|watch|improve|problem|issue/.test(q)) {
    return `${context.attentionAgent.name} needs the most attention. It has a ${context.attentionAgent.reliabilityScore}% score, ${percentFormat(agentErrorRate(context.attentionAgent))} error rate, ${percentFormat(context.attentionAgent.uptime)} uptime, and ${msFormat(context.attentionAgent.p95ResponseMs)} P95 latency.`;
  }
  if (/score|slo|health|healthy/.test(q)) {
    const posture = context.summary.reliabilityScore >= 93 ? "inside SLO" : "needs watch";
    return `The selected view has a ${context.summary.reliabilityScore}% reliability score, so it is ${posture}. ${summaryLine}`;
  }
  if (/judge|challenge|show|video|demo/.test(q)) {
    return "For judges, show the agent filter, the four required stats, the 24-hour charts, the Live/Demo telemetry label, and the mobile responsive layout. That proves the page meets Challenge 2 and is review-ready.";
  }
  if (/help|what can you do|commands|ask/.test(q)) {
    return "You can ask me normal questions, but I am strongest on Nasiko and this dashboard. Try asking: which agent is risky, why uptime matters, what Demo telemetry means, or what to show judges.";
  }
  return `I can help with that from a Nasiko dashboard point of view. ${summaryLine} You can also ask me normal follow-up questions, or ask specifically about agents, latency, errors, uptime, capacity, live data, or the Challenge 2 demo.`;
}
function ChartCanvas({
  config,
  className
}) {
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
  return /*#__PURE__*/React.createElement("div", {
    className: className
  }, /*#__PURE__*/React.createElement("canvas", {
    ref: canvasRef
  }));
}
function TrendPill({
  value,
  positiveGood = false
}) {
  const isGood = positiveGood ? value >= 0 : value <= 0;
  const isFlat = Math.abs(value) <= 8;
  return /*#__PURE__*/React.createElement("span", {
    className: `trend-pill ${isFlat ? "flat" : isGood ? "good" : "bad"}`
  }, trendCopy(value));
}
function StatTile({
  label,
  value,
  detail,
  tone,
  children
}) {
  return /*#__PURE__*/React.createElement("section", {
    className: `stat-tile ${tone}`
  }, /*#__PURE__*/React.createElement("span", null, label), /*#__PURE__*/React.createElement("strong", null, value), /*#__PURE__*/React.createElement("small", null, detail), children);
}
function ProgressBar({
  value,
  color
}) {
  return /*#__PURE__*/React.createElement("span", {
    className: "progress-track",
    "aria-hidden": "true"
  }, /*#__PURE__*/React.createElement("span", {
    className: "progress-fill",
    style: {
      width: `${Math.min(100, value)}%`,
      background: color
    }
  }));
}
function SparkBars({
  points,
  color
}) {
  const values = points.map(point => point.responseMs);
  const min = Math.min(...values);
  const max = Math.max(...values);
  return /*#__PURE__*/React.createElement("span", {
    className: "spark-bars",
    "aria-hidden": "true"
  }, values.map((value, index) => {
    const height = 24 + (value - min) / Math.max(1, max - min) * 56;
    return /*#__PURE__*/React.createElement("span", {
      key: `${value}-${index}`,
      style: {
        height: `${height}%`,
        background: color
      }
    });
  }));
}
function FleetHero({
  summary,
  activeLabel,
  telemetry
}) {
  const responseTone = summary.responseTrendMs <= 0 ? "good" : "bad";
  const telemetryLabel = telemetry.mode === "live" ? "Live telemetry" : "Demo telemetry";
  return /*#__PURE__*/React.createElement("section", {
    className: "command-panel"
  }, /*#__PURE__*/React.createElement("div", {
    className: "command-copy"
  }, /*#__PURE__*/React.createElement("span", {
    className: "eyebrow"
  }, "Nasiko observability"), /*#__PURE__*/React.createElement("h1", null, "Agent Performance Metrics"), /*#__PURE__*/React.createElement("p", null, "Last 24 hours across response latency, request outcomes, uptime, and fleet pressure.")), /*#__PURE__*/React.createElement("div", {
    className: "command-score"
  }, /*#__PURE__*/React.createElement("div", {
    className: "score-ring",
    "aria-label": `Reliability score ${percentFormat(summary.reliabilityScore)}`
  }, /*#__PURE__*/React.createElement("svg", {
    className: "score-ring-chart",
    viewBox: "0 0 120 120",
    "aria-hidden": "true",
    focusable: "false"
  }, /*#__PURE__*/React.createElement("circle", {
    className: "score-ring-track",
    cx: "60",
    cy: "60",
    r: "48",
    pathLength: "100"
  }), /*#__PURE__*/React.createElement("circle", {
    className: "score-ring-value",
    cx: "60",
    cy: "60",
    r: "48",
    pathLength: "100",
    style: {
      "--score-offset": 100 - summary.reliabilityScore
    }
  })), /*#__PURE__*/React.createElement("span", {
    className: "score-ring-copy"
  }, /*#__PURE__*/React.createElement("strong", null, summary.reliabilityScore, /*#__PURE__*/React.createElement("small", null, "%")), /*#__PURE__*/React.createElement("span", null, "Score"))), /*#__PURE__*/React.createElement("div", {
    className: "score-copy"
  }, /*#__PURE__*/React.createElement("span", {
    className: `freshness ${telemetry.mode}`
  }, /*#__PURE__*/React.createElement("span", {
    className: "pulse"
  }), telemetryLabel), /*#__PURE__*/React.createElement("strong", null, activeLabel), /*#__PURE__*/React.createElement("small", null, summary.activeAgents, " agent view"))), /*#__PURE__*/React.createElement("div", {
    className: "command-brief"
  }, /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("small", null, "P95 latency"), /*#__PURE__*/React.createElement("strong", null, msFormat(summary.p95ResponseMs))), /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("small", null, "Error rate"), /*#__PURE__*/React.createElement("strong", null, percentFormat(summary.errorRate))), /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("small", null, "Capacity"), /*#__PURE__*/React.createElement("strong", null, summary.saturation, "%")), /*#__PURE__*/React.createElement("span", {
    className: `brief-trend ${responseTone}`
  }, /*#__PURE__*/React.createElement("small", null, "Latency movement"), /*#__PURE__*/React.createElement("strong", null, trendCopy(summary.responseTrendMs)))));
}
function ExecutiveReadout({
  summary,
  activeAgent,
  agents
}) {
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
  return /*#__PURE__*/React.createElement("section", {
    className: "readout-grid",
    "aria-label": "Executive telemetry readout"
  }, items.map(item => /*#__PURE__*/React.createElement("article", {
    className: `readout-card ${item.tone}`,
    key: item.label
  }, /*#__PURE__*/React.createElement("span", null, item.label), /*#__PURE__*/React.createElement("strong", null, item.value), /*#__PURE__*/React.createElement("small", null, item.detail))));
}
function AgentCard({
  agent,
  isActive,
  onSelect
}) {
  const reliability = getReliability(agent);
  return /*#__PURE__*/React.createElement("button", {
    className: `agent-card ${isActive ? "active" : ""}`,
    onClick: () => onSelect(agent.id),
    type: "button",
    style: {
      "--agent-color": agent.color
    }
  }, /*#__PURE__*/React.createElement("span", {
    className: "agent-card-header"
  }, /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("strong", null, agent.name), /*#__PURE__*/React.createElement("small", null, agent.mission)), /*#__PURE__*/React.createElement("span", {
    className: `status-pill ${reliability.className}`
  }, reliability.label)), /*#__PURE__*/React.createElement(SparkBars, {
    points: agent.hourly,
    color: agent.color
  }), /*#__PURE__*/React.createElement("span", {
    className: "agent-card-metrics"
  }, /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("small", null, "Response"), /*#__PURE__*/React.createElement("strong", null, msFormat(agent.avgResponseMs))), /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("small", null, "Success"), /*#__PURE__*/React.createElement("strong", null, numberFormat(agent.successCount))), /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("small", null, "Errors"), /*#__PURE__*/React.createElement("strong", null, numberFormat(agent.errorCount))), /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("small", null, "Uptime"), /*#__PURE__*/React.createElement("strong", null, percentFormat(agent.uptime)))), /*#__PURE__*/React.createElement("span", {
    className: "agent-card-footer"
  }, /*#__PURE__*/React.createElement("span", null, agent.owner), /*#__PURE__*/React.createElement("span", null, agent.region), /*#__PURE__*/React.createElement("span", null, agent.version)));
}
function ResponseChart({
  selectedAgents,
  hourly
}) {
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
  return /*#__PURE__*/React.createElement(ChartCanvas, {
    config: config,
    className: "chart-shell tall"
  });
}
function TrafficChart({
  hourly
}) {
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
  return /*#__PURE__*/React.createElement(ChartCanvas, {
    config: config,
    className: "chart-shell"
  });
}
function UptimeChart({
  hourly
}) {
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
  return /*#__PURE__*/React.createElement(ChartCanvas, {
    config: config,
    className: "chart-shell"
  });
}
function PressureChart({
  hourly
}) {
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
  return /*#__PURE__*/React.createElement(ChartCanvas, {
    config: config,
    className: "chart-shell"
  });
}
function InsightPanel({
  summary,
  activeAgent,
  agents
}) {
  const watchedAgents = [...agents].sort((a, b) => a.reliabilityScore - b.reliabilityScore).slice(0, 3);
  const bestAgent = [...agents].sort((a, b) => a.avgResponseMs - b.avgResponseMs)[0];
  const focusedAgent = activeAgent || watchedAgents[0];
  return /*#__PURE__*/React.createElement("section", {
    className: "panel insight-panel"
  }, /*#__PURE__*/React.createElement("div", {
    className: "panel-heading"
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("span", {
    className: "eyebrow"
  }, "Operations pulse"), /*#__PURE__*/React.createElement("h2", null, "Judge-ready signals")), /*#__PURE__*/React.createElement("span", {
    className: "range-pill"
  }, "24h")), /*#__PURE__*/React.createElement("div", {
    className: "signal-stack"
  }, /*#__PURE__*/React.createElement("div", {
    className: "signal-card primary"
  }, /*#__PURE__*/React.createElement("span", null, "Highest attention"), /*#__PURE__*/React.createElement("strong", null, focusedAgent.name), /*#__PURE__*/React.createElement("small", null, percentFormat(focusedAgent.errorRate), " error rate - ", msFormat(focusedAgent.p95ResponseMs), " P95"), /*#__PURE__*/React.createElement(ProgressBar, {
    value: focusedAgent.saturation,
    color: focusedAgent.color
  })), /*#__PURE__*/React.createElement("div", {
    className: "signal-card"
  }, /*#__PURE__*/React.createElement("span", null, "Fastest performer"), /*#__PURE__*/React.createElement("strong", null, bestAgent.name), /*#__PURE__*/React.createElement("small", null, msFormat(bestAgent.avgResponseMs), " average response")), /*#__PURE__*/React.createElement("div", {
    className: "signal-card"
  }, /*#__PURE__*/React.createElement("span", null, "Peak traffic hour"), /*#__PURE__*/React.createElement("strong", null, summary.hottestHour.hour), /*#__PURE__*/React.createElement("small", null, numberFormat(summary.hottestHour.successCount + summary.hottestHour.errorCount), " requests"))), /*#__PURE__*/React.createElement("div", {
    className: "watch-list"
  }, watchedAgents.map(agent => {
    const reliability = getReliability(agent);
    return /*#__PURE__*/React.createElement("span", {
      key: agent.id
    }, /*#__PURE__*/React.createElement("span", {
      className: "watch-name"
    }, /*#__PURE__*/React.createElement("span", {
      className: "color-dot",
      style: {
        backgroundColor: agent.color
      }
    }), agent.name), /*#__PURE__*/React.createElement("strong", null, agent.reliabilityScore), /*#__PURE__*/React.createElement("small", {
      className: reliability.className
    }, reliability.label));
  })));
}
function HeatmapPanel({
  agents
}) {
  return /*#__PURE__*/React.createElement("section", {
    className: "panel heatmap-panel"
  }, /*#__PURE__*/React.createElement("div", {
    className: "panel-heading"
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("span", {
    className: "eyebrow"
  }, "Latency heatmap"), /*#__PURE__*/React.createElement("h2", null, "Agent pressure by hour"))), /*#__PURE__*/React.createElement("div", {
    className: "heatmap"
  }, agents.map(agent => /*#__PURE__*/React.createElement("div", {
    className: "heatmap-row",
    key: agent.id
  }, /*#__PURE__*/React.createElement("span", {
    className: "heatmap-label"
  }, agent.name), /*#__PURE__*/React.createElement("span", {
    className: "heatmap-cells"
  }, agent.hourly.map((point, index) => {
    const intensity = Math.min(1, Math.max(0.1, point.responseMs / agent.p95ResponseMs));
    return /*#__PURE__*/React.createElement("span", {
      key: `${agent.id}-${point.hour}-${index}`,
      title: `${agent.name} ${point.hour}: ${point.responseMs} ms`,
      style: {
        backgroundColor: agent.color,
        opacity: 0.24 + intensity * 0.66
      }
    });
  }))))));
}
function AgentTable({
  agents,
  selectedAgentId,
  onSelect
}) {
  return /*#__PURE__*/React.createElement("section", {
    className: "panel table-panel"
  }, /*#__PURE__*/React.createElement("div", {
    className: "panel-heading"
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("span", {
    className: "eyebrow"
  }, "Per-agent stats"), /*#__PURE__*/React.createElement("h2", null, "Current 24-hour rollup"))), /*#__PURE__*/React.createElement("div", {
    className: "table-wrap"
  }, /*#__PURE__*/React.createElement("table", null, /*#__PURE__*/React.createElement("thead", null, /*#__PURE__*/React.createElement("tr", null, /*#__PURE__*/React.createElement("th", null, "Agent"), /*#__PURE__*/React.createElement("th", null, "Avg response"), /*#__PURE__*/React.createElement("th", null, "P95"), /*#__PURE__*/React.createElement("th", null, "Success"), /*#__PURE__*/React.createElement("th", null, "Errors"), /*#__PURE__*/React.createElement("th", null, "Uptime"), /*#__PURE__*/React.createElement("th", null, "Capacity"))), /*#__PURE__*/React.createElement("tbody", null, agents.map(agent => /*#__PURE__*/React.createElement("tr", {
    key: agent.id,
    className: selectedAgentId === agent.id ? "selected" : "",
    onClick: () => onSelect(agent.id)
  }, /*#__PURE__*/React.createElement("td", null, /*#__PURE__*/React.createElement("span", {
    className: "agent-name"
  }, /*#__PURE__*/React.createElement("span", {
    className: "color-dot",
    style: {
      backgroundColor: agent.color
    }
  }), /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("strong", null, agent.name), /*#__PURE__*/React.createElement("small", null, agent.lane, " - ", agent.owner)))), /*#__PURE__*/React.createElement("td", null, msFormat(agent.avgResponseMs)), /*#__PURE__*/React.createElement("td", null, msFormat(agent.p95ResponseMs)), /*#__PURE__*/React.createElement("td", null, numberFormat(agent.successCount)), /*#__PURE__*/React.createElement("td", null, numberFormat(agent.errorCount)), /*#__PURE__*/React.createElement("td", null, percentFormat(agent.uptime)), /*#__PURE__*/React.createElement("td", null, agent.saturation, "%")))))));
}
function MetricsChatbot({
  summary,
  activeAgent,
  activeLabel,
  agents,
  telemetry
}) {
  const [isOpen, setIsOpen] = useState(false);
  const [input, setInput] = useState("");
  const [isThinking, setIsThinking] = useState(false);
  const [bridgeStatus, setBridgeStatus] = useState("checking");
  const [messages, setMessages] = useState([{
    role: "assistant",
    content: "Hi, I represent Nasiko on this dashboard. Ask me about Nasiko, these metrics, agents, uptime, errors, latency, or what to show judges."
  }]);
  const messagesRef = useRef(null);
  const context = useMemo(() => buildMetricsChatContext(summary, activeAgent, activeLabel, agents, telemetry), [summary, activeAgent, activeLabel, agents, telemetry]);
  const cursorEndpoint = getCursorAgentEndpoint();
  const bridgeLabel = bridgeStatus === "ready" ? "Cursor bridge ready" : bridgeStatus === "checking" ? "Checking Cursor bridge" : "Local metrics context";
  const suggestions = ["What is Nasiko?", "Which agent needs attention?", "What should I show judges?"];
  useEffect(() => {
    if (!isOpen || !messagesRef.current) return;
    messagesRef.current.scrollTop = messagesRef.current.scrollHeight;
  }, [messages, isThinking, isOpen]);
  useEffect(() => {
    let isActive = true;
    setBridgeStatus("checking");
    checkCursorAgentBridge(cursorEndpoint).then(isReady => {
      if (isActive) setBridgeStatus(isReady ? "ready" : "local");
    });
    return () => {
      isActive = false;
    };
  }, [cursorEndpoint]);
  async function askQuestion(rawQuestion) {
    const question = rawQuestion.trim();
    if (!question || isThinking) return;
    setInput("");
    setIsOpen(true);
    setMessages(current => [...current, {
      role: "user",
      content: question
    }]);
    setIsThinking(true);
    try {
      const cursorAnswer = bridgeStatus === "ready" ? await askCursorAgent(question, context, cursorEndpoint) : null;
      const answer = cursorAnswer || createLocalMetricsAnswer(question, context);
      setMessages(current => [...current, {
        role: "assistant",
        content: answer,
        source: cursorAnswer ? "Cursor SDK" : "Metrics context"
      }]);
    } catch (_error) {
      setMessages(current => [...current, {
        role: "assistant",
        content: createLocalMetricsAnswer(question, context),
        source: "Metrics context"
      }]);
    } finally {
      setIsThinking(false);
    }
  }
  function handleSubmit(event) {
    event.preventDefault();
    askQuestion(input);
  }
  return /*#__PURE__*/React.createElement("aside", {
    className: `metrics-chatbot ${isOpen ? "open" : ""}`,
    "aria-label": "Nasiko assistant chatbot"
  }, isOpen ? /*#__PURE__*/React.createElement("section", {
    className: "chatbot-panel"
  }, /*#__PURE__*/React.createElement("header", {
    className: "chatbot-header"
  }, /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("small", null, "Nasiko representative"), /*#__PURE__*/React.createElement("strong", null, "Nasiko Assistant")), /*#__PURE__*/React.createElement("button", {
    type: "button",
    onClick: () => setIsOpen(false),
    "aria-label": "Close metrics chatbot"
  }, "x")), /*#__PURE__*/React.createElement("div", {
    className: "chatbot-status"
  }, /*#__PURE__*/React.createElement("span", {
    className: bridgeStatus === "ready" ? "live" : "demo"
  }), bridgeLabel), /*#__PURE__*/React.createElement("div", {
    className: "chatbot-messages",
    ref: messagesRef
  }, messages.map((message, index) => /*#__PURE__*/React.createElement("div", {
    className: `chatbot-message ${message.role}`,
    key: `${message.role}-${index}`
  }, /*#__PURE__*/React.createElement("p", null, message.content), message.source && /*#__PURE__*/React.createElement("small", null, message.source))), isThinking && /*#__PURE__*/React.createElement("div", {
    className: "chatbot-message assistant thinking"
  }, /*#__PURE__*/React.createElement("p", null, "Reading the current metrics..."))), /*#__PURE__*/React.createElement("div", {
    className: "chatbot-prompts"
  }, suggestions.map(suggestion => /*#__PURE__*/React.createElement("button", {
    type: "button",
    key: suggestion,
    onClick: () => askQuestion(suggestion)
  }, suggestion))), /*#__PURE__*/React.createElement("form", {
    className: "chatbot-form",
    onSubmit: handleSubmit
  }, /*#__PURE__*/React.createElement("input", {
    "aria-label": "Ask the Nasiko assistant",
    placeholder: "Ask Nasiko anything...",
    value: input,
    onChange: event => setInput(event.target.value)
  }), /*#__PURE__*/React.createElement("button", {
    type: "submit",
    disabled: !input.trim() || isThinking
  }, "Send"))) : /*#__PURE__*/React.createElement("button", {
    className: "chatbot-toggle",
    type: "button",
    onClick: () => setIsOpen(true)
  }, /*#__PURE__*/React.createElement("span", null, "AI"), "Ask metrics"));
}
function App() {
  const [selectedAgentId, setSelectedAgentId] = useState("all");
  const [telemetry, setTelemetry] = useState({
    mode: "demo",
    agents: demoAgents,
    reason: "Using bundled demo telemetry while checking for live data."
  });
  useEffect(() => {
    let isActive = true;
    loadLiveTelemetry().then(result => {
      if (isActive) setTelemetry(result);
    });
    return () => {
      isActive = false;
    };
  }, []);
  const agents = telemetry.agents.length ? telemetry.agents : demoAgents;
  useEffect(() => {
    if (selectedAgentId !== "all" && !agents.some(agent => agent.id === selectedAgentId)) {
      setSelectedAgentId("all");
    }
  }, [agents, selectedAgentId]);
  const selectedAgents = useMemo(() => {
    if (selectedAgentId === "all") return agents;
    const matchingAgents = agents.filter(agent => agent.id === selectedAgentId);
    return matchingAgents.length ? matchingAgents : agents;
  }, [agents, selectedAgentId]);
  const activeAgent = selectedAgentId === "all" ? null : agents.find(agent => agent.id === selectedAgentId) || null;
  const hourly = useMemo(() => aggregateHourly(selectedAgents), [selectedAgents]);
  const summary = useMemo(() => aggregateSummary(selectedAgents, hourly), [selectedAgents, hourly]);
  const activeLabel = activeAgent ? activeAgent.name : "All agents";
  return /*#__PURE__*/React.createElement("main", {
    className: "page-shell"
  }, /*#__PURE__*/React.createElement(FleetHero, {
    summary: summary,
    activeLabel: activeLabel,
    telemetry: telemetry
  }), /*#__PURE__*/React.createElement("section", {
    className: "toolbar",
    "aria-label": "Agent filter"
  }, /*#__PURE__*/React.createElement("button", {
    className: selectedAgentId === "all" ? "selected" : "",
    onClick: () => setSelectedAgentId("all"),
    type: "button"
  }, "All agents"), agents.map(agent => /*#__PURE__*/React.createElement("button", {
    key: agent.id,
    className: selectedAgentId === agent.id ? "selected" : "",
    onClick: () => setSelectedAgentId(agent.id),
    type: "button"
  }, agent.name))), /*#__PURE__*/React.createElement("section", {
    className: "stats-grid"
  }, /*#__PURE__*/React.createElement(StatTile, {
    label: "Avg response",
    value: msFormat(summary.avgResponseMs),
    detail: activeLabel,
    tone: "latency"
  }, /*#__PURE__*/React.createElement(TrendPill, {
    value: summary.responseTrendMs
  })), /*#__PURE__*/React.createElement(StatTile, {
    label: "Success count",
    value: numberFormat(summary.successCount),
    detail: "Completed requests",
    tone: "success"
  }), /*#__PURE__*/React.createElement(StatTile, {
    label: "Error count",
    value: numberFormat(summary.errorCount),
    detail: `${percentFormat(summary.errorRate)} error rate`,
    tone: "error"
  }), /*#__PURE__*/React.createElement(StatTile, {
    label: "Uptime",
    value: percentFormat(summary.uptime),
    detail: `${summary.activeAgents} active agents`,
    tone: "uptime"
  }, /*#__PURE__*/React.createElement(ProgressBar, {
    value: summary.uptime,
    color: "#c47a14"
  }))), /*#__PURE__*/React.createElement("p", {
    className: `telemetry-note ${telemetry.mode}`
  }, telemetry.reason), /*#__PURE__*/React.createElement(ExecutiveReadout, {
    summary: summary,
    activeAgent: activeAgent,
    agents: agents
  }), /*#__PURE__*/React.createElement("section", {
    className: "agent-grid"
  }, agents.map(agent => /*#__PURE__*/React.createElement(AgentCard, {
    key: agent.id,
    agent: agent,
    isActive: selectedAgentId === agent.id,
    onSelect: setSelectedAgentId
  }))), /*#__PURE__*/React.createElement("section", {
    className: "dashboard-grid"
  }, /*#__PURE__*/React.createElement("section", {
    className: "panel response-panel"
  }, /*#__PURE__*/React.createElement("div", {
    className: "panel-heading"
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("span", {
    className: "eyebrow"
  }, "Latency trend"), /*#__PURE__*/React.createElement("h2", null, "Average response time")), /*#__PURE__*/React.createElement("span", {
    className: "range-pill"
  }, "24h")), /*#__PURE__*/React.createElement(ResponseChart, {
    selectedAgents: selectedAgents,
    hourly: hourly
  })), /*#__PURE__*/React.createElement(InsightPanel, {
    summary: summary,
    activeAgent: activeAgent,
    agents: agents
  }), /*#__PURE__*/React.createElement("section", {
    className: "panel"
  }, /*#__PURE__*/React.createElement("div", {
    className: "panel-heading"
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("span", {
    className: "eyebrow"
  }, "Outcomes"), /*#__PURE__*/React.createElement("h2", null, "Success vs errors"))), /*#__PURE__*/React.createElement(TrafficChart, {
    hourly: hourly
  })), /*#__PURE__*/React.createElement("section", {
    className: "panel"
  }, /*#__PURE__*/React.createElement("div", {
    className: "panel-heading"
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("span", {
    className: "eyebrow"
  }, "Availability"), /*#__PURE__*/React.createElement("h2", null, "Uptime percentage"))), /*#__PURE__*/React.createElement(UptimeChart, {
    hourly: hourly
  })), /*#__PURE__*/React.createElement("section", {
    className: "panel"
  }, /*#__PURE__*/React.createElement("div", {
    className: "panel-heading"
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("span", {
    className: "eyebrow"
  }, "Capacity guardrail"), /*#__PURE__*/React.createElement("h2", null, "Saturation and P95 pressure"))), /*#__PURE__*/React.createElement(PressureChart, {
    hourly: hourly
  }))), /*#__PURE__*/React.createElement(HeatmapPanel, {
    agents: agents
  }), /*#__PURE__*/React.createElement(AgentTable, {
    agents: agents,
    selectedAgentId: selectedAgentId,
    onSelect: setSelectedAgentId
  }), /*#__PURE__*/React.createElement(MetricsChatbot, {
    summary: summary,
    activeAgent: activeAgent,
    activeLabel: activeLabel,
    agents: agents,
    telemetry: telemetry
  }));
}
ReactDOM.createRoot(document.getElementById("root")).render(/*#__PURE__*/React.createElement(App, null));
