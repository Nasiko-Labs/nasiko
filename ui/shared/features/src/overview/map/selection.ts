/**
 * What the map has selected, as the `sel` search param (pure): `node:<id>`, `problem:<id>`, `flow:<id>`,
 * `chain:<id>`, `opp:<id>`, `panel:<card>[:<kind>]`, `tile:<node>:<day>` or `sess:<harness>:<index>`. Node ids may
 * themselves hold colons (`model:gpt-4o`), so only the type prefix and a trailing number are split off.
 */
import { related, type EstateNode } from '../estate/model'
import { PANELS, PROBLEM_KINDS, type Estate, type PanelId, type ProblemKind } from '../estate/types'

export type Selection =
  | { type: 'node'; id: string }
  | { type: 'problem'; id: string }
  | { type: 'flow'; id: string }
  | { type: 'chain'; id: string }
  | { type: 'opp'; id: string }
  | { type: 'panel'; id: PanelId; kind: ProblemKind | null }
  | { type: 'tile'; id: string; day: number }
  | { type: 'sess'; id: string; index: number }

export function parseSel(raw: string | undefined): Selection | null {
  if (!raw) return null
  const i = raw.indexOf(':')
  if (i <= 0) return null
  const type = raw.slice(0, i)
  const rest = raw.slice(i + 1)
  if (!rest) return null
  switch (type) {
    case 'node':
    case 'problem':
    case 'flow':
    case 'chain':
    case 'opp':
      return { type, id: rest }
    case 'panel': {
      const [id, kind] = rest.split(':')
      if (!(PANELS as readonly string[]).includes(id ?? '')) return null
      return {
        type,
        id: id as PanelId,
        kind: (PROBLEM_KINDS as readonly string[]).includes(kind ?? '')
          ? (kind as ProblemKind)
          : null,
      }
    }
    case 'tile':
    case 'sess': {
      const j = rest.lastIndexOf(':')
      const n = Number(rest.slice(j + 1))
      if (j <= 0 || !Number.isInteger(n) || n < 0) return null
      return type === 'tile'
        ? { type, id: rest.slice(0, j), day: n }
        : { type, id: rest.slice(0, j), index: n }
    }
    default:
      return null
  }
}

export function formatSel(s: Selection): string {
  switch (s.type) {
    case 'panel':
      return `panel:${s.id}${s.kind ? `:${s.kind}` : ''}`
    case 'tile':
      return `tile:${s.id}:${s.day}`
    case 'sess':
      return `sess:${s.id}:${s.index}`
    default:
      return `${s.type}:${s.id}`
  }
}

/** The node a selection is about, when there is one. */
export function anchorOf(e: Estate, s: Selection | null): string | null {
  if (!s) return null
  switch (s.type) {
    case 'node':
    case 'tile':
    case 'sess':
      return s.id
    case 'problem':
      return e.problems.find((p) => p.id === s.id)?.entities[0] ?? null
    case 'opp':
      return e.opportunities.find((o) => o.id === s.id)?.entities[0] ?? null
    default:
      return null
  }
}

/** The nodes a selection keeps at full strength (the rest dim); null keeps everything. */
export function focusOf(
  e: Estate,
  nodes: ReadonlyMap<string, EstateNode>,
  s: Selection | null,
): Set<string> | null {
  if (!s) return null
  switch (s.type) {
    case 'chain': {
      const c = e.chains.find((x) => x.id === s.id)
      return c ? new Set(c.steps.map((st) => st.entity)) : null
    }
    case 'flow': {
      const f = e.flows.find((x) => x.id === s.id)
      if (!f) return null
      return new Set(
        [...nodes.values()].filter((n) => n.panel === f.from || n.panel === f.to).map((n) => n.id),
      )
    }
    case 'panel':
      return new Set([...nodes.values()].filter((n) => n.panel === s.id).map((n) => n.id))
    case 'tile':
    case 'sess':
      return new Set([s.id])
    case 'problem': {
      const p = e.problems.find((x) => x.id === s.id)
      if (!p) return null
      const keep = p.entities[0] ? related(e, nodes, p.entities[0]) : new Set<string>()
      p.entities.forEach((x) => keep.add(x))
      return keep
    }
    case 'opp': {
      const o = e.opportunities.find((x) => x.id === s.id)
      if (!o) return null
      const keep = new Set<string>(o.entities)
      for (const x of o.entities) related(e, nodes, x).forEach((y) => keep.add(y))
      return keep
    }
    case 'node':
      return nodes.has(s.id) ? related(e, nodes, s.id) : null
  }
}

/** The flows a selection keeps at full strength; null keeps them all. */
export function focusFlows(
  e: Estate,
  s: Selection | null,
  keep: Set<string> | null,
): Set<string> | null {
  if (!s || !keep) return null
  if (s.type === 'flow') return new Set([s.id])
  if (s.type === 'problem') {
    const f = e.problems.find((p) => p.id === s.id)?.flow_id
    return new Set(f ? [f] : [])
  }
  if (s.type === 'chain')
    return new Set(
      e.problems.filter((p) => p.chain_id === s.id && p.flow_id).map((p) => p.flow_id ?? ''),
    )
  if (s.type === 'node')
    return new Set(
      e.problems.filter((p) => p.entities.includes(s.id) && p.flow_id).map((p) => p.flow_id ?? ''),
    )
  return new Set()
}
