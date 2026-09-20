import {
  AgentExecutionContext,
  AgentName,
  AgentResult,
  BaseAgent,
  NasikoTraceSpan,
} from './contracts';
import { nasikoObservability } from './observability';
import { nasikoClientBridge } from './clientBridge';

class NasikoOrchestrator {
  private agents: Map<AgentName, BaseAgent<any, any>> = new Map();

  registerAgent(agent: BaseAgent<any, any>) {
    this.agents.set(agent.name, agent);
    console.log(`[Nasiko Orchestrator] Registered agent: ${agent.name} (${agent.displayName})`);
  }

  getAgent(name: AgentName): BaseAgent<any, any> | undefined {
    return this.agents.get(name);
  }

  async execute<TInput, TOutput>(
    agentName: AgentName,
    input: TInput,
    context?: Partial<AgentExecutionContext>
  ): Promise<AgentResult<TOutput>> {
    const traceId = context?.traceId || `trace_${Date.now()}_${Math.random().toString(36).substring(2, 8)}`;
    const executionContext: AgentExecutionContext = {
      traceId,
      sessionId: context?.sessionId,
      candidateId: context?.candidateId,
      tags: context?.tags,
    };

    const agent = this.agents.get(agentName);
    if (!agent) {
      const err = `Agent "${agentName}" is not registered in Nasiko Orchestration layer.`;
      const failedSpan: NasikoTraceSpan = {
        id: `span_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
        traceId,
        sessionId: context?.sessionId,
        agentName,
        startedAt: new Date().toISOString(),
        completedAt: new Date().toISOString(),
        durationMs: 0,
        llmLatencyMs: 0,
        status: 'failed',
        error: err,
        tokens: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      };
      nasikoObservability.recordSpan(failedSpan);
      throw new Error(err);
    }

    // 1. Input Contract Validation
    const inputValidation = agent.validateInput(input);
    if (!inputValidation.valid) {
      const err = `Input contract violation for ${agentName}: ${inputValidation.error}`;
      const failedSpan: NasikoTraceSpan = {
        id: `span_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
        traceId,
        sessionId: context?.sessionId,
        agentName,
        startedAt: new Date().toISOString(),
        completedAt: new Date().toISOString(),
        durationMs: 0,
        llmLatencyMs: 0,
        status: 'failed',
        error: err,
        tokens: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      };
      nasikoObservability.recordSpan(failedSpan);
      throw new Error(err);
    }

    const startTime = Date.now();
    try {
      // 2. Agent Execution
      const result = await agent.execute(input, executionContext);
      const durationMs = Date.now() - startTime;

      // 3. Output Contract Validation
      const outputValidation = agent.validateOutput(result.data);
      if (!outputValidation.valid) {
        throw new Error(
          `Output contract violation from ${agentName}: ${outputValidation.error}`
        );
      }

      // 4. Record Trace in Nasiko
      const span: NasikoTraceSpan = {
        id: `span_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
        traceId,
        sessionId: context?.sessionId,
        agentName,
        startedAt: new Date(startTime).toISOString(),
        completedAt: new Date().toISOString(),
        durationMs,
        llmLatencyMs: result.llmLatencyMs,
        status: 'success',
        tokens: result.tokens,
        inputSummary: this.summarizePayload(input),
        outputSummary: this.summarizePayload(result.data),
      };
      nasikoObservability.recordSpan(span);
      nasikoClientBridge.syncSpanToNasikoDocker(span).catch(() => {});

      return {
        success: true,
        data: result.data as TOutput,
        agentName,
        latencyMs: durationMs,
        llmLatencyMs: result.llmLatencyMs,
        tokens: result.tokens,
        traceId,
      };
    } catch (error: any) {
      const durationMs = Date.now() - startTime;
      const errorMessage = error.message || String(error);
      const span: NasikoTraceSpan = {
        id: `span_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
        traceId,
        sessionId: context?.sessionId,
        agentName,
        startedAt: new Date(startTime).toISOString(),
        completedAt: new Date().toISOString(),
        durationMs,
        llmLatencyMs: 0,
        status: 'failed',
        error: errorMessage,
        tokens: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      };
      nasikoObservability.recordSpan(span);
      nasikoClientBridge.syncSpanToNasikoDocker(span).catch(() => {});
      throw error;
    }
  }

  private summarizePayload(payload: any): any {
    if (!payload || typeof payload !== 'object') return payload;
    const summary: Record<string, any> = {};
    for (const key of Object.keys(payload)) {
      const val = payload[key];
      if (typeof val === 'string' && val.length > 100) {
        summary[key] = `${val.substring(0, 100)}... (${val.length} chars)`;
      } else if (Array.isArray(val)) {
        summary[key] = `Array(${val.length})`;
      } else if (typeof val === 'object' && val !== null) {
        summary[key] = `{keys: ${Object.keys(val).join(', ')}}`;
      } else {
        summary[key] = val;
      }
    }
    return summary;
  }
}

export const nasikoOrchestrator = new NasikoOrchestrator();
