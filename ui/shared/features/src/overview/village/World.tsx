/**
 * The village's world: one SVG (both lands, the streets and token lines, every place in depth order, the scenery,
 * people and cars, weather, links and tokens) and an HTML layer of label chips that stay the same size at any zoom.
 * Pan and zoom are the map's viewport (`useViewport`, on the canvas); this component only draws.
 *
 * Every place and piece is a `role="button"` group with a name (Tab, then Enter or Space), like the estate map: there
 * is no shadcn primitive for a picture. A selection lights what it connects to, dims the rest and draws a line to each
 * lit place (the prototype's click grammar); a click on open ground clears it. While placing (V28), the open plots of
 * the kind being placed turn green and become buttons. Pieces move through `motion.ts`, which writes their transforms;
 * the coins run on the ground's trails.
 */
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent,
  type MouseEvent,
} from 'react'
import { deferred } from '@nasiko/app-core/deferred'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { cn } from '@nasiko/ui/lib/utils'
import { copy as overviewCopy } from '../copy'
import type { EstateView } from '../estate/useEstateView'
import type { ViewState } from '../map/useViewport'
import { villageCopy as copy } from './copy'
import type { Stream } from './coins'
import { OUTSIDE, P, toMap, up, WORLD, type Pt } from './geo'
import type { Vacant, Village } from './model'
import { restingSpots, type Motion } from './motion'
import { MotionOn } from './motionContext'
import {
  Ground,
  GroundSign,
  LostBadge,
  PieceSprite,
  PlotMark,
  ProjectPlot,
  UngovernedGlow,
} from './places'
import { type Entity, isDim, type Tone } from './scene'
import { beaconOf } from './model'
import { Foot } from './solids'
import type { HoverStore } from './stores'
import { lastBakedTheme } from './voxel/cache'
import { world } from './worlds/current'
import { SIGNS, type Kind } from './zones'

// The voxel rasterizer is its own chunk: the flat village draws first, as it does while a world bakes.
const VoxelLayer = deferred(() => import('./voxel/VoxelLayer').then((m) => m.VoxelLayer))
type Sprites = typeof import('./voxel/sprites')

function activate(fn: () => void) {
  return (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      fn()
    }
  }
}

export interface WorldProps {
  village: Village
  scene: readonly Entity[]
  v: EstateView
  focus: { key: string | null; keep: Set<string> | null }
  onSelect(sel: string): void
  /** A click on open ground (not a place, a car or a plot). */
  onBackground(): void
  motion: Motion
  motionOn: boolean
  flow: boolean
  grid: boolean
  hover: HoverStore
  /** The kind being placed, if any, and what a green plot does. */
  placing: Kind | null
  placingName: string
  onPlot(p: Vacant): void
  /** The viewport, for a 3D world's camera. */
  subscribe(l: (v: ViewState) => void): () => void
  viewNow(): ViewState
  saved: number | null
}

/** Labels the `key` mode always shows; and the zoom from which it shows every label. */
const KEY_PLACES = new Set(['townhall', 'tank'])
const ZOOM_ALL = 1.6

/** Places the voxel world doesn't draw yet: their SVG art stays visible over it. */
const FLAT_ONLY = /^yard:/
/** Places whose flat art moves (the observability eye walks its loop): in the voxel world they stand still. */
const STILL = /^(observatory|outpost)$/

export function World({
  village,
  scene,
  v,
  focus,
  onSelect,
  onBackground,
  motion,
  motionOn,
  flow,
  grid,
  hover,
  placing,
  placingName,
  onPlot,
  subscribe,
  viewNow,
  saved,
}: WorldProps) {
  const { voxel } = world()
  // The flat drawing shows only until the first voxel world is baked, so the canvas is never empty while it draws.
  // After that, switching worlds keeps the last one on screen until the next is baked (it fades in over it).
  // A world baked earlier this visit counts from the first frame, so coming back never shows the flat drawing.
  const [ready, setReady] = useState<string | null>(lastBakedTheme)
  const live = !!voxel && ready !== null
  // Before the first world of the visit is baked, nothing is drawn but the loader: the flat drawing is only the
  // fallback where there are no workers to bake in (tests, very old browsers).
  const waiting = !!voxel && ready === null && typeof Worker !== 'undefined'
  // The agents' robot sprites come with the rasterizer, once a voxel world is drawn.
  const [sprites, setSprites] = useState<Sprites | null>(null)
  useEffect(() => {
    if (live && !sprites) void import('./voxel/sprites').then(setSprites)
  }, [live, sprites])
  const units = useRef<SVGGElement>(null)
  const links = useRef<SVGGElement>(null)
  const rest = restingSpots(village)
  const selected = focus.key ? scene.find((e) => e.key === focus.key) : undefined
  const selCar = focus.key?.startsWith('car:') ? focus.key.slice(4) : null

  useLayoutEffect(() => {
    if (units.current) motion.bind(village, units.current)
  }, [motion, village])
  useEffect(() => motion.set(motionOn), [motion, motionOn])

  // Links from the selection to every lit place; a selected car's follow it as it drives.
  const keep = focus.keep
  const targets = keep
    ? scene
        .filter((e) => e.sel && e.key !== focus.key && e.nodes.some((n) => keep.has(n)))
        .slice(0, 18)
    : []
  const targetKey = targets.map((t) => t.key).join('|')
  useEffect(() => {
    const g = links.current
    if (!g) return
    const draw = () => {
      let from: Pt | null = selected ? selected.top : null
      if (selCar) {
        const at = motion.at(selCar)
        from = at ? up(P(at[0], at[1]), 24) : null
      }
      const paths = g.querySelectorAll<SVGPathElement>('path[data-link]')
      if (!from) return paths.forEach((p) => p.setAttribute('d', ''))
      paths.forEach((p) => {
        const to: Pt = [Number(p.dataset.x), Number(p.dataset.y)]
        const dist = Math.hypot(from[0] - to[0], from[1] - to[1])
        const mx = (from[0] + to[0]) / 2
        const my = Math.min(from[1], to[1]) - 30 - dist * 0.14
        p.setAttribute(
          'd',
          `M${from[0].toFixed(1)},${from[1].toFixed(1)}Q${mx.toFixed(1)},${my.toFixed(1)} ${to[0].toFixed(1)},${to[1].toFixed(1)}`,
        )
      })
    }
    draw()
    return selCar ? motion.onFrame(draw) : undefined
  }, [motion, selected, selCar, targetKey])

  // A token stream stays bright while nothing is selected. A place that owns token lines (a team, a project, a
  // provider, a router, the batteries, the piggy bank, a coding tool) lights exactly its own; any other selection (an
  // agent, a model, a person) lights the lines that serve what it keeps lit.
  const owners = new Set(village.tokens.flatMap((t) => t.places))
  const streamLit = (st: Stream) => {
    if (!focus.key && !focus.keep) return true
    if (focus.key && owners.has(focus.key)) return st.places.has(focus.key)
    const keep = focus.keep
    return !!keep && [...st.nodes].some((n) => keep.has(n))
  }
  const carRing = (id: string) =>
    selCar === id ? 'sel' : focus.keep?.has(id) && selCar ? 'rel' : null
  const dimNode = (id: string) => {
    if (selCar === id) return false
    if (focus.keep && !focus.keep.has(id)) return true
    const s = v.state.get(id)
    return s === 'ghost' || s === 'muted'
  }
  // Empty plots: open ones always, faint (bays are painted on the forecourt); while placing, that kind's open plots are
  // green buttons and its locked ones show their padlock.
  const plots = village.vacant.filter((p) =>
    placing ? p.kind === placing || (p.open && p.kind !== 'bay') : p.open && p.kind !== 'bay',
  )
  const background = (e: MouseEvent) => {
    if ((e.target as Element).closest('[role="button"]')) return
    onBackground()
  }
  // Spend no budget governs (V58): a team with no budget, and the coding tools' plot while they spend (a budget can
  // only alert on them). Each glow dims with the place that stands for it.
  const ungoverned = [
    ...village.teams
      .filter((t) => !t.budget)
      .map((t) => ({ key: `team:${t.id}`, i: t.i, j: t.j, w: t.w, d: t.d })),
    ...(village.outpost && village.outpost.spend > 0
      ? [{ key: 'xtank', i: OUTSIDE.i0, j: OUTSIDE.j0, w: OUTSIDE.ni, d: OUTSIDE.nj }]
      : []),
  ].map((u) => ({ ...u, place: scene.find((e) => e.key === u.key) }))

  const dim = voxel
    ? scene
        .filter((e) => (e.sel && isDim(e, focus, v)) || (placing && e.sel))
        .map((e) => e.key)
        .join('|')
    : ''
  const hidden = (e: Entity) => live && !FLAT_ONLY.test(e.key)

  return (
    <MotionOn.Provider value={motionOn}>
      {voxel ? (
        <VoxelLayer
          theme={voxel}
          village={village}
          saved={saved}
          dim={dim}
          hover={hover}
          subscribe={subscribe}
          viewNow={viewNow}
          onReady={() => setReady(voxel)}
        />
      ) : null}
      {waiting ? (
        <PageLoader
          inline
          label={overviewCopy.views.loading('village')}
          className="absolute inset-0"
        />
      ) : null}
      {/* A click on open ground clears the selection; Escape does the same from the keyboard (VillageCanvas). */}
      {/* Its pieces and labels fade in with the world they stand on. */}
      <svg
        className={cn(
          'absolute inset-0 size-full transition-opacity duration-[250ms] select-none motion-reduce:transition-none',
          waiting && 'pointer-events-none opacity-0',
        )}
        aria-hidden={false}
        onClick={background}
      >
        <g transform={`translate(${WORLD.ox},${WORLD.oy})`}>
          <g aria-hidden>
            <Ground tokens={flow ? village.tokens : []} grid={grid} lit={streamLit} bare={live} />
          </g>
          <g aria-hidden className={cn(placing && 'opacity-30')}>
            {ungoverned.map((u) => (
              <g
                key={u.key}
                className={cn(
                  'transition-opacity duration-200',
                  u.place && isDim(u.place, focus, v) && 'opacity-20',
                )}
              >
                <UngovernedGlow i={u.i} j={u.j} w={u.w} d={u.d} />
              </g>
            ))}
          </g>
          <g aria-hidden className="pointer-events-none">
            {Object.values(village.districts).map((d) => {
              const over = d.total - d.drawn
              if (over <= 0 && placing !== d.kind) return null
              const s = SIGNS[d.kind]
              const more = village.unlimited ? copy.chip.more(over) : copy.chip.overPlanShort(over)
              return (
                <GroundSign
                  key={d.kind}
                  i={s.i}
                  j={s.j}
                  axis={s.axis}
                  text={over > 0 ? `${copy.district[d.kind]} · ${more}` : copy.district[d.kind]}
                />
              )
            })}
          </g>
          <g aria-hidden={!placing}>
            {plots.map((p) => {
              const mode = placing === p.kind ? (p.open ? 'open' : 'locked') : 'idle'
              const mark =
                p.kind === 'project' && mode === 'idle' ? (
                  <ProjectPlot y={{ ...p, org: false, color: null, unfiled: false }} bare />
                ) : (
                  <PlotMark i={p.i} j={p.j} w={p.w} d={p.d} mode={mode} />
                )
              return mode === 'open' ? (
                <g
                  key={`${p.kind}${p.plot}`}
                  role="button"
                  tabIndex={0}
                  aria-label={copy.placing.plot(placingName, p.plot)}
                  className="cursor-pointer outline-none hover:brightness-110 focus-visible:brightness-125"
                  onClick={() => onPlot(p)}
                  onKeyDown={activate(() => onPlot(p))}
                >
                  {mark}
                </g>
              ) : (
                <g
                  key={`${p.kind}${p.plot}`}
                  aria-hidden
                  className={cn(
                    'pointer-events-none',
                    placing && placing !== p.kind && 'opacity-30',
                  )}
                >
                  {mark}
                </g>
              )
            })}
          </g>
          {selected ? (
            <g aria-hidden className="pointer-events-none">
              <Foot
                i={selected.foot.i - 0.1}
                j={selected.foot.j - 0.1}
                w={selected.foot.w + 0.2}
                d={selected.foot.d + 0.2}
                fill="rgba(255,255,255,.3)"
                stroke="#fff"
                width={2.4}
                dash="8 5"
              />
            </g>
          ) : null}
          {scene.map((e) =>
            e.sel ? (
              <g
                key={e.key}
                role="button"
                tabIndex={placing ? -1 : 0}
                aria-label={e.label}
                aria-pressed={focus.key === e.key}
                data-place={e.key}
                className={cn(
                  'cursor-pointer transition-opacity duration-200 outline-none hover:brightness-105 focus-visible:brightness-110',
                  (isDim(e, focus, v) || placing) && 'opacity-20',
                  placing && 'pointer-events-none',
                )}
                onClick={() => onSelect(e.sel)}
                onKeyDown={activate(() => onSelect(e.sel))}
                onPointerEnter={() => hover.set(e.key)}
                onPointerLeave={() => hover.set(null)}
                onFocus={() => hover.set(e.key)}
                onBlur={() => hover.set(null)}
              >
                {hidden(e) ? (
                  <g opacity={0}>{STILL.test(e.key) ? <StillHit e={e} /> : e.art}</g>
                ) : (
                  e.art
                )}
              </g>
            ) : e.art && !hidden(e) ? (
              <g key={e.key} aria-hidden className={cn(placing && 'opacity-40')}>
                {e.art}
              </g>
            ) : null,
          )}
          <g aria-hidden className="pointer-events-none">
            {scene.map((e) =>
              e.fx ? (
                <g
                  key={e.key}
                  className={cn(
                    'transition-opacity duration-200',
                    isDim(e, focus, v) && 'opacity-20',
                  )}
                >
                  {e.fx}
                </g>
              ) : null,
            )}
          </g>
          <g ref={links} aria-hidden className="pointer-events-none">
            {targets.map((t) => (
              <g key={t.key}>
                <path
                  data-link=""
                  data-x={t.top[0]}
                  data-y={t.top[1]}
                  fill="none"
                  stroke="rgba(255,255,255,.92)"
                  strokeWidth={5.5}
                  strokeLinecap="round"
                />
                <path
                  data-link=""
                  data-x={t.top[0]}
                  data-y={t.top[1]}
                  fill="none"
                  stroke="#26262a"
                  strokeWidth={2.2}
                  strokeDasharray="8 6"
                  strokeLinecap="round"
                />
                <circle
                  cx={t.top[0]}
                  cy={t.top[1]}
                  r={4.5}
                  fill="#26262a"
                  stroke="#fff"
                  strokeWidth={1.6}
                />
              </g>
            ))}
          </g>
          <g ref={units} className={cn(placing && 'pointer-events-none opacity-40')}>
            {village.cars.map((c) => {
              const [x, y] = P(...(rest.get(c.id) ?? ([c.i, c.j] as const)))
              const beacon = beaconOf(c.pose)
              return (
                <g
                  key={c.id}
                  data-unit={c.id}
                  role="button"
                  tabIndex={placing ? -1 : 0}
                  aria-label={`${c.name}, ${copy.beacon[beacon]}${c.lost === null ? '' : `, ${copy.lost.label(c.lost)}`}`}
                  aria-pressed={selCar === c.id}
                  transform={`translate(${x.toFixed(1)},${y.toFixed(1)})`}
                  className={cn(
                    'cursor-pointer transition-opacity duration-200 outline-none',
                    dimNode(c.id) && 'opacity-20',
                  )}
                  onClick={() => onSelect(`node:${c.id}`)}
                  onKeyDown={activate(() => onSelect(`node:${c.id}`))}
                >
                  <PieceSprite
                    beacon={beacon}
                    ring={carRing(c.id)}
                    sprite={
                      live && ready && sprites ? sprites.agentSprite(beacon, ready) : undefined
                    }
                  />
                  {c.lost === null ? null : <LostBadge text={copy.lost.badge(c.lost)} />}
                </g>
              )
            })}
          </g>
        </g>
      </svg>
    </MotionOn.Provider>
  )
}

const PILL: Record<Tone, string> = {
  bad: 'bg-destructive/15 text-destructive',
  warn: 'bg-warning/15 text-warning',
  ok: 'bg-success/15 text-success',
  none: 'border border-dashed border-muted-foreground/50 text-muted-foreground',
}

const pill = (p: { text: string; tone: Tone }) => (
  <span className={cn('rounded px-1.5 py-px text-2xs font-semibold tabular-nums', PILL[p.tone])}>
    {p.text}
  </span>
)

/** The label chips: HTML over the SVG, placed from the viewport so they keep their size at any zoom. */
export function Chips({
  scene,
  focus,
  v,
  labels,
  hover,
  view,
}: {
  scene: readonly Entity[]
  focus: { key: string | null; keep: Set<string> | null }
  v: EstateView
  labels: 'key' | 'always' | 'hover' | 'off'
  hover: HoverStore
  view: { view(): ViewState; subscribe(l: (s: ViewState) => void): () => void }
}) {
  const host = useRef<HTMLDivElement>(null)
  const hovered = useSyncExternalStore(hover.subscribe, hover.get, hover.get)
  // Zoomed in there is room for every label; the key labels mode shows them all from here.
  const [close, setClose] = useState(() => view.view().z >= ZOOM_ALL)
  useEffect(() => view.subscribe((s) => setClose(s.z >= ZOOM_ALL)), [view])
  /** In `key` mode: the town hall, the battery and anything with a problem stand labelled. */
  const key = (e: Entity) => KEY_PLACES.has(e.key) || e.chip?.pill?.tone === 'bad'
  const shown = labels === 'off' ? [] : scene.filter((e) => e.chip)
  const keys = shown.map((e) => e.key).join('|')
  useLayoutEffect(() => {
    const el = host.current
    if (!el) return
    const place = (s: ViewState) => {
      el.querySelectorAll<HTMLElement>('[data-chip]').forEach((c) => {
        const e = scene.find((x) => x.key === c.dataset.chip)
        if (!e) return
        const [mx, my] = toMap(e.top)
        c.style.transform = `translate(${((mx - s.x) * s.z).toFixed(1)}px,${((my - 6 - s.y) * s.z).toFixed(1)}px) translate(-50%,-100%)`
      })
    }
    place(view.view())
    return view.subscribe(place)
  }, [view, scene, keys])
  return (
    <div
      ref={host}
      aria-hidden
      className="pointer-events-none absolute inset-0 z-1 overflow-hidden"
    >
      {shown.flatMap((e) => {
        const c = e.chip
        if (!c) return []
        const related = !!focus.key && !!focus.keep && !isDim(e, focus, v)
        const visible =
          (labels === 'always' && c.always) ||
          (labels === 'key' && c.always && (key(e) || close || related)) ||
          hovered === e.key ||
          focus.key === e.key
        return (
          <div
            key={e.key}
            data-chip={e.key}
            className={cn(
              'absolute top-0 left-0 flex items-center gap-1 rounded-md border bg-popover/90 px-1.5 py-px text-[11px] font-medium whitespace-nowrap text-popover-foreground shadow-sm backdrop-blur-sm transition-opacity duration-200',
              !visible && 'opacity-0',
              visible && isDim(e, focus, v) && 'opacity-30',
            )}
          >
            {c.swatch ? (
              <i className="size-2.5 shrink-0 rounded-sm" style={{ background: c.swatch }} />
            ) : null}
            {c.pill && !c.pillAfter ? pill(c.pill) : null}
            <span>{c.name}</span>
            {c.muted ? (
              <span className="font-normal text-muted-foreground tabular-nums">{c.muted}</span>
            ) : null}
            {c.pill && c.pillAfter ? pill(c.pill) : null}
          </div>
        )
      })}
    </div>
  )
}

/** A still hit area over a place the voxel world draws standing (a box from its foot up to its chip's anchor). */
function StillHit({ e }: { e: Entity }) {
  const [x, y] = P(e.foot.i + e.foot.w / 2, e.foot.j + e.foot.d / 2)
  const top = e.top[1]
  return <rect x={x - 16} y={top} width={32} height={Math.max(20, y - top + 8)} fill="#000" />
}
