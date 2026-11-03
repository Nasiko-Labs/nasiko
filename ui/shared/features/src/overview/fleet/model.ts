/**
 * The Map view's board (the "Fleet Runtime / TokenOps" design, 2026-10-16, laid out by Ans's wireframe of 2026-10-17):
 * what each card shows, derived once from the estate (pure, tested in model.test.ts). Fleet Runtime: agents (today's
 * state and the last 18 days), workflows (one segment per step), connectors and tools (most used first, a recently
 * turned-off one faded), local harnesses (each install, by spend), the LLM routers and the providers' models on one
 * scale. TokenOps: the bill (`billOf`: by provider and by source, each adding up to the same total), total spend's daily
 * line. Org: every team against its budget, its people as tiles.
 *
 * Tones carry meaning and nothing else: `bad` (errors) is red, `warn` (slow or costly) orange, `ok` green, `idle`
 * grey; harness installs are green by spend, red or orange only when a problem explains them.
 */
import { pace, problemsOf, type DayState, type NodeState } from '../estate/model'
import type { Spend } from '../api'
import type { Estate, EstateAgent, Person, Problem } from '../estate/types'
import type { EstateView } from '../estate/useEstateView'

export type Tone = 'ok' | 'bad' | 'warn' | 'idle' | 'none'
export type HarnessTone = 'high' | 'low' | 'empty' | 'bad' | 'warn'

/** How many day tiles an agent shows (two rows of nine), how many installs a harness column (four rows of eleven). */
export const AGENT_DAYS = 18
export const HARNESS_CELLS = 44
export const SHOWN = {
  agents: 9,
  /** As many as fit beside the agents, in the Teams card's row shape. */
  workflows: 4,
  tools: 24,
  harnesses: 3,
  routers: 4,
  providers: 4,
  /** Models in a provider's column; the rest add up to one "other models" row. */
  models: 3,
  /** Providers on the bill; the rest are one "other providers" line. */
  billProviders: 6,
}
/** A tool turned off within this many days is drawn faded; one off for longer leaves the board. */
export const TOOL_OFF_RECENT_DAYS = 14
/** A person's tile in their team's row, by how they use it: the agents' colours, no names. */
const SEGMENT_TONE: Record<Person['segment'], Tone> = {
  value: 'ok',
  expensive: 'warn',
  stuck: 'bad',
  light: 'idle',
}
export function toneOf(s: NodeState | DayState | undefined): Tone {
  switch (s) {
    case 'errors':
      return 'bad'
    case 'slow':
    case 'cost':
      return 'warn'
    case 'ok':
      return 'ok'
    case 'future':
    case undefined:
      return 'none'
    default:
      return 'idle'
  }
}

export interface AgentRow {
  id: string
  name: string
  spend: number
  /** A cost problem is on it: its spend reads red. */
  costly: boolean
  now: Tone
  /** The last AGENT_DAYS days, oldest first (`none` before the window). */
  days: Tone[]
}
export interface StepCell {
  key: string
  label: string
  ref: string | null
  tone: Tone
}
export interface WorkflowRow {
  id: string
  name: string
  runs: number
  failing: boolean
  steps: StepCell[]
}
export interface ToolRow {
  id: string
  name: string
  brand: string | null
  agents: number
  harnesses: number
  /** Workflow steps that call it. */
  steps: number
  tone: Tone
  connector: boolean
  /** Turned off within TOOL_OFF_RECENT_DAYS: when (ISO), so the tile can say so. */
  offSince: string | null
}
export interface HarnessCell {
  id: string
  name: string
  spend: number | null
  tone: HarnessTone
}
export interface HarnessCol {
  key: string
  label: string
  spend: number
  installs: number
  failing: boolean
  cells: HarnessCell[]
  /** The column's free seats, drawn grey: room for the installs still to come. */
  slots: number
}
/** A team in the Org column: its budget, where it stands against it, and its people as tiles. */
export interface TeamRow {
  /** Null for the people in no team. */
  id: string | null
  name: string
  /** What the budget has counted so far (the server's), else what its agents and harnesses spent. */
  spent: number
  limit: number | null
  projected: number | null
  /** Projected over the limit. */
  pace: number | null
  hard: boolean
  /** What flows to it on the map: its share of its agents' spend and its people's coding-harness spend. */
  usd: number
  people: Tone[]
}
export interface Share {
  id: string
  name: string
  brand: string | null
  spend: number
  /** 0..1 of the whole it is part of. */
  share: number
}
export interface RouterCard {
  id: string
  name: string
  spend: number
  /** Of all agent spend. */
  share: number
  tone: Tone
  providers: Share[]
}
export interface ModelBar extends Share {
  /** 0..1 of the biggest shown model's spend, on every provider's column: one scale for the whole card. */
  bar: number
}
export interface ProviderRow {
  id: string
  name: string
  brand: string | null
  spend: number
  models: ModelBar[]
  /** Spend on the models past the shown ones. */
  rest: number
}
/** A line on the bill. */
export interface BillLine {
  id: string
  name: string
  brand: string | null
  spend: number
}
/**
 * The range's bill, added up from both sides: by provider (each agent's and coding harness's spend split over its
 * models, the token lines' rule) and by source (agents, coding harnesses). Both come to `total`; spend on no known
 * model is its own line, so neither side loses a cent.
 */
export interface Bill {
  providers: BillLine[]
  /** The providers past SHOWN.billProviders, as one line. */
  otherProviders: number
  agents: number
  harnesses: number
  /** Spend that names no model a provider is known for. */
  unattributed: number
  total: number
}
export interface SpendCard {
  total: number
  /** Against the window before; null without one. */
  changePct: number | null
  series: number[]
  /** The daily budget pace line, else the range's mean; null without data. */
  threshold: number | null
  /** Days over the line that peak (at most four). */
  spikes: number[]
}
export interface Board {
  agents: AgentRow[]
  agentCount: number
  workflows: WorkflowRow[]
  workflowCount: number
  /** Most used first; a tool turned off long ago is left out. */
  tools: ToolRow[]
  toolCount: number
  harnesses: HarnessCol[]
  harnessCount: number
  routers: RouterCard[]
  routerCount: number
  providers: ProviderRow[]
  bill: Bill
  /** Every team, worst pace first, then the people in no team. */
  teams: TeamRow[]
  people: number
}

/**
 * The teams an agent works for: the groups whose people ran it (proposed `team_ids`, V-1), else its owner's group,
 * the village's rule. Its spend is split evenly over them.
 */
export function teamsOf(e: Estate, a: EstateAgent): string[] {
  const owner = a.owner_id ? e.people.find((p) => p.id === a.owner_id)?.group_id : null
  const known = new Set(e.groups.map((g) => g.id))
  return [...new Set((a.team_ids ?? [owner]).filter((x): x is string => !!x && known.has(x)))]
}

/** Everything that calls a tool: agents, coding harnesses and workflow steps. */
export const uses = (t: ToolRow) => t.agents + t.harnesses + t.steps

const byDesc =
  <T>(f: (x: T) => number) =>
  (a: T, b: T) =>
    f(b) - f(a)

/** The harness kind's label from an install's name ("Claude Code (Rae)" → "Claude Code"). */
export const harnessLabel = (name: string) => name.replace(/\s*\([^)]*\)\s*$/, '') || name

/** Each model's share of an agent's spend: by the models' own spend, evenly when none has any. */
export function modelWeights(e: Estate, ids: readonly string[]): Map<string, number> {
  const spend = ids.map((id) => e.models.find((m) => m.id === id)?.spend_usd ?? 0)
  const sum = spend.reduce((n, x) => n + x, 0)
  return new Map(ids.map((id, i) => [id, sum > 0 ? (spend[i] ?? 0) / sum : 1 / ids.length]))
}

function shares<T extends { spend: number }>(rows: T[]): (T & { share: number })[] {
  const sum = rows.reduce((n, r) => n + r.spend, 0)
  return rows.map((r) => ({ ...r, share: sum > 0 ? r.spend / sum : 1 / rows.length }))
}

export function buildBoard(v: EstateView): Board {
  const e = v.estate
  const st = (id: string) => v.state.get(id)
  const last = e.days.length - 1
  const kinds = (id: string): Problem[] => problemsOf(v.nodes, v.problems, id)

  const agents = [...e.agents].sort(byDesc((a) => a.spend_usd)).map((a): AgentRow => {
    const days: Tone[] = []
    for (let k = v.view.day - AGENT_DAYS + 1; k <= v.view.day; k++)
      days.push(k < 0 || k > last ? 'none' : toneOf(v.day(a.id, k)))
    return {
      id: a.id,
      name: a.name,
      spend: a.spend_usd,
      costly: kinds(a.id).some((p) => p.kind === 'cost' || p.also === 'cost'),
      now: toneOf(st(a.id)),
      days,
    }
  })

  const workflows = [...e.workflows].sort(byDesc((w) => w.runs)).map((w): WorkflowRow => {
    const steps = w.steps.map((s, i) => ({
      key: `${i}`,
      label: s.label,
      ref: s.ref,
      tone: s.ref && v.nodes.has(s.ref) ? toneOf(st(s.ref)) : ('ok' as Tone),
    }))
    return {
      id: w.id,
      name: w.name,
      runs: w.runs,
      failing: toneOf(st(w.id)) === 'bad' || steps.some((s) => s.tone === 'bad'),
      steps,
    }
  })

  const asOf = Date.parse(e.as_of)
  const offFor = (t: Estate['tools'][number]) =>
    t.disabled_at ? (asOf - Date.parse(t.disabled_at)) / 86_400_000 : null
  const toolRow = (t: Estate['tools'][number]): ToolRow => ({
    id: t.id,
    name: t.name,
    brand: t.brand,
    agents: e.agents.filter((a) => a.tool_ids.includes(t.id)).length,
    harnesses: e.harnesses.filter((h) => h.tool_ids.includes(t.id)).length,
    steps: e.workflows.reduce((n, w) => n + w.steps.filter((s) => s.ref === t.id).length, 0),
    tone: toneOf(st(t.id)),
    connector: !!t.connector_id,
    offSince: t.disabled_at ?? null,
  })
  // Most used first, the ones turned off last; one off for longer than TOOL_OFF_RECENT_DAYS is gone from the board.
  const tools = e.tools
    .filter((t) => (offFor(t) ?? 0) <= TOOL_OFF_RECENT_DAYS)
    .map(toolRow)
    .sort(
      (a, b) =>
        Number(!!a.offSince) - Number(!!b.offSince) ||
        uses(b) - uses(a) ||
        a.name.localeCompare(b.name),
    )

  const byKind = new Map<string, Estate['harnesses']>()
  for (const h of e.harnesses) byKind.set(h.harness, [...(byKind.get(h.harness) ?? []), h])
  const harnesses = [...byKind.entries()]
    .map(([key, hs]): HarnessCol => {
      const sorted = [...hs].sort(byDesc((h) => h.spend_usd ?? -1))
      const priced = sorted.map((h) => h.spend_usd ?? 0).filter((x) => x > 0)
      const median = priced.length ? (priced[Math.floor(priced.length / 2)] ?? 0) : 0
      const cells = sorted.slice(0, HARNESS_CELLS).map((h): HarnessCell => {
        const t = toneOf(st(h.id))
        const s = h.spend_usd
        return {
          id: h.id,
          name: h.name,
          spend: s,
          tone: t === 'bad' || t === 'warn' ? t : !s ? 'empty' : s >= median ? 'high' : 'low',
        }
      })
      return {
        key,
        label: harnessLabel(sorted[0]?.name ?? key),
        spend: hs.reduce((n, h) => n + (h.spend_usd ?? 0), 0),
        installs: hs.length,
        failing: cells.some((c) => c.tone === 'bad'),
        cells,
        slots: HARNESS_CELLS - cells.length,
      }
    })
    .sort(byDesc((c) => c.spend))

  // Routed spend: each agent's spend split over its models, so routers and providers reconcile with the agents.
  const agentTotal = e.agents.reduce((n, a) => n + a.spend_usd, 0)
  const providerOf = (m: string) => e.models.find((x) => x.id === m)?.provider_id ?? null
  const routers = e.routers
    .filter((r) => !r.direct)
    .map((r): RouterCard => {
      const by = new Map<string, number>()
      let spend = 0
      for (const a of e.agents) {
        if (a.router_id !== r.id) continue
        spend += a.spend_usd
        for (const [m, w] of modelWeights(e, a.model_ids)) {
          const p = providerOf(m)
          if (p) by.set(p, (by.get(p) ?? 0) + a.spend_usd * w)
        }
      }
      const providers = shares(
        [...by.entries()]
          .map(([id, s]) => {
            const p = e.providers.find((x) => x.id === id)
            return { id, name: p?.name ?? id, brand: p?.brand ?? null, spend: s }
          })
          .sort(byDesc((p) => p.spend)),
      )
      return {
        id: r.id,
        name: r.name,
        spend,
        share: agentTotal > 0 ? spend / agentTotal : 0,
        tone: toneOf(st(r.id)),
        providers,
      }
    })
    .sort(byDesc((r) => r.spend))

  const provRows = e.providers
    .map((p) => {
      const models = e.models
        .filter((m) => m.provider_id === p.id)
        .map((m) => ({ id: m.id, name: m.short || m.name, brand: p.brand, spend: m.spend_usd }))
        .sort(byDesc((m) => m.spend))
      return { p, models, spend: models.reduce((n, m) => n + m.spend, 0) }
    })
    .sort((a, b) => b.spend - a.spend || b.models.length - a.models.length)
    .slice(0, SHOWN.providers)
  // One scale for every column, so a model's bar compares across providers.
  const topModel = Math.max(
    0,
    ...provRows.flatMap((r) => r.models.slice(0, SHOWN.models).map((m) => m.spend)),
  )
  const providers = provRows.map(({ p, models, spend }): ProviderRow => {
    const shown = models.slice(0, SHOWN.models)
    const rest = models.slice(SHOWN.models).reduce((n, m) => n + m.spend, 0)
    return {
      id: p.id,
      name: p.name,
      brand: p.brand,
      spend,
      models: shown.map((m) => ({
        ...m,
        share: spend > 0 ? m.spend / spend : 1 / shown.length,
        bar: topModel > 0 ? m.spend / topModel : 0,
      })),
      rest,
    }
  })

  // The Org column: every team's budget and people, and what flows to it (its agents' share, its harnesses).
  const flowTo = new Map<string | null, number>()
  const addTo = (k: string | null, usd: number) => flowTo.set(k, (flowTo.get(k) ?? 0) + usd)
  for (const a of e.agents) {
    const ts = teamsOf(e, a)
    if (!ts.length) addTo(null, a.spend_usd)
    for (const t of ts) addTo(t, a.spend_usd / ts.length)
  }
  const groupOf = (person: string | null) =>
    person ? (e.people.find((p) => p.id === person)?.group_id ?? null) : null
  for (const h of e.harnesses) addTo(groupOf(h.owner_id), h.spend_usd ?? 0)
  const peopleOf = (g: string | null) =>
    e.people.filter((p) => p.group_id === g).map((p) => SEGMENT_TONE[p.segment])
  const teams: TeamRow[] = e.groups
    .map((g): TeamRow => {
      const b = g.budget && g.budget.limit_usd > 0 ? g.budget : null
      return {
        id: g.id,
        name: g.name,
        spent: b ? b.spent_usd : (flowTo.get(g.id) ?? 0),
        limit: b?.limit_usd ?? null,
        projected: b?.projected_usd ?? null,
        pace: pace(b),
        hard: !!b?.hard,
        usd: flowTo.get(g.id) ?? 0,
        people: peopleOf(g.id),
      }
    })
    // Over pace first, then the ones with no budget (nothing holds them), then by pace.
    .sort((a, b) => (b.pace ?? 1.001) - (a.pace ?? 1.001) || b.usd - a.usd)
  const loose = peopleOf(null)
  if (loose.length || (flowTo.get(null) ?? 0) > 0)
    teams.push({
      id: null,
      name: '',
      spent: flowTo.get(null) ?? 0,
      limit: null,
      projected: null,
      pace: null,
      hard: false,
      usd: flowTo.get(null) ?? 0,
      people: loose,
    })

  return {
    agents: agents.slice(0, SHOWN.agents),
    agentCount: agents.length,
    workflows: workflows.slice(0, SHOWN.workflows),
    workflowCount: workflows.length,
    tools: tools.slice(0, SHOWN.tools),
    toolCount: e.tools.length,
    harnesses: harnesses.slice(0, SHOWN.harnesses),
    harnessCount: e.harnesses.length,
    routers: routers.slice(0, SHOWN.routers),
    routerCount: routers.length,
    providers,
    bill: billOf(e),
    teams,
    people: e.people.length,
  }
}

/**
 * What each provider was paid in the range: every agent's and coding harness's spend split over its models (the token
 * lines' rule), and what named no model a provider is known for.
 */
export function providerSpend(e: Estate): { by: Map<string, number>; unattributed: number } {
  const providerOf = (m: string) => e.models.find((x) => x.id === m)?.provider_id ?? null
  const by = new Map<string, number>()
  let unattributed = 0
  const split = (ids: readonly string[], usd: number) => {
    let placed = 0
    for (const [m, w] of ids.length ? modelWeights(e, ids) : []) {
      const p = providerOf(m)
      if (!p) continue
      by.set(p, (by.get(p) ?? 0) + usd * w)
      placed += w
    }
    unattributed += usd * (1 - placed)
  }
  for (const a of e.agents) split(a.model_ids, a.spend_usd)
  for (const h of e.harnesses) split(h.model_ids, h.spend_usd ?? 0)
  return { by, unattributed }
}

/** The range's bill (see `Bill`): by provider and by source, both adding up to the same total. */
export function billOf(e: Estate): Bill {
  const { by, unattributed } = providerSpend(e)
  const lines = e.providers
    .map((p) => ({ id: p.id, name: p.name, brand: p.brand, spend: by.get(p.id) ?? 0 }))
    .filter((p) => p.spend > 0)
    .sort(byDesc((p) => p.spend))
  const agents = e.agents.reduce((n, a) => n + a.spend_usd, 0)
  const harnesses = e.harnesses.reduce((n, h) => n + (h.spend_usd ?? 0), 0)
  // A cent of rounding is not a line of its own.
  const loose = unattributed > 0.005 ? unattributed : 0
  return {
    providers: lines.slice(0, SHOWN.billProviders),
    otherProviders: lines.slice(SHOWN.billProviders).reduce((n, p) => n + p.spend, 0),
    agents,
    harnesses,
    unattributed: loose,
    total: agents + harnesses,
  }
}

/** Total spend over the range: the fleet's daily line, the change on the window before, and the days over the line. */
export function spendCard(
  daily: readonly number[],
  total: number,
  previous: number | null,
  dailyBudget: number | null,
): SpendCard {
  const mean = daily.length ? daily.reduce((n, x) => n + x, 0) / daily.length : 0
  const threshold = dailyBudget && dailyBudget > 0 ? dailyBudget : daily.length ? mean : null
  const spikes: number[] = []
  if (threshold !== null)
    daily.forEach((x, i) => {
      const peak = x >= (daily[i - 1] ?? -Infinity) && x >= (daily[i + 1] ?? -Infinity)
      if (x > threshold && peak) spikes.push(i)
    })
  spikes.sort((a, b) => (daily[b] ?? 0) - (daily[a] ?? 0))
  return {
    total,
    changePct: previous && previous > 0 ? ((total - previous) / previous) * 100 : null,
    series: [...daily],
    threshold,
    spikes: spikes.slice(0, 4).sort((a, b) => a - b),
  }
}

/**
 * The bill's daily line and the UTC day of each point: the estate's own days when it reports them (V-11), else the
 * range's fleet timeseries. The total is the estate's when it has days, else the timeseries'.
 */
export function billSeries(
  e: Estate,
  spend: Pick<Spend, 'days' | 'totals'>,
  now: number,
): { card: SpendCard; dates: string[] } {
  const own = !!e.daily?.length
  const daily = own
    ? (e.daily ?? []).map((d) => d.budgeted_usd + d.unbudgeted_usd)
    : spend.days.map((d) => d.spend)
  const dates = own ? e.days.slice(-daily.length) : spend.days.map((d) => d.iso.slice(0, 10))
  const sum = daily.reduce((n, x) => n + x, 0)
  const total = own ? sum : (spend.totals?.spend ?? sum)
  const t = spend.totals
  const prev = t?.previous && t.spend > 0 ? (total * t.previous.spend) / t.spend : null
  return { card: spendCard(daily, total, prev, dailyBudget(e, now)), dates }
}

/** The month's budget spread over its days: the line the daily spend is held to. */
export function dailyBudget(e: Estate, now: number): number | null {
  const limit = e.groups.reduce((n, g) => n + (g.budget?.limit_usd ?? 0), 0)
  if (limit <= 0) return null
  const d = new Date(now)
  const days = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 0)).getUTCDate()
  return limit / days
}
