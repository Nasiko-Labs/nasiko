export function cn(...parts: Array<string | false | null | undefined>) {
  return parts.filter(Boolean).join(" ");
}

export const METHODS = {
  A: { name: "Stock cache", color: "#71817b" },
  B: { name: "Stable prefixes", color: "#237d77" },
  C: { name: "Workflow policy", color: "#ac7b38" },
  D: { name: "Frequency baseline", color: "#8178a6" },
  E: { name: "Jev-assisted", color: "#37652f" },
} as const;

export type Method = keyof typeof METHODS;

export type CacheUsage = {
  prompt_tokens?: number;
  cached_tokens?: number;
  generated_tokens?: number;
  cached_fraction: number | null;
  requests_with_cache_usage?: number;
};

export type TrialRow = {
  method: Method;
  repetition: number;
  concurrency: number;
  tasks: number;
  successes: number;
  failed: number;
  p50_seconds: number | null;
  p95_seconds: number | null;
  successes_per_hour: number | null;
  cache_usage: CacheUsage;
  acknowledged_workflows?: number | null;
  cost_per_thousand: number | null;
  cost_basis?: string;
};

export type WorkflowEvent = {
  agent: string;
  status: string;
  time: number;
};

export type EngineAck = { status: string };

export type Hint = { lease_seconds?: number };

export type PolicyCost = { resident_bytes?: number };

export type WorkflowRecord = {
  task_id: string;
  repository?: string;
  method: Method;
  repetition: number;
  concurrency?: number;
  flow_id?: string;
  events?: WorkflowEvent[];
  success: boolean;
  seconds?: number;
  jev?: unknown;
  hints?: Hint[];
  engine_acknowledgements?: EngineAck[];
  repair_probability?: number | null;
  policy_cost?: PolicyCost | null;
  decision_after_test?: boolean | null;
  error?: string | null;
};

export type Report = {
  status: string;
  model: string;
  gpu: string;
  rows: TrialRow[];
  records: WorkflowRecord[];
  notes?: string[];
  nasiko_url?: string | null;
};

export const PUBLISHED = {
  workflows: 300,
  passed: 300,
  model: "Qwen3.8-27B BF16",
  gpu: "A100 80 GB",
  engine: "vLLM 0.29.0",
  aP95: 39.95,
  aCost: 2.6,
  reuseCache: 78,
  acknowledgements: 135,
} as const;
