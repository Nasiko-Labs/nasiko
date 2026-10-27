/**
 * The village, derived (pure): the estate view turned into districts of plots, cars with a light on top, people where
 * they work, the token routes, the HUD's numbers and the builder's log (plans/feat-village-overview.md §3, V17–V30):
 *
 * - groups → team houses with their budget battery (V50), people → where they work (V26);
 * - providers → tents: the product's on the island, the coding harnesses' as brick stacks on the dirt plot (V19);
 * - routers → the blue troughs the tokens pass under (V30), MCP gateways and tools beside them;
 * - agents → pieces on the parking, walking to the teams and projects that use them (V24, V52);
 * - coding harnesses → one ball per kind on the dirt plot, watched by its eye tower (V19, V20);
 * - projects → dirt plots with a flag (V27, V50); their workflows are the clock tower's (V53);
 * - tokens run from the battery to the teams down the sand strips, and to the providers under a router or past them
 *   all; the dirt plot's battery feeds the coding tools, and gets the island's tokens when billed together (V21, V54).
 *
 * Every district holds the free plan's number of open plots (V17); broken things are placed first, so a cap never
 * hides a fire, and the district says how many more exist. Layout is deterministic: the same estate always draws the
 * same village. Nothing here reads the DOM or the clock.
 */
import { pace as paceOf, problemsOf } from '../estate/model'
import type { Opportunity, Problem } from '../estate/types'
import type { EstateView, PlacedRequest } from '../estate/useEstateView'
import { PROVIDER_ART, PROVIDER_FALLBACK, TEAM_COLORS } from './palette'
import { openPlots } from './plan'
import {
  DIRECT_J,
  doorOf,
  GATE_STREET_I,
  LANE_J,
  passOf,
  route,
  SPOUT,
  tentDoor,
  toTent,
  yardDoor,
  type Tile,
} from './streets'
import { LOST_MIN_USD } from './tuning'
import { BAYS, CORE, PLOTS, type Kind, type Plot } from './zones'

export type Pose = 'live' | 'idle' | 'hurt' | 'down'
export type Weather = 'ok' | 'warn' | 'down'
export type Statue = 'gear' | 'flask' | 'heart' | 'rocket' | 'bars' | 'star'
/** An agent's colour: white while it walks to work or stands ready, orange slow, red broken, pale grey asleep. */
export type Beacon = 'active' | 'running' | 'slow' | 'broken' | 'asleep'

/** Places the village has that are not estate nodes: their selection is `place:<id>` (a yard: `place:yard:<id>`). */
export const PLACES = [
  'townhall',
  'tank',
  'piggy',
  'orb',
  'orchestrator',
  'observatory',
  'workflows',
] as const
export type PlaceId = (typeof PLACES)[number]

export interface Placed extends Plot {
  /** The plot it sits on, in its district's order. */
  plot: number
}
export interface Team extends Placed {
  id: string
  name: string
  color: string
  statue: Statue
  budget: { limit: number; spent: number; projected: number; hard: boolean } | null
  /** Projected month over the limit; null without a budget. */
  pace: number | null
  members: number
}
export interface Mine extends Placed {
  id: string
  name: string
  color: string
  mono: string
  spend: number
  models: { id: string; name: string; spend: number }[]
  weather: Weather
  /** On the outside plot: the coding harnesses' use of this provider. */
  outside: boolean
  /** Its harness use is billed on the same account as its product use (V-7). */
  shared: boolean
}
export interface Gate extends Placed {
  id: string
  name: string
  weather: Weather
}
export interface Connector extends Placed {
  id: string
  name: string
  /** A brand mark the UI knows, for a tool's stone. */
  brand: string | null
  weather: Weather
}
export interface Car {
  id: string
  name: string
  pose: Pose
  /** Its bay in the depot (a point, not tiles). */
  bay: number
  i: number
  j: number
  /** The doors it drives to: its teams' plots, then the yards whose workflows use it. */
  targets: Tile[]
  teams: string[]
  /** Its router's gate, or null when it skips the routers. */
  gate: string | null
  spend: number
  /** What its failed runs cost in the range, floated over it while it is broken or slow (V56); null: no badge. */
  lost: number | null
}
export interface HarnessPlot extends Placed {
  kind: string
  name: string
  spend: number
  seats: number
  weather: Weather
  providers: string[]
}
export interface Rail {
  id: string
  name: string
  runs: number
  stops: { kind: string; ref: string | null; label: string }[]
}
export interface Yard extends Placed {
  id: string
  name: string
  /** Org-wide (no team), so it sits nearest the town hall. */
  org: boolean
  /** The team's colour, for a team's project. */
  color: string | null
  /** The workflows with no project share this one. */
  unfiled: boolean
  rails: Rail[]
  agents: string[]
}
export interface Person {
  id: string
  name: string
  k: number
  i: number
  j: number
  /** What they stand beside (a team, car, gate or yard id). */
  at: string
}
export interface TokenEdge {
  key: string
  /** alloc: battery → team; project: battery → project plot; routed / direct: battery → provider under a router or
   *  past them; saved: battery → piggy bank; shared: an island provider → the dirt plot's battery (billed together);
   *  outside: the dirt plot's battery → a coding tool. */
  kind: 'alloc' | 'project' | 'routed' | 'direct' | 'saved' | 'shared' | 'outside'
  path: Tile[]
  /** What flows along it in the range, in USD: how thick its queue of coins is (V55). */
  spend: number
  /** The estate nodes it serves: a selection that keeps any of them lit keeps the line lit. */
  nodes: string[]
  /** The village places it belongs to (scene keys): selecting one lights the line too. */
  places: string[]
}
export interface Vacant extends Plot {
  kind: Kind
  plot: number
  /** Open on this plan; a locked plot opens on a paid plan. */
  open: boolean
}
export interface District {
  kind: Kind
  /** How many exist, how many are drawn, and how many plots the plan opens. */
  total: number
  drawn: number
  open: number
}
export interface Outpost {
  spend: number
  seats: number
  byKind: { kind: string; spend: number }[]
  weather: Weather
}
export interface Watch {
  runs: number
  success: number | null
  agents: number
}

export type LogKind = 'fix' | 'approve' | 'watch' | 'upgrade'
export const LOG_KINDS: readonly LogKind[] = ['fix', 'approve', 'watch', 'upgrade']
export interface LogItem {
  key: string
  kind: LogKind
  title: string
  detail: string
  /** The node a "Take me there" pans to. */
  anchor: string | null
  /** What selecting it opens (the `sel` param). */
  sel: string | null
  problem?: Problem
  request?: PlacedRequest
  opp?: Opportunity
}

export interface Hud {
  counts: Record<Pose, number>
  /** Runs-weighted success over the agents that report it; null without any. */
  reliability: number | null
  budget: { limit: number; spent: number; left: number; pace: number } | null
  /** Spend by people in no team with a budget. */
  unbudgeted: number
  /** The budgeted teams' coding-harness spend: counted in a budget, but a budget can only alert on it. */
  uncapped: number
  /** What no budget governs (V58): unbudgeted plus uncapped. Zero in Mine, which reads the viewer's allowance. */
  ungoverned: number
  routedShare: number | null
  /** The org's spend per day, oldest first, for the meters' trend lines (V59); null in Mine or without V-11. */
  trend: { spent: number[]; ungoverned: number[]; saved: number[] } | null
}

/**
 * The meters' one scale (V57): the budget (the viewer's allowance in Mine), so each fill reads as a share of it. With
 * no budget, the ungoverned spend is the whole of it; with neither, there is nothing to measure.
 */
export function meterScale(hud: Hud): number | null {
  if (hud.budget && hud.budget.limit > 0) return hud.budget.limit
  return hud.ungoverned > 0 ? hud.ungoverned : null
}

export interface Village {
  teams: Team[]
  mines: Mine[]
  xmines: Mine[]
  gates: Gate[]
  mcps: Connector[]
  tools: Connector[]
  cars: Car[]
  harnesses: HarnessPlot[]
  yards: Yard[]
  /** Every workflow, busiest first: the clock tower's. */
  workflows: Rail[]
  people: Person[]
  outpost: Outpost | null
  watch: Watch
  core: typeof CORE
  tokens: TokenEdge[]
  vacant: Vacant[]
  districts: Record<Kind, District>
  unlimited: boolean
  hud: Hud
  log: Record<LogKind, LogItem[]>
}

/** A remembered placement (V28): the next new thing of `kind` that isn't in `known` lands on plot `plot`. */
export interface Claim {
  plot: number
  known: string[]
}
export type Claims = Partial<Record<Kind, Claim>>

/** The dirt plot's battery spout, on its front-left: the coding tools' tokens start here. */
const XSPOUT: Tile = [CORE.xtank.i + 0.3, CORE.xtank.j + 2.05]

const STATUES: readonly [Statue, ...Statue[]] = ['gear', 'flask', 'heart', 'rocket', 'bars', 'star']
/** The k-th item of a fixed list, wrapping round. */
export const cycle = <T>(a: readonly [T, ...T[]], k: number): T => a[k % a.length] ?? a[0]
export const hash = (s: string) => [...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7)

const SEVERITY: Record<Weather, number> = { down: 2, warn: 1, ok: 0 }
const POSE_ORDER: Record<Pose, number> = { down: 0, hurt: 1, idle: 2, live: 3 }

/**
 * Which plot each thing takes: a claimed plot goes to the first newcomer (not in the claim's `known`), everything
 * else fills the open plots in order. `ids` are already capped to the open count.
 */
export function assignPlots(
  ids: readonly string[],
  open: number,
  claim?: Claim,
): Map<string, number> {
  const out = new Map<string, number>()
  if (claim && claim.plot < open) {
    const newcomer = ids.find((id) => !claim.known.includes(id))
    if (newcomer !== undefined) out.set(newcomer, claim.plot)
  }
  const used = new Set(out.values())
  let k = 0
  for (const id of ids) {
    if (out.has(id)) continue
    while (used.has(k)) k++
    if (k >= open) break
    out.set(id, k)
    used.add(k++)
  }
  return out
}

/**
 * A place's weather, from the problems that are about it (their first entity is one of `ids`): broken errors are an
 * outage, any other error or slowness a storm. A problem that only mentions the place (a crash that used a model)
 * leaves its sky clear; the problem's own subject carries it.
 */
export function weatherOf(ps: readonly Problem[], ids: readonly string[]): Weather {
  const about = ps.filter((p) => ids.includes(p.entities[0] ?? ''))
  const health = (p: Problem) => p.kind !== 'cost' || (p.also !== null && p.also !== 'cost')
  if (about.some((p) => p.tier === 'broken' && p.kind === 'errors')) return 'down'
  return about.some(health) ? 'warn' : 'ok'
}

/** An agent's pose: idle sleeps, a broken error about it knocks it out, any error or slowness it is in makes it limp. */
export function poseOf(idle: boolean, id: string, ps: readonly Problem[]): Pose {
  if (idle) return 'idle'
  if (ps.some((p) => p.tier === 'broken' && p.kind === 'errors' && p.entities[0] === id))
    return 'down'
  if (ps.some((p) => p.kind !== 'cost' || (p.also !== null && p.also !== 'cost'))) return 'hurt'
  return 'live'
}

/**
 * What a piece's badge says its failed runs cost (V56): only on a broken or slow piece, whose pose already says
 * something is wrong, and only from `LOST_MIN_USD` up; null draws no badge.
 */
export function lostOf(pose: Pose, failed: number | null | undefined): number | null {
  if (pose !== 'down' && pose !== 'hurt') return null
  return failed !== null && failed !== undefined && failed >= LOST_MIN_USD ? failed : null
}

/** A car's light at rest (parked); a working car turns `active` while it drives. */
export const beaconOf = (pose: Pose): Beacon =>
  pose === 'down' ? 'broken' : pose === 'idle' ? 'asleep' : pose === 'hurt' ? 'slow' : 'running'

/** Where people stand beside a place, the k-th of up to three. */
function standAt(kind: 'team' | 'bay' | 'gate' | 'yard', p: Plot, k: number): Tile {
  switch (kind) {
    case 'team':
      return (
        [
          [p.i + 0.4, p.j + p.d + 0.35],
          [p.i + 1.0, p.j + p.d + 0.35],
          [p.i + p.w + 0.3, p.j + 0.4],
        ] as const
      )[k % 3] as Tile
    case 'bay':
      return [p.i + 0.2 + k * 0.35, p.j - 0.25]
    case 'gate':
      return [GATE_STREET_I - 0.3, p.j + 0.3 + k * 0.5]
    default:
      return [p.i - 0.3, p.j + 0.3 + k * 0.45]
  }
}

export function buildVillage(
  v: EstateView,
  opts: { mine?: boolean; claims?: Claims; saved?: number | null } = {},
): Village {
  const e = v.estate
  const unlimited = e.plan === 'team'
  const claims = opts.claims ?? {}
  const ps = (id: string) => problemsOf(v.nodes, v.problems, id)
  const providerOfModel = new Map(e.models.map((m) => [m.id, m.provider_id]))
  const districts = {} as Record<Kind, District>
  const vacant: Vacant[] = []

  /** Caps `items` to the open plots, assigns their plots, and records the district and its empty plots. */
  function settle<T extends { id: string }>(kind: Kind, items: readonly T[]) {
    const plots = PLOTS[kind]
    const open = openPlots(kind, plots.length, unlimited)
    const shown = items.slice(0, open)
    const at = assignPlots(
      shown.map((x) => x.id),
      open,
      claims[kind],
    )
    districts[kind] = { kind, total: items.length, drawn: shown.length, open }
    const used = new Set(at.values())
    plots.forEach((p, k) => {
      if (!used.has(k)) vacant.push({ ...p, kind, plot: k, open: k < open })
    })
    return shown.flatMap((x) => {
      const k = at.get(x.id)
      const p = k === undefined ? undefined : plots[k]
      return p && k !== undefined ? [{ item: x, plot: { ...p, plot: k } }] : []
    })
  }

  // Teams: over pace first, then spend, then name.
  const groups = [...e.groups].sort((a, b) => {
    const pa = paceOf(a.budget) ?? 0
    const pb = paceOf(b.budget) ?? 0
    return (
      Number(pb > 1) - Number(pa > 1) ||
      (b.budget?.spent_usd ?? 0) - (a.budget?.spent_usd ?? 0) ||
      a.name.localeCompare(b.name)
    )
  })
  const teams: Team[] = settle('team', groups).map(({ item: g, plot }) => {
    const b = g.budget
    return {
      ...plot,
      id: g.id,
      name: g.name,
      // The house's roof, so the island keeps the reference's colours whichever teams it draws.
      color: cycle(TEAM_COLORS, plot.plot),
      statue: cycle(STATUES, hash(g.name)),
      budget: b
        ? { limit: b.limit_usd, spent: b.spent_usd, projected: b.projected_usd, hard: b.hard }
        : null,
      pace: paceOf(b),
      members: e.people.filter((p) => p.group_id === g.id).length,
    }
  })
  const teamById = new Map(teams.map((t) => [t.id, t]))

  // Inside mines: the providers the product uses: an agent's model, or a model a problem ties to an agent or a router
  // (a router's own model isn't in the estate). Broken first, then spend.
  const productIds = new Set([...e.agents.map((a) => a.id), ...e.routers.map((r) => r.id)])
  const agentModels = new Set([
    ...e.agents.flatMap((a) => a.model_ids),
    ...v.problems
      .filter((p) => p.entities.some((x) => productIds.has(x)))
      .flatMap((p) => p.entities.filter((x) => providerOfModel.has(x))),
  ])
  const mineOf = (
    p: (typeof e.providers)[number],
    spend: number,
    models: Mine['models'],
    outside: boolean,
  ) => {
    const art = PROVIDER_ART[p.brand ?? ''] ?? PROVIDER_ART[p.id.replace(/^provider:/, '')]
    return {
      id: p.id,
      name: p.name,
      color: art?.color ?? PROVIDER_FALLBACK,
      mono: art?.mono ?? (p.name[0] ?? '?').toUpperCase(),
      spend,
      models,
      weather: weatherOf(
        [...ps(p.id), ...models.flatMap((m) => ps(m.id))],
        [p.id, ...models.map((m) => m.id)],
      ),
      outside,
      shared: !!p.billed_with_harnesses,
    }
  }
  const insideProviders = e.providers
    .map((p) => {
      const models = e.models
        .filter((m) => m.provider_id === p.id && agentModels.has(m.id))
        .map((m) => ({ id: m.id, name: m.short ?? m.name, spend: m.spend_usd }))
        .sort((a, b) => b.spend - a.spend)
      return models.length
        ? mineOf(
            p,
            models.reduce((n, m) => n + m.spend, 0),
            models,
            false,
          )
        : null
    })
    .filter((x): x is Omit<Mine, keyof Placed> => !!x)
    .sort(
      (a, b) =>
        Number(b.weather === 'down') - Number(a.weather === 'down') ||
        b.spend - a.spend ||
        a.name.localeCompare(b.name),
    )
  const mines: Mine[] = settle('mine', insideProviders).map(({ item, plot }) => ({
    ...item,
    ...plot,
  }))
  const mineIds = new Set(mines.map((m) => m.id))

  // Gates: one per router that routes. Broken first, then the estate's order.
  const routers = e.routers
    .filter((r) => !r.direct)
    .map((r, k) => ({ id: r.id, name: r.name, weather: weatherOf(ps(r.id), [r.id]), k }))
    .sort((a, b) => SEVERITY[b.weather] - SEVERITY[a.weather] || a.k - b.k)
  const gates: Gate[] = settle('gate', routers).map(({ item, plot }) => ({
    id: item.id,
    name: item.name,
    weather: item.weather,
    ...plot,
  }))
  const gateById = new Map(gates.map((g) => [g.id, g]))

  // Connectors: MCP servers and the tools beside them, by the gates. Broken first, then name.
  const connectors = e.tools
    .map((t) => ({
      id: t.id,
      name: t.name,
      brand: t.brand,
      server: !!t.connector_id,
      weather: weatherOf(ps(t.id), [t.id]),
    }))
    .sort((a, b) => SEVERITY[b.weather] - SEVERITY[a.weather] || a.name.localeCompare(b.name))
  const toConnector = ({ item, plot }: { item: (typeof connectors)[number]; plot: Placed }) => ({
    id: item.id,
    name: item.name,
    brand: item.brand,
    weather: item.weather,
    ...plot,
  })
  const mcps: Connector[] = settle(
    'mcp',
    connectors.filter((c) => c.server),
  ).map(toConnector)
  const tools: Connector[] = settle(
    'tool',
    connectors.filter((c) => !c.server),
  ).map(toConnector)

  // Agents: every pose (the HUD counts them all); the cars are the first ten, broken, slow and asleep first.
  const agents = e.agents.map((a) => ({ a, pose: poseOf(a.idle, a.id, ps(a.id)) }))
  const ordered = [...agents].sort(
    (x, y) =>
      POSE_ORDER[x.pose] - POSE_ORDER[y.pose] ||
      y.a.spend_usd - x.a.spend_usd ||
      x.a.name.localeCompare(y.a.name),
  )
  // Broken cars take the service row first, so their bays are claimed before the rest fill in.
  const service = BAYS.map((b, k) => (b.service ? k : -1)).filter((k) => k >= 0)

  // Projects: org-wide first, then by name; the workflows with no project share the last yard.
  const projects = [...(e.projects ?? [])].sort(
    (a, b) => Number(!!a.group_id) - Number(!!b.group_id) || a.name.localeCompare(b.name),
  )
  const filed = new Set(projects.flatMap((p) => p.workflow_ids))
  const unfiled = e.workflows.filter((w) => !filed.has(w.id) && !w.project_id)
  const railOf = (w: (typeof e.workflows)[number]): Rail => ({
    id: w.id,
    name: w.name,
    runs: w.runs,
    stops: w.steps.map((s) => ({ kind: s.kind, ref: s.ref, label: s.label })),
  })
  const yardItems = [
    ...projects.map((p) => ({
      id: p.id,
      name: p.name,
      org: !p.group_id,
      color: (p.group_id && teamById.get(p.group_id)?.color) || null,
      unfiled: false,
      rails: e.workflows
        .filter((w) => p.workflow_ids.includes(w.id) || w.project_id === p.id)
        .map(railOf),
    })),
    ...(unfiled.length
      ? [
          {
            id: 'unfiled',
            name: '',
            org: true,
            color: null,
            unfiled: true,
            rails: unfiled.map(railOf),
          },
        ]
      : []),
  ]
  // The unfiled yard always shows: it takes the last open yard when projects would fill them all.
  const yardOpen = openPlots('project', PLOTS.project.length, unlimited)
  const keptYards =
    yardItems.length > yardOpen && unfiled.length
      ? [...yardItems.slice(0, yardOpen - 1), ...yardItems.slice(-1)]
      : yardItems
  const yards: Yard[] = settle('project', keptYards).map(({ item, plot }) => ({
    ...item,
    ...plot,
    rails: item.rails.slice(0, 3),
    agents: [
      ...new Set(
        item.rails.flatMap((r) =>
          r.stops.filter((s) => s.kind === 'agent').map((s) => s.ref ?? ''),
        ),
      ),
    ].filter(Boolean),
  }))
  if (districts.project) districts.project.total = yardItems.length

  const carIds = ordered.map((x) => x.a.id)
  const bayOpen = openPlots('bay', BAYS.length, unlimited)
  const drawnCars = ordered.slice(0, bayOpen)
  const downIds = drawnCars.filter((x) => x.pose === 'down').map((x) => x.a.id)
  const bayOf = new Map<string, number>()
  downIds.forEach((id, k) => {
    const b = service[k]
    if (b !== undefined && b < bayOpen) bayOf.set(id, b)
  })
  const rest = assignPlots(
    drawnCars.map((x) => x.a.id).filter((id) => !bayOf.has(id)),
    bayOpen - bayOf.size,
    undefined,
  )
  const freeBays = [...Array(bayOpen).keys()].filter((k) => ![...bayOf.values()].includes(k))
  for (const [id, k] of rest) {
    const b = freeBays[k]
    if (b !== undefined) bayOf.set(id, b)
  }
  const claimBay = claims.bay
  if (claimBay && claimBay.plot < bayOpen) {
    const newcomer = drawnCars.find((x) => !claimBay.known.includes(x.a.id))
    const taken = [...bayOf.entries()].find(([, b]) => b === claimBay.plot)
    if (newcomer && taken && taken[0] !== newcomer.a.id) {
      const was = bayOf.get(newcomer.a.id)
      bayOf.set(newcomer.a.id, claimBay.plot)
      if (was !== undefined) bayOf.set(taken[0], was)
    }
  }
  districts.bay = { kind: 'bay', total: carIds.length, drawn: drawnCars.length, open: bayOpen }
  const usedBays = new Set(bayOf.values())
  PLOTS.bay.forEach((p, k) => {
    if (!usedBays.has(k)) vacant.push({ ...p, kind: 'bay', plot: k, open: k < bayOpen })
  })
  const cars: Car[] = drawnCars.flatMap(({ a, pose }) => {
    const b = bayOf.get(a.id)
    const p = b === undefined ? undefined : BAYS[b]
    if (!p || b === undefined) return []
    const owner = a.owner_id ? v.nodes.get(a.owner_id)?.group : null
    const used = (a.team_ids ?? [owner]).filter((x): x is string => !!x && teamById.has(x))
    const its = [...new Set(used)]
    return [
      {
        id: a.id,
        name: a.name,
        pose,
        bay: b,
        i: p.i + p.w / 2,
        j: p.j + p.d / 2,
        targets: [
          ...its.flatMap((t) => {
            const team = teamById.get(t)
            return team ? [doorOf(team)] : []
          }),
          ...yards.filter((y) => y.agents.includes(a.id)).map((y) => yardDoor(y)),
        ],
        teams: its,
        gate: a.router_id && gateById.has(a.router_id) ? a.router_id : null,
        spend: a.spend_usd,
        lost: lostOf(pose, a.failed_spend_usd),
      },
    ]
  })
  const carById = new Map(cars.map((c) => [c.id, c]))

  // Coding harnesses: one plot per kind, by spend; each kind's spend split over its providers.
  const kinds = new Map<
    string,
    { spend: number; seats: number; ids: string[]; providers: Set<string> }
  >()
  for (const h of e.harnesses) {
    const k = kinds.get(h.harness) ?? { spend: 0, seats: 0, ids: [], providers: new Set<string>() }
    k.spend += h.spend_usd ?? 0
    k.seats += 1
    k.ids.push(h.id)
    h.model_ids.forEach((m) => {
      const p = providerOfModel.get(m)
      if (p) k.providers.add(p)
    })
    kinds.set(h.harness, k)
  }
  const kindList = [...kinds.entries()]
    .map(([kind, k]) => ({
      id: kind,
      kind,
      ...k,
      weather: weatherOf(
        k.ids.flatMap((id) => ps(id)),
        k.ids,
      ),
    }))
    .sort((a, b) => b.spend - a.spend || a.kind.localeCompare(b.kind))
  const harnesses: HarnessPlot[] = settle('harness', kindList).map(({ item, plot }) => ({
    ...plot,
    kind: item.kind,
    name: item.kind,
    spend: item.spend,
    seats: item.seats,
    weather: item.weather,
    providers: [...item.providers],
  }))

  // Outside mines: the providers the drawn harnesses call, with their share of those harnesses' spend.
  const harnessBy = new Map<string, number>()
  for (const h of e.harnesses) {
    if (!harnesses.some((x) => x.kind === h.harness)) continue
    const provs = h.model_ids.map((m) => providerOfModel.get(m)).filter((x): x is string => !!x)
    for (const p of provs)
      harnessBy.set(p, (harnessBy.get(p) ?? 0) + (h.spend_usd ?? 0) / provs.length)
  }
  const outsideProviders = e.providers
    .filter((p) => harnessBy.has(p.id))
    .map((p) => {
      const models = e.models
        .filter((m) => m.provider_id === p.id && !agentModels.has(m.id))
        .map((m) => ({ id: m.id, name: m.short ?? m.name, spend: m.spend_usd }))
      return mineOf(p, harnessBy.get(p.id) ?? 0, models, true)
    })
    .sort((a, b) => b.spend - a.spend || a.name.localeCompare(b.name))
  const xmines: Mine[] = settle('xmine', outsideProviders).map(({ item, plot }) => ({
    ...item,
    ...plot,
  }))

  // People: the ones with sessions, where they work now (V26), up to three per spot.
  const crowd = new Map<string, number>()
  const people: Person[] = [...e.people]
    .filter((p) => p.sessions > 0)
    .sort((a, b) => b.spend_usd - a.spend_usd || a.name.localeCompare(b.name))
    .flatMap((p) => {
      const w = p.working_on
      const spot = ((): {
        at: string
        kind: 'team' | 'bay' | 'gate' | 'yard'
        plot: Plot
      } | null => {
        const car = w?.kind === 'agent' ? carById.get(w.id) : undefined
        if (car) {
          const b = BAYS[car.bay]
          if (b) return { at: car.id, kind: 'bay', plot: b }
        }
        const gate = w?.kind === 'router' ? gateById.get(w.id) : undefined
        if (gate) return { at: gate.id, kind: 'gate', plot: gate }
        const yard = w?.kind === 'project' ? yards.find((y) => y.id === w.id) : undefined
        if (yard) return { at: yard.id, kind: 'yard', plot: yard }
        const team = p.group_id ? teamById.get(p.group_id) : undefined
        return team ? { at: team.id, kind: 'team', plot: team } : null
      })()
      if (!spot) return []
      const k = crowd.get(spot.at) ?? 0
      if (k >= 3) return []
      crowd.set(spot.at, k + 1)
      const [i, j] = standAt(spot.kind, spot.plot, k)
      return [{ id: p.id, name: p.name, k: hash(p.id), i, j, at: spot.at }]
    })

  // Tokens (V54, V55): what flows from the battery's spout, in USD over the range, to where it is used. Down the sand
  // strips to each team (the spend of the agents it uses) and to each project (its workflows' share of their agents'
  // spend, an estimate until the server says); east to each provider under its agents' router or past the routers on
  // the direct lane; into the piggy bank, what was saved; and on the dirt, the coding tools' battery to each tool,
  // fed from an island provider only when the two are billed together.
  const tokens: TokenEdge[] = []
  const teamOf = (a: (typeof e.agents)[number]) => {
    const owner = a.owner_id ? v.nodes.get(a.owner_id)?.group : null
    return [...new Set((a.team_ids ?? [owner]).filter((x): x is string => !!x && teamById.has(x)))]
  }
  const teamUse = new Map<string, { spend: number; agents: string[] }>()
  for (const { a } of agents) {
    const ts = teamOf(a)
    for (const t of ts) {
      const u = teamUse.get(t) ?? { spend: 0, agents: [] }
      u.spend += a.spend_usd / ts.length
      u.agents.push(a.id)
      teamUse.set(t, u)
    }
  }
  for (const t of teams) {
    const u = teamUse.get(t.id)
    if (u && u.spend > 0)
      tokens.push({
        key: `alloc:${t.id}`,
        kind: 'alloc',
        path: route(SPOUT, doorOf(t)),
        spend: u.spend,
        nodes: [t.id, ...u.agents],
        places: ['tank', `team:${t.id}`],
      })
  }
  const agentById = new Map(e.agents.map((a) => [a.id, a]))
  for (const y of yards) {
    let spend = 0
    for (const r of y.rails)
      for (const st of r.stops) {
        const a = st.kind === 'agent' && st.ref ? agentById.get(st.ref) : undefined
        if (a) spend += a.spend_usd * Math.min(1, r.runs / Math.max(1, a.runs))
      }
    if (spend > 0)
      tokens.push({
        key: `project:${y.id}`,
        kind: 'project',
        path: route(SPOUT, yardDoor(y)),
        spend,
        nodes: [y.id, ...y.agents],
        places: ['tank', `yard:${y.id}`, 'workflows'],
      })
  }
  const flow = new Map<string, { spend: number; agents: string[] }>()
  for (const { a } of agents) {
    if (a.spend_usd <= 0) continue
    const provs = [
      ...new Set(
        a.model_ids
          .map((m) => providerOfModel.get(m))
          .filter((p): p is string => !!p && mineIds.has(p)),
      ),
    ]
    const gate = a.router_id && gateById.has(a.router_id) ? a.router_id : 'direct'
    for (const p of provs) {
      const k = `${gate}|${p}`
      const f = flow.get(k) ?? { spend: 0, agents: [] }
      f.spend += a.spend_usd / provs.length
      f.agents.push(a.id)
      flow.set(k, f)
    }
  }
  for (const [k, f] of flow) {
    const [g, p] = k.split('|') as [string, string]
    const mine = mines.find((m) => m.id === p)
    const gate = gateById.get(g)
    if (!mine) continue
    const j = gate ? passOf(gate) : DIRECT_J
    tokens.push({
      key: `${gate ? 'routed' : 'direct'}:${k}`,
      kind: gate ? 'routed' : 'direct',
      path: [...route(SPOUT, [GATE_STREET_I, j]), ...toTent(j, mine)],
      spend: f.spend,
      nodes: [...(gate ? [gate.id] : []), mine.id, ...mine.models.map((x) => x.id), ...f.agents],
      places: ['tank', `mine:${mine.id}`, ...(gate ? [`gate:${gate.id}`] : [])],
    })
  }
  if (opts.saved && opts.saved > 0) {
    const pg = CORE.piggy
    const x = pg.i - 0.25
    tokens.push({
      key: 'saved',
      kind: 'saved',
      path: [...route(SPOUT, [x, 1.7]), [x, pg.j + pg.d / 2], [pg.i + 0.15, pg.j + pg.d / 2]],
      spend: opts.saved,
      nodes: [],
      places: ['tank', 'piggy', 'orb'],
    })
  }
  const harnessNodes = (kind: string) =>
    e.harnesses.filter((h) => h.harness === kind).map((h) => h.id)
  for (const x of xmines) {
    const m = mines.find((y) => y.id === x.id)
    if (!m || !m.shared || x.spend <= 0) continue
    const door = tentDoor(m)
    tokens.push({
      key: `shared:${m.id}`,
      kind: 'shared',
      path: [door, [door[0], LANE_J], [XSPOUT[0], LANE_J], XSPOUT],
      spend: x.spend,
      nodes: [m.id, x.id],
      places: [`mine:${m.id}`, `xmine:${x.id}`, 'xtank', 'outpost'],
    })
  }
  for (const h of harnesses)
    if (h.spend > 0) {
      const at: Tile = [h.i - 0.2, h.j + h.d * 0.45]
      tokens.push({
        key: `outside:${h.kind}`,
        kind: 'outside',
        path: [XSPOUT, [XSPOUT[0], LANE_J], [at[0], LANE_J], at],
        spend: h.spend,
        nodes: [...harnessNodes(h.kind), ...h.providers],
        places: ['xtank', 'outpost', `harness:${h.kind}`],
      })
    }

  // The outside observatory: coding harnesses, by kind.
  const hSpend = e.harnesses.reduce((n, h) => n + (h.spend_usd ?? 0), 0)
  const outpost: Outpost | null = e.harnesses.length
    ? {
        spend: hSpend,
        seats: e.harnesses.length,
        byKind: kindList.map((k) => ({ kind: k.kind, spend: k.spend })),
        weather: weatherOf(
          e.harnesses.flatMap((h) => ps(h.id)),
          e.harnesses.map((h) => h.id),
        ),
      }
    : null
  // The inside observatory: the product's own agents, as their traces report them.
  let runs = 0
  let ok = 0
  for (const a of e.agents)
    if (a.success_pct !== null && a.runs > 0) {
      runs += a.runs
      ok += (a.runs * a.success_pct) / 100
    }

  return {
    teams,
    mines,
    xmines,
    gates,
    mcps,
    tools,
    cars,
    harnesses,
    yards,
    workflows: [...e.workflows].sort((a, b) => b.runs - a.runs).map(railOf),
    people,
    outpost,
    watch: { runs, success: runs ? (ok / runs) * 100 : null, agents: e.agents.length },
    core: CORE,
    tokens,
    vacant,
    districts,
    unlimited,
    hud: hudOf(
      v,
      agents.map((x) => ({ id: x.a.id, pose: x.pose })),
      !!opts.mine,
    ),
    log: logOf(v),
  }
}

function hudOf(v: EstateView, poses: readonly { id: string; pose: Pose }[], mine: boolean): Hud {
  const e = v.estate
  const mineOnly = (id: string) => !mine || v.mine.has(id)
  const counts: Record<Pose, number> = { live: 0, idle: 0, hurt: 0, down: 0 }
  for (const t of poses) if (mineOnly(t.id)) counts[t.pose]++
  let runs = 0
  let ok = 0
  for (const a of e.agents)
    if (mineOnly(a.id) && a.success_pct !== null && a.runs > 0) {
      runs += a.runs
      ok += (a.runs * a.success_pct) / 100
    }
  const budgets = e.groups.flatMap((g) => (g.budget ? [g.budget] : []))
  const own = e.allowance
  const budget = mine
    ? own && own.limit_usd > 0
      ? {
          limit: own.limit_usd,
          spent: own.spent_usd,
          left: Math.max(0, own.limit_usd - own.spent_usd),
          pace: own.projected_usd / own.limit_usd,
        }
      : null
    : budgets.length
      ? (() => {
          const limit = budgets.reduce((n, b) => n + b.limit_usd, 0)
          const spent = budgets.reduce((n, b) => n + b.spent_usd, 0)
          const projected = budgets.reduce((n, b) => n + b.projected_usd, 0)
          return {
            limit,
            spent,
            left: Math.max(0, limit - spent),
            pace: limit > 0 ? projected / limit : 0,
          }
        })()
      : null
  const funded = new Set(e.groups.filter((g) => g.budget).map((g) => g.id))
  const unbudgeted = mine
    ? 0
    : e.people.reduce((n, p) => n + (p.group_id && funded.has(p.group_id) ? 0 : p.spend_usd), 0)
  // A budget counts its team's coding-harness spend but can only alert on it (V58).
  const uncapped = mine ? 0 : budgets.reduce((n, b) => n + b.harness_usd, 0)
  const days = mine ? null : e.daily
  const trend = days?.length
    ? {
        spent: days.map((d) => d.budgeted_usd),
        ungoverned: days.map((d) => d.unbudgeted_usd + d.uncapped_usd),
        saved: days.map((d) => d.saved_usd),
      }
    : null
  const direct = new Set(e.routers.filter((r) => r.direct).map((r) => r.id))
  let routed = 0
  let all = 0
  for (const a of e.agents) {
    if (!mineOnly(a.id)) continue
    all += a.spend_usd
    if (a.router_id && !direct.has(a.router_id)) routed += a.spend_usd
  }
  for (const h of e.harnesses) if (mineOnly(h.id)) all += h.spend_usd ?? 0
  return {
    counts,
    reliability: runs ? (ok / runs) * 100 : null,
    budget,
    unbudgeted,
    uncapped,
    ungoverned: unbudgeted + uncapped,
    routedShare: all > 0 ? routed / all : null,
    trend,
  }
}

function logOf(v: EstateView): Record<LogKind, LogItem[]> {
  const problem = (p: Problem): LogItem => ({
    key: p.id,
    kind: p.tier === 'broken' ? 'fix' : 'watch',
    title: p.title,
    detail: p.why,
    anchor: p.entities[0] ?? null,
    sel: `problem:${p.id}`,
    problem: p,
  })
  const requests: LogItem[] = (v.live ? v.requests : []).map((r) => ({
    key: r.id,
    kind: 'approve',
    title: r.title || r.kind,
    detail: r.kind,
    anchor: r.nodes[0] ?? null,
    sel: r.nodes[0] ? `node:${r.nodes[0]}` : null,
    request: r,
  }))
  const opps: LogItem[] = v.estate.opportunities
    .filter((o) => v.view.scope === 'org' || o.entities.some((x) => v.mine.has(x)))
    .map((o) => ({
      key: o.id,
      kind: 'upgrade',
      title: o.title,
      detail: o.detail,
      anchor: o.entities[0] ?? null,
      sel: `opp:${o.id}`,
      opp: o,
    }))
  const items = v.problems.map(problem)
  return {
    fix: items.filter((x) => x.kind === 'fix'),
    approve: requests,
    watch: items.filter((x) => x.kind === 'watch'),
    upgrade: opps,
  }
}

/** Kiko's state: a flare for something broken, a raised hand for a decision, a sign for a warning, else calm. */
export type Mood = 'flare' | 'hand' | 'sign' | 'calm'
export function moodOf(log: Village['log']): Mood {
  if (log.fix.length) return 'flare'
  if (log.approve.length) return 'hand'
  if (log.watch.length) return 'sign'
  return 'calm'
}

/** What Kiko walks through: everything broken, every request, then the top three warnings. */
export const guideItems = (log: Village['log']) => [
  ...log.fix,
  ...log.approve,
  ...log.watch.slice(0, 3),
]
