/**
 * The estate's pure rules (docs/requirements/overview.md OV-1): which problems show, the one colour each node takes,
 * what a selection keeps in focus, and each day tile's state. Everything the map, the insight panel and the sentence
 * agree on comes from here, so they can never disagree.
 */
import type { Estate, NodeKind, PanelId, Problem, ProblemKind, SessionOutcome, Tier } from './types'

/** What a node's square shows: the worst problem's kind, healthy, idle, a ghost (not the viewer's) or muted (filtered out). */
export type NodeState = ProblemKind | 'ok' | 'idle' | 'ghost' | 'muted'
/** A day tile also has days the History slider hasn't reached. */
export type DayState = NodeState | 'future'

export interface EstateNode {
  id: string
  kind: NodeKind
  name: string
  panel: PanelId | null
  /** The person who owns it (agents, workflows, harnesses). */
  owner: string | null
  /** A person's group. */
  group: string | null
}

export type Scope = 'org' | 'mine'

export interface Filters {
  group?: string
  /** A person id, or `none` for problems nobody is on. */
  owner?: string
  kind?: ProblemKind
  tier?: Tier
}

export interface View {
  scope: Scope
  /** Index into `estate.days`; the last is today (live). */
  day: number
  filters: Filters
  /** Epoch ms: snoozes end. */
  now: number
}

export function indexEstate(e: Estate): Map<string, EstateNode> {
  const m = new Map<string, EstateNode>()
  const add = (n: Omit<EstateNode, 'owner' | 'group'> & Partial<EstateNode>) =>
    m.set(n.id, { owner: null, group: null, ...n })
  for (const g of e.groups) add({ id: g.id, kind: 'group', name: g.name, panel: 'people' })
  for (const p of e.people)
    add({ id: p.id, kind: 'person', name: p.name, panel: 'people', group: p.group_id })
  for (const w of e.workflows)
    add({ id: w.id, kind: 'workflow', name: w.name, panel: 'workflows', owner: w.owner_id })
  for (const r of e.routers) add({ id: r.id, kind: 'router', name: r.name, panel: 'models' })
  for (const p of e.providers) add({ id: p.id, kind: 'provider', name: p.name, panel: 'models' })
  for (const md of e.models) add({ id: md.id, kind: 'model', name: md.name, panel: 'models' })
  for (const a of e.agents)
    add({ id: a.id, kind: 'agent', name: a.name, panel: 'agents', owner: a.owner_id })
  for (const t of e.tools) add({ id: t.id, kind: 'tool', name: t.name, panel: 'tools' })
  for (const h of e.harnesses)
    add({ id: h.id, kind: 'harness', name: h.name, panel: 'harnesses', owner: h.owner_id })
  return m
}

/** The group a node belongs to, through its owner for things people own. */
export function groupOf(nodes: ReadonlyMap<string, EstateNode>, id: string): string | null {
  const n = nodes.get(id)
  if (!n) return null
  if (n.kind === 'group') return n.id
  if (n.kind === 'person') return n.group
  return n.owner ? (nodes.get(n.owner)?.group ?? null) : null
}

const dayIndexOf = (e: Estate, date: string) => {
  const i = e.days.indexOf(date)
  if (i >= 0) return i
  // Before the window: active from its first day.
  return date < (e.days[0] ?? '') ? 0 : e.days.length
}

/** A problem's day index in the window: it colours tiles from that day on. */
export const sinceIndex = (e: Estate, p: Problem) => dayIndexOf(e, p.since)

/** Snoozed or handled problems leave every list and every colour (OV-2), as do others' problems in Mine. */
export function isOpen(p: Problem, scope: Scope, now: number): boolean {
  if (p.handled_at) return false
  if (p.snoozed_until && Date.parse(p.snoozed_until) > now) return false
  return scope === 'org' || p.mine
}

/** The problems a view shows, highest score first. */
export function visibleProblems(
  e: Estate,
  nodes: ReadonlyMap<string, EstateNode>,
  v: View,
): Problem[] {
  const f = v.filters
  return e.problems
    .filter((p) => {
      if (!isOpen(p, v.scope, v.now)) return false
      if (sinceIndex(e, p) > v.day) return false
      if (f.kind && p.kind !== f.kind && p.also !== f.kind) return false
      if (f.tier && p.tier !== f.tier) return false
      if (f.owner && (f.owner === 'none' ? !!p.owner_id : p.owner_id !== f.owner)) return false
      if (f.group && !p.entities.some((id) => groupOf(nodes, id) === f.group)) return false
      return true
    })
    .sort(byScore)
}

export const byScore = (a: Problem, b: Problem) => b.score - a.score || a.id.localeCompare(b.id)

/** A group's problems include its members' and theirs (what its people own). */
export function involves(nodes: ReadonlyMap<string, EstateNode>, p: Problem, id: string): boolean {
  if (p.entities.includes(id)) return true
  const n = nodes.get(id)
  return n?.kind === 'group' && p.entities.some((x) => groupOf(nodes, x) === id)
}

export function problemsOf(
  nodes: ReadonlyMap<string, EstateNode>,
  problems: readonly Problem[],
  id: string,
): Problem[] {
  return problems.filter((p) => involves(nodes, p, id))
}

/** A node outside the filters' team or owner is muted; in Mine, what the viewer doesn't touch is a ghost. */
export function nodeState(
  e: Estate,
  nodes: ReadonlyMap<string, EstateNode>,
  problems: readonly Problem[],
  v: View,
  mine: ReadonlySet<string>,
  id: string,
): NodeState {
  if (v.scope === 'mine' && !mine.has(id)) return 'ghost'
  if (idle(e, id)) return 'idle'
  const worst = problemsOf(nodes, problems, id)[0]
  if (worst) return worst.kind
  return passesFilters(nodes, v.filters, id) ? 'ok' : 'muted'
}

function idle(e: Estate, id: string) {
  return e.agents.some((a) => a.id === id && a.idle)
}

export function passesFilters(
  nodes: ReadonlyMap<string, EstateNode>,
  f: Filters,
  id: string,
): boolean {
  if (f.group) {
    const g = groupOf(nodes, id)
    if (g && g !== f.group) return false
  }
  if (f.owner && f.owner !== 'none') {
    const n = nodes.get(id)
    if (n?.owner && n.owner !== f.owner && n.id !== f.owner) return false
    if (n?.kind === 'person' && n.id !== f.owner) return false
  }
  return true
}

/** A day tile: the worst problem involving the node that had started by that day. */
export function dayState(
  e: Estate,
  nodes: ReadonlyMap<string, EstateNode>,
  v: View,
  mine: ReadonlySet<string>,
  id: string,
  day: number,
): DayState {
  if (v.scope === 'mine' && !mine.has(id)) return 'ghost'
  if (day > v.day) return 'future'
  if (idle(e, id)) return 'idle'
  const worst = e.problems
    .filter((p) => isOpen(p, v.scope, v.now) && sinceIndex(e, p) <= day && involves(nodes, p, id))
    .sort(byScore)[0]
  return worst ? worst.kind : 'ok'
}

/** A coding-harness session tile: its own outcome, quiet unless a problem on the harness explains it. */
export function sessionState(
  outcome: SessionOutcome,
  index: number,
  total: number,
  v: View,
  lastDay: number,
  explained: boolean,
): DayState {
  // Sessions spread evenly over the window: the slider hides the ones after its day.
  if (lastDay > 0 && index >= Math.round((total * (v.day + 1)) / (lastDay + 1))) return 'future'
  if (outcome === 'ok') return 'ok'
  if (outcome === 'failed') return 'errors'
  return explained ? outcome : 'ok'
}

/** What a selection keeps in focus: the node, what it connects to, and everything its problems involve. */
export function related(
  e: Estate,
  nodes: ReadonlyMap<string, EstateNode>,
  id: string,
): Set<string> {
  const s = new Set<string>([id])
  const add = (x: string | null | undefined) => {
    if (x && nodes.has(x)) s.add(x)
  }
  const n = nodes.get(id)
  if (!n) return s
  const providerOf = (m: string) => e.models.find((x) => x.id === m)?.provider_id
  switch (n.kind) {
    case 'agent': {
      const a = e.agents.find((x) => x.id === id)
      add(a?.owner_id)
      add(a?.router_id)
      a?.model_ids.forEach((m) => (add(m), add(providerOf(m))))
      a?.tool_ids.forEach(add)
      e.workflows.filter((w) => w.steps.some((st) => st.ref === id)).forEach((w) => add(w.id))
      break
    }
    case 'harness': {
      const h = e.harnesses.find((x) => x.id === id)
      add(h?.owner_id)
      e.routers.filter((r) => r.direct).forEach((r) => add(r.id))
      h?.model_ids.forEach((m) => (add(m), add(providerOf(m))))
      h?.tool_ids.forEach(add)
      break
    }
    case 'workflow': {
      const w = e.workflows.find((x) => x.id === id)
      add(w?.owner_id)
      w?.steps.forEach((st) => add(st.ref))
      break
    }
    case 'person': {
      add(n.group)
      for (const x of [...e.agents, ...e.harnesses])
        if (x.owner_id === id) {
          add(x.id)
          x.tool_ids.forEach(add)
        }
      for (const w of e.workflows) if (w.owner_id === id) add(w.id)
      break
    }
    case 'group': {
      for (const p of e.people)
        if (p.group_id === id) {
          add(p.id)
          for (const x of [...e.agents, ...e.harnesses]) if (x.owner_id === p.id) add(x.id)
        }
      break
    }
    case 'provider':
      e.models.filter((m) => m.provider_id === id).forEach((m) => add(m.id))
      break
    case 'router':
    case 'model':
    case 'tool': {
      for (const a of e.agents)
        if (a.router_id === id || a.model_ids.includes(id) || a.tool_ids.includes(id)) {
          add(a.id)
          add(a.owner_id)
        }
      for (const h of e.harnesses)
        if (
          h.model_ids.includes(id) ||
          h.tool_ids.includes(id) ||
          (n.kind === 'router' && isDirect(e, id))
        ) {
          add(h.id)
          add(h.owner_id)
        }
      if (n.kind === 'model') add(providerOf(id))
      e.workflows.filter((w) => w.steps.some((st) => st.ref === id)).forEach((w) => add(w.id))
      break
    }
  }
  for (const p of e.problems) if (involves(nodes, p, id)) p.entities.forEach(add)
  return s
}

const isDirect = (e: Estate, id: string) => e.routers.some((r) => r.id === id && r.direct)

/** Per kind, what a flow's problems touched: the badges on its line. */
export function flowBadges(
  problems: readonly Problem[],
  flowId: string,
): Partial<Record<ProblemKind, number>> {
  const by: Partial<Record<ProblemKind, number>> = {}
  for (const p of problems) if (p.flow_id === flowId) by[p.kind] = (by[p.kind] ?? 0) + p.count
  return by
}

/** A card's corner: how many of its nodes show each kind, how many wait on input, and its opportunities. */
export interface PanelCounts {
  errors: number
  slow: number
  cost: number
  input: number
  opportunities: number
}

export const KIND_ORDER: readonly ProblemKind[] = ['errors', 'slow', 'cost']

export const isProblemState = (s: NodeState | DayState): s is ProblemKind =>
  s === 'errors' || s === 'slow' || s === 'cost'

/** The ratio of a team's projected month to its budget, or null without one. */
export function pace(b: { limit_usd: number; projected_usd: number } | null): number | null {
  return b && b.limit_usd > 0 ? b.projected_usd / b.limit_usd : null
}
