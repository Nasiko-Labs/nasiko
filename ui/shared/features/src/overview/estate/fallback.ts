/**
 * The map on a server without OV-1 (pure): built from reads the Overview already makes. The agent directory gives the
 * agents and coding harnesses, the fleet ratings (`health.ts`) give their problems, and the viewer is the one person.
 * There are no teams, workflows, routers, models or tools to draw, no cause chains, opportunities or flows' volumes,
 * and nothing can be assigned: each of those cards says what it waits for instead of guessing.
 */
import { displayStatus, isHarness } from '@nasiko/features/agents/status'
import type { Agent } from '@nasiko/features/agents/types'
import type { FleetHealthSummary } from '../health'
import { estateCopy as copy } from './copy'
import type { Estate, Problem, ProblemKind } from './types'

export interface FallbackInput {
  me: { sub: string; username: string }
  agents: readonly Agent[]
  ratings: FleetHealthSummary | null
  /** Epoch ms. */
  now: number
}

const DAY = 86_400_000

/** The harness behind a coding-harness row, from its name (`maya-claude-code`, `admin-codex`). */
export function harnessKind(name: string): string {
  const n = name.toLowerCase()
  for (const k of ['claude', 'codex', 'cursor', 'opencode', 'windsurf']) if (n.includes(k)) return k
  return 'other'
}

const KIND: Record<string, ProblemKind> = {
  reliability: 'errors',
  activity: 'errors',
  latency: 'slow',
  cost: 'cost',
}

export function fallbackEstate({ me, agents, ratings, now }: FallbackInput): Estate {
  const days = Array.from({ length: 14 }, (_, i) =>
    new Date(now - (13 - i) * DAY).toISOString().slice(0, 10),
  )
  const today = days[13] ?? new Date(now).toISOString().slice(0, 10)
  const fleet = agents.filter((a) => !isHarness(a))
  const coding = agents.filter((a) => isHarness(a))
  const problems: Problem[] = []
  for (const a of fleet) {
    const r = ratings?.byId.get(a.id)
    if (!r || (r.rating !== 'action' && r.rating !== 'watch')) continue
    const reason = r.reasons[0]
    if (!reason) continue
    const broken = r.rating === 'action'
    problems.push({
      id: `fleet:${a.id}`,
      kind: KIND[reason.dimension] ?? 'errors',
      also: null,
      tier: broken ? 'broken' : 'worse',
      title: `${r.name}: ${reason.text}`,
      why: broken ? copy.fallback.whyAction : copy.fallback.whyWatch,
      metric: { label: '', current: 0, target: 0, direction: 'lower', unit: '', min: 0, max: 0 },
      trend: [],
      since: today,
      confidence: 'high',
      evidence: r.reasons.map((x) => x.text),
      fixed_when: copy.fallback.fixedWhen,
      action: { label: copy.fallback.openAgent, target: { page: 'agent', id: a.id } },
      entities: [a.id],
      flow_id: 'people-agents',
      count: 1,
      chain_id: null,
      owner_id: null,
      snoozed_until: null,
      handled_at: null,
      score: broken ? 120 : 60,
      mine: a.owner_id === me.sub,
    })
  }
  const owners = [...new Set(agents.map((a) => a.owner_id))]
  return {
    as_of: new Date(now).toISOString(),
    days,
    me: me.sub,
    mine: [me.sub, ...agents.filter((a) => a.owner_id === me.sub).map((a) => a.id)],
    groups: [],
    // Only the viewer has a name here; other owners are counted, never guessed.
    people: [
      {
        id: me.sub,
        name: me.username,
        group_id: null,
        spend_usd: 0,
        sessions: 0,
        success_pct: null,
        avg_context_tokens: null,
        segment: 'value',
      },
      ...owners
        .filter((o) => o !== me.sub)
        .map((o, i) => ({
          id: o,
          name: copy.fallback.person(i + 1),
          group_id: null,
          spend_usd: 0,
          sessions: 0,
          success_pct: null,
          avg_context_tokens: null,
          segment: 'value' as const,
        })),
    ],
    workflows: [],
    routers: [],
    providers: [],
    models: [],
    agents: fleet.map((a) => {
      const d = displayStatus(a.status, false)
      return {
        id: a.id,
        name: a.display_name || a.name,
        owner_id: a.owner_id,
        router_id: null,
        model_ids: [],
        tool_ids: [],
        spend_usd: 0,
        runs: 0,
        success_pct: null,
        p95_ms: null,
        idle: d === 'stopped' || d === 'not-deployed',
      }
    }),
    tools: [],
    harnesses: coding.map((a) => ({
      id: a.id,
      name: a.display_name || a.name,
      harness: harnessKind(a.name),
      owner_id: a.owner_id,
      model_ids: [],
      tool_ids: [],
      spend_usd: null,
      sessions: [],
    })),
    flows: [
      {
        id: 'people-agents',
        from: 'people',
        to: 'agents',
        volume: 1,
        unit: '',
        spend_usd: null,
        direct: false,
      },
      {
        id: 'people-harnesses',
        from: 'people',
        to: 'harnesses',
        volume: 1,
        unit: '',
        spend_usd: null,
        direct: false,
      },
    ],
    problems,
    chains: [],
    opportunities: [],
    allowance: null,
  }
}
