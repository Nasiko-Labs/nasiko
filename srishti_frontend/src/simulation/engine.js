// ============================================================
// SIMULATION ENGINE — Deterministic, offline-ready
// Same timing every run. No randomness.
// ============================================================

import { DEPLOY_STEPS, TRACE_TIMINGS, DEMO_QUERIES } from './mockData.js';

const delay = (ms) => new Promise(resolve => setTimeout(resolve, ms));

// ── Upload / Deploy Simulation ─────────────────────────────
export async function simulateUpload(onStep) {
  for (let i = 0; i < DEPLOY_STEPS.length; i++) {
    const step = DEPLOY_STEPS[i];
    onStep({ index: i, status: 'active', step });
    await delay(step.duration);
    onStep({ index: i, status: 'done', step });
    await delay(120);
  }
}

// ── Tool Invocation Simulation ─────────────────────────────
export async function simulateInvocation(queryId, { onTraceEvent, onNodeActivate, onComplete }) {
  const query = DEMO_QUERIES.find(q => q.id === queryId) || DEMO_QUERIES[0];
  const timings = TRACE_TIMINGS[query.traceId] || TRACE_TIMINGS['trace-fa-001'];
  const baseTime = Date.now();

  // Node activation sequence: 0=Agent, 1=Gateway, 2=Bridge, 3=MCP
  const nodeSequence = [0, 1, 2, 3];
  let nodeIndex = 0;

  for (let i = 0; i < timings.length; i++) {
    const event = timings[i];
    await delay(i === 0 ? 200 : timings[i].ms - timings[i - 1].ms);

    onTraceEvent({ ...event, timestamp: Date.now(), baseTime });

    if (nodeIndex < nodeSequence.length) {
      onNodeActivate(nodeSequence[nodeIndex], i === timings.length - 1 ? 'done' : 'active');
      nodeIndex++;
    }

    if (i > 0 && nodeIndex > 1) {
      onNodeActivate(nodeSequence[nodeIndex - 2], 'done');
    }
  }

  // Mark all nodes done
  nodeSequence.forEach(n => onNodeActivate(n, 'done'));
  onComplete(query);
}

// ── Stats ─────────────────────────────────────────────────
const _stats = { calls: 0, totalMs: 0, errors: 0 };

export function recordCall(ms) {
  _stats.calls += 1;
  _stats.totalMs += ms;
}

export function getStats(mcpCount = 1) {
  return {
    mcpServers: mcpCount,
    toolCalls: _stats.calls,
    avgLatency: _stats.calls > 0 ? Math.round(_stats.totalMs / _stats.calls) : 0,
    successRate: _stats.calls === 0 ? 100 : Math.max(0, 100 - Math.round((_stats.errors / _stats.calls) * 100)),
  };
}
