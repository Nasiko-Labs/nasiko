/**
 * Routed-turn Activity, attribution and the routed metric (plan §5.7, §5.8, §5.14). Pure: live
 * steps and the flows fallback go through the same top-level selection, so attribution reads the
 * same before and after a reload (EN-7).
 */
import type { Step, TurnState } from './a2aReducer'

/** `FlowStep` (oss/server/src/flows.rs, cb3aaf0c); `GET /api/flows/{id}` → `{flow, steps}`. */
export interface FlowStep {
  step_order: number
  depth: number
  agent_id?: string | null
  agent_name: string
  caller_agent_name?: string | null
  status: string
  latency_ms?: number | null
}

export interface FlowDetail {
  flow: Record<string, unknown>
  steps: FlowStep[]
}

const ORCHESTRATOR = 'orchestrator'

/** Top-level agent calls of a live turn: not relayed by a nested agent (`via`), not the orchestrator. */
const topLevelSteps = (steps: readonly Step[]): Step[] =>
  steps.filter((s) => s.kind === 'agent' && !s.via && s.name !== ORCHESTRATOR)

/** Distinct agents in first-call order (E-A4: a selector, never stored state). */
const distinctNames = (names: readonly string[]): string[] => [...new Set(names)]

export const agentsAsked = (state: Pick<TurnState, 'steps'>): string[] =>
  distinctNames(topLevelSteps(state.steps).map((s) => s.name))

/**
 * The flows fallback (G-2): `flow_steps` has no `via_agent`, so a top-level call is
 * `depth == 1 && caller_agent_name == 'orchestrator'`; paused rows aren't calls. Names and order
 * only: the server overwrites every step of a turn with its last result (G-1).
 */
export const flowAgents = (steps: readonly FlowStep[]): string[] =>
  distinctNames(
    [...steps]
      .sort((a, b) => a.step_order - b.step_order)
      .filter(
        (s) =>
          s.depth === 1 &&
          s.caller_agent_name === ORCHESTRATOR &&
          s.status !== 'awaiting_human' &&
          s.agent_name !== ORCHESTRATOR,
      )
      .map((s) => s.agent_name),
  )

export type AgentSummary = 'running' | 'completed' | 'failed' | 'mixed' | 'didnt_finish'

export interface AgentActivity {
  name: string
  calls: Step[]
  summary: AgentSummary
  failed: number
  totalMs: number
}

/**
 * One row per agent, in first-call order (DP5, EN-8). While the turn is live an open call reads
 * Running; once it ended, an open call reads Didn't finish. Fail-then-ok is Mixed results: the
 * server marks no retry correlation.
 */
export function agentActivity(steps: readonly Step[], turnEnded: boolean): AgentActivity[] {
  const byName = new Map<string, Step[]>()
  for (const s of topLevelSteps(steps)) {
    const calls = byName.get(s.name)
    if (calls) calls.push(s)
    else byName.set(s.name, [s])
  }
  return [...byName].map(([name, calls]) => {
    const open = calls.some((c) => c.status === 'running')
    const failed = calls.filter((c) => c.status === 'error').length
    const ok = calls.filter((c) => c.status === 'ok').length
    const summary: AgentSummary = open
      ? turnEnded
        ? 'didnt_finish'
        : 'running'
      : failed && ok
        ? 'mixed'
        : failed
          ? 'failed'
          : 'completed'
    return {
      name,
      calls,
      summary,
      failed,
      totalMs: calls.reduce((n, c) => n + (c.durationMs ?? 0), 0),
    }
  })
}

export type StatusLine =
  { kind: 'working' } | { kind: 'asking'; agent: string } | { kind: 'writing' }

/**
 * The one live status line (§5.5): Asking while a top-level call is open (the newest one),
 * Writing from the first reply text, Working otherwise (before the first call, and between the
 * last call resolving and the first text).
 */
export function statusLine(state: Pick<TurnState, 'steps' | 'artifacts'>): StatusLine {
  const open = topLevelSteps(state.steps).filter((s) => s.status === 'running')
  if (open.length) return { kind: 'asking', agent: open[open.length - 1].name }
  if (state.artifacts.some((a) => a.text)) return { kind: 'writing' }
  return { kind: 'working' }
}

/** "a", "a and b", "a, b and c" (en conjunction). */
const AGENT_LIST = new Intl.ListFormat('en', { style: 'long', type: 'conjunction' })
export const listAgents = (names: readonly string[]) => AGENT_LIST.format(names)

export interface MetricTurn {
  agents: number
  empty: boolean
}

/** NC-2: how often OpenRuntime called no agent, one, or several, and how often a turn ended empty. */
export function routedMetric(turns: readonly MetricTurn[]) {
  return {
    total: turns.length,
    zero: turns.filter((t) => t.agents === 0).length,
    one: turns.filter((t) => t.agents === 1).length,
    twoPlus: turns.filter((t) => t.agents >= 2).length,
    empty: turns.filter((t) => t.empty).length,
  }
}
