/**
 * Pure rules for context optimization (plans/feat-context-optimization.md §0, §3, §11), tested in logic.test.ts.
 */
import {
  LEVELS,
  STRATEGIES,
  type FullTiers,
  type Level,
  type PerLevel,
  type Strategy,
  type Tiers,
} from './types'

/**
 * Design review 1A: section numbers count what is shown. A hidden section (the on/off switch before CX-5) takes no
 * number, so the rest read 1, 2, 3 with no gap. Returns the visible keys with their numbers, in order.
 */
export function numberVisible<K extends string>(
  sections: readonly { key: K; shown: boolean }[],
): { key: K; n: number }[] {
  return sections.filter((s) => s.shown).map((s, i) => ({ key: s.key, n: i + 1 }))
}

/** An owned agent as the compression rule reads it: a full `GET /api/agents` row. */
export interface OwnedAgent {
  id: string
  name: string
  /** `agents.compress_enabled` (catalog/models.rs `Agent`); a row without it reads as off, like the server's default. */
  compressEnabled: boolean
}

export type CompressionState =
  | { on: false; reason: 'server-off' }
  | { on: false; reason: 'no-agents' }
  | { on: false; reason: 'some-off'; off: OwnedAgent[]; total: number }
  | { on: true; reason: 'all-on' }
  /** Every listed agent allows it, but something the page can't see could still turn it off (review finding 1). */
  | { on: 'likely'; reason: 'likely-on' }

/**
 * Whether the caller's history (and the Orchestrator's tool results) are compressed: nasiko-cloud-rs
 * `oss/orchestrator/src/context_selection.rs` `compression_opt_in` is `count(*) > 0 AND bool_and(compress_enabled)`
 * over the caller's live agents, harness rows included (eng review F1, E2), and the deployment flag
 * `TOKEN_COMPRESS_HISTORY` gates it all. "On" is said only when it is certain:
 * - `serverOn`: that flag when the server reports it (CX-T1), else unknown;
 * - `hiddenMayCount`: the rule also counts `is_internal` agents, which `GET /api/agents` hides (EE seeds one for the
 *   first superuser, CX-3b);
 * - `capped`: the owned list stopped at its page cap, so an agent past it is unseen.
 */
export function compressionState(
  agents: readonly OwnedAgent[],
  opts: { serverOn?: boolean; hiddenMayCount?: boolean; capped?: boolean } = {},
): CompressionState {
  if (opts.serverOn === false) return { on: false, reason: 'server-off' }
  if (agents.length === 0) return { on: false, reason: 'no-agents' }
  const off = agents.filter((a) => !a.compressEnabled)
  if (off.length > 0) return { on: false, reason: 'some-off', off, total: agents.length }
  if (opts.serverOn === true && !opts.hiddenMayCount && !opts.capped)
    return { on: true, reason: 'all-on' }
  return { on: 'likely', reason: 'likely-on' }
}

/** A `GET /api/agents` row (the generated `Agent` type predates `compress_enabled`) as the rule reads it. */
export function toOwnedAgent(row: {
  id: string
  name: string
  display_name?: string | null
}): OwnedAgent {
  const flag = (row as { compress_enabled?: unknown }).compress_enabled
  return { id: row.id, name: row.display_name || row.name, compressEnabled: flag === true }
}

/**
 * The figure beside a budget level (design review 2B): only when the server reports its tiers. PACMS levels are token
 * budgets; Top-K keeps pairs, Last-K messages.
 */
export function tierFigure(
  tiers: Tiers | undefined,
  strategy: Strategy,
  level: Level,
): { kind: 'tokens' | 'pairs' | 'messages'; value: number } | null {
  if (!tiers) return null
  const table: PerLevel = strategy === 'pacms' ? tiers.pacms_budget : tiers.context_k
  const kind = strategy === 'pacms' ? 'tokens' : strategy === 'topk' ? 'pairs' : 'messages'
  return { kind, value: table[level] }
}

/** Save sends only what changed (CX-2: two independent routes). */
export interface PreferenceEdit {
  strategy?: Strategy
  level?: Level
  enabled?: boolean
}

type PreferenceField = keyof PreferenceEdit

export function changedFields(
  saved: Required<PreferenceEdit>,
  draft: Required<PreferenceEdit>,
  withSwitch: boolean,
): PreferenceEdit {
  const out: PreferenceEdit = {}
  if (draft.strategy !== saved.strategy) out.strategy = draft.strategy
  if (draft.level !== saved.level) out.level = draft.level
  if (withSwitch && draft.enabled !== saved.enabled) out.enabled = draft.enabled
  return out
}

/**
 * What a Save did (design review 2C): each field's request settles on its own. The strategy route carries the switch
 * too (CX-5 proposes `enabled` beside `strategy`), so they succeed or fail together.
 */
export interface SaveOutcome {
  saved: PreferenceField[]
  failed: { field: PreferenceField; message: string }[]
}

/** Rounded for "~" figures: small numbers exact, larger ones to `digits` significant digits (two by default). */
export function approx(n: number, digits = 2): number {
  if (n < 100) return Math.round(n)
  const p = 10 ** (Math.floor(Math.log10(n)) - (digits - 1))
  return Math.round(n / p) * p
}

/**
 * What one request carried (CX-V1, proposed): the `nasiko.context.*` attributes the server would record on the
 * `a2a.dispatch` / `a2a.proxy` span, plus the L0 flag. Parsed only here (eng review F5), from flat dotted attributes
 * (`flattenAttributes`); `null` when the span has no report, so the trace renders exactly as before (E4).
 */
export interface ContextReport {
  strategy: Strategy | null
  level: Level | null
  /** The baseline (2A): the pool selection chose from. */
  pool: { messages: number; tokens: number }
  kept: { messages: number; tokens: number }
  compressedBytesSaved: number | null
  /** F2: only `true` is shown; OSS records `false`. */
  orgPolicyApplied: boolean
}

const num = (v: unknown): number | null =>
  typeof v === 'number' && Number.isFinite(v) && v >= 0
    ? v
    : typeof v === 'string' && v.trim() !== '' && Number.isFinite(Number(v)) && Number(v) >= 0
      ? Number(v)
      : null
const oneOf = <T extends string>(v: unknown, set: readonly T[]): T | null =>
  typeof v === 'string' && (set as readonly string[]).includes(v) ? (v as T) : null

/** L0's flag on the request span (`nasiko.prompt_context.org_applied`); the l0 branch records it without counts. */
export function orgPolicyApplied(attrs: Record<string, unknown>): boolean {
  const v = attrs['nasiko.prompt_context.org_applied']
  return v === true || v === 'true'
}

export function contextReport(attrs: Record<string, unknown>): ContextReport | null {
  const pool = num(attrs['nasiko.context.pool'])
  const poolTokens = num(attrs['nasiko.context.pool_tokens'])
  const kept = num(attrs['nasiko.context.kept'])
  const keptTokens = num(attrs['nasiko.context.kept_tokens_est'])
  if (pool === null || poolTokens === null || kept === null || keptTokens === null) return null
  const saved = num(attrs['nasiko.context.compressed_bytes_saved'])
  return {
    strategy: oneOf(attrs['nasiko.context.strategy'], STRATEGIES),
    level: oneOf(attrs['nasiko.context.level'], LEVELS),
    pool: { messages: pool, tokens: poolTokens },
    kept: { messages: kept, tokens: keptTokens },
    compressedBytesSaved: saved && saved > 0 ? saved : null,
    orgPolicyApplied: orgPolicyApplied(attrs),
  }
}

/** The change from the baseline as a whole percent (negative = fewer tokens), or null with no baseline. */
export function tokenDelta(r: ContextReport): number | null {
  if (r.pool.tokens <= 0) return null
  return Math.round(((r.kept.tokens - r.pool.tokens) / r.pool.tokens) * 100)
}

/** The span a request's report lives on (design review 1B): the nearest `a2a.dispatch` / `a2a.proxy` at or above. */
const REQUEST_SPAN = /^a2a\.(dispatch|proxy)\b/
export function requestSpanOf<
  S extends { node: { id: string; name: string; parent_id?: string | null } },
>(spans: readonly S[], span: S): S | null {
  const byId = new Map(spans.map((s) => [s.node.id, s]))
  const seen = new Set<string>()
  for (let s: S | undefined = span; s && !seen.has(s.node.id);) {
    if (REQUEST_SPAN.test(s.node.name)) return s
    seen.add(s.node.id)
    s = s.node.parent_id ? byId.get(s.node.parent_id) : undefined
  }
  return null
}

/** A tier value's form field: `pacms_budget.low`, `pool_size`, … */
export type TierField =
  | `${'pacms_budget' | 'context_k'}.${Level}`
  | 'pool_size'
  | 'mandatory_recent'
  | 'compress_min_bytes'

export type TierProblem = 'whole' | 'order' | 'pool'

/**
 * The tiers form's rules (plans/feat-context-optimization.md §4): whole numbers ≥ 1, Low ≤ Medium ≤ High in each
 * table, and a pool at least as large as the always-kept messages. The mock's PUT applies the same rules.
 */
export function tierProblems(t: FullTiers): Partial<Record<TierField, TierProblem>> {
  const out: Partial<Record<TierField, TierProblem>> = {}
  const whole = (v: number) => Number.isSafeInteger(v) && v >= 1
  for (const table of ['pacms_budget', 'context_k'] as const) {
    for (const l of LEVELS) if (!whole(t[table][l])) out[`${table}.${l}`] = 'whole'
    if (!out[`${table}.medium`] && !out[`${table}.low`] && t[table].low > t[table].medium)
      out[`${table}.medium`] = 'order'
    if (!out[`${table}.high`] && !out[`${table}.medium`] && t[table].medium > t[table].high)
      out[`${table}.high`] = 'order'
  }
  for (const k of ['pool_size', 'mandatory_recent', 'compress_min_bytes'] as const)
    if (!whole(t[k])) out[k] = 'whole'
  if (!out.pool_size && !out.mandatory_recent && t.pool_size < t.mandatory_recent)
    out.pool_size = 'pool'
  return out
}
