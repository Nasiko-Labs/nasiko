/**
 * Token optimisation section: a pure summary of the savings payload.
 *
 * Deliberately thin. Both reduction percentages are computed server-side and passed through
 * untouched — re-deriving them here would let this panel and any other consumer disagree about
 * what the denominator was, which is the one arithmetic mistake that would discredit the whole
 * feature. What this file does is ordering, bar scaling, and the empty/zero wording.
 */
import type { ProgramSavings, SavingsBasis, SavingsData } from './types'

const pct = (part: number, whole: number) => (whole > 0 ? (part / whole) * 100 : 0)

/** Rows the agent table renders, ordered by what they saved. */
function agentRows(s: SavingsData) {
  const rows = s.by_agent
    .filter((a) => a.saved_tokens !== 0)
    .map((a) => ({
      id: a.agent_id,
      name: a.agent_name,
      calls: a.calls,
      before: a.input_tokens_before,
      after: a.input_tokens_after,
      savedPct: a.token_reduction_pct ?? 0,
      costSaved: a.saved_cost_usd,
    }))
    .sort((a, b) => b.savedPct - a.savedPct)
  const maxPct = rows[0]?.savedPct ?? 0
  return rows.map((r) => ({ ...r, barPct: pct(r.savedPct, maxPct) }))
}

/**
 * Category rows — "Caveman saved this much, Ponytail saved this much".
 *
 * Programs with no eligible traffic are kept rather than filtered out: a zero that explains itself
 * is actionable ("nobody turned it on"), while an absent row reads as a feature that does nothing.
 * The server sends the reason in `note` for exactly that case.
 */
function categoryRows(s: SavingsData) {
  const rows = s.by_program.map((p: ProgramSavings) => ({
    program: p.program,
    label: p.label,
    savedTokens: p.saved_tokens,
    savedCost: p.saved_cost_usd,
    tokenPct: p.token_reduction_pct,
    costPct: p.cost_reduction_pct,
    basis: p.basis,
    note: p.note,
    /** Shown on hover for any figure that is not a measurement. */
    factorNotes: p.layers.find((l) => l.factor)?.factor?.notes,
    byTier: p.by_tier ?? [],
    layers: p.layers.map((l) => ({
      layer: l.layer,
      savedTokens: l.saved_tokens,
      savedCost: l.saved_cost_usd,
      basis: l.basis,
      notes: l.factor?.notes,
      eligibleTokens: l.factor
        ? l.factor.eligible_input_tokens + l.factor.eligible_output_tokens
        : null,
    })),
  }))
  const max = Math.max(0, ...rows.map((r) => Math.abs(r.savedTokens)))
  return rows.map((r) => ({ ...r, barPct: pct(Math.abs(r.savedTokens), max) }))
}

/** Sessions, biggest saver first — "which conversations did this actually help". */
function sessionRows(s: SavingsData) {
  return s.by_session
    .filter((x) => x.saved_tokens !== 0)
    .map((x) => ({
      id: x.session_id,
      startedAt: x.started_at,
      turns: x.turn_count,
      agents: x.agent_names,
      savedTokens: x.saved_tokens,
      savedCost: x.saved_cost_usd,
      tokenPct: x.token_reduction_pct,
    }))
}

export function summarizeOptimisation(s: SavingsData) {
  const rows = agentRows(s)
  const c = s.coverage
  return {
    rows,
    sessions: sessionRows(s),
    categories: categoryRows(s),
    tokensBefore: s.total.baseline_tokens,
    tokensSaved: s.total.saved_tokens,
    /** Server-computed. Null when nothing was billed in the window. */
    savedPct: s.total.token_reduction_pct,
    costSaved: s.total.saved_cost_usd,
    costSavedPct: s.total.cost_reduction_pct,
    basis: s.total.basis as SavingsBasis,
    calibratedPct: c.calibrated_pct,
    optimisedCount: c.agents_optimized,
    totalAgents: c.agents_total,
    optimisedSharePct: pct(c.optimized_spend_usd, c.optimized_spend_usd + c.unoptimized_spend_usd),
    unoptimisedCount: Math.max(0, c.agents_total - c.agents_optimized),
    unoptimisedSpend: c.unoptimized_spend_usd,
    unoptimisedSharePct: pct(
      c.unoptimized_spend_usd,
      c.optimized_spend_usd + c.unoptimized_spend_usd,
    ),
    topUnoptimised: c.top_unoptimized ?? null,
    /** Per-layer adoption: which switch is under-used, not just how many agents have something on. */
    adoption: [
      { label: 'Compression', on: c.agents_with_compress_enabled },
      { label: 'Minimal code', on: c.agents_with_minimal_code_enabled },
      { label: 'Prompt comments', on: c.agents_with_prompt_comments },
    ],
    /** The biggest contributor, for the one-line headline. */
    topCategory:
      categoryRows(s)
        .filter((r) => r.savedTokens > 0)
        .sort((a, b) => b.savedTokens - a.savedTokens)[0] ?? null,
    callsInWindow: c.calls_in_window,
    callsOptimised: c.calls_with_any_layer_enabled,
  }
}

export type OptimisationView = ReturnType<typeof summarizeOptimisation>

/** True when nothing in the fleet has any layer switched on — an empty state, not a zero. */
export function isUnconfigured(v: OptimisationView) {
  return v.optimisedCount === 0 && v.tokensSaved === 0
}
