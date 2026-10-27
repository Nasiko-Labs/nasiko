/**
 * The Map's System card (pure, tested in system.test.ts): every part of the workspace in one list, by the board's own
 * sections (Org, Fleet Runtime) and kinds, each with its tone and its one number, so a part the board leaves out (past
 * a card's shown rows, or a tool turned off long ago) is still one click away. It replaced the village's List view
 * (2026-10-17).
 *
 * Needs attention is the board's red and orange (failing, slow or costly; a team over or near its budget's pace, or
 * with nothing holding its spend); the rest is healthy, idle included.
 */
import type { EstateView } from '../estate/useEstateView'
import { boardCopy as copy } from './copy'
import { harnessLabel, providerSpend, toneOf, type Board, type Tone } from './model'

export interface SystemRow {
  /** The `sel` it opens. */
  sel: string
  name: string
  value: string
  tone: Tone
}
export interface SystemGroup {
  key: string
  label: string
  rows: SystemRow[]
}
export interface SystemSection {
  key: 'org' | 'fleet'
  label: string
  groups: SystemGroup[]
}

export type SystemFilter = 'all' | 'attention' | 'healthy'

export const needsAttention = (t: Tone) => t === 'bad' || t === 'warn'

const RANK: Record<Tone, number> = { bad: 4, warn: 3, ok: 2, idle: 1, none: 0 }
/** The worst tone among rows: a group's dot. */
export const worst = (rows: readonly SystemRow[]): Tone =>
  rows.reduce<Tone>((w, r) => (RANK[r.tone] > RANK[w] ? r.tone : w), 'none')

/** A team's tone from its budget: over pace red, near it (or no budget at all) orange. */
function teamTone(limit: number | null, pace: number | null): Tone {
  if (!limit) return 'warn'
  if ((pace ?? 0) > 1) return 'bad'
  return (pace ?? 0) > 0.9 ? 'warn' : 'ok'
}

export function systemSections(v: EstateView, board: Board): SystemSection[] {
  const e = v.estate
  const tone = (id: string) => toneOf(v.state.get(id))
  const node = (id: string) => `node:${id}`
  const bySpend = <T extends { spend: number }>(rows: T[]) =>
    [...rows].sort((a, b) => b.spend - a.spend)

  const teams: SystemRow[] = board.teams.map((t) => ({
    sel: t.id ? node(t.id) : 'panel:people',
    name: t.id ? t.name : copy.noTeam,
    value: copy.usd(t.spent),
    tone: t.id ? teamTone(t.limit, t.pace) : 'warn',
  }))

  const agents = bySpend(e.agents.map((a) => ({ ...a, spend: a.spend_usd }))).map((a) => ({
    sel: node(a.id),
    name: a.name,
    value: copy.usd(a.spend),
    tone: tone(a.id),
  }))
  const workflows = [...e.workflows]
    .sort((a, b) => b.runs - a.runs)
    .map((w) => ({ sel: node(w.id), name: w.name, value: copy.runs(w.runs), tone: tone(w.id) }))

  const kinds = new Map<string, typeof e.harnesses>()
  for (const h of e.harnesses) kinds.set(h.harness, [...(kinds.get(h.harness) ?? []), h])
  const harnesses = bySpend(
    [...kinds.values()].map((hs) => ({
      name: harnessLabel(hs[0]?.name ?? ''),
      spend: hs.reduce((n, h) => n + (h.spend_usd ?? 0), 0),
      tone: hs.map((h) => tone(h.id)).reduce((w, t) => (RANK[t] > RANK[w] ? t : w), 'none'),
    })),
  ).map((k) => ({ sel: 'panel:harnesses', name: k.name, value: copy.usd(k.spend), tone: k.tone }))

  const tools = e.tools
    .map((t) => {
      // What calls it: agents, coding harnesses and workflow steps (the board's `uses`).
      const n =
        e.agents.filter((a) => a.tool_ids.includes(t.id)).length +
        e.harnesses.filter((h) => h.tool_ids.includes(t.id)).length +
        e.workflows.reduce((k, w) => k + w.steps.filter((s) => s.ref === t.id).length, 0)
      return { sel: node(t.id), name: t.name, value: copy.uses(n), tone: tone(t.id), n }
    })
    .sort((a, b) => b.n - a.n || a.name.localeCompare(b.name))
    .map(({ n: _, ...r }) => r)

  const agentSpend = (pred: (a: (typeof e.agents)[number]) => boolean) =>
    e.agents.filter(pred).reduce((n, a) => n + a.spend_usd, 0)
  const routers = bySpend(
    e.routers
      .filter((r) => !r.direct)
      .map((r) => ({ ...r, spend: agentSpend((a) => a.router_id === r.id) })),
  ).map((r) => ({ sel: node(r.id), name: r.name, value: copy.usd(r.spend), tone: tone(r.id) }))
  const models = bySpend(e.models.map((m) => ({ ...m, spend: m.spend_usd })))
  // The bill's amounts (agents and coding harnesses), so the list and the bill never disagree.
  const paid = providerSpend(e).by
  const providers = bySpend(e.providers.map((p) => ({ ...p, spend: paid.get(p.id) ?? 0 }))).map(
    (p) => ({ sel: node(p.id), name: p.name, value: copy.usd(p.spend), tone: tone(p.id) }),
  )

  const sections: SystemSection[] = [
    {
      key: 'org',
      label: copy.system.org,
      groups: [{ key: 'teams', label: copy.system.teams, rows: teams }],
    },
    {
      key: 'fleet',
      label: copy.system.fleet,
      groups: [
        { key: 'harnesses', label: copy.system.harnesses, rows: harnesses },
        { key: 'agents', label: copy.agents, rows: agents },
        { key: 'workflows', label: copy.workflows, rows: workflows },
        { key: 'tools', label: copy.system.tools, rows: tools },
        { key: 'routers', label: copy.routers, rows: routers },
        { key: 'providers', label: copy.system.providers, rows: providers },
        {
          key: 'models',
          label: copy.system.models,
          rows: models.map((m) => ({
            sel: node(m.id),
            name: m.short || m.name,
            value: copy.usd(m.spend),
            tone: tone(m.id),
          })),
        },
      ],
    },
  ]
  return sections.map((s) => ({ ...s, groups: s.groups.filter((g) => g.rows.length) }))
}

/** The sections a search and a filter leave, empty groups and sections dropped. */
export function filterSystem(
  sections: readonly SystemSection[],
  query: string,
  filter: SystemFilter,
): SystemSection[] {
  const q = query.trim().toLowerCase()
  const keep = (r: SystemRow) =>
    (!q || r.name.toLowerCase().includes(q)) &&
    (filter === 'all' || needsAttention(r.tone) === (filter === 'attention'))
  return sections
    .map((s) => ({
      ...s,
      groups: s.groups
        .map((g) => ({ ...g, rows: g.rows.filter(keep) }))
        .filter((g) => g.rows.length),
    }))
    .filter((s) => s.groups.length)
}

/** How many parts each filter shows (before the search). */
export function systemCounts(sections: readonly SystemSection[]): Record<SystemFilter, number> {
  const rows = sections.flatMap((s) => s.groups.flatMap((g) => g.rows))
  const attention = rows.filter((r) => needsAttention(r.tone)).length
  return { all: rows.length, attention, healthy: rows.length - attention }
}
