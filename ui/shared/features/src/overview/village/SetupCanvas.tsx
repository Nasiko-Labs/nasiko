/**
 * The setup plot (plans/feat-village-overview.md §3c, §3d): the village's own ground, empty, on which the setup guide
 * builds one site per step: the town hall, the coding harnesses' observatory on the outside plot, a team and a project
 * yard, the token tank, a provider's mine, the router gate (when the person wants one), the orchestrator's depot with
 * the first agent's car, the inside observatory, and the orb with the piggy bank. The guide
 * (`features/onboarding/VillageGuide.tsx`) owns the route, what Kiko says and the forms; this owns the drawing, the
 * camera, the scaffolding and the celebrations.
 *
 * Sites stand where the village puts them (`zones.ts`), so the plot grows into the Overview's village as it is. The
 * camera follows the step's site; a click on a site eases in closer (SELECT_ZOOM, as in the village) and opens its
 * step, and a click on open ground eases back out to the step's view. Building takes BUILD_MS under scaffolding with a
 * progress bar; it is instant under reduced motion and before the canvas has a size (jsdom).
 */
import { X } from 'lucide-react'
import { motion, useReducedMotion } from 'motion/react'
import {
  useEffect,
  useEffectEvent,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import { useViewport, type ViewState } from '../map/useViewport'
import {
  Battery,
  Bush,
  EyeArt,
  Flag,
  GateArt,
  KikoArt,
  Loop,
  MineArt,
  OpportunityArt,
  PiggyArt,
  TeamArt,
  Townhall,
  Tree,
} from './art'
import { villageCopy } from './copy'
import { P, toMap, up, WORLD, type Pt } from './geo'
import { Resources, ZoomControls } from './Hud'
import type { Hud, Mine, TokenEdge } from './model'
import { MotionOn } from './motionContext'
import { ART, PROVIDER_ART, PROVIDER_FALLBACK, TEAM_COLORS } from './palette'
import { useWorld } from './worlds'
import { GlobeArt, Ground, HarnessBall, PieceSprite, PlotMark, ProjectPlot } from './places'
import { BUSHES, TREES } from './scenery'
import { Foot, Line, Shadow } from './solids'
import { DIRECT_J, GATE_STREET_I, passOf, route, SPOUT, toTent, yardDoor } from './streets'
import { CARD } from './styles'
import { SELECT_ZOOM } from './tuning'
import { BAYS, CORE, PLOTS } from './zones'

const copy = villageCopy.setup

/** How long a site stands under scaffolding. */
const BUILD_MS = 1400
const BANNER_MS = 2600

export type Site =
  | 'townhall'
  | 'outpost'
  | 'team'
  | 'yard'
  | 'tank'
  | 'mine'
  | 'gate'
  | 'depot'
  | 'observatory'
  | 'orb'
const SITES: readonly Site[] = [
  'townhall',
  'outpost',
  'team',
  'yard',
  'tank',
  'mine',
  'gate',
  'depot',
  'observatory',
  'orb',
]

/** What stands on the plot. */
export interface SetupPlot {
  townhall: boolean
  /** The role is saved: a banner flies beside the town hall. */
  banner: boolean
  /** A coding harness is connected: its kind, for the bot beside the outside observatory. */
  outpost: string | null
  /** EE: a team's name; its plot stands with its statue. */
  team: string | null
  /** A project's name; its yard stands. */
  yard: string | null
  /** A budget is set: the token tank stands full. */
  tank: boolean
  /** The lit mine; null before a provider is connected. */
  mine: { provider: string; name: string } | null
  /** Calls go through the LLM router: the gate stands. */
  gate: boolean
  /** One model is pinned: calls take the direct lane past the gates. */
  pinned: boolean
  depot: boolean
  /** The first agent was started in this visit: its car waits amber while it builds. */
  firstAgent: boolean
  /** A first session ran: the car's light is green and its calls' road is drawn. */
  session: boolean
  observatory: boolean
  orb: boolean
}

export interface Celebration {
  /** A new key shows the banner again. */
  key: number
  title: string
  line: string
}

type Foot4 = { i: number; j: number; w: number; d: number }
/** Each site's ground and height (for the camera, the chip and the scaffolding). */
const GATE = PLOTS.gate[0]
const MINE = PLOTS.mine[0]
const YARD = PLOTS.project[0]
const TEAM = PLOTS.team[0]
const BOT = PLOTS.harness[0]
const OBS = CORE.observatory
const OUTPOST = CORE.outpost
// The first bay: the back row's, nearest the globe.
const BAY = BAYS[0]
const FOOT: Record<Site, Foot4 & { h: number }> = {
  townhall: { ...CORE.townhall, h: 86 },
  outpost: { ...OUTPOST, h: 82 },
  team: { ...TEAM, h: 44 },
  yard: { ...YARD, h: 50 },
  tank: { ...CORE.tank, h: 100 },
  mine: { ...MINE, h: 54 },
  gate: { ...GATE, h: 58 },
  depot: { ...CORE.orchestrator, h: 92 },
  observatory: { ...OBS, h: 82 },
  orb: { ...CORE.orb, h: 56 },
}
const centerOf = (s: Site): Pt => {
  const f = FOOT[s]
  return up(P(f.i + f.w / 2, f.j + f.d / 2), f.h / 2)
}
function standingSet(p: SetupPlot): Set<Site> {
  const on: Record<Site, boolean> = {
    townhall: p.townhall,
    outpost: p.outpost !== null,
    team: p.team !== null,
    yard: p.yard !== null,
    tank: p.tank,
    mine: p.mine !== null,
    gate: p.gate,
    depot: p.depot,
    observatory: p.observatory,
    orb: p.orb,
  }
  return new Set(SITES.filter((s) => on[s]))
}
/** A box of tiles in map units, with room above for the tallest building. */
function boxOf(tiles: readonly Foot4[]) {
  const c = tiles.flatMap((f) =>
    [P(f.i, f.j), P(f.i + f.w, f.j), P(f.i, f.j + f.d), P(f.i + f.w, f.j + f.d)].map(toMap),
  )
  const xs = c.map((p) => p[0])
  const ys = c.map((p) => p[1])
  const x0 = Math.min(...xs)
  const y0 = Math.min(...ys) - 110
  return { x0, y0, w: Math.max(...xs) - x0, h: Math.max(...ys) - y0 }
}
/** The island's north part: the plot the setup always frames. */
const BASE: Foot4 = { i: 0, j: 0, w: 19, d: 11 }
/** The whole plot as it stands: the north half, plus the yards or the outside plot once something stands there or
 *  the step builds there. */
const regionOf = (p: SetupPlot, focus: Site | 'all') =>
  boxOf([
    BASE,
    ...[...standingSet(p)].map((s) => FOOT[s]),
    ...(focus === 'all' ? [] : [FOOT[focus]]),
  ])

/** Confetti, fixed per piece so a render stays pure. */
const CONFETTI = Array.from({ length: 42 }, (_, k) => ({
  id: `c${k}`,
  left: 8 + ((k * 37) % 84),
  dx: ((k * 53) % 160) - 80,
  rot: ((k * 97) % 720) - 360,
  delay: ((k * 7) % 35) / 100,
  color: ['#ffd75e', '#f3a521', '#8b5cf6', '#3fbf6f', '#4c8dff', '#e5484d'][k % 6],
}))

export function SetupCanvas({
  plot,
  building,
  onBuilt,
  next,
  focus,
  clickable,
  onSite,
  kiko,
  top,
  celebrate,
}: {
  plot: SetupPlot
  /** The sites going up now; `onBuilt` fires once they stand. */
  building: readonly Site[]
  onBuilt(): void
  /** The site the step builds next: a green plot that does what the step's primary action does. */
  next: Site | null
  /** The step's site: the whole plot is framed with it in view. */
  focus: Site | 'all'
  /** Sites a click opens. */
  clickable: readonly Site[]
  onSite(s: Site): void
  kiko: {
    mood: 'calm' | 'hand'
    label: string
    open: boolean
    onToggle(): void
    /** What Kiko says, with the step's form: everything the setup asks happens here (the prototype's bubble). */
    bubble: ReactNode
  }
  /** The progress card hanging from the top edge, in place of the village's view switch, log and settings. */
  top: ReactNode
  celebrate: Celebration | null
}) {
  const theme = useWorld()
  const reduced = !!useReducedMotion()
  const wrap = useRef<HTMLDivElement>(null)
  const zone = useRef<HTMLDivElement>(null)
  const [height, setHeight] = useState<number | null>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const el = wrap.current
    if (!el) return
    // As the village: the canvas fills the window below it, between 560 and 940 px.
    const measure = () => {
      setWidth(el.clientWidth)
      const room = window.innerHeight - el.getBoundingClientRect().top - 16
      setHeight(Math.round(Math.min(940, Math.max(560, Number.isFinite(room) ? room : 680))))
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const sized = width > 0
  const animate = sized && !reduced
  const vp = useViewport(zone, WORLD, 0, { anywhere: true })

  // ── Building: the guide says what goes up; this says when it stands. ──
  const built = useEffectEvent(onBuilt)
  const busy = building.length > 0
  useEffect(() => {
    if (!busy) return
    const t = setTimeout(built, animate ? BUILD_MS : 0)
    return () => clearTimeout(t)
  }, [busy, animate])

  // ── The camera: the whole plot (with the step's site in it), closer on a click, back out on open ground. ──
  const [target, setTarget] = useState<{ site: Site | 'all'; zoom: number }>({
    site: 'all',
    zoom: 1,
  })
  const region = regionOf(plot, focus)
  const regionKey = `${region.x0.toFixed(0)},${region.y0.toFixed(0)},${region.w.toFixed(0)},${region.h.toFixed(0)}`
  const look = useEffectEvent((t: typeof target, instant: boolean) => {
    const el = zone.current
    if (!el || !el.clientWidth) return
    const fit = Math.min((el.clientWidth - 64) / region.w, (el.clientHeight - 120) / region.h)
    const z = t.site === 'all' ? fit : fit * t.zoom
    const pt: Pt =
      t.site === 'all'
        ? [region.x0 + region.w / 2, region.y0 + region.h / 2]
        : toMap(centerOf(t.site))
    // A little under the progress card.
    vp.center(pt[0], pt[1] - 20 / z, z, instant || reduced)
  })
  // The first framing jumps; later ones ease.
  const first = useRef(true)
  useEffect(() => {
    if (!sized) return
    look(target, first.current)
    first.current = false
    // A resized canvas, or a plot that grew (the outside plot, the yards), frames again.
  }, [sized, target, regionKey, width, height])

  const openSite = (s: Site) => {
    if (s !== next && !clickable.includes(s)) return
    setTarget({ site: s, zoom: SELECT_ZOOM })
    onSite(s)
  }
  const whole = () => setTarget({ site: 'all', zoom: 1 })

  // Escape eases back to the whole plot.
  const escape = useEffectEvent((e: globalThis.KeyboardEvent) => {
    if (e.key !== 'Escape' || target.site === 'all') return
    whole()
    e.preventDefault()
  })
  useEffect(() => {
    const el = wrap.current
    if (!el) return
    el.addEventListener('keydown', escape)
    return () => el.removeEventListener('keydown', escape)
  }, [])

  // ── The banner: shown on each new celebration, gone after BANNER_MS. ──
  const [lastKey, setLastKey] = useState(celebrate?.key ?? null)
  const [banner, setBanner] = useState<Celebration | null>(null)
  if (celebrate && celebrate.key !== lastKey) {
    setLastKey(celebrate.key)
    setBanner(celebrate)
  }
  useEffect(() => {
    if (!banner) return
    const t = setTimeout(() => setBanner(null), BANNER_MS)
    return () => clearTimeout(t)
  }, [banner])

  const items = drawables(plot, building, next)
  const chips = chipsOf(plot, building, next)
  const hud = hudOf(plot)

  return (
    <MotionOn.Provider value={animate}>
      <div className="@container/village" data-testid="village-setup">
        <div
          ref={wrap}
          className="relative overflow-hidden rounded-xl border bg-card bg-[radial-gradient(var(--border)_1.1px,transparent_1.2px)] [background-size:22px_22px]"
          style={{ ...(height ? { height } : null), ...theme.backdrop }}
        >
          <div
            ref={zone}
            // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- the plot pans and zooms from the keyboard, so it takes focus
            tabIndex={0}
            role="application"
            aria-label={copy.instructions}
            className="absolute inset-0 cursor-grab touch-none outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset data-[dragging]:cursor-grabbing"
          >
            <svg
              className="absolute inset-0 size-full select-none"
              onClick={(e) => {
                if (!(e.target as Element).closest('[role="button"]')) whole()
              }}
            >
              <g transform={`translate(${WORLD.ox},${WORLD.oy})`}>
                <g aria-hidden>
                  <Ground tokens={tokensOf(plot)} grid={false} />
                </g>
                {items.map((d) =>
                  // Only a site that does something is a button: the next plot, or one that stands.
                  d.site && (d.site === next || clickable.includes(d.site)) ? (
                    <g
                      key={d.key}
                      role="button"
                      tabIndex={0}
                      aria-label={
                        d.site === next
                          ? copy.next(copy.site[d.site])
                          : copy.open(copy.site[d.site])
                      }
                      data-site={d.key}
                      className="cursor-pointer outline-none hover:brightness-105 focus-visible:brightness-110"
                      onClick={() => openSite(d.site as Site)}
                      onKeyDown={activate(() => openSite(d.site as Site))}
                    >
                      {d.art}
                    </g>
                  ) : (
                    <g key={d.key} aria-hidden>
                      {d.art}
                    </g>
                  ),
                )}
              </g>
            </svg>
          </div>
          <SetupChips chips={chips} view={vp} />

          {/* The village's HUD, empty until the setup fills it: the meters top right. */}
          <div className="absolute top-3 right-3 z-10 hidden @[760px]/village:block">
            <Resources hud={hud} saved={null} mine={false} onPlace={whole} />
          </div>
          <div className="absolute top-0 left-1/2 z-10 -translate-x-1/2">{top}</div>

          <div className="absolute bottom-2 left-3 z-10 flex items-end gap-2.5">
            <Button
              variant="ghost"
              className="relative h-24 w-20 shrink-0 rounded-2xl border border-black/5 bg-white p-1.5 shadow-md hover:bg-white"
              aria-label={kiko.label}
              aria-expanded={kiko.open}
              onClick={kiko.onToggle}
            >
              <KikoArt mood={kiko.mood} />
            </Button>
            {kiko.open ? (
              <div
                data-vp-ignore=""
                role="group"
                aria-label={copy.kiko}
                className={cn(
                  CARD,
                  'relative mb-3 flex w-[min(400px,calc(100vw-140px))] flex-col overflow-hidden bg-card text-sm shadow-md',
                )}
                // It grows up from the corner, leaving room for the progress card at the top.
                style={height ? { maxHeight: height - 104 } : undefined}
              >
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="absolute top-2 right-2 z-10 text-muted-foreground"
                  aria-label={copy.hideKiko}
                  onClick={kiko.onToggle}
                >
                  <X aria-hidden />
                </Button>
                <div className="min-h-0 overflow-y-auto p-4 pt-3">
                  <p className="mb-1 text-xs text-muted-foreground">{copy.kiko}</p>
                  {kiko.bubble}
                </div>
              </div>
            ) : null}
          </div>

          <div className="absolute right-3 bottom-3 z-10">
            <ZoomControls onZoomIn={vp.zoomIn} onZoomOut={vp.zoomOut} onFit={whole} />
          </div>

          {banner ? <Banner key={banner.key} banner={banner} confetti={animate} /> : null}
        </div>
      </div>
    </MotionOn.Provider>
  )
}

/** The HUD's numbers while setting up: no agents, budgets or savings yet, so it says so. */
function hudOf(p: SetupPlot): Hud {
  return {
    counts: { live: 0, idle: 0, hurt: 0, down: 0 },
    reliability: null,
    budget: null,
    unbudgeted: 0,
    uncapped: 0,
    ungoverned: 0,
    routedShare: p.gate ? 1 : null,
    trend: null,
  }
}

function activate(fn: () => void) {
  return (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      fn()
    }
  }
}

/** The celebration: a gold banner (a status, so it is announced) and, with motion on, confetti. */
function Banner({ banner, confetti }: { banner: Celebration; confetti: boolean }) {
  return (
    <div className="pointer-events-none absolute inset-0 z-30 overflow-hidden">
      {confetti
        ? CONFETTI.map((c) => (
            <motion.i
              key={c.id}
              className="absolute -top-3 h-3 w-2 rounded-xs"
              style={{ left: `${c.left}%`, background: c.color }}
              initial={{ y: 0, x: 0, rotate: 0, opacity: 1 }}
              animate={{ y: 900, x: c.dx, rotate: c.rot, opacity: 0.2 }}
              transition={{ duration: 1.7, delay: c.delay, ease: 'easeIn' }}
            />
          ))
        : null}
      <motion.div
        role="status"
        className="absolute top-[34%] left-1/2 rounded-2xl border-2 px-6 py-3.5 text-center"
        style={{
          x: '-50%',
          y: '-50%',
          background: `linear-gradient(${ART.goldHi},${ART.gold})`,
          borderColor: ART.goldLine,
          color: '#3d2600',
          boxShadow: `0 6px 0 ${ART.goldLine}, 0 14px 30px rgba(0,0,0,.25)`,
        }}
        initial={confetti ? { scale: 0.6, opacity: 0 } : false}
        animate={{ scale: 1, opacity: 1 }}
        transition={{ type: 'spring', stiffness: 420, damping: 16 }}
      >
        <b className="block text-xl">{banner.title}</b>
        <span className="text-sm font-semibold">{banner.line}</span>
      </motion.div>
    </div>
  )
}

/* ---------- the plot's drawing ---------- */

function minePlaced(provider: string, name: string): Mine {
  const art = PROVIDER_ART[provider]
  return {
    ...MINE,
    id: `setup-mine-${provider}`,
    name,
    color: art?.color ?? PROVIDER_FALLBACK,
    mono: art?.mono ?? (name[0] ?? '?').toUpperCase(),
    spend: 0,
    models: [],
    weather: 'ok',
    outside: false,
    shared: false,
    plot: 0,
  }
}
/** The road the first session's calls took: off the parking, under the router or past it, to the provider's tent. */
function firstCall(gate: boolean): TokenEdge {
  const j = gate ? passOf(GATE) : DIRECT_J
  return {
    key: 'setup-first-call',
    kind: gate ? 'routed' : 'direct',
    path: [
      ...route([BAY.i + BAY.w / 2, BAY.j + BAY.d / 2], [GATE_STREET_I, j]),
      ...toTent(j, MINE),
    ],
    spend: 1,
    nodes: [],
    places: [],
  }
}
/** The project's budget, from the battery to its plot. */
const ALLOCATION: TokenEdge = {
  key: 'setup-allocation',
  kind: 'project',
  path: route(SPOUT, yardDoor(YARD)),
  spend: 1,
  nodes: [],
  places: [],
}
function tokensOf(p: SetupPlot): TokenEdge[] {
  const out: TokenEdge[] = []
  if (p.session && p.mine) out.push(firstCall(p.gate))
  if (p.tank && p.yard) out.push(ALLOCATION)
  return out
}

interface Drawable {
  key: string
  site: Site | null
  depth: number
  art: ReactNode
}

const depthOf = (f: { i: number; j: number; w: number; d: number }) => f.i + f.j + (f.w + f.d) / 2

/** Each site: what stands once built, how high its scaffolding is, and the plot it covers. */
function drawables(plot: SetupPlot, building: readonly Site[], next: Site | null): Drawable[] {
  const out: Drawable[] = []
  const stands = standingSet(plot)
  const add = (s: Site, key: string, f: Foot4, art: ReactNode) =>
    out.push({ key, site: s, depth: depthOf(f), art })
  /** A site: standing art, scaffolding while it builds, else its plot (green when it's next). */
  const site = (s: Site, art: () => ReactNode, scaffold: number) => {
    const f = FOOT[s]
    if (building.includes(s)) add(s, s, f, <Scaffold f={f} h={scaffold} building />)
    else if (stands.has(s)) add(s, s, f, art())
    else if (s === 'townhall')
      // Before it is raised, the town hall stands under scaffolding (the prototype's arrival).
      add(
        s,
        s,
        f,
        <>
          {next === s ? <PlotMark {...f} mode="open" /> : null}
          <Scaffold f={f} h={scaffold} building={false} />
        </>,
      )
    else add(s, s, f, <PlotMark {...f} mode={next === s ? 'open' : 'idle'} />)
  }
  const th = CORE.townhall
  site(
    'townhall',
    () => (
      <>
        <Townhall i={th.i} j={th.j} />
        {plot.banner ? <Flag p={P(th.i + 0.2, th.j + th.d + 0.3)} color="#4f9dff" /> : null}
      </>
    ),
    92,
  )
  const eye = (f: Foot4, look?: readonly [number, number]) => {
    const [x, y] = P(f.i + f.w / 2, f.j + f.d / 2)
    return (
      <g transform={`translate(${x.toFixed(1)},${y.toFixed(1)})`}>
        <EyeArt look={look} />
      </g>
    )
  }
  site('observatory', () => eye(OBS), 64)
  site('outpost', () => eye(OUTPOST, [16, -4]), 64)
  if (plot.outpost && !building.includes('outpost'))
    add('outpost', 'bot', BOT, <HarnessBall h={{ ...BOT, kind: plot.outpost }} />)
  site(
    'team',
    () => (
      <TeamArt
        t={{
          i: TEAM.i,
          j: TEAM.j,
          color: TEAM_COLORS[0],
          budget: null,
          pace: null,
        }}
      />
    ),
    44,
  )
  site('yard', () => <ProjectPlot y={{ ...YARD, org: true, color: null, unfiled: false }} />, 30)
  const tank = CORE.tank
  site(
    'tank',
    () => <Battery ci={tank.i + 1.06} cj={tank.j + 1.06} r={1.06} h={82} left={1} spout="right" />,
    90,
  )
  site(
    'mine',
    () => (plot.mine ? <MineArt m={minePlaced(plot.mine.provider, plot.mine.name)} /> : null),
    56,
  )
  site('gate', () => <GateArt i={GATE.i} j={GATE.j} w={GATE.w} d={GATE.d} />, 50)
  const o = CORE.orchestrator
  site('depot', () => <GlobeArt i={o.i} j={o.j} lit={plot.session} />, 80)
  if (stands.has('depot') && !building.includes('depot')) {
    const [x, y] = P(BAY.i + BAY.w / 2, BAY.j + BAY.d / 2)
    add(
      'depot',
      'car',
      BAY,
      <g transform={`translate(${x.toFixed(1)},${y.toFixed(1)})`}>
        <PieceSprite beacon={plot.session ? 'active' : plot.firstAgent ? 'slow' : 'running'} />
      </g>,
    )
  }
  // The opportunities observatory, its spark off (nothing to take yet), and the piggy bank, empty.
  const orb = CORE.orb
  const piggy = CORE.piggy
  site('orb', () => <OpportunityArt i={orb.i} j={orb.j} on={false} />, 60)
  if (stands.has('orb') && !building.includes('orb'))
    add('orb', 'piggy', piggy, <PiggyArt i={piggy.i} j={piggy.j} full={false} />)

  TREES.forEach((t, k) =>
    out.push({ key: `tree${k}`, site: null, depth: t[0] + t[1] + 0.2, art: <Tree t={t} /> }),
  )
  BUSHES.forEach((b, k) =>
    out.push({ key: `bush${k}`, site: null, depth: b[0] + b[1] + 0.2, art: <Bush b={b} /> }),
  )
  return out.sort((a, b) => a.depth - b.depth)
}

/** Scaffolding: four poles, three rings of planks and, while it builds, a progress bar filling over BUILD_MS. */
function Scaffold({
  f,
  h,
  building,
}: {
  f: { i: number; j: number; w: number; d: number }
  h: number
  building: boolean
}) {
  const c: Pt[] = [P(f.i, f.j), P(f.i + f.w, f.j), P(f.i + f.w, f.j + f.d), P(f.i, f.j + f.d)]
  const [bx, by] = up(P(f.i + f.w / 2, f.j + f.d / 2), h + 22)
  return (
    <>
      <Shadow i={f.i} j={f.j} w={f.w} d={f.d} />
      <Foot
        i={f.i + 0.1}
        j={f.j + 0.1}
        w={f.w - 0.2}
        d={f.d - 0.2}
        fill="rgba(168,116,63,.22)"
        stroke={ART.wood}
        dash="4 3"
      />
      {[0.3, 0.65, 1].map((k) => (
        <polygon
          key={k}
          points={c
            .map((p) =>
              up(p, h * k)
                .map((n) => n.toFixed(1))
                .join(','),
            )
            .join(' ')}
          fill="none"
          stroke={ART.plank}
          strokeWidth={2.4}
          strokeLinejoin="round"
        />
      ))}
      {c.map((p) => (
        <Line key={p.join(',')} a={p} b={up(p, h + 6)} c={ART.scaffold} w={2.8} />
      ))}
      <Line a={up(c[0] ?? [0, 0], h * 0.3)} b={up(c[1] ?? [0, 0], h * 0.65)} c={ART.post} w={1.6} />
      <Line a={up(c[3] ?? [0, 0], h * 0.3)} b={up(c[2] ?? [0, 0], h * 0.65)} c={ART.post} w={1.6} />
      {building ? (
        <g>
          <rect x={bx - 31} y={by - 4} width={62} height={9} rx={4.5} fill="#3b2a1a" />
          <rect x={bx - 29} y={by - 2} width={58} height={5} rx={2.5} fill={ART.gold}>
            <Loop>
              <animate attributeName="width" from="0" to="58" dur={`${BUILD_MS}ms`} fill="freeze" />
            </Loop>
          </rect>
        </g>
      ) : null}
    </>
  )
}

/* ---------- chips ---------- */

interface SetupChip {
  key: string
  top: Pt
  name: string
  muted?: string
  tone?: 'next' | 'busy'
}

function chipsOf(plot: SetupPlot, building: readonly Site[], next: Site | null): SetupChip[] {
  const at = (s: Site, lift = 0): Pt => {
    const f = FOOT[s]
    return up(P(f.i + f.w / 2, f.j + f.d / 2), f.h + lift)
  }
  const stands = standingSet(plot)
  const out: SetupChip[] = []
  for (const s of SITES) {
    if (building.includes(s))
      out.push({ key: s, top: at(s, -30), name: copy.building, tone: 'busy' })
    else if (s === 'townhall' && !plot.townhall)
      out.push({
        key: s,
        top: at(s, -40),
        name: copy.scaffold,
        tone: next === s ? 'next' : undefined,
      })
    else if (next === s) out.push({ key: s, top: at(s, -40), name: copy.buildHere, tone: 'next' })
    else if (stands.has(s)) out.push({ key: s, top: at(s), ...nameOf(s, plot) })
  }
  if (stands.has('orb') && !building.includes('orb'))
    out.push({
      key: 'piggy',
      top: up(P(CORE.piggy.i + 0.62, CORE.piggy.j + 0.62), 52),
      name: copy.piggy,
      muted: copy.piggyMuted,
    })
  return out
}

function nameOf(s: Site, p: SetupPlot): { name: string; muted?: string } {
  switch (s) {
    case 'townhall':
      return { name: copy.townhall }
    case 'outpost':
      return { name: copy.outpost }
    case 'team':
      return { name: p.team ?? copy.team }
    case 'yard':
      return { name: p.yard ?? copy.yard }
    case 'tank':
      return { name: copy.tank, muted: copy.tankMuted }
    case 'mine':
      return {
        name: copy.mine(p.mine?.name ?? ''),
        muted: p.pinned ? copy.pinnedMuted : undefined,
      }
    case 'gate':
      return { name: copy.gate }
    case 'depot':
      return { name: copy.depot, muted: p.firstAgent && !p.session ? copy.depotMuted : undefined }
    case 'observatory':
      return { name: copy.observatory }
    case 'orb':
      return { name: copy.orb, muted: copy.orbMuted }
  }
}

function SetupChips({
  chips,
  view,
}: {
  chips: readonly SetupChip[]
  view: { view(): ViewState; subscribe(l: (s: ViewState) => void): () => void }
}) {
  const host = useRef<HTMLDivElement>(null)
  const keys = chips.map((c) => `${c.key}:${c.top.join(',')}`).join('|')
  useLayoutEffect(() => {
    const el = host.current
    if (!el) return
    const place = (s: ViewState) => {
      el.querySelectorAll<HTMLElement>('[data-chip]').forEach((c) => {
        const chip = chips.find((x) => x.key === c.dataset.chip)
        if (!chip) return
        const [mx, my] = toMap(chip.top)
        c.style.transform = `translate(${((mx - s.x) * s.z).toFixed(1)}px,${((my - 6 - s.y) * s.z).toFixed(1)}px) translate(-50%,-100%)`
      })
    }
    place(view.view())
    return view.subscribe(place)
    // `keys` stands for the chips' identity and position.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, keys])
  return (
    <div
      ref={host}
      aria-hidden
      className="pointer-events-none absolute inset-0 z-1 overflow-hidden"
    >
      {chips.map((c) => (
        <div
          key={c.key}
          data-chip={c.key}
          className={cn(
            'absolute top-0 left-0 flex items-center gap-1.5 rounded-md border bg-popover/90 px-2 py-0.5 text-xs font-medium whitespace-nowrap text-popover-foreground shadow-sm backdrop-blur-sm',
            c.tone === 'next' && 'border-success/60 bg-success/15 text-success',
          )}
        >
          <span>{c.name}</span>
          {c.muted ? <span className="font-normal text-muted-foreground">{c.muted}</span> : null}
        </div>
      ))}
    </div>
  )
}
