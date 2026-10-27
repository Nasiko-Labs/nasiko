/** The Map board's tests: the OV-1 mock's estate, and the view the page derives from it (Org scope, today, no filters). */
import {
  indexEstate,
  isOpen,
  nodeState,
  visibleProblems,
  type NodeState,
  type View,
} from '@nasiko/features/overview/estate/model'
import { estateSchema, type Estate } from '@nasiko/features/overview/estate/types'
import type { EstateView } from '@nasiko/features/overview/estate/useEstateView'
import { layoutMap } from '@nasiko/features/overview/map/layout'

export async function estate(): Promise<Estate> {
  const r = await fetch(new URL('/api/overview/estate?range=30d', globalThis.location.origin))
  return estateSchema.parse(((await r.json()) as { data: unknown }).data)
}

export function viewOf(e: Estate): EstateView {
  const nodes = indexEstate(e)
  const now = Date.parse(e.as_of)
  const view: View = { scope: 'org', day: e.days.length - 1, filters: {}, now }
  const problems = visibleProblems(e, nodes, view)
  const state = new Map<string, NodeState>()
  for (const id of nodes.keys())
    state.set(id, nodeState(e, nodes, problems, view, new Set(e.mine), id))
  return {
    estate: e,
    fallback: false,
    nodes,
    view,
    live: true,
    mine: new Set(e.mine),
    problems,
    today: e.problems.filter((p) => isOpen(p, 'org', now)),
    state,
    day: () => 'ok',
    broken: new Set(),
    inputs: new Map(),
    requests: [],
    layout: layoutMap(e, new Set()),
  }
}
