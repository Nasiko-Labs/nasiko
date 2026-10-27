/**
 * Wire types for context optimization (plans/feat-context-optimization.md §3–§5). None of these routes are in the
 * OpenAPI spec (CX-1), so each is a zod schema checked by `apiFetch(path, { schema })`.
 *
 * Merged (nasiko-cloud-rs `development` 05f22246, `oss/server/src/context_selection.rs`): `ContextStrategyResponse`
 * `{strategy}` and `PacmsBudgetResponse` `{level}`, bare JSON; a value outside the enum is Axum's 422 Json rejection
 * (plain text). Proposed, mocked until the server has them: the `enabled` flag (CX-5), the tier values (CX-T1) and the
 * last-chat preview (CX-6).
 */
import { z } from 'zod'

export const STRATEGIES = ['pacms', 'topk', 'lastk'] as const
export type Strategy = (typeof STRATEGIES)[number]
export const LEVELS = ['low', 'medium', 'high'] as const
export type Level = (typeof LEVELS)[number]

export interface StrategyBody {
  strategy: Strategy
  /** CX-5 (proposed): optimization on/off. Absent on today's server, which is always on. */
  enabled?: boolean
}
export const strategyBodySchema = z.looseObject({
  strategy: z.enum(STRATEGIES),
  enabled: z.boolean().optional(),
})

export interface BudgetBody {
  level: Level
}
export const budgetBodySchema = z.looseObject({ level: z.enum(LEVELS) })

/** One value per tier. */
export type PerLevel = Record<Level, number>
const perLevel = z.looseObject({ low: z.number(), medium: z.number(), high: z.number() })

/** CX-T1 (proposed) `GET /api/settings/context-tiers`: the env values the server runs with. */
export interface Tiers {
  /** PACMS token budgets (`PACMS_BUDGET_*`). */
  pacms_budget: PerLevel
  /** Top-K pairs / Last-K messages (`CONTEXT_K_*`). */
  context_k: PerLevel
  /** `TOKEN_COMPRESS_HISTORY`. */
  compress_history: boolean
  // Read by the tiers page only; optional so the Optimization page's figure never fails on them.
  /** `PACMS_HISTORY_POOL_SIZE`. */
  pool_size?: number
  /** `PACMS_HISTORY_MANDATORY_RECENT`. */
  mandatory_recent?: number
  /** `TOKEN_COMPRESS_HISTORY_MIN_BYTES`. */
  compress_min_bytes?: number
  /** The write guard (ledger V2): the PUT sends it back as `expected_updated_at` and gets a 409 when it moved. */
  updated_at?: string
}
/** Every value, without the guard: what the form edits and the PUT writes. */
export type FullTiers = Required<Omit<Tiers, 'updated_at'>>
export const tiersSchema = z.looseObject({
  pacms_budget: perLevel,
  context_k: perLevel,
  compress_history: z.boolean(),
  pool_size: z.number().optional(),
  mandatory_recent: z.number().optional(),
  compress_min_bytes: z.number().optional(),
  updated_at: z.string().optional(),
})

/** The server's built-in defaults (nasiko-cloud-rs `oss/config/src/lib.rs` at 05f22246), shown on a server that can't
 *  report its own values (design review 2B: labelled as defaults the environment may override). */
export const SERVER_DEFAULT_TIERS: FullTiers = {
  pacms_budget: { low: 500, medium: 1000, high: 5000 },
  context_k: { low: 1, medium: 5, high: 20 },
  pool_size: 150,
  mandatory_recent: 3,
  compress_history: true,
  compress_min_bytes: 2048,
}

interface Carried {
  messages: number
  tokens: number
}
const carried = z.looseObject({ messages: z.number(), tokens: z.number() })

/**
 * CX-6 (proposed) `POST /api/me/context-preview` `{strategy}`: selection run on the caller's latest session at every
 * tier, nothing sent. `pool` is the baseline (design review 2A): the messages selection chooses from (up to the pool
 * size), before selection. `session_messages` is the whole chat's length, for the caption. 404 coded `no_session`: no
 * chats yet.
 */
export interface Preview {
  strategy: Strategy
  session_messages: number
  pool: Carried
  tiers: Record<Level, Carried>
}
export const previewSchema = z.looseObject({
  strategy: z.enum(STRATEGIES),
  session_messages: z.number(),
  pool: carried,
  tiers: z.looseObject({ low: carried, medium: carried, high: carried }),
})

/**
 * CX-V3 (proposed) `GET /api/observability/finops/context-savings?<window>&agent_id=[&series=daily|hourly]`, in the
 * finops `{data}` envelope: the window's baseline (pool) against what was sent, at the request's input price, plus
 * coverage. One report per selection, tokens counted once (eng C3). CX-V3a adds the per-bucket `series` (E1, C2).
 */
export interface SavingsPoint {
  bucket_start: string
  eligible_requests: number
  reports: number
  pool_tokens: number
  sent_tokens: number
}

export interface ContextSavings {
  /** Requests that carried a CX-V1 report in the window. */
  reports: number
  /** Requests that ran selection in the window (C2); absent on the first CX-V3 draft. */
  eligible_requests?: number
  /** Reports with a known input price; costs cover only these (C3). */
  priced_reports?: number
  pool_tokens: number
  sent_tokens: number
  pool_cost_usd: number | null
  sent_cost_usd: number | null
  messages_dropped: number
  compressed_bytes: number
  /** The first report the server has (ISO); a window starting earlier isn't fully counted (2H). */
  recorded_since: string | null
  /** CX-V3a, with `series=`: hourly for 24h, daily otherwise. */
  series?: { bucket: 'hour' | 'day'; points: SavingsPoint[] }
}
/**
 * A timestamp the page will format: an unparseable one fails the read (a recoverable block error) instead of throwing
 * while rendering and taking the whole page down (review: Codex).
 */
const timestamp = z.string().refine((s) => !Number.isNaN(Date.parse(s)), 'not a timestamp')

const savingsPoint = z.looseObject({
  bucket_start: timestamp,
  eligible_requests: z.number(),
  reports: z.number(),
  pool_tokens: z.number(),
  sent_tokens: z.number(),
})
export const contextSavingsSchema = z.looseObject({
  reports: z.number(),
  eligible_requests: z.number().optional(),
  priced_reports: z.number().optional(),
  pool_tokens: z.number(),
  sent_tokens: z.number(),
  pool_cost_usd: z.number().nullable(),
  sent_cost_usd: z.number().nullable(),
  messages_dropped: z.number(),
  compressed_bytes: z.number(),
  recorded_since: timestamp.nullable(),
  series: z
    .looseObject({ bucket: z.enum(['hour', 'day']), points: z.array(savingsPoint) })
    .optional(),
})

/** CX-V3b (proposed) `GET …/context-savings/agents?<window>`: per agent, history volume first. */
export interface SavingsAgent {
  agent_id: string
  name: string
  requests: number
  reports: number
  pool_tokens: number
  sent_tokens: number
  compress_enabled: boolean
}
export const savingsAgentsSchema = z.array(
  z.looseObject({
    agent_id: z.string(),
    name: z.string(),
    requests: z.number(),
    reports: z.number(),
    pool_tokens: z.number(),
    sent_tokens: z.number(),
    compress_enabled: z.boolean(),
  }),
)

/**
 * CX-V3c (proposed) `GET …/context-savings/top-requests?<window>&from=&to=&limit=`: the requests that carried the
 * most history. `own: false` rows are another user's chat: no title, session or trace (eng C4).
 */
export interface TopRequest {
  own: boolean
  trace_id: string | null
  span_id: string | null
  session_id: string | null
  chat_title: string | null
  agent_id: string
  agent_name: string
  pool_tokens: number
  sent_tokens: number
  compress_enabled: boolean
  started_at: string
}
export const topRequestsSchema = z.array(
  z.looseObject({
    own: z.boolean(),
    trace_id: z.string().nullable(),
    span_id: z.string().nullable(),
    session_id: z.string().nullable(),
    chat_title: z.string().nullable(),
    agent_id: z.string(),
    agent_name: z.string(),
    pool_tokens: z.number(),
    sent_tokens: z.number(),
    compress_enabled: z.boolean(),
    started_at: timestamp,
  }),
)

/** CX-H (proposed) `GET /api/me/settings-history?<window>`: your optimization setting changes, newest first. */
export interface SettingsChange {
  at: string
  field: 'strategy' | 'level' | 'enabled'
  from: string
  to: string
}
export const settingsHistorySchema = z.array(
  z.looseObject({
    at: timestamp,
    field: z.enum(['strategy', 'level', 'enabled']),
    // Strings per CX-H; a boolean for `enabled` is accepted rather than failing the list, and api.ts turns it into
    // "true"/"false" (apiFetch validates, it doesn't transform; review: api-contract, Codex).
    from: z.union([z.string(), z.boolean()]),
    to: z.union([z.string(), z.boolean()]),
  }),
)
