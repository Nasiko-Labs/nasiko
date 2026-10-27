/**
 * The village's other places (plans/feat-village-overview.md V50–V54), redrawn from the reference design (playground
 * ref-026): the ground (the grass island, the dirt plot, the sand strips, the parking and the coin trails), the
 * orchestrator's globe, the barn, the clock tower, MCP gateways, tools, project plots, the coding tools' ball and the
 * brick stacks of the providers they call, the agents' pieces, empty plots and district signs, and the size lift
 * (`Scaled`). Plain SVG from `solids.tsx`, colours from `palette.ts`; ambient motion is SMIL behind `Loop`, so reduced
 * motion draws everything at rest.
 */
import type { ReactNode } from 'react'
import { BRAND_PATHS } from '../map/brands'
import { Loop, Tuft } from './art'
import { CLIFF, MAIN, OUTSIDE, P, pts, up, type Land } from './geo'
import type { HarnessPlot, TokenEdge, Weather, Yard } from './model'
import { ART, BEACON, HARNESS_ART, HARNESS_FALLBACK, INK } from './palette'
import { world } from './worlds/current'
import { TUFTS } from './scenery'
import { Box, FaceL, Foot, Gable, Line, Poly, Roof, Shadow } from './solids'
import { busiestBySource, cadenceOf, sourceOf, streamsOf, type Stream } from './coins'
import { BAY_LINES, CORE } from './zones'

/** Draws `children` `s` times their size around the ground point (ci, cj): iso art scales uniformly. */
export function Scaled({
  ci,
  cj,
  s,
  children,
}: {
  ci: number
  cj: number
  s: number
  children: ReactNode
}) {
  const [x, y] = P(ci, cj)
  return (
    <g
      transform={`translate(${x.toFixed(1)},${y.toFixed(1)}) scale(${s}) translate(${(-x).toFixed(1)},${(-y).toFixed(1)})`}
    >
      {children}
    </g>
  )
}

/* ---------- the fleet's corner ---------- */

/**
 * The orchestrator (V50): a white wire globe on a black pole beside the parking. While it coordinates (`lit`), its
 * meridians turn.
 */
export function GlobeArt({ i, j, lit }: { i: number; j: number; lit: boolean }) {
  const [x, y] = P(i + 0.4, j + 0.4)
  const R = 25
  const cy = y - 34 - R
  const meridian = (rx: number, k: string) => (
    <ellipse key={k} cx={0} cy={0} rx={rx} ry={R} fill="none">
      {lit ? (
        <Loop>
          <animate
            attributeName="rx"
            values={`${rx};${R - rx};${rx}`}
            dur="6s"
            repeatCount="indefinite"
          />
        </Loop>
      ) : null}
    </ellipse>
  )
  return (
    <>
      <ellipse cx={x + 6} cy={y + 1} rx={12} ry={4.5} fill={ART.shadow} />
      <Line a={[x, y]} b={[x, cy + R - 2]} c="#000" w={4} />
      <g transform={`translate(${x.toFixed(1)},${cy.toFixed(1)}) rotate(-18)`}>
        <g stroke={INK} strokeWidth={5.4}>
          <circle r={R} fill="rgba(255,255,255,.08)" />
        </g>
        <g stroke="#fff" strokeWidth={3}>
          <circle r={R} fill="none" />
          {meridian(R * 0.42, 'm1')}
          {meridian(R * 0.8, 'm2')}
          <path
            d={`M${-R * 0.92},${-R * 0.38}h${R * 1.84}M${-R},0h${R * 2}M${-R * 0.92},${R * 0.38}h${R * 1.84}`}
            fill="none"
          />
        </g>
      </g>
    </>
  )
}

/** The barn on the parking's edge (V50): the service bay; broken agents stand in front of it. A red lamp while any do. */
export function BarnArt({ down }: { down: number }) {
  const b = CORE.barn
  const [lx, ly] = up(P(b.i + b.w * 0.25, b.j + b.d * 0.5), 22 + 30)
  return (
    <>
      <Foot i={b.i + 0.3} j={b.j + 0.1} w={b.w} d={b.d} fill={ART.shadow} />
      <Box i={b.i} j={b.j} w={b.w} d={b.d} h={22} color="#4e6085" right="#3d4e70" />
      <FaceL i0={b.i + 0.55} i1={b.i + 1.05} jp={b.j + b.d} z0={0} z1={14} color="#f2f3f7" />
      <Gable
        i={b.i}
        j={b.j}
        w={b.w}
        d={b.d}
        z={22}
        rh={30}
        color="#3e4e6e"
        wall="#4e6085"
        o={0.16}
      />
      {down ? (
        <circle cx={lx} cy={ly} r={5.5} fill={BEACON.broken} stroke="#fff" strokeWidth={1.4}>
          <Loop>
            <animate attributeName="opacity" values="1;.35;1" dur="1s" repeatCount="indefinite" />
          </Loop>
        </circle>
      ) : null}
    </>
  )
}

/** The clock tower (V50): workflows. Its minute hand turns while any workflow ran in the range. */
export function ClockTower({ running }: { running: boolean }) {
  const c = CORE.clock
  const H = 73
  const [fx, fy] = up(P(c.i + c.w * 0.45, c.j + c.d), H * 0.7)
  return (
    <>
      <Foot i={c.i + 0.4} j={c.j + 0.1} w={c.w} d={c.d} fill={ART.shadow} />
      <Box i={c.i} j={c.j} w={c.w} d={c.d} h={H} color="#e4e6ec" right="#c6cad3" />
      <FaceL i0={c.i + 0.55} i1={c.i + 1.05} jp={c.j + c.d} z0={0} z1={24} color="#4a5060" />
      <g transform={`translate(${fx.toFixed(1)},${fy.toFixed(1)}) matrix(1,0.5,0,1,0,0)`}>
        <circle r={11} fill="#d9dadf" stroke={INK} strokeWidth={1.2} />
        <line x1={0} y1={0} x2={0} y2={-8} stroke={INK} strokeWidth={1.4} strokeLinecap="round">
          {running ? (
            <Loop>
              <animateTransform
                attributeName="transform"
                type="rotate"
                values="0;360"
                dur="12s"
                repeatCount="indefinite"
              />
            </Loop>
          ) : null}
        </line>
        <line x1={0} y1={0} x2={5} y2={0} stroke={INK} strokeWidth={1.4} strokeLinecap="round" />
      </g>
      <Roof i={c.i} j={c.j} w={c.w} d={c.d} z={H} rh={30} color="#4a78d6" o={0.2} />
    </>
  )
}

/* ---------- connectors ---------- */

const FLAG: Record<Weather, string> = { ok: '#3aa86b', warn: '#f5a524', down: '#e5513b' }

/** An MCP gateway (V50): a grey house with a slate gable roof and its flag: green fine, amber slow, red failing. */
export function McpHouse({ i, j, weather }: { i: number; j: number; weather: Weather }) {
  const s = 1.4
  const [px, py] = up(P(i + 0.25, j + s / 2), 28.8 + 15)
  return (
    <>
      <Foot i={i + 0.3} j={j + 0.1} w={s} d={s} fill={ART.shadow} />
      <Box i={i} j={j} w={s} d={s} h={28.8} color="#c9c1b4" right="#aaa194" />
      <FaceL i0={i + 0.2} i1={i + 0.48} jp={j + s} z0={0} z1={17.6} color="#4b3d33" />
      <Gable i={i} j={j} w={s} d={s} z={28.8} rh={16} color="#5a6a80" wall="#c9c1b4" o={0.15} />
      <Line a={[px, py]} b={[px, py - 19]} c={INK} w={1.2} />
      <path
        d={`M${px},${py - 19}l12,3.6l-12,3.6z`}
        fill={FLAG[weather]}
        stroke={INK}
        strokeWidth={0.8}
      />
    </>
  )
}

/**
 * A tool (V50): a small white box, 0.75 tiles, with the tool's mark faint on its front face (the map's single-ink
 * brand marks, `map/brands.ts`; a monogram when there is none).
 */
export function ToolBox({
  i,
  j,
  brand,
  name,
}: {
  i: number
  j: number
  brand: string | null
  name: string
}) {
  const s = 0.75
  const H = 13.2
  const [fx, fy] = up(P(i + s / 2, j + s), H / 2)
  const path = brand ? BRAND_PATHS[brand] : undefined
  return (
    <>
      <Shadow i={i} j={j} w={s} d={s} />
      <Box i={i} j={j} w={s} d={s} h={H} color="#d5d5d5" right="#b8b8b8" top="#e6e6e6" />
      <g
        transform={`translate(${fx.toFixed(1)},${fy.toFixed(1)}) matrix(1,0.5,0,1,0,0)`}
        opacity={0.55}
      >
        {path ? (
          <path d={path} fill="#8b8b8b" transform="scale(0.36) translate(-12,-12)" />
        ) : (
          <text
            y={3}
            textAnchor="middle"
            fontSize={8}
            fontWeight={800}
            fill="#8b8b8b"
            className="font-sans"
          >
            {(name[0] ?? '?').toUpperCase()}
          </text>
        )}
      </g>
    </>
  )
}

/* ---------- the dirt plot ---------- */

/**
 * A coding tool on the dirt plot (V50): a black ball on its marked square, its glints in the tool's own colour (our
 * own drawing, never a copied mark).
 */
export function HarnessBall({ h }: { h: Pick<HarnessPlot, 'i' | 'j' | 'w' | 'd' | 'kind'> }) {
  const a = HARNESS_ART[h.kind] ?? HARNESS_FALLBACK
  const [x, y] = P(h.i + h.w * 0.32, h.j + h.d * 0.32)
  const r = 21
  return (
    <>
      <Foot i={h.i} j={h.j} w={h.w} d={h.d} fill="none" stroke={ART.dirtLine} width={1.2} />
      <ellipse cx={x + 6} cy={y + 2} rx={22} ry={8} fill={ART.shadow} />
      <circle cx={x} cy={y - r} r={r} fill="#0b0b0e" stroke={INK} />
      <path
        d={`M${x - 11},${y - r - 6}l2,-6M${x - 4},${y - r - 4}l2.4,-7`}
        stroke={a.body}
        strokeWidth={4.2}
        strokeLinecap="round"
      />
    </>
  )
}

/**
 * A provider the coding tools call (V50): a two-tier brick stack, its windows dark. On w × d tiles at (i, j).
 */
export function BrickStack({ i, j, w, d }: { i: number; j: number; w: number; d: number }) {
  const t = 0.24
  const z = 13
  return (
    <>
      <Foot i={i + 0.25} j={j + 0.08} w={w} d={d} fill={ART.shadow} />
      <Box i={i} j={j} w={w} d={d} h={z} color="#dc7f68" right="#bf6a55" top="#e58f79" />
      <Box
        i={i + t}
        j={j + t}
        w={w - t * 2.2}
        d={d - t * 2.2}
        h={15}
        z={z}
        color="#d4806c"
        right="#c46f5c"
        top="#eaa08c"
      />
      {[0.32, 0.62].map((k) => (
        <FaceL
          key={k}
          i0={i + t + (w - t * 2.2) * k}
          i1={i + t + (w - t * 2.2) * k + 0.14}
          jp={j + d - t * 1.2}
          z0={z + 5}
          z1={z + 10}
          color="#2b2b2b"
        />
      ))}
    </>
  )
}

/* ---------- projects ---------- */

/**
 * A project (V50): a dirt plot with its flag, red for the org's, a team's colour for a team's. The workflows with no
 * project share a dashed plot with no flag; an empty plot (`bare`) is plain dirt, waiting for the next one.
 */
export function ProjectPlot({
  y,
  bare = false,
}: {
  y: Pick<Yard, 'i' | 'j' | 'w' | 'd' | 'org' | 'color' | 'unfiled'>
  bare?: boolean
}) {
  const { i, j, w, d } = y
  const flag = y.org ? '#e5513b' : (y.color ?? '#60a5fa')
  const [px, py] = P(i + w / 2, j + d / 2)
  return (
    <>
      <Foot
        i={i}
        j={j}
        w={w}
        d={d}
        fill={y.unfiled ? 'rgba(210,167,116,.35)' : ART.dirt}
        stroke={ART.dirtLine}
        width={1.2}
        dash={y.unfiled ? '5 4' : undefined}
      />
      <Tuft p={P(i + w * 0.62, j + d * 0.3)} />
      {y.unfiled || bare ? null : (
        <>
          <Line a={[px, py]} b={[px, py - 41]} c="#000" w={1.4} />
          <path d={`M${px},${py - 41}l26,7.5l-26,7.5z`} fill={flag} stroke={INK} strokeWidth={0.8}>
            <Loop>
              <animate
                attributeName="d"
                values={`M${px},${py - 41}l26,7.5l-26,7.5z;M${px},${py - 41}l22,9l-22,6z;M${px},${py - 41}l26,7.5l-26,7.5z`}
                dur="2.2s"
                repeatCount="indefinite"
              />
            </Loop>
          </path>
        </>
      )}
    </>
  )
}

/* ---------- plots and signs ---------- */

/**
 * An empty plot (V28): a faint outline where the next one of its kind goes. While placing, the open ones of the kind
 * being placed turn green, and the ones a paid plan opens show a padlock.
 */
export function PlotMark({
  i,
  j,
  w,
  d,
  mode,
}: {
  i: number
  j: number
  w: number
  d: number
  mode: 'idle' | 'open' | 'locked'
}) {
  const inset = w < 1.2 ? 0.06 : 0.14
  const [x, y] = P(i + w / 2, j + d / 2)
  return (
    <>
      <Foot
        i={i + inset}
        j={j + inset}
        w={w - inset * 2}
        d={d - inset * 2}
        fill={mode === 'open' ? ART.plotOpen : mode === 'locked' ? ART.plotLocked : ART.plot}
        stroke={mode === 'open' ? ART.plotOpenLine : ART.plotLine}
        dash={mode === 'open' ? undefined : '5 4'}
        width={mode === 'open' ? 2.2 : 1.2}
      />
      {mode === 'locked' && w >= 1 ? (
        <g transform={`translate(${x.toFixed(1)},${(y - 2).toFixed(1)})`}>
          <path
            d="M-3.5,-3v-2.5a3.5,3.5 0 0 1 7,0v2.5"
            fill="none"
            stroke="#55565e"
            strokeWidth={1.6}
          />
          <rect x={-5} y={-3} width={10} height={7.5} rx={1.6} fill="#55565e" />
        </g>
      ) : null}
    </>
  )
}

/**
 * A district's name painted on the ground at its corner (V17), like a map's district names, with what the plan hides:
 * "TEAMS · +3 over the free plan". It lies along i, so it scales with the map and never covers a label chip.
 */
export function GroundSign({
  i,
  j,
  axis,
  text,
}: {
  i: number
  j: number
  axis: 'i' | 'j'
  text: string
}) {
  const [x, y] = P(i, j)
  const m = axis === 'i' ? 'matrix(1,0.5,-1,0.5,0,0)' : 'matrix(-1,0.5,1,0.5,0,0)'
  return (
    <text
      transform={`translate(${x.toFixed(1)},${y.toFixed(1)}) ${m}`}
      fontSize={7}
      fontWeight={700}
      letterSpacing={0.5}
      fill="rgba(255,255,255,.88)"
      stroke="rgba(40,50,25,.45)"
      strokeWidth={2.4}
      paintOrder="stroke"
      className="font-sans uppercase"
    >
      {text}
    </text>
  )
}

/* ---------- sprites ---------- */

/** An agent's piece: a tiered pawn, its base on the origin. */
const PIECE =
  'M-10,0Q-11.5,-3.5 -7,-6.5Q-9.5,-9 -6.5,-12.5Q-8.2,-15.2 -4.6,-18.4Q-3.6,-24.2 0,-25.6Q3.6,-24.2 4.6,-18.4Q8.2,-15.2 6.5,-12.5Q9.5,-9 7,-6.5Q11.5,-3.5 10,0Z'

/**
 * An agent (V52): a chess-like piece on the parking, drawn at the origin; a unit's group moves it. Its colour is
 * `data-beacon` (white running, orange slow, red broken, pale grey asleep with its z); the motion controller keeps it.
 */
export function PieceSprite({
  beacon,
  ring,
  sprite,
}: {
  beacon: keyof typeof BEACON
  ring?: 'sel' | 'rel' | null
  /** A pixel sprite drawn instead of the pawn (the voxel world), w × h logical pixels at 3 px each. */
  sprite?: { href: string; w: number; h: number }
}) {
  return (
    <g>
      {ring ? (
        <ellipse
          rx={15}
          ry={6.5}
          fill="none"
          stroke={ring === 'sel' ? '#fff' : '#ffd75e'}
          strokeWidth={2.2}
        />
      ) : null}
      <ellipse cx={3} cy={1} rx={11} ry={3.8} fill="rgba(0,0,0,.18)" opacity={sprite ? 0 : 1} />
      {sprite ? (
        <g data-face="">
          <image
            href={sprite.href}
            x={-(sprite.w * 3) / 2}
            y={-sprite.h * 3 + 2}
            width={sprite.w * 3}
            height={sprite.h * 3}
            style={{ imageRendering: 'pixelated' }}
          />
        </g>
      ) : null}
      <g data-face={sprite ? undefined : ''} opacity={sprite ? 0 : 1}>
        <path
          data-beacon=""
          d={PIECE}
          fill={BEACON[beacon]}
          stroke={INK}
          strokeWidth={1.3}
          strokeLinejoin="round"
        />
      </g>
      {beacon === 'asleep' ? (
        <text x={7} y={-26} fontSize={9} fontWeight={700} fill="#5b6b88" className="font-sans">
          z
          <Loop>
            <animate attributeName="y" values="-24;-36" dur="2.4s" repeatCount="indefinite" />
            <animate attributeName="opacity" values="0;1;0" dur="2.4s" repeatCount="indefinite" />
          </Loop>
        </text>
      ) : null}
    </g>
  )
}

/**
 * What a broken or slow piece's failed runs cost (V56), floating over its head like a game's damage number: red, a
 * minus sign, bobbing a little while motion is on. Inside the piece's own group, so it moves and dims with it.
 */
export function LostBadge({ text }: { text: string }) {
  const w = 10 + text.length * 5.6
  return (
    <g aria-hidden className="pointer-events-none">
      <path d="M-3.5,-31L0,-27.5L3.5,-31Z" fill={BEACON.broken} stroke={INK} strokeWidth={1} />
      <rect
        x={-w / 2}
        y={-44}
        width={w}
        height={13}
        rx={4}
        fill={BEACON.broken}
        stroke={INK}
        strokeWidth={1}
      />
      <text
        y={-34.6}
        textAnchor="middle"
        fontSize={9.5}
        fontWeight={700}
        fill="#fff"
        className="font-sans tabular-nums"
      >
        {text}
      </text>
      <Loop>
        <animateTransform
          attributeName="transform"
          type="translate"
          values="0 0;0 -3;0 0"
          dur="1.8s"
          repeatCount="indefinite"
        />
      </Loop>
    </g>
  )
}

/**
 * Spend no budget governs (V58): a red outline with a soft halo round a place's footprint, breathing while motion is
 * on. A team with no budget gets it on its plot; the coding tools' dirt plot gets it while they spend, because a
 * budget can only alert on them.
 */
export function UngovernedGlow({ i, j, w, d }: { i: number; j: number; w: number; d: number }) {
  return (
    <g className="pointer-events-none">
      <Foot
        i={i}
        j={j}
        w={w}
        d={d}
        fill={ART.ungovernedFill}
        stroke={ART.ungovernedHalo}
        width={9}
      />
      <Foot i={i} j={j} w={w} d={d} fill="none" stroke={ART.ungoverned} width={2.2} />
      <Loop>
        <animate attributeName="opacity" values=".55;1;.55" dur="2.4s" repeatCount="indefinite" />
      </Loop>
    </g>
  )
}

/* ---------- ground ---------- */

function landFaces(l: Land, top: string, lip: boolean) {
  const T = P(l.i0, l.j0)
  const R = P(l.i0 + l.ni, l.j0)
  const B = P(l.i0 + l.ni, l.j0 + l.nj)
  const L = P(l.i0, l.j0 + l.nj)
  return (
    <>
      <Poly p={[L, B, up(B, -CLIFF), up(L, -CLIFF)]} fill={ART.cliffLeft} stroke={INK} />
      <Poly p={[B, R, up(R, -CLIFF), up(B, -CLIFF)]} fill={ART.cliffRight} stroke={INK} />
      {lip ? (
        <>
          <Poly p={[L, B, up(B, -6), up(L, -6)]} fill={ART.grassLipLeft} stroke={INK} />
          <Poly p={[B, R, up(R, -6), up(B, -6)]} fill={ART.grassLipRight} stroke={INK} />
        </>
      ) : null}
      <Poly p={[T, R, B, L]} fill={top} stroke={INK} />
    </>
  )
}

function checker(l: Land) {
  let d = ''
  for (let a = l.i0; a < l.i0 + l.ni; a++)
    for (let c = l.j0; c < l.j0 + l.nj; c++)
      if ((a + c) % 2 === 0) {
        const [q0, q1, q2, q3] = [P(a, c), P(a + 1, c), P(a + 1, c + 1), P(a, c + 1)]
        d += `M${q0.join(',')}L${q1.join(',')}L${q2.join(',')}L${q3.join(',')}Z`
      }
  return d
}
const CHECK_MAIN = checker(MAIN)
const CHECK_OUT = checker(OUTSIDE)

/** The sand strips (the reference's three): B along the town hall's front, A down between the teams, C across. */
const SAND = [
  { i: 0, j: 8.25, w: 5.25, d: 1.2 },
  { i: 4.05, j: 8.25, w: 1.2, d: 9.75 },
  { i: 0, j: 12.65, w: 11.9, d: 1.2 },
] as const

/**
 * The token streams (V55): each a queue of small coins running from the battery toward where the tokens are used, its
 * coins as close together and as quick as its share of the flow. Under reduced motion the queues stand still, their
 * spacing still saying how much flows. A stream the selection doesn't touch fades back.
 */
function Streams({ tokens, lit }: { tokens: readonly TokenEdge[]; lit: (s: Stream) => boolean }) {
  const streams = streamsOf(tokens)
  const busiest = busiestBySource(tokens)
  return (
    <>
      {streams.map((s) => {
        const { gap, speed } = cadenceOf(s.spend, busiest[sourceOf(s)])
        const points = pts([P(...s.from), P(...s.to)])
        const run = (
          <Loop>
            <animate
              attributeName="stroke-dashoffset"
              values={`0;${-gap}`}
              dur={`${(gap / speed).toFixed(2)}s`}
              repeatCount="indefinite"
            />
          </Loop>
        )
        const line = (w: number, c: string) => (
          <polyline
            points={points}
            fill="none"
            stroke={c}
            strokeWidth={w}
            strokeDasharray={`0 ${gap}`}
            strokeLinecap="round"
          >
            {run}
          </polyline>
        )
        return (
          <g
            key={s.key}
            data-stream={s.key}
            className="transition-opacity duration-200"
            opacity={lit(s) ? 1 : 0.18}
          >
            {line(8.6, INK)}
            {line(7, s.kind === 'saved' ? '#f390b4' : ART.coin)}
          </g>
        )
      })}
    </>
  )
}

export function Ground({
  tokens,
  grid,
  lit = () => true,
  bare = false,
}: {
  tokens: readonly TokenEdge[]
  grid: boolean
  /** A 3D world draws the land itself: only the token streams (and the grid). */
  bare?: boolean
  /** Whether a stream stays bright under the current selection. */
  lit?: (s: Stream) => boolean
}) {
  const d = CORE.depot
  let gridPath = ''
  if (grid) {
    for (let a = 0; a <= MAIN.ni; a++)
      gridPath += `M${P(a, 0).join(',')}L${P(a, MAIN.nj).join(',')}`
    for (let a = 0; a <= MAIN.nj; a++)
      gridPath += `M${P(0, a).join(',')}L${P(MAIN.ni, a).join(',')}`
  }
  const { under: Under, onGround: OnGround } = world()
  if (bare)
    return (
      <>
        <Streams tokens={tokens} lit={lit} />
        {grid ? (
          <path d={gridPath} stroke="rgba(255,255,255,.4)" strokeWidth={1} fill="none" />
        ) : null}
      </>
    )
  return (
    <>
      {Under ? <Under /> : null}
      {landFaces(MAIN, ART.grass, true)}
      <path d={CHECK_MAIN} fill={ART.grassTile} />
      {landFaces(OUTSIDE, ART.dirt, false)}
      <path d={CHECK_OUT} fill={ART.dirtTile} opacity={0.45} />
      {OnGround ? <OnGround /> : null}
      {/* The coding tools' yard, marked out on the dirt. */}
      <Foot i={19.65} j={3.2} w={4.7} d={4.1} fill="none" stroke={ART.dirtLine} width={1.2} />
      {SAND.map((s) => (
        <Foot key={`${s.i},${s.j}`} i={s.i} j={s.j} w={s.w} d={s.d} fill={ART.sand} />
      ))}
      {/* The parking and its painted bays. */}
      <Foot
        i={d.i}
        j={d.j}
        w={d.w}
        d={d.d}
        fill={ART.asphalt}
        stroke={ART.asphaltLine}
        width={1.2}
      />
      {BAY_LINES.map(([i, j0, j1]) => (
        <Line key={`${i},${j0}`} a={P(i, j0)} b={P(i, j1)} c={ART.bayLine} w={1.6} />
      ))}
      {TUFTS.map(([i, j]) => (
        <Tuft key={`${i},${j}`} p={P(i, j)} />
      ))}
      <Streams tokens={tokens} lit={lit} />
      {grid ? (
        <path d={gridPath} stroke="rgba(255,255,255,.4)" strokeWidth={1} fill="none" />
      ) : null}
    </>
  )
}

/** A token, for the shop's art. */
export function Coin({ x = 0, y = 0 }: { x?: number; y?: number }) {
  return <circle cx={x} cy={y} r={6} fill={ART.coin} stroke={INK} />
}
