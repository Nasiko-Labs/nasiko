/**
 * Types for Nasiko P2 Request Classifier
 */

export interface ClassifyRequestBody {
  query: string;
  context?: string;
  demo?: boolean;
}

/**
 * Flexible raw backend response interface.
 * Accommodates snake_case and camelCase from varying backend implementations.
 */
export interface RawClassifierResponse {
  request_type?: string;
  requestType?: string;
  complexity?: number | string | null;
  confidence?: number | string | null;
  tier?: string | null;
  classifier?: string | null;
  backend?: string | null;
  model?: string | null;
  fallback_used?: boolean;
  fallbackUsed?: boolean;
  latency_ms?: number;
  latency_us?: number;
  latencyMs?: number;
  baseline?: RawClassifierResponse | null;
  comparison?: RawClassifierResponse | null;
  [key: string]: unknown;
}

export interface NormalizedClassifierResult {
  requestType: string;
  requestTypeLabel: string;
  requestTypeDescription: string;
  complexity: number | null; // 1 to 5 scale
  confidence: number | null; // 0 to 1 float
  confidencePct: number | null; // 0 to 100 percentage
  tier: string | null; // e.g. "tier_1", "tier_2", "tier_3"
  tierLabel: string | null; // e.g. "Tier 1 · Complex Reasoning"
  classifier: string | null; // e.g. "hybrid-router-v2"
  latencyMs: number | null;
  fallbackUsed: boolean;
  baseline?: NormalizedClassifierResult | null;
  raw: Record<string, unknown>;
  isDemo?: boolean;
}

export interface ApiResponse<T = NormalizedClassifierResult> {
  success: boolean;
  data?: T;
  error?: string;
  details?: string;
  statusCode?: number;
}

export interface HealthCheckResponse {
  configured: boolean;
  railwayUrlConfigured: boolean;
  environment: string;
  backendHealthy?: boolean;
  backendStatus?: string;
}
