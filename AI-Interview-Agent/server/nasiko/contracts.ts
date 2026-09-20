export type AgentName =
  | 'resume_agent'
  | 'interview_agent'
  | 'evaluation_agent'
  | 'report_agent'
  | 'preparation_agent';

export interface NasikoTraceSpan {
  id: string;
  traceId: string;
  sessionId?: string;
  agentName: AgentName;
  startedAt: string;
  completedAt?: string;
  durationMs: number;
  llmLatencyMs: number;
  status: 'success' | 'failed';
  error?: string;
  tokens: {
    promptTokens: number;
    completionTokens: number;
    totalTokens: number;
  };
  inputSummary?: any;
  outputSummary?: any;
}

export interface NasikoAgentMetadata {
  name: AgentName;
  displayName: string;
  version: string;
  description: string;
  responsibility: string;
  healthy: boolean;
  totalExecutions: number;
  successfulExecutions: number;
  failedExecutions: number;
  avgDurationMs: number;
  lastExecutedAt?: string;
}

export interface AgentExecutionContext {
  traceId: string;
  sessionId?: string;
  candidateId?: string;
  tags?: Record<string, string>;
}

export interface AgentResult<TOutput> {
  success: boolean;
  data: TOutput;
  error?: string;
  agentName: AgentName;
  latencyMs: number;
  llmLatencyMs: number;
  tokens: {
    promptTokens: number;
    completionTokens: number;
    totalTokens: number;
  };
  traceId: string;
}

export interface BaseAgent<TInput, TOutput> {
  name: AgentName;
  displayName: string;
  version: string;
  responsibility: string;
  validateInput(input: unknown): { valid: boolean; error?: string };
  validateOutput(output: unknown): { valid: boolean; error?: string };
  execute(input: TInput, context: AgentExecutionContext): Promise<{
    data: TOutput;
    llmLatencyMs: number;
    tokens: { promptTokens: number; completionTokens: number; totalTokens: number };
    rawOutput?: string;
  }>;
}
