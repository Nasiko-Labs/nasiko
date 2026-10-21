import { AgentName, NasikoAgentMetadata, NasikoTraceSpan } from './contracts';

class NasikoObservability {
  private spans: NasikoTraceSpan[] = [];
  private agentStats: Map<AgentName, NasikoAgentMetadata> = new Map();
  private maxSpans = 200;

  constructor() {
    this.registerAgentDefaults('resume_agent', 'Resume Reading Agent', 'Extracts verified structured profile from resume documents');
    this.registerAgentDefaults('interview_agent', 'Interview Agent', 'Conducts adaptive technical questioning tailored to resume and candidate performance');
    this.registerAgentDefaults('evaluation_agent', 'Evaluation Agent', 'Independently evaluates technical accuracy, depth, problem solving, and concepts');
    this.registerAgentDefaults('report_agent', 'Interview Report Agent', 'Aggregates evaluation history, detects multi-question patterns and weaknesses');
    this.registerAgentDefaults('preparation_agent', 'Personalized Preparation Plan Agent', 'Synthesizes targeted daily roadmap, concepts, coding exercises and priorities');
  }

  private registerAgentDefaults(name: AgentName, displayName: string, responsibility: string) {
    this.agentStats.set(name, {
      name,
      displayName,
      version: '1.0.0',
      description: responsibility,
      responsibility,
      healthy: true,
      totalExecutions: 0,
      successfulExecutions: 0,
      failedExecutions: 0,
      avgDurationMs: 0,
    });
  }

  recordSpan(span: NasikoTraceSpan) {
    this.spans.unshift(span);
    if (this.spans.length > this.maxSpans) {
      this.spans.pop();
    }

    const stat = this.agentStats.get(span.agentName);
    if (stat) {
      stat.totalExecutions += 1;
      if (span.status === 'success') {
        stat.successfulExecutions += 1;
        stat.healthy = true;
      } else {
        stat.failedExecutions += 1;
      }
      stat.avgDurationMs = Math.round(
        (stat.avgDurationMs * (stat.totalExecutions - 1) + span.durationMs) / stat.totalExecutions
      );
      stat.lastExecutedAt = span.completedAt || new Date().toISOString();
    }
  }

  getDashboardStats() {
    const agents = Array.from(this.agentStats.values());
    // Defensive: older/corrupt spans may lack tokens — never crash the stats endpoint.
    const totalTokens = this.spans.reduce((sum, s) => sum + (s.tokens?.totalTokens ?? 0), 0);
    const totalCalls = this.spans.length;
    const avgLatency =
      totalCalls > 0
        ? Math.round(this.spans.reduce((sum, s) => sum + (s.durationMs ?? 0), 0) / totalCalls)
        : 0;

    return {
      agents,
      summary: {
        totalCalls,
        totalTokens,
        avgLatencyMs: avgLatency,
        activeAgentsCount: agents.filter((a) => a.healthy).length,
      },
      // Map to a shape both the API contract (startedAt/inputSummary) and the
      // legacy frontend (timestamp/input/output) understand.
      recentTraces: this.spans.slice(0, 25).map((s: any) => ({
        ...s,
        timestamp: s.completedAt || s.startedAt,
        input: s.inputSummary ?? s.input,
        output: s.outputSummary ?? s.output,
        inputSummary: s.inputSummary ?? s.input,
        outputSummary: s.outputSummary ?? s.output,
      })),
    };
  }

  getTracesForSession(sessionId: string): NasikoTraceSpan[] {
    return this.spans.filter((s) => s.sessionId === sessionId);
  }
}

export const nasikoObservability = new NasikoObservability();
