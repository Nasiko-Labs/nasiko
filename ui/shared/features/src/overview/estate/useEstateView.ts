/**
 * One derived view of the estate for the map, the insight panel, the sentence and the KPIs: OV-1's estate (or the
 * fallback built from the fleet when the server has none), with the scope, History day and filters applied, and the
 * waiting requests placed on the nodes they block. Computed once per change, read everywhere.
 */
import { useMemo } from 'react'
import type { Waiting } from '@nasiko/features/chat/waiting'
import type { FleetHealth } from '../api'
import { layoutMap, type MapLayout } from '../map/layout'
import type { OverviewSearch } from '../search'
import { fallbackEstate } from './fallback'
import {
  dayState,
  indexEstate,
  isOpen,
  nodeState,
  visibleProblems,
  type DayState,
  type EstateNode,
  type NodeState,
  type Scope,
  type View,
} from './model'
import type { Estate, Problem } from './types'

/** A waiting request, placed on the agent or workflow it blocks. */
export interface PlacedRequest {
  id: string
  sessionId: string
  kind: string
  title: string
  at: string
  /** Map nodes it blocks (its agent, its workflow). */
  nodes: string[]
}

export interface EstateView {
  estate: Estate
  /** OV-1 is missing: this is the smaller map built from the fleet. */
  fallback: boolean
  nodes: Map<string, EstateNode>
  view: View
  /** Live today (the History slider is at its end). */
  live: boolean
  mine: Set<string>
  /** The scope's open problems, filtered, for the view's day, highest score first. */
  problems: Problem[]
  /** The scope's open problems today, unfiltered: what the sentence and KPIs count. */
  today: Problem[]
  state: Map<string, NodeState>
  day(id: string, k: number): DayState
  /** Nodes with a broken problem (they pulse while live). */
  broken: Set<string>
  /** Requests by node id (the blue outline). */
  inputs: Map<string, PlacedRequest[]>
  requests: PlacedRequest[]
  layout: MapLayout
}

export function scopeOf(search: OverviewSearch, admin: boolean): Scope {
  return admin && search.scope !== 'mine' ? 'org' : 'mine'
}

export function useEstateView({
  estate,
  absent,
  fleet,
  me,
  search,
  admin,
  waiting,
  now,
}: {
  estate: Estate | null | undefined
  absent: boolean
  fleet: FleetHealth
  me: { sub: string; username: string }
  search: OverviewSearch
  admin: boolean
  waiting: Waiting | null
  now: number
}): EstateView | null {
  const agents = useMemo(() => [...fleet.byId.values()], [fleet.byId])
  const base = useMemo(() => {
    if (estate) return { estate, fallback: false }
    if (!absent || !fleet.summary) return null
    return {
      estate: fallbackEstate({ me, agents, ratings: fleet.summary, now }),
      fallback: true,
    }
  }, [estate, absent, fleet.summary, agents, me, now])

  const scope = scopeOf(search, admin)
  const requests = useMemo(() => (base ? placeRequests(base.estate, waiting) : []), [base, waiting])

  return useMemo(() => {
    if (!base) return null
    const e = base.estate
    const nodes = indexEstate(e)
    const last = e.days.length - 1
    const at = search.at ? e.days.indexOf(search.at) : -1
    const dayIdx = at >= 0 ? at : last
    const view: View = {
      scope,
      day: dayIdx,
      filters: {
        group: search.team,
        owner: search.owner,
        kind: search.kind,
        tier: search.tier,
      },
      now,
    }
    const mine = new Set(e.mine)
    const problems = visibleProblems(e, nodes, view)
    const today = e.problems.filter((p) => isOpen(p, scope, now))
    const state = new Map<string, NodeState>()
    for (const id of nodes.keys()) state.set(id, nodeState(e, nodes, problems, view, mine, id))
    const broken = new Set<string>()
    for (const p of problems)
      if (p.tier === 'broken') p.entities.slice(0, 1).forEach((x) => broken.add(x))
    const live = dayIdx === last
    const inputs = new Map<string, PlacedRequest[]>()
    if (live)
      for (const r of requests)
        for (const n of r.nodes)
          if (scope === 'org' || mine.has(n)) inputs.set(n, [...(inputs.get(n) ?? []), r])
    const flagged = new Set(problems.flatMap((p) => p.entities))
    return {
      estate: e,
      fallback: base.fallback,
      nodes,
      view,
      live,
      mine,
      problems,
      today,
      state,
      day: (id: string, k: number) => dayState(e, nodes, view, mine, id, k),
      broken,
      inputs,
      requests,
      layout: layoutMap(e, flagged),
    }
  }, [base, scope, search.at, search.team, search.owner, search.kind, search.tier, now, requests])
}

/** Requests from the waiting list, placed on the estate's agents (by id or name). */
export function placeRequests(e: Estate, waiting: Waiting | null): PlacedRequest[] {
  if (!waiting) return []
  const byName = new Map(e.agents.map((a) => [a.name.toLowerCase(), a.id]))
  const ids = new Set(e.agents.map((a) => a.id))
  return waiting.match.chats.flatMap((c) =>
    c.requests.map((r) => {
      const agent = r.execution.agent_id
      const nodes: string[] = []
      if (agent) {
        const hit = ids.has(agent)
          ? agent
          : byName.get(
              agent
                .toLowerCase()
                .replace(/^seed-/, '')
                .replace(/-/g, ' '),
            )
        if (hit) nodes.push(hit)
      }
      return {
        id: r.id,
        sessionId: c.sessionId,
        kind: r.kind,
        title: questionText(r.question),
        at: r.created_at,
        nodes,
      }
    }),
  )
}

function questionText(q: unknown): string {
  if (!q || typeof q !== 'object') return ''
  const o = q as Record<string, unknown>
  for (const k of ['message', 'header', 'title', 'prompt', 'text'])
    if (typeof o[k] === 'string' && o[k]) return o[k]
  return ''
}
