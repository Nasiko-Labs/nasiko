/**
 * Swimlane lanes (plans/feat-flows.md F3, F21, F27; eng review O6).
 *
 * One lane per agent, ordered by first call. An agent's lane sits under the lane of whoever made its FIRST call; a
 * later call from a different parent stays on the agent's lane and names its caller (`fromCaller`). The tree is built
 * once from first calls and a parent must have called first, so a call back to an ancestor (A → B → A) can't cycle.
 * Calls that overlap on one lane take sub-rows (at most SUBROW_CAP, the rest counted as `overflow`). Past LANE_CAP
 * lanes the rest wait behind "Show N more agents"; past TABLE_FIRST_CALLS calls the panel opens on its Table.
 */
import type { Call } from './calls'
import { AGENT_COLOURS, LANE_CAP, SUBROW_CAP, TABLE_FIRST_CALLS } from './tuning'

/** @public The swimlane draws these (plan T2). */
export interface Lane {
  id: string
  agentId: string | null
  agentName: string | null
  /** 0 for a lane with no caller lane, its parent's indent + 1 below. */
  indent: number
  parentId: string | null
  /** Sub-rows of non-overlapping calls, at most SUBROW_CAP. */
  rows: Call[][]
  /** Calls that didn't fit in the sub-rows ("+N"). */
  overflow: Call[]
  /** DESIGN.md Two-tone series index by first call, or null for Other (the sixth agent on, F21). */
  colour: number | null
}

export interface LaneModel {
  /** The lanes shown, in display order (each lane followed by its callees). */
  lanes: Lane[]
  /** Lanes behind "Show N more agents". */
  hidden: Lane[]
  /** Call key → the caller's agent name, for calls whose caller isn't the lane's parent (O6). */
  fromCaller: Map<string, string>
  /** Open on the Table view (F27). */
  tableFirst: boolean
}

const laneIdOf = (c: Pick<Call, 'agentId' | 'agentName' | 'key'>) =>
  c.agentId ? `agent:${c.agentId}` : c.agentName ? `name:${c.agentName}` : `call:${c.key}`

const end = (c: Call) => c.endMs ?? Number.POSITIVE_INFINITY

function pack(calls: readonly Call[]): { rows: Call[][]; overflow: Call[] } {
  const rows: Call[][] = []
  const overflow: Call[] = []
  for (const c of calls) {
    const row = rows.find((r) => {
      const last = r[r.length - 1]
      return !last || end(last) <= c.startMs
    })
    if (row) row.push(c)
    else if (rows.length < SUBROW_CAP) rows.push([c])
    else overflow.push(c)
  }
  return { rows, overflow }
}

export function buildLanes(calls: readonly Call[], laneCap = LANE_CAP): LaneModel {
  const sorted = [...calls].sort((a, b) => a.startMs - b.startMs || a.key.localeCompare(b.key))
  const byKey = new Map(sorted.map((c) => [c.key, c]))
  const groups = new Map<string, Call[]>()
  for (const c of sorted) {
    const id = laneIdOf(c)
    const g = groups.get(id)
    if (g) g.push(c)
    else groups.set(id, [c])
  }

  // Lane order: first call, then call key; insertion order of `groups` already follows `sorted`.
  const order = [...groups.keys()]
  const rank = new Map(order.map((id, i) => [id, i]))
  const parentOf = new Map<string, string | null>()
  order.forEach((id, i) => {
    const first = groups.get(id)?.[0]
    const caller = first?.parentKey ? byKey.get(first.parentKey) : undefined
    const parent = caller ? laneIdOf(caller) : null
    // A parent must have called first, so the tree can't cycle.
    parentOf.set(id, parent !== null && (rank.get(parent) ?? Infinity) < i ? parent : null)
  })

  const children = new Map<string | null, string[]>()
  for (const id of order) {
    const p = parentOf.get(id) ?? null
    children.set(p, [...(children.get(p) ?? []), id])
  }
  const display: { id: string; indent: number }[] = []
  const walk = (parent: string | null, indent: number) => {
    for (const id of children.get(parent) ?? []) {
      display.push({ id, indent })
      walk(id, indent + 1)
    }
  }
  walk(null, 0)

  const fromCaller = new Map<string, string>()
  for (const [id, cs] of groups) {
    for (const c of cs) {
      const caller = c.parentKey ? byKey.get(c.parentKey) : undefined
      if (caller && laneIdOf(caller) !== (parentOf.get(id) ?? null) && caller.agentName)
        fromCaller.set(c.key, caller.agentName)
    }
  }

  const all: Lane[] = display.flatMap(({ id, indent }) => {
    const cs = groups.get(id) ?? []
    const first = cs[0]
    const i = rank.get(id) ?? Infinity
    if (!first) return []
    return {
      id,
      agentId: first.agentId,
      agentName: first.agentName,
      indent,
      parentId: parentOf.get(id) ?? null,
      ...pack(cs),
      colour: i < AGENT_COLOURS ? i : null,
    }
  })
  return {
    lanes: all.slice(0, laneCap),
    hidden: all.slice(laneCap),
    fromCaller,
    tableFirst: calls.length > TABLE_FIRST_CALLS,
  }
}
