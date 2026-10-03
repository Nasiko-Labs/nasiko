/**
 * Finops wire types, hand-written from the Rust structs in
 * nasiko-cloud-rs @ cb3aaf0c, `oss/server/src/observability/service.rs`
 * (FinopsDashboardData, AgentFinopsRow, KpiValue, FinopsSpendTimeseries,
 * FinopsSpendCalendar, FinopsDayDrilldown, WorkflowFinopsRow).
 *
 * Cite struct names, not line numbers. Replace with `npm run gen:api` output once
 * the finops paths are registered in the server's OpenAPI spec (TODOS.md).
 *
 * Checked against live responses: src/test/__live__/oss/tokenops/ (recorded at ea233d20 by
 * `npm run record:live`), replayed by liveParity.test.ts and validated by liveTypes.test.ts.
 *
 * Quirks worth knowing (mirrored by the mocks):
 * - `AgentFinopsRow.agent_name` and day-drilldown slices carry the agent's DISPLAY name
 *   (the server resolves them through live agents; a deleted agent keeps its raw trace
 *   name, and display names aren't unique); timeseries `top_agent_name`
 *   is the raw trace agent name. Neither is accepted as `agent_id` unless it happens to
 *   equal the raw name: map through dashboard rows to the UUID.
 * - Day-drilldown top agents and hourly slices only include agents with spend > 0.
 * - `KpiValue.change_pct` is null when the previous value is 0.
 * - `is_capped` is always false on the current server.
 * - Timeseries/calendar arrays are sparse: only buckets with data appear.
 */
import { z } from 'zod'
import type { WireSubset } from '@/lib/api/client'
import type { components } from '@/lib/api/schema.gen'

export interface KpiValue {
  current: number
  previous: number
  change_pct: number | null
}

export interface FinopsKpis {
  total_spend: KpiValue
  total_tokens: KpiValue
  cost_per_operation: KpiValue
  /** Fleet-wide p50 (label kept for backward compat server-side). */
  avg_latency_ms: KpiValue
  total_agents: KpiValue
  active_agents: KpiValue
  total_operations: KpiValue
  total_tool_calls: KpiValue
  latency_p95_ms: KpiValue
  latency_p99_ms: KpiValue
}

interface FinopsSummary {
  total_cost: number
  total_operations: number
  operations_last_24h: number
  average_cost: number
  active_agents: number
  total_agents: number
  total_container_hours: number
  unpriced_calls: number
  /** Spend priced from inferred rates or usage evidence (`cost_estimated`), ea233d20. */
  estimated_cost: number
  /** Rows materialized before pricing confidence was recorded; counted over every agent name in the window. */
  unknown_confidence_calls: number
}

export interface AgentFinopsRow {
  agent_id: string
  /** Display name, not the raw trace agent name. */
  agent_name: string
  total_cost: number
  operations: number
  is_capped: boolean
  avg_cost_per_operation: number
  prompt_tokens: number
  completion_tokens: number
  cache_read_tokens: number
  cache_creation_tokens: number
  total_tokens: number
  avg_latency_ms: number | null
  avg_latency_p95_ms: number | null
  avg_latency_p99_ms: number | null
  tool_call_count: number
  version: string | null
  container_hours: number
}

export interface WorkflowFinopsRow {
  maf_id: string
  workflow_name: string
  total_cost: number
  executions: number
  avg_cost_per_execution: number
  prompt_tokens: number
  completion_tokens: number
  cache_read_tokens: number
  cache_creation_tokens: number
  total_tokens: number
  avg_latency_ms: number | null
}

export type FinopsAttributions =
  { view: 'agent'; rows: AgentFinopsRow[] } | { view: 'workflow'; rows: WorkflowFinopsRow[] }

export interface FinopsDashboardData {
  summary: FinopsSummary
  agents: AgentFinopsRow[]
  token_usage: {
    total_tokens: number
    prompt_tokens: number
    completion_tokens: number
    cache_read_tokens: number
    cache_creation_tokens: number
    avg_tokens_per_operation: number
  }
  kpis: FinopsKpis
  attributions: FinopsAttributions
  spend_by_agent: {
    slices: { agent_name: string; spend_usd: number; pct: number }[]
    total_spend_usd: number
  }
}

export interface SpendTimeseriesPoint {
  bucket_start: string
  spend_usd: number
  operations: number
  tool_calls: number
  /** Raw trace agent name. */
  top_agent_name: string | null
  top_agent_spend_usd: number | null
  p50_latency_ms: number | null
  p95_latency_ms: number | null
  p99_latency_ms: number | null
}

export interface FinopsSpendTimeseries {
  bucket: 'hour' | 'day'
  points: SpendTimeseriesPoint[]
}

export interface SpendCalendarDay {
  date: string
  spend_usd: number
  operations: number
  intensity: number
}

export interface FinopsSpendCalendar {
  days: SpendCalendarDay[]
  highlighted_dates: string[]
}

interface AgentSpendSlice {
  /** Raw trace agent name. */
  agent_name: string
  spend_usd: number
}

interface SpendHourPoint {
  hour: number
  spend_usd: number
  top_agents: AgentSpendSlice[]
  others_spend_usd: number
}

export interface FinopsDayDrilldown {
  date: string
  hours: SpendHourPoint[]
  avg_hourly_spend_usd: number
  top_agents: AgentSpendSlice[]
  others_spend_usd: number
}

/**
 * PROPOSED contract, not implemented server-side (see plans/feat-tokenops-page.md,
 * "Upstream recommendations"). Mocked in the lab; live mode shows a
 * "needs a newer nasiko-server" state on 404.
 */
interface TopTraceRow {
  trace_id: string
  session_id: string | null
  agent_id: string | null
  agent_name: string
  model: string | null
  provider: string | null
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_creation_tokens: number
  cost_usd: number
  latency_ms: number | null
  tool_call_count: number
  started_at: string
}

export interface TopTracesData {
  rows: TopTraceRow[]
  has_more: boolean
}

/**
 * One provider group of `GET /api/llm-router/providers` (llm_router/providers.rs `ProviderCatalog`): models with
 * their current prices (`pricing_available: false` means served but unpriced) and, for a custom provider, its id
 * and display name. TokenOps reads provider and model names; the router page reads the rest.
 */
export type ProviderCatalogEntry = components['schemas']['ProviderCatalog']

// ─── response schemas (lib/api/client.ts `Checked`): the fields the UI reads, extras allowed ────────────────────
// These endpoints drifted before (CHANGELOG 0.9.0.0); every recorded live response passes them (schemas.live.test.ts).

const num = z.number()
const kpi = z.looseObject({ current: num, previous: num, change_pct: num.nullable() })
const agentRow = z.looseObject({
  agent_id: z.string(),
  agent_name: z.string(),
  total_cost: num,
  operations: num,
  is_capped: z.boolean(),
  avg_cost_per_operation: num,
  prompt_tokens: num,
  cache_read_tokens: num,
  total_tokens: num,
  avg_latency_ms: num.nullable(),
  avg_latency_p95_ms: num.nullable(),
  container_hours: num,
})
const workflowRow = z.looseObject({
  maf_id: z.string(),
  workflow_name: z.string(),
  total_cost: num,
  executions: num,
  avg_cost_per_execution: num,
  prompt_tokens: num,
  cache_read_tokens: num,
  total_tokens: num,
  avg_latency_ms: num.nullable(),
})

/** `GET /api/observability/finops/dashboard` data (KpiStrip, the attribution table, the summary line). */
export const finopsDashboardSchema = z.looseObject({
  summary: z.looseObject({ total_cost: num, total_agents: num, unpriced_calls: num }),
  agents: z.array(agentRow),
  kpis: z.looseObject({
    total_spend: kpi,
    total_tokens: kpi,
    cost_per_operation: kpi,
    avg_latency_ms: kpi,
    active_agents: kpi,
    total_operations: kpi,
    total_tool_calls: kpi,
    latency_p95_ms: kpi,
    latency_p99_ms: kpi,
  }),
  attributions: z.discriminatedUnion('view', [
    z.looseObject({ view: z.literal('agent'), rows: z.array(agentRow) }),
    z.looseObject({ view: z.literal('workflow'), rows: z.array(workflowRow) }),
  ]),
}) satisfies z.ZodType<WireSubset<FinopsDashboardData>>

/** The dashboard fields Harnesses' individual fallback reads (the per-agent rows and the summary). */
export const finopsDashboardRowsSchema = finopsDashboardSchema.pick({ summary: true, agents: true })

const slice = z.looseObject({ agent_name: z.string(), spend_usd: num })

/** `GET /api/observability/finops/spend-calendar/day` data (the day panel). */
export const finopsDaySchema = z.looseObject({
  date: z.string(),
  hours: z.array(
    z.looseObject({
      hour: num,
      spend_usd: num,
      top_agents: z.array(slice),
      others_spend_usd: num,
    }),
  ),
  avg_hourly_spend_usd: num,
  top_agents: z.array(slice),
  others_spend_usd: num,
}) satisfies z.ZodType<WireSubset<FinopsDayDrilldown>>

/** `GET /api/llm-router/providers` body (TokenOps filters, the router's Providers section, `dedupeCatalog`). */
export const providerCatalogSchema = z.looseObject({
  data: z.array(
    z.looseObject({
      provider: z.string(),
      display_name: z.string().nullish(),
      provider_id: z.string().nullish(),
      models: z.array(
        z.looseObject({
          model: z.string(),
          pricing_available: z.boolean(),
          input_price_per_1m: num.nullish(),
          output_price_per_1m: num.nullish(),
          effective_from: z.string().nullish(),
        }),
      ),
    }),
  ),
}) satisfies z.ZodType<WireSubset<{ data: ProviderCatalogEntry[] }>>

/**
 * Proposed: token optimisation savings for a window. No endpoint exists yet, so the Token
 * optimisation section shows `SAMPLE_OPTIMISATION` (optimisation.ts) until one does. Lists only
 * agents with optimisation on; savings are input tokens the optimiser removed before the call.
 */
export interface OptimisationAgentRow {
  agent_id: string
  agent_name: string
  calls: number
  input_tokens_before: number
  input_tokens_after: number
  /** At API list price. */
  est_cost_saved_usd: number
}

export interface OptimisationSummary {
  agents: OptimisationAgentRow[]
  total_agents: number
  fleet_spend_usd: number
  /** What the optimised agents were billed (after optimisation). */
  optimised_spend_usd: number
  /** The biggest spender with optimisation off. */
  top_unoptimised: { agent_id: string; agent_name: string; spend_usd: number } | null
}

/**
 * Savings wire types — `GET /api/observability/finops/savings`
 * (`oss/server/src/observability/savings.rs`: SavingsData, Savings, ProgramSavings, LayerSavings,
 * AgentSavings, Coverage).
 *
 * `Savings` is `#[serde(flatten)]`-ed into every level, so a program, a layer, an agent and the
 * total all carry the same ten fields. Both percentages are server-computed on purpose: deriving
 * them here would let two surfaces disagree about what the denominator was.
 */
export interface Savings {
  saved_tokens: number
  saved_input_tokens: number
  saved_output_tokens: number
  saved_cost_usd: number
  actual_tokens: number
  actual_cost_usd: number
  /** `actual + saved` — what it would have cost without the layer. */
  baseline_tokens: number
  baseline_cost_usd: number
  /** Null when the baseline is zero: undefined, not a fabricated 0. */
  token_reduction_pct: number | null
  cost_reduction_pct: number | null
  basis: SavingsBasis
}

/**
 * How a figure was arrived at. `measured` is a subtraction the server actually performed;
 * `seed_default` and `fixture` are a percentage applied to counted eligible traffic, for the two
 * layers whose counterfactual cannot be observed. `mixed` is a roll-up of both kinds.
 */
export type SavingsBasis = 'measured' | 'fixture' | 'seed_default' | 'mixed'

export interface SavingsFactor {
  input_token_delta_pct: number
  output_token_delta_pct: number
  basis: string
  measured_at: string
  /** One sentence on where the number came from. Served verbatim; show it, don't paraphrase. */
  notes: string
  sample_count?: number
  confidence_pct?: number
  /** Counted, never assumed — which is why a seeded row still reacts to the feature being off. */
  eligible_input_tokens: number
  eligible_output_tokens: number
}

export interface LayerSavings extends Savings {
  layer: string
  factor?: SavingsFactor
}

export interface ProgramSavings extends Savings {
  program: string
  /** How this program is named to users, decided server-side. */
  label: string
  layers: LayerSavings[]
  by_tier?: { tier: string; saved_tokens: number; saved_cost_usd: number }[]
  /** Why this program's figure is zero, when it is. */
  note?: string
}

export interface AgentSavingsRow extends Savings {
  agent_id: string
  agent_name: string
  calls: number
  input_tokens_before: number
  input_tokens_after: number
}

export interface SessionSavingsRow extends Savings {
  session_id: string
  started_at: string
  turn_count: number
  agent_names: string[]
}

export interface SavingsCoverage {
  calls_in_window: number
  calls_with_any_layer_enabled: number
  agents_total: number
  agents_optimized: number
  agents_with_compress_enabled: number
  agents_with_minimal_code_enabled: number
  agents_with_prompt_comments: number
  optimized_spend_usd: number
  unoptimized_spend_usd: number
  top_unoptimized?: { agent_id: string; agent_name: string; spend_usd: number }
  /** Share of measured savings calibrated against real usage rather than the fallback divisor. */
  calibrated_pct: number | null
}

export interface SavingsData {
  window: { start: string; end: string }
  total: Savings
  by_program: ProgramSavings[]
  by_agent: AgentSavingsRow[]
  by_session: SessionSavingsRow[]
  coverage: SavingsCoverage
}
