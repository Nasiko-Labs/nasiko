const BASE_AGENTS = [
  {
    id: "compliance-checker",
    name: "Compliance Checker",
    lane: "Policy",
    mission: "Guardrails and policy review",
    owner: "Trust Ops",
    region: "us-east",
    version: "v1.8.2",
    color: "#157a6e",
    responseBase: 820,
    successBase: 9,
    errorBase: 0.25,
    uptimeBase: 99.4,
    loadBase: 58,
  },
  {
    id: "github-agent",
    name: "GitHub Agent",
    lane: "Engineering",
    mission: "Repository actions and pull request context",
    owner: "Dev Platform",
    region: "us-west",
    version: "v2.3.1",
    color: "#5c5ff0",
    responseBase: 1040,
    successBase: 7,
    errorBase: 0.45,
    uptimeBase: 98.8,
    loadBase: 66,
  },
  {
    id: "translator",
    name: "Translator",
    lane: "Language",
    mission: "Cross-language message translation",
    owner: "Localization",
    region: "eu-central",
    version: "v1.5.7",
    color: "#c47a14",
    responseBase: 690,
    successBase: 11,
    errorBase: 0.2,
    uptimeBase: 99.7,
    loadBase: 49,
  },
  {
    id: "webhook-runner",
    name: "Webhook Runner",
    lane: "Automation",
    mission: "Event callbacks and workflow triggers",
    owner: "Automation",
    region: "ap-south",
    version: "v1.11.0",
    color: "#cc4052",
    responseBase: 910,
    successBase: 8,
    errorBase: 0.35,
    uptimeBase: 99.1,
    loadBase: 61,
  },
];

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function formatHour(date) {
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function buildHourlySeries(agent, agentIndex) {
  const now = new Date();
  const start = new Date(now);
  start.setMinutes(0, 0, 0);
  start.setHours(start.getHours() - 23);

  return Array.from({ length: 24 }, (_, hourIndex) => {
    const timestamp = new Date(start);
    timestamp.setHours(start.getHours() + hourIndex);

    const wave = Math.sin((hourIndex + agentIndex * 1.7) / 2.25);
    const secondaryWave = Math.cos((hourIndex + agentIndex) / 3.3);
    const lateSpike = Math.max(0, Math.sin((hourIndex - 13 + agentIndex) / 1.7));
    const peakPressure = hourIndex >= 8 && hourIndex <= 17 ? 1 : 0.35;
    const responseMs = Math.round(
      agent.responseBase + wave * 120 + secondaryWave * 54 + peakPressure * 35 + lateSpike * 42
    );
    const p95ResponseMs = Math.round(responseMs * (1.22 + peakPressure * 0.05 + lateSpike * 0.04));
    const successCount = Math.max(
      0,
      Math.round(agent.successBase + peakPressure * 3 + wave * 2 + ((hourIndex + agentIndex) % 3))
    );
    const errorCount = Math.max(
      0,
      Math.round(agent.errorBase + (secondaryWave > 0.72 ? 1 : 0) + (hourIndex === 14 + agentIndex ? 1 : 0))
    );
    const uptime = clamp(
      Number((agent.uptimeBase - errorCount * 0.18 + wave * 0.08 - lateSpike * 0.03).toFixed(2)),
      95,
      100
    );
    const saturation = clamp(
      Math.round(agent.loadBase + peakPressure * 12 + wave * 11 + secondaryWave * 5 + errorCount * 2),
      20,
      96
    );

    return {
      hour: formatHour(timestamp),
      responseMs,
      p95ResponseMs,
      successCount,
      errorCount,
      uptime,
      saturation,
    };
  });
}

function average(values) {
  return values.reduce((total, value) => total + value, 0) / Math.max(1, values.length);
}

function summarizeAgent(agent, hourly) {
  const totals = hourly.reduce(
    (acc, point) => {
      acc.responseMs += point.responseMs;
      acc.p95ResponseMs += point.p95ResponseMs;
      acc.successCount += point.successCount;
      acc.errorCount += point.errorCount;
      acc.uptime += point.uptime;
      acc.saturation += point.saturation;
      return acc;
    },
    { responseMs: 0, p95ResponseMs: 0, successCount: 0, errorCount: 0, uptime: 0, saturation: 0 }
  );

  const recentResponse = average(hourly.slice(-6).map((point) => point.responseMs));
  const previousResponse = average(hourly.slice(-12, -6).map((point) => point.responseMs));
  const totalRequests = totals.successCount + totals.errorCount;
  const errorRate = Number(((totals.errorCount / Math.max(1, totalRequests)) * 100).toFixed(2));
  const uptime = Number((totals.uptime / hourly.length).toFixed(2));
  const avgResponseMs = Math.round(totals.responseMs / hourly.length);

  return {
    ...agent,
    avgResponseMs,
    p95ResponseMs: Math.round(totals.p95ResponseMs / hourly.length),
    successCount: totals.successCount,
    errorCount: totals.errorCount,
    totalRequests,
    errorRate,
    uptime,
    saturation: Math.round(totals.saturation / hourly.length),
    responseTrendMs: Math.round(recentResponse - previousResponse),
    reliabilityScore: clamp(Math.round(uptime - errorRate * 1.8 - avgResponseMs / 1200), 0, 100),
    hourly,
  };
}

window.NASIKO_DEMO_AGENT_METRICS = BASE_AGENTS.map((agent, index) =>
  summarizeAgent(agent, buildHourlySeries(agent, index))
);
window.NASIKO_AGENT_METRICS = window.NASIKO_DEMO_AGENT_METRICS;
