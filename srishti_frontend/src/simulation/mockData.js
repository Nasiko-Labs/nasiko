// ============================================================
// MOCK DATA — AI Failure Analysis System
// Deterministic, offline-ready. All data is static.
// ============================================================

export const MCP_SERVER = {
  name: 'failure-analysis',
  displayName: 'AI Failure Analysis',
  fileName: 'failure_analysis_server.py',
  version: '2.1.0',
  description: 'MCP server for autonomous root-cause analysis, failure chain prediction, and remediation planning.',
  tools: [
    {
      id: 'analyze_failure_logs',
      name: 'analyze_failure_logs',
      description: 'Analyze system failure logs and identify root causes across distributed services',
      icon: '🔍',
      inputSchema: { query: 'string', cluster: 'string', time_range: 'string' }
    },
    {
      id: 'predict_failure_chain',
      name: 'predict_failure_chain',
      description: 'Predict cascading failure patterns before they propagate to critical systems',
      icon: '⚡',
      inputSchema: { initial_event: 'string', depth: 'number', include_probability: 'boolean' }
    },
    {
      id: 'suggest_prevention_fix',
      name: 'suggest_prevention_fix',
      description: 'Generate actionable remediation steps prioritized by impact and implementation cost',
      icon: '🛠',
      inputSchema: { failure_id: 'string', urgency: 'string', team: 'string' }
    }
  ]
};

export const MANIFEST = {
  name: 'failure-analysis',
  version: '2.1.0',
  description: 'MCP server for autonomous root-cause analysis and failure chain prediction',
  transport: { type: 'stdio', command: 'python', args: ['server.py'] },
  tools: MCP_SERVER.tools.map(t => ({
    name: t.name,
    description: t.description,
    inputSchema: { type: 'object', properties: t.inputSchema }
  }))
};

export const DEPLOY_STEPS = [
  { id: 1, label: 'MCP Server detected',          detail: 'failure_analysis_server.py',      duration: 600 },
  { id: 2, label: 'Manifest generated',            detail: '3 tools registered',              duration: 900 },
  { id: 3, label: 'stdio→HTTP bridge active',      detail: 'Port :8421 listening',            duration: 700 },
  { id: 4, label: 'Kong gateway configured',       detail: '/mcp/failure-analysis/* routed', duration: 500 },
  { id: 5, label: 'Tools registered with agents',  detail: 'Discovery service updated',       duration: 400 },
];

export const DEMO_QUERIES = [
  {
    id: 'q1',
    text: 'Analyze the memory overflow failures in prod-cluster-7',
    tool: 'analyze_failure_logs',
    traceId: 'trace-fa-001',
    totalMs: 847,
    result: {
      summary: '3 critical memory overflow events detected in prod-cluster-7 (2026-04-18 08:12–08:47 UTC)',
      rootCause: 'Pod memory limits under-provisioned for ML inference workload spike (peak: 4.2 GB, limit: 2 GB)',
      affectedServices: ['inference-worker-7a', 'inference-worker-7b', 'model-cache-shard-3'],
      findings: [
        { title: 'OOM Kill detected × 3', meta: 'inference-worker pods · 08:12, 08:31, 08:47 UTC' },
        { title: 'Memory pressure 212% above limit', meta: 'model-cache-shard-3 · peak 4.2GB/2GB limit' },
        { title: 'Container restart loop (CrashLoopBackOff)', meta: 'inference-worker-7b · 8 restarts in 35min' },
      ]
    }
  },
  {
    id: 'q2',
    text: 'Predict failure chain from the current memory pressure',
    tool: 'predict_failure_chain',
    traceId: 'trace-fa-002',
    totalMs: 612,
    result: {
      summary: 'High-probability cascade to API gateway and database connection pool within 18–25 min',
      rootCause: 'Memory exhaustion triggers OOM kill → pod restart → connection pool drain → request timeout cascade',
      affectedServices: ['api-gateway-primary', 'postgres-pool-a', 'redis-session-store'],
      findings: [
        { title: 'P(cascade to api-gateway) = 0.87', meta: 'Estimated ΔT: 18 min if untreated' },
        { title: 'P(postgres pool exhaustion) = 0.71', meta: 'Connection limit: 200 · Current used: 178' },
        { title: 'P(user-facing 502 errors) = 0.94', meta: 'SLA breach risk: HIGH · ETA: 23 min' },
      ]
    }
  },
  {
    id: 'q3',
    text: 'Suggest fixes for the memory overflow failures',
    tool: 'suggest_prevention_fix',
    traceId: 'trace-fa-003',
    totalMs: 1103,
    result: {
      summary: '4 prioritized remediation actions generated. Estimated MTTR reduction: 72%',
      rootCause: 'Root cause: memory limit misconfiguration + absent horizontal pod autoscaling',
      affectedServices: ['infra-team', 'ml-platform-team'],
      findings: [
        { title: 'IMMEDIATE: Increase pod memory limit to 6GB', meta: '5 min · kubectl patch deployment inference-worker' },
        { title: 'SHORT-TERM: Add HPA with memory trigger at 70%', meta: '2 hrs · Prevents future OOM cascades' },
        { title: 'LONG-TERM: Implement memory-aware scheduler', meta: '1 sprint · Bin-packing optimization for ML workloads' },
      ]
    }
  }
];

export const TRACE_TIMINGS = {
  'trace-fa-001': [
    { ms: 0,   event: 'Agent received query',                    color: 'gray' },
    { ms: 187, event: 'Routing via API Gateway (Kong)',          color: 'amber', latency: '+187ms' },
    { ms: 692, event: 'Bridge processing (stdio→HTTP)',          color: 'amber', latency: '+505ms' },
    { ms: 786, event: 'MCP tool executed (analyze_failure_logs)',color: 'green', latency: '+94ms'  },
    { ms: 847, event: 'Response returned ✓',                    color: 'green', latency: '+61ms'  },
  ],
  'trace-fa-002': [
    { ms: 0,   event: 'Agent received query',                    color: 'gray' },
    { ms: 145, event: 'Routing via API Gateway (Kong)',          color: 'amber', latency: '+145ms' },
    { ms: 312, event: 'Bridge processing (stdio→HTTP)',          color: 'amber', latency: '+167ms' },
    { ms: 574, event: 'MCP tool executed (predict_failure_chain)',color: 'green', latency: '+262ms'},
    { ms: 612, event: 'Response returned ✓',                    color: 'green', latency: '+38ms'  },
  ],
  'trace-fa-003': [
    { ms: 0,   event: 'Agent received query',                    color: 'gray' },
    { ms: 203, event: 'Routing via API Gateway (Kong)',          color: 'amber', latency: '+203ms' },
    { ms: 891, event: 'Bridge processing (stdio→HTTP)',          color: 'amber', latency: '+688ms' },
    { ms: 1057,event: 'MCP tool executed (suggest_prevention_fix)',color: 'green',latency: '+166ms'},
    { ms: 1103,event: 'Response returned ✓',                    color: 'green', latency: '+46ms'  },
  ],
};

// Converts ms to HH:MM:SS.mmm starting from a base time
export function toTimestamp(baseMs, offsetMs) {
  const d = new Date(baseMs + offsetMs);
  const h  = String(d.getHours()).padStart(2, '0');
  const m  = String(d.getMinutes()).padStart(2, '0');
  const s  = String(d.getSeconds()).padStart(2, '0');
  const ms = String(d.getMilliseconds()).padStart(3, '0');
  return `${h}:${m}:${s}.${ms}`;
}
