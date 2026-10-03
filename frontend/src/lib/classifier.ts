import {
  RawClassifierResponse,
  NormalizedClassifierResult,
} from './types';

export const REQUEST_TYPE_META: Record<string, { label: string; description: string }> = {
  code_generation: {
    label: 'Code Generation',
    description: 'Synthesis of new functions, classes, modules, or automated algorithms.',
  },
  code_understanding: {
    label: 'Code Understanding',
    description: 'Code comprehension, debugging, syntax review, or AST inspection.',
  },
  technical_design: {
    label: 'Technical Design',
    description: 'Architecture blueprints, API schemas, systems design, and trade-off analysis.',
  },
  analytical_reasoning: {
    label: 'Analytical Reasoning',
    description: 'Multi-step deduction, math, probabilistic modeling, or logic puzzles.',
  },
  writing: {
    label: 'Writing & Composition',
    description: 'Prose drafting, documentation, long-form synthesis, and editorial refinement.',
  },
  factual_lookup: {
    label: 'Factual Lookup',
    description: 'Deterministic queries, definitions, API lookups, and direct factual answers.',
  },
  general: {
    label: 'General Query',
    description: 'General conversational requests or unspecialized instructions.',
  },
};

export const TIER_META: Record<string, { label: string; description: string; badge: string }> = {
  tier_1: {
    label: 'Tier 1 · High Reasoning',
    description: 'Most capable models (e.g. Claude 3.5 Sonnet, GPT-4o) for high-complexity tasks.',
    badge: 'T1 · HEAVY',
  },
  tier_2: {
    label: 'Tier 2 · Balanced',
    description: 'Balanced throughput/cost models (e.g. GPT-4o-mini, Claude 3.5 Haiku).',
    badge: 'T2 · BALANCED',
  },
  tier_3: {
    label: 'Tier 3 · Fast / Compact',
    description: 'Lightweight, ultra-low latency small models for routine or simple queries.',
    badge: 'T3 · COMPACT',
  },
};

/**
 * Normalizes any backend response into a standardized shape.
 * Backend engineers can tweak field names here in one place.
 */
export function normalizeClassifierResponse(
  rawInput: unknown,
  measuredLatencyMs?: number
): NormalizedClassifierResult {
  const raw = (typeof rawInput === 'object' && rawInput !== null ? rawInput : {}) as RawClassifierResponse;

  // 1. Request Type
  const rawType = (raw.request_type || raw.requestType || 'general').toString().toLowerCase().trim();
  const meta = REQUEST_TYPE_META[rawType] || {
    label: rawType.replace(/_/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase()),
    description: 'Classifier detected request type.',
  };

  // 2. Complexity (Expected 1 - 5)
  let complexity: number | null = null;
  if (raw.complexity !== undefined && raw.complexity !== null) {
    const parsed = Number(raw.complexity);
    if (!Number.isNaN(parsed)) {
      complexity = Math.max(1, Math.min(5, Math.round(parsed)));
    }
  }

  // 3. Confidence (Expected 0.0 - 1.0)
  let confidence: number | null = null;
  let confidencePct: number | null = null;
  if (raw.confidence !== undefined && raw.confidence !== null) {
    const parsed = Number(raw.confidence);
    if (!Number.isNaN(parsed)) {
      // If backend returned percentage > 1 (e.g., 85), normalize to 0..1
      const normalized0to1 = parsed > 1 ? parsed / 100 : parsed;
      confidence = Math.max(0, Math.min(1, Math.round(normalized0to1 * 1000) / 1000));
      confidencePct = Math.round(confidence * 100);
    }
  }

  // 4. Model Tier
  let tier: string | null = null;
  let tierLabel: string | null = null;
  if (raw.tier) {
    const rawTier = raw.tier.toString().toLowerCase().replace(/-/g, '_').trim();
    tier = rawTier;
    tierLabel = TIER_META[rawTier]?.label || rawTier.toUpperCase();
  }

  // 5. Classifier / Backend Name
  const classifier = (raw.classifier || raw.backend || raw.model || null)?.toString() || null;

  // 6. Latency in milliseconds
  let latencyMs: number | null = null;
  if (raw.latency_ms !== undefined && raw.latency_ms !== null) {
    latencyMs = Math.round(Number(raw.latency_ms));
  } else if (raw.latency_us !== undefined && raw.latency_us !== null) {
    latencyMs = Math.round(Number(raw.latency_us) / 1000);
  } else if (raw.latencyMs !== undefined && raw.latencyMs !== null) {
    latencyMs = Math.round(Number(raw.latencyMs));
  } else if (measuredLatencyMs !== undefined) {
    latencyMs = measuredLatencyMs;
  }

  // 7. Fallback Flag
  const fallbackUsed = Boolean(raw.fallback_used ?? raw.fallbackUsed ?? false);

  // 8. Baseline comparison (if backend supplied one)
  let baseline: NormalizedClassifierResult | null = null;
  const rawBaseline = raw.baseline || raw.comparison;
  if (rawBaseline && typeof rawBaseline === 'object') {
    baseline = normalizeClassifierResponse(rawBaseline);
  }

  return {
    requestType: rawType,
    requestTypeLabel: meta.label,
    requestTypeDescription: meta.description,
    complexity,
    confidence,
    confidencePct,
    tier,
    tierLabel,
    classifier,
    latencyMs,
    fallbackUsed,
    baseline,
    raw: raw as Record<string, unknown>,
  };
}

/**
 * Deterministic mock data generator ONLY for explicit demo mode.
 * Never used silently when the backend fails.
 */
export function generateDemoResponse(query: string, context?: string): NormalizedClassifierResult {
  const lower = `${query} ${context || ''}`.toLowerCase();

  let requestType = 'general';
  let complexity = 2;
  let confidence = 0.88;
  let tier = 'tier_2';
  let fallbackUsed = false;

  if (
    lower.includes('code') ||
    lower.includes('script') ||
    lower.includes('python') ||
    lower.includes('sql') ||
    lower.includes('refactor') ||
    lower.includes('function')
  ) {
    requestType = 'code_generation';
    complexity = lower.includes('refactor') || lower.includes('trade-off') ? 4 : 3;
    confidence = 0.94;
    tier = complexity >= 4 ? 'tier_1' : 'tier_2';
  } else if (
    lower.includes('design') ||
    lower.includes('architecture') ||
    lower.includes('schema') ||
    lower.includes('system')
  ) {
    requestType = 'technical_design';
    complexity = 4;
    confidence = 0.91;
    tier = 'tier_1';
  } else if (
    lower.includes('explain') ||
    lower.includes('why') ||
    lower.includes('understand') ||
    lower.includes('analyze')
  ) {
    requestType = 'code_understanding';
    complexity = 3;
    confidence = 0.86;
    tier = 'tier_2';
  } else if (
    lower.includes('what is') ||
    lower.includes('who') ||
    lower.includes('lookup') ||
    lower.includes('capital')
  ) {
    requestType = 'factual_lookup';
    complexity = 1;
    confidence = 0.97;
    tier = 'tier_3';
  } else if (
    lower.includes('linear b') ||
    lower.includes('frobenius') ||
    lower.includes('elliptic') ||
    lower.includes('quantum')
  ) {
    requestType = 'analytical_reasoning';
    complexity = 5;
    confidence = 0.79;
    tier = 'tier_1';
  }

  // Simulated noisy / typo edge cases might trigger fallback
  if (lower.includes('plz') || lower.includes('wrte') || lower.includes('nevr')) {
    confidence = 0.74;
    if (lower.length < 20) {
      fallbackUsed = true;
    }
  }

  const result = normalizeClassifierResponse(
    {
      request_type: requestType,
      complexity,
      confidence,
      tier,
      classifier: 'nasiko-router-v2/adaptive',
      fallback_used: fallbackUsed,
      latency_ms: Math.floor(Math.random() * 25 + 15),
      baseline: {
        request_type: 'general',
        complexity: 3,
        confidence: 0.5,
        tier: 'tier_2',
        classifier: 'regex-baseline',
        fallback_used: true,
        latency_ms: 2,
      },
    },
    22
  );

  result.isDemo = true;
  return result;
}
