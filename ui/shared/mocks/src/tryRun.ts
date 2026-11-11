/**
 * The `try-run` mock workspace (Home's try-a-run card, shared/features/src/overview/tryRun.ts): it starts with no agent
 * activity, like `no-spend`, and each simulated run the card finished in this tab becomes one trace, so the refresh
 * the card asks for shows it in Spend, Tokens, runs and the dashboards. The optimized run sends fewer tokens and costs
 * less, and `/finops/savings` reports the difference, so the second refresh shows the optimization.
 *
 * The runs are the card's (the figures are the demo's, never a real model's): the baseline costs the card's 50 cents.
 */
import type { SavingsData, Savings } from '@nasiko/features/tokenops/types'
import { TRY_RUN_AGENT, type TryRunKind } from '@nasiko/features/overview/tryRun'
import type { Seed, SeedAgent, SeedTrace } from './seed'

interface RunFigures {
  input: number
  output: number
  cost: number
  latencyMs: number
}

/** One board-brief run: a few LLM calls (figures, risks, competitors, questions) rolled into one trace. */
export const TRY_RUN_FIGURES: Record<TryRunKind, RunFigures> = {
  baseline: { input: 41_600, output: 6_400, cost: 0.5, latencyMs: 38_400 },
  // Token optimization trims the payload and history the calls carry, and asks for concise answers.
  optimized: { input: 24_300, output: 4_200, cost: 0.29, latencyMs: 27_900 },
}

const total = (f: RunFigures) => f.input + f.output

/** The agent the runs are recorded against: the one the card says the viewer picked (else the first live seed agent). */
export const tryRunAgent = (seed: Seed): SeedAgent | undefined =>
  seed.agents.find((a) => !a.deleted && a.display_name === TRY_RUN_AGENT) ??
  seed.agents.find((a) => !a.deleted)

/** A finished run as a trace at `at` (epoch ms). */
function traceFor(kind: TryRunKind, agent: SeedAgent, at: number): SeedTrace {
  const f = TRY_RUN_FIGURES[kind]
  const started = new Date(at - f.latencyMs).toISOString()
  // Split as the seed does: prompt and completion priced separately, summing to the run's cost.
  const prompt = Math.round(f.cost * 0.62 * 1e4) / 1e4
  return {
    trace_id:
      kind === 'baseline' ? '5eed0020000000000000000000000001' : '5eed0020000000000000000000000002',
    session_id:
      kind === 'baseline'
        ? '5eed0020-0000-4000-8000-000000000001'
        : '5eed0020-0000-4000-8000-000000000002',
    agent_id: agent.id,
    agent_name: agent.name,
    model: 'gpt-4o',
    provider: 'openai',
    input_tokens: f.input,
    output_tokens: f.output,
    cache_read_tokens: 0,
    cache_creation_tokens: 0,
    cost_usd: f.cost,
    prompt_cost_usd: prompt,
    completion_cost_usd: Math.round((f.cost - prompt) * 1e4) / 1e4,
    latency_ms: f.latencyMs,
    tool_call_count: 4,
    started_at: started,
    ts: Date.parse(started),
    workflow_id: null,
  }
}

/** The workspace's traces: none, then one per run done, at the time this mock first saw it done. */
export function tryRunTraces(
  seed: Seed,
  done: readonly TryRunKind[],
  at: Partial<Record<TryRunKind, number>>,
): SeedTrace[] {
  const agent = tryRunAgent(seed)
  if (!agent) return []
  return done.flatMap((k) => (at[k] === undefined ? [] : [traceFor(k, agent, at[k])]))
}

const round2 = (n: number) => Math.round(n * 100) / 100
const pct = (part: number, whole: number) =>
  whole > 0 ? Math.round((part / whole) * 1000) / 10 : null

/**
 * `/finops/savings` over the demo's runs, shaped like savings.rs: only the optimized run saved anything, and what it
 * saved is what the same run sent without optimization (the baseline run's figures). `base` supplies the window and
 * the coverage the handler then overlays from the agents mock.
 */
export function tryRunSavings(
  base: SavingsData,
  seed: Seed,
  done: readonly TryRunKind[],
): SavingsData {
  const b = TRY_RUN_FIGURES.baseline
  const o = TRY_RUN_FIGURES.optimized
  const ran = done.map((k) => TRY_RUN_FIGURES[k])
  const actualTokens = ran.reduce((n, f) => n + total(f), 0)
  const actualCost = round2(ran.reduce((n, f) => n + f.cost, 0))
  const optimized = done.includes('optimized')
  const savedIn = optimized ? b.input - o.input : 0
  const savedOut = optimized ? b.output - o.output : 0
  const saved = savedIn + savedOut
  const savedCost = optimized ? round2(b.cost - o.cost) : 0
  const block: Savings = {
    saved_tokens: saved,
    saved_input_tokens: savedIn,
    saved_output_tokens: savedOut,
    saved_cost_usd: savedCost,
    actual_tokens: actualTokens,
    actual_cost_usd: actualCost,
    baseline_tokens: actualTokens + saved,
    baseline_cost_usd: round2(actualCost + savedCost),
    token_reduction_pct: pct(saved, actualTokens + saved),
    cost_reduction_pct: pct(savedCost, actualCost + savedCost),
    basis: 'measured',
  }
  const agent = tryRunAgent(seed)
  return {
    ...base,
    total: block,
    by_program: [
      {
        ...block,
        program: 'caveman',
        label: 'Smaller prompts',
        layers: [{ ...block, layer: 'compress_payload' }],
        ...(optimized ? {} : { note: 'No run has had Token optimization on yet.' }),
      },
    ],
    by_agent:
      agent && optimized
        ? [
            {
              ...block,
              agent_id: agent.id,
              agent_name: agent.display_name,
              calls: ran.length,
              input_tokens_before: b.input * 2,
              input_tokens_after: b.input + o.input,
            },
          ]
        : [],
    by_session: [],
    coverage: {
      ...base.coverage,
      calls_in_window: ran.length,
      calls_with_any_layer_enabled: optimized ? 1 : 0,
      optimized_spend_usd: optimized ? o.cost : 0,
      unoptimized_spend_usd: done.includes('baseline') ? b.cost : 0,
      top_unoptimized: undefined,
    },
  }
}
