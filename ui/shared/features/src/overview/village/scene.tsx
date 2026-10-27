/**
 * The village as drawable entities: each place's art, its selection (`sel`), the estate nodes it stands for (what a
 * selection elsewhere keeps lit), its depth order, its anchor for links, labels and the zoom-in, and its label chip.
 * Selecting a place opens the insight panel on what it stands for: a team house its team, a tent its provider, a router
 * its router, an MCP gateway or tool box its tool, a piece its agent, a coding tool's ball, the dirt plot's battery or
 * its eye tower the coding harnesses, the barn the agents with errors. The town hall, the battery, the piggy bank, the
 * opportunities observatory, the globe, the walking eye, the clock tower and the project plots have panels of their
 * own (`place:<id>`).
 */
import type { ReactNode } from 'react'
import { anchorOf, focusOf, parseSel } from '../map/selection'
import type { EstateView } from '../estate/useEstateView'
import {
  Battery,
  Bush,
  EyeArt,
  GateArt,
  MineArt,
  OpportunityArt,
  PiggyArt,
  Smoke,
  Storm,
  TeamArt,
  Townhall,
  Tree,
  WalkingEye,
} from './art'
import { villageCopy as copy } from './copy'
import { P, up, type Pt } from './geo'
import { EYE_WALK } from './zones'
import {
  BarnArt,
  BrickStack,
  ClockTower,
  GlobeArt,
  HarnessBall,
  McpHouse,
  ProjectPlot,
  ToolBox,
} from './places'
import { BUSHES, TREES } from './scenery'
import type { PlaceId, Village, Weather } from './model'

export type Tone = 'bad' | 'warn' | 'ok' | 'none'
export interface Chip {
  /** Swatch colour (a team's), drawn as a small square. */
  swatch?: string
  name: string
  pill?: { text: string; tone: Tone }
  muted?: string
  /** The pill after the name (a team's pace), not before it (a mine's weather). */
  pillAfter?: boolean
  /** Shown whatever the labels setting, unless labels are off. */
  always: boolean
}

export interface Entity {
  /** Which kind of place (scenery has none). */
  kind?: string
  key: string
  sel: string
  label: string
  /** Estate nodes it stands for. */
  nodes: string[]
  depth: number
  foot: { i: number; j: number; w: number; d: number }
  top: Pt
  art: ReactNode
  fx?: ReactNode
  chip?: Chip
}

/** Where a place's chip and links hang, in px above its middle (the reference's heights). */
const HEIGHT = {
  townhall: 86,
  tank: 100,
  xtank: 100,
  orchestrator: 92,
  observatory: 82,
  outpost: 82,
  piggy: 52,
  orb: 56,
  team: 44,
  mine: 54,
  xmine: 34,
  gate: 62,
  mcp: 52,
  tool: 16,
  harness: 46,
  project: 50,
  canopy: 58,
  clock: 112,
} as const

function make(
  kind: keyof typeof HEIGHT,
  s: { i: number; j: number; w: number; d: number },
  e: Omit<Entity, 'depth' | 'foot' | 'top'>,
): Entity {
  const c = P(s.i + s.w / 2, s.j + s.d / 2)
  return {
    ...e,
    kind,
    depth: s.i + s.j + (s.w + s.d) / 2,
    foot: { i: s.i, j: s.j, w: s.w, d: s.d },
    top: up(c, HEIGHT[kind]),
  }
}

const weatherPill = (w: Weather, down: string, warn: string): Chip['pill'] =>
  w === 'down'
    ? { text: `✕ ${down}`, tone: 'bad' }
    : w === 'warn'
      ? { text: `▲ ${warn}`, tone: 'warn' }
      : undefined

export function buildScene(
  village: Village,
  v: EstateView,
  opts: { saved: number | null },
): Entity[] {
  const out: Entity[] = []
  const { core } = village
  const down = village.cars.filter((t) => t.pose === 'down')
  const live = village.cars.filter((t) => t.pose === 'live').length
  const b = village.hud.budget
  const harnessIds = (kind: string) =>
    v.estate.harnesses.filter((h) => h.harness === kind).map((h) => h.id)

  const th = core.townhall
  out.push(
    make('townhall', th, {
      key: 'townhall',
      sel: 'place:townhall',
      label: copy.place.townhall,
      nodes: [],
      art: <Townhall i={th.i} j={th.j} />,
      chip: { name: copy.place.townhall, always: true },
    }),
    make('tank', core.tank, {
      key: 'tank',
      sel: 'place:tank',
      label: copy.place.tank,
      nodes: [],
      art: (
        <Battery
          ci={core.tank.i + 1.06}
          cj={core.tank.j + 1.06}
          r={1.06}
          h={82}
          left={b && b.limit ? b.left / b.limit : 0}
          over={!!b && b.pace > 1}
          spout="left"
        />
      ),
      chip: b
        ? {
            name: copy.chip.left(b.left),
            pill: b.pace > 1 ? { text: `▲ ${copy.chip.pace(b.pace)}`, tone: 'bad' } : undefined,
            pillAfter: true,
            always: true,
          }
        : { name: copy.place.tank, muted: copy.chip.noBudget, always: true },
    }),
    make('piggy', core.piggy, {
      key: 'piggy',
      sel: 'place:piggy',
      label: copy.place.piggy,
      nodes: [],
      art: <PiggyArt i={core.piggy.i} j={core.piggy.j} full={!!opts.saved} />,
      chip: opts.saved
        ? { name: copy.chip.saved(opts.saved), always: true }
        : { name: copy.chip.fills, always: true },
    }),
    make('orb', core.orb, {
      key: 'orb',
      sel: 'place:orb',
      label: copy.place.orb,
      nodes: [],
      art: <OpportunityArt i={core.orb.i} j={core.orb.j} on={village.log.upgrade.length > 0} />,
      chip: {
        name: copy.place.orb,
        muted: village.log.upgrade.length ? copy.chip.ideas(village.log.upgrade.length) : undefined,
        always: false,
      },
    }),
    make('observatory', core.observatory, {
      key: 'observatory',
      sel: 'place:observatory',
      label: copy.place.observatory,
      nodes: [],
      art: <WalkingEye points={EYE_WALK.map(([i, j]) => P(i, j))} />,
      chip: { name: copy.place.observatory, always: false },
    }),
    make('orchestrator', core.orchestrator, {
      key: 'orchestrator',
      sel: 'place:orchestrator',
      label: `${copy.place.orchestrator}, ${copy.chip.agents(village.cars.length)}`,
      nodes: [],
      art: <GlobeArt i={core.orchestrator.i} j={core.orchestrator.j} lit={live > 0} />,
      chip: {
        name: copy.place.orchestrator,
        muted: copy.chip.agents(village.cars.length),
        always: false,
      },
    }),
    make('canopy', core.barn, {
      key: 'canopy',
      sel: 'panel:agents:errors',
      label: copy.place.service,
      nodes: down.map((t) => t.id),
      art: <BarnArt down={down.length} />,
      chip: down.length
        ? {
            name: copy.place.service,
            pill: { text: `✕ ${copy.chip.down(down.length)}`, tone: 'bad' },
            always: true,
          }
        : { name: copy.place.service, always: false },
    }),
    make('clock', core.clock, {
      key: 'workflows',
      sel: 'place:workflows',
      label: copy.place.workflows,
      nodes: village.workflows.map((w) => w.id),
      art: <ClockTower running={village.workflows.some((w) => w.runs > 0)} />,
      chip: {
        name: copy.place.workflows,
        muted: copy.chip.workflows(village.workflows.length),
        always: false,
      },
    }),
  )

  for (const t of village.teams)
    out.push(
      make('team', t, {
        key: `team:${t.id}`,
        sel: `node:${t.id}`,
        label: `${t.name} ${copy.place.team}`,
        nodes: [t.id, ...village.people.filter((p) => p.at === t.id).map((p) => p.id)],
        art: <TeamArt t={t} />,
        chip: {
          swatch: t.color,
          name: t.name,
          pill:
            t.pace === null
              ? { text: copy.chip.noBudget, tone: 'bad' } // ungoverned (V58), like its glow
              : {
                  text: `${t.pace > 1 ? '▲ ' : ''}${copy.chip.pace(t.pace)}`,
                  tone: t.pace > 1 ? 'bad' : 'ok',
                },
          pillAfter: true,
          always: true,
        },
      }),
    )

  for (const m of [...village.mines, ...village.xmines]) {
    const kind = m.outside ? 'xmine' : 'mine'
    const [x, y] = up(P(m.i + m.w / 2, m.j + m.d / 2), HEIGHT[kind] + 26)
    out.push(
      make(kind, m, {
        key: `${kind}:${m.id}`,
        sel: `node:${m.id}`,
        label: `${m.name} ${m.outside ? copy.place.xmine : copy.place.mine}`,
        nodes: [m.id, ...m.models.map((x) => x.id)],
        art: m.outside ? <BrickStack i={m.i} j={m.j} w={m.w} d={m.d} /> : <MineArt m={m} />,
        fx: m.weather === 'ok' ? undefined : <Storm x={x} y={y} dark={m.weather === 'down'} />,
        chip: {
          name: m.name,
          pill: weatherPill(m.weather, copy.chip.outage, copy.chip.slow),
          muted: fmtSpend(m.spend),
          always: true,
        },
      }),
    )
  }

  for (const g of village.gates)
    out.push(
      make('gate', g, {
        key: `gate:${g.id}`,
        sel: `node:${g.id}`,
        label: `${copy.place.gate}, ${g.name}`,
        nodes: [g.id, ...village.people.filter((p) => p.at === g.id).map((p) => p.id)],
        art: <GateArt i={g.i} j={g.j} w={g.w} d={g.d} />,
        chip: {
          name: g.name,
          pill: weatherPill(g.weather, copy.chip.failing, copy.chip.slow),
          always: g.weather !== 'ok',
        },
      }),
    )

  for (const s of village.mcps) {
    const [x, y] = up(P(s.i + 0.7, s.j + 0.7), 50)
    out.push(
      make('mcp', s, {
        key: `mcp:${s.id}`,
        sel: `node:${s.id}`,
        label: `${copy.place.mcp}, ${s.name}`,
        nodes: [s.id],
        art: <McpHouse i={s.i} j={s.j} weather={s.weather} />,
        fx: s.weather === 'ok' ? undefined : <Smoke x={x} y={y} dark={s.weather === 'down'} />,
        chip: {
          name: s.name,
          pill: s.weather === 'down' ? { text: `▲ ${copy.chip.failing}`, tone: 'bad' } : undefined,
          always: s.weather === 'down',
        },
      }),
    )
  }

  for (const h of village.tools) {
    const [x, y] = up(P(h.i + 0.37, h.j + 0.37), 16)
    out.push(
      make('tool', h, {
        key: `tool:${h.id}`,
        sel: `node:${h.id}`,
        label: `${copy.place.tool}, ${h.name}`,
        nodes: [h.id],
        art: <ToolBox i={h.i} j={h.j} brand={h.brand} name={h.name} />,
        fx: h.weather === 'ok' ? undefined : <Smoke x={x} y={y} dark={h.weather === 'down'} />,
        chip: {
          name: h.name,
          pill: h.weather === 'down' ? { text: `▲ ${copy.chip.failing}`, tone: 'bad' } : undefined,
          always: h.weather === 'down',
        },
      }),
    )
  }

  for (const h of village.harnesses)
    out.push(
      make('harness', h, {
        key: `harness:${h.kind}`,
        sel: 'panel:harnesses',
        label: `${copy.harnessName(h.kind)}, ${copy.chip.seats(h.seats)}`,
        nodes: [...harnessIds(h.kind), ...h.providers],
        art: <HarnessBall h={h} />,
        chip: {
          name: copy.harnessName(h.kind),
          muted: fmtSpend(h.spend),
          pill: weatherPill(h.weather, copy.chip.failing, copy.chip.slow),
          always: true,
        },
      }),
    )

  const o = village.outpost
  const harnessNodes = [
    ...v.estate.harnesses.map((h) => h.id),
    ...v.estate.routers.filter((r) => r.direct).map((r) => r.id),
  ]
  if (o) {
    const [ex, ey] = P(core.outpost.i + 0.4, core.outpost.j + 0.4)
    out.push(
      make('outpost', core.outpost, {
        key: 'outpost',
        sel: 'panel:harnesses',
        label: copy.place.outpost,
        nodes: harnessNodes,
        art: (
          <g transform={`translate(${ex.toFixed(1)},${ey.toFixed(1)})`}>
            <EyeArt look={[16, -4]} />
          </g>
        ),
        chip: { name: copy.place.outpost, always: false },
      }),
      make('xtank', core.xtank, {
        key: 'xtank',
        sel: 'panel:harnesses',
        label: copy.place.xtank,
        nodes: harnessNodes,
        art: (
          <Battery
            ci={core.xtank.i + 1.06}
            cj={core.xtank.j + 1.06}
            r={1.06}
            h={82}
            left={0.62}
            spout="left"
          />
        ),
        chip: {
          name: copy.chip.codingTools,
          muted: fmtSpend(o.spend),
          pill: { text: copy.chip.noCap, tone: 'bad' }, // ungoverned (V58), like its glow
          pillAfter: true,
          always: true,
        },
      }),
    )
  }

  for (const y of village.yards)
    out.push(
      make('project', y, {
        key: `yard:${y.id}`,
        sel: `place:yard:${y.id}`,
        label: y.unfiled ? copy.place.unfiled : `${y.name} ${copy.place.project}`,
        nodes: [y.id, ...y.rails.map((r) => r.id), ...y.agents],
        art: <ProjectPlot y={y} />,
        chip: {
          swatch: y.unfiled ? undefined : y.org ? '#e5513b' : (y.color ?? undefined),
          name: y.unfiled ? copy.place.unfiled : y.name,
          muted: copy.chip.workflows(y.rails.length),
          always: !y.unfiled,
        },
      }),
    )

  const scenery = (key: string, i: number, j: number, art: ReactNode) =>
    out.push({
      key,
      sel: '',
      label: '',
      nodes: [],
      depth: i + j + 0.2,
      foot: { i: 0, j: 0, w: 0, d: 0 },
      top: [0, 0],
      art,
    })
  TREES.forEach((t, k) => scenery(`tree${k}`, t[0], t[1], <Tree t={t} />))
  BUSHES.forEach((t, k) => scenery(`bush${k}`, t[0], t[1], <Bush b={t} />))
  return out.sort((a, b) => a.depth - b.depth)
}

const fmtSpend = (v: number) =>
  v >= 1 ? `$${Math.round(v).toLocaleString('en-US')}` : v > 0 ? '<$1' : '$0'

/** A village place in a `place:` selection: one of `PLACES`, or a project yard (`yard:<id>`). */
export function placeOf(sel: string | undefined): PlaceId | `yard:${string}` | null {
  const p = /^place:(.+)$/.exec(sel ?? '')?.[1]
  if (!p) return null
  if (p.startsWith('yard:') && p.length > 5) return p as `yard:${string}`
  return (
    (
      ['townhall', 'tank', 'piggy', 'orb', 'orchestrator', 'observatory', 'workflows'] as const
    ).find((x) => x === p) ?? null
  )
}

/** Which place a selection is about (the one drawn selected), and what it keeps lit (null: everything). */
export function focusFor(
  sel: string | undefined,
  v: EstateView,
  scene: readonly Entity[],
  village: Village,
): { key: string | null; keep: Set<string> | null } {
  if (!sel) return { key: null, keep: null }
  const place = placeOf(sel)
  if (place) {
    const keep = new Set<string>()
    if (place.startsWith('yard:')) {
      const y = village.yards.find((x) => `yard:${x.id}` === place)
      y?.agents.forEach((a) => keep.add(a))
      return { key: place, keep }
    }
    if (place === 'townhall') {
      village.teams.forEach((c) => keep.add(c.id))
      village.yards.forEach((y) => keep.add(y.id))
    } else if (place === 'orb') {
      // What the opportunities are about.
      village.log.upgrade.forEach((o) => o.opp?.entities.forEach((x) => keep.add(x)))
    } else if (place === 'workflows') {
      village.yards.forEach((y) => {
        keep.add(y.id)
        y.agents.forEach((a) => keep.add(a))
      })
    } else if (place === 'orchestrator' || place === 'observatory')
      village.cars.forEach((t) => keep.add(t.id))
    else village.teams.forEach((c) => keep.add(c.id))
    return { key: place, keep }
  }
  const direct = scene.find((x) => x.sel === sel)
  const s = parseSel(sel)
  const keep = s ? focusOf(v.estate, v.nodes, s) : null
  if (direct)
    return { key: direct.key, keep: s && s.type === 'panel' ? new Set(direct.nodes) : keep }
  const anchor = s ? anchorOf(v.estate, s) : null
  if (anchor && village.cars.some((t) => t.id === anchor)) return { key: `car:${anchor}`, keep }
  // A model's mine, a person's team: the first place that stands for it.
  const owner = anchor ? scene.find((x) => x.sel && x.nodes.includes(anchor)) : undefined
  return { key: owner?.key ?? null, keep }
}

/** A place is dimmed when a selection keeps other things lit, or when the view's scope or filters hide it. */
export function isDim(
  e: Entity,
  focus: { key: string | null; keep: Set<string> | null },
  v: EstateView,
): boolean {
  if (!e.sel) return false
  if (focus.key === e.key) return false
  const keep = focus.keep
  if (keep && !e.nodes.some((n) => keep.has(n))) return true
  const s = e.nodes[0] ? v.state.get(e.nodes[0]) : undefined
  return s === 'ghost' || s === 'muted'
}
