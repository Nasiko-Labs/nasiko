/**
 * The village's landmark drawings (plans/feat-village-overview.md V50), redrawn from the reference design (playground
 * ref-026, its Figma export): the town hall, the token batteries, team houses, LLM routers, provider tents, the
 * opportunities observatory, the observability eye, the piggy bank, trees, bushes and grass, weather and Kiko. The
 * other places (the ground, the parking and its pieces, the globe, the barn, the clock tower, MCP gateways, tools,
 * projects, the dirt plot's stacks and ball) are in `places.tsx`. Every face is outlined in one ink (`INK`); shape
 * carries the meaning and colour backs it up.
 *
 * Ambient motion (the eye's walk, a spark, smoke, rain, a flag) is SMIL inside the SVG, rendered only while motion is
 * on (`MotionOn`), so reduced motion draws everything at rest and the shell's CSS gains no keyframes.
 */
import { useContext, type ReactNode } from 'react'
import { deferred } from '@nasiko/app-core/deferred'
import { HH, HW, P, up, type Pt } from './geo'
import type { Mine, Team } from './model'
import { MotionOn } from './motionContext'
import { ART, INK, shade } from './palette'
import { Box, Cyl, FaceL, FaceR, Foot, Line, Poly, Roof, Shadow } from './solids'
import { world } from './worlds/current'

export function Loop({ children }: { children: ReactNode }) {
  return useContext(MotionOn) ? <>{children}</> : null
}

/* ---------- small marks ---------- */

/** A flag on a pole: the pole stands at `p`, the cloth flies `h` px up. */
export function Flag({ p, color, h = 41 }: { p: Pt; color: string; h?: number }) {
  const [x, y] = p
  const top = y - h
  return (
    <>
      <Line a={[x, y]} b={[x, top]} c="#000" w={1.4} />
      <path d={`M${x},${top}l26,7.5l-26,7.5z`} fill={color} stroke={INK} strokeWidth={0.8}>
        <Loop>
          <animate
            attributeName="d"
            values={`M${x},${top}l26,7.5l-26,7.5z;M${x},${top}l22,9l-22,6z;M${x},${top}l26,7.5l-26,7.5z`}
            dur="2.2s"
            repeatCount="indefinite"
          />
        </Loop>
      </path>
    </>
  )
}

/** A rune: the Route upgrade's mark (shop only). */
export function Rune({ x, y, on }: { x: number; y: number; on: boolean }) {
  return (
    <>
      <circle
        cx={x}
        cy={y}
        r={7.5}
        fill={on ? '#d9fbff' : '#3a4a63'}
        stroke={on ? '#1aa7d6' : '#222c3c'}
        strokeWidth={1.5}
      />
      <path
        d={`M${x - 3.5},${y + 3}L${x},${y - 0.5}L${x + 3.5},${y + 3}M${x},${y - 0.5}V${y - 4}`}
        stroke={on ? '#0c7fa8' : '#9fb4d4'}
        strokeWidth={1.5}
        fill="none"
        strokeLinecap="round"
      />
    </>
  )
}

/** A white knob with an ink ring: the tip of a roof, a tent's pole, a battery's cap. */
const Knob = ({ x, y, r = 3 }: { x: number; y: number; r?: number }) => (
  <circle cx={x} cy={y} r={r} fill="#e9eaec" stroke="#1b1c1f" strokeWidth={1.2} />
)

/* ---------- landmarks ---------- */

/** The town hall (tier 1): a cream house with a red hip roof on a stone platform, 3.7 × 3.7 tiles at (i, j). */
export function Townhall({ i, j }: { i: number; j: number }) {
  const S = world().skin.Townhall
  if (S) return <S i={i} j={j} />
  const w = 3.7
  const wi = i + 0.35
  const wj = j + 0.35
  const base = 6
  const wall = 28.8
  const front = wj + 3
  const side = wi + 3
  const window = (i0: number, c: string, k: string) => (
    <FaceL key={k} i0={i0} i1={i0 + 0.5} jp={front} z0={base + 10} z1={base + 24} color={c} />
  )
  return (
    <>
      <Foot i={i + 0.45} j={j + 0.1} w={w} d={w} fill={ART.shadow} />
      <Box i={i} j={j} w={w} d={w} h={base} color="#ddd0b4" right="#c7b998" top="#ebe1cb" />
      <Box
        i={wi}
        j={wj}
        w={3}
        d={3}
        h={wall}
        z={base}
        color="#f1e1bc"
        right="#dac79c"
        top="#f1e1bc"
      />
      {window(wi + 0.43, '#5d8bd8', 'w1')}
      <FaceL i0={wi + 1.2} i1={wi + 1.8} jp={front} z0={base} z1={base + 22.8} color="#8a5530" />
      {window(wi + 2.05, '#5d8bd8', 'w2')}
      {[0.64, 1.86].map((k) => (
        <FaceR
          key={k}
          ip={side}
          j0={wj + 3 - k - 0.55}
          j1={wj + 3 - k}
          z0={base + 10}
          z1={base + 24}
          color="#4a74b9"
        />
      ))}
      <Roof i={wi} j={wj} w={3} d={3} z={base + wall} rh={44} color="#e5513b" o={0.35} />
    </>
  )
}

/**
 * A token battery: a glass cylinder whose gold is what's left (0..1), orange when the month will go over, with a cap
 * and a spout on the side the tokens leave from. `left` null draws the ghost of one: no budget yet.
 */
export function Battery({
  ci,
  cj,
  r,
  h,
  left,
  over = false,
  spout,
  cap = true,
}: {
  ci: number
  cj: number
  r: number
  h: number
  left: number | null
  over?: boolean
  spout?: 'right' | 'left'
  cap?: boolean
}) {
  const [cx, c0] = P(ci, cj)
  const rx = r * HW * 1.414
  const ry = r * HH * 1.414
  if (left === null)
    return (
      <path
        d={`M${cx - rx},${c0 - h}L${cx - rx},${c0}A${rx},${ry} 0 0 0 ${cx + rx},${c0}L${cx + rx},${c0 - h}A${rx},${ry} 0 0 0 ${cx - rx},${c0 - h}Z`}
        fill="rgba(40,90,30,.16)"
        stroke="rgba(40,90,30,.28)"
        strokeDasharray="3 3"
      />
    )
  const fill = Math.max(0, Math.min(1, left))
  const gh = Math.max(3, h * 0.92 * fill)
  const gold = over ? '#f08a3c' : ART.gold
  const [sx, sy] =
    spout === 'left' ? [cx - rx * 0.78, c0 + ry * 0.62] : [cx + rx * 0.78, c0 + ry * 0.62]
  const sz = Math.min(gh, h) * 0.5
  return (
    <>
      <ellipse cx={cx + rx * 0.35} cy={c0 + 2} rx={rx * 1.15} ry={ry * 1.1} fill={ART.shadow} />
      <Cyl ci={ci} cj={cj} r={r} h={gh} color={gold} top={shade(gold, 0.62)} />
      <Cyl ci={ci} cj={cj} r={r} h={h - gh} z={gh} color={ART.glass} top={ART.glassTop} />
      {cap ? (
        <>
          <Cyl ci={ci} cj={cj} r={r * 0.36} h={r * 6} z={h} color={ART.cap} top={ART.capTop} />
          <Knob x={cx} y={c0 - h - r * 6} r={Math.max(1.6, r * 3)} />
        </>
      ) : null}
      {spout ? (
        <g
          transform={`translate(${sx.toFixed(1)},${(sy - sz).toFixed(1)}) rotate(${spout === 'left' ? 155 : 25})`}
        >
          <rect x={-2} y={-4.5} width={13} height={9} rx={2} fill="#d5a928" stroke={INK} />
          <ellipse cx={11} cy={0} rx={2.6} ry={4.5} fill="#d6d9de" stroke={INK} />
        </g>
      ) : null}
    </>
  )
}

/**
 * A team (V50): its house, a pyramid roof in the team's colour, and beside it its budget battery: the gold is what's
 * left of the month's budget, orange when the month will go over, a ghost when it has none. 2.45 × 1.3 tiles.
 */
export function TeamArt({ t }: { t: Pick<Team, 'i' | 'j' | 'color' | 'budget' | 'pace'> }) {
  const S = world().skin.TeamArt
  if (S) return <S t={t} />
  const { i, j } = t
  const s = 1.31
  const apex = up(P(i + s / 2, j + s / 2), 19.3 + 18)
  const b = t.budget
  const left = b && b.limit > 0 ? Math.max(0, 1 - b.spent / b.limit) : null
  return (
    <>
      <Shadow i={i} j={j} w={s} d={s} />
      <Box i={i} j={j} w={s} d={s} h={19.3} color="#ece0c4" right="#d4c4a2" />
      <Roof i={i} j={j} w={s} d={s} z={19.3} rh={18} color={t.color} o={0.18} />
      <Line a={apex} b={up(apex, 6)} c={INK} w={1.2} />
      <Knob x={apex[0]} y={apex[1] - 7} r={2.4} />
      <Battery
        ci={i + 1.97}
        cj={j + 0.85}
        r={0.42}
        h={32}
        left={left}
        over={(t.pace ?? 0) > 1}
        cap={false}
      />
    </>
  )
}

/**
 * An LLM router (V50): a blue trough on four black stilts, astride the token lines; the tokens pass under it along i.
 * Its footprint is w × d tiles at (i, j).
 */
export function GateArt({
  i,
  j,
  w = 0.56,
  d = 1.58,
}: {
  i: number
  j: number
  w?: number
  d?: number
}) {
  const z = 40
  const legs: [number, number][] = [
    [i + 0.06, j + 0.08],
    [i + w - 0.06, j + 0.08],
    [i + 0.06, j + d - 0.08],
    [i + w - 0.06, j + d - 0.08],
  ]
  return (
    <>
      <Foot i={i + 0.3} j={j + 0.05} w={w} d={d} fill={ART.shadow} />
      {legs.map(([li, lj]) => (
        <Line key={`${li},${lj}`} a={P(li, lj)} b={up(P(li, lj), z)} c="#000" w={1.9} />
      ))}
      <Box i={i} j={j} w={w} d={d} h={16} z={z} color="#60a5fa" right="#4b90ec" top="#2180f5" />
      <Poly
        p={[
          up(P(i + 0.07, j + 0.1), z + 16),
          up(P(i + w - 0.07, j + 0.1), z + 16),
          up(P(i + w - 0.07, j + d - 0.1), z + 16),
          up(P(i + 0.07, j + d - 0.1), z + 16),
        ]}
        fill="#3b8ff7"
        stroke={INK}
        width={0.6}
      />
    </>
  )
}

/**
 * A provider (V50): a grey-brown tent with a dark door on its front, a pole knob on top and a sign board beside it that
 * carries the provider's mark in its colour. Its footprint is w × w tiles at (i, j).
 */
export function MineArt({ m }: { m: Pick<Mine, 'i' | 'j' | 'w' | 'color' | 'mono'> }) {
  const S = world().skin.MineArt
  if (S) return <S m={m} />
  return <MineShape m={m} />
}

/** The tent's colours: its two faces, its door and its sign board. A world theme passes its own. */
export interface MineTone {
  left: string
  right: string
  door: string
  board: string
}
const MINE_TONE: MineTone = { left: '#6d625c', right: '#524a45', door: '#2c2420', board: '#aaa194' }

export function MineShape({
  m,
  tone = MINE_TONE,
}: {
  m: Pick<Mine, 'i' | 'j' | 'w' | 'color' | 'mono'>
  tone?: MineTone
}) {
  const { i, j, w } = m
  const k = w / 2.42
  const apex = up(P(i + w / 2, j + w / 2), 42 * k)
  const A = P(i, j + w)
  const B = P(i + w, j + w)
  const C = P(i + w, j)
  // The door: an arch on the front-left face, low and left of centre.
  const [dx, dy] = up(P(i + w * 0.36, j + w), 0)
  const bi = i + w * 0.86
  const [mx, my] = up(P(bi, j + w * 0.55), 26 * k)
  return (
    <>
      <Foot i={i + 0.35} j={j + 0.1} w={w} d={w} fill={ART.shadow} />
      <Poly p={[A, B, apex]} fill={tone.left} stroke={INK} />
      <Poly p={[B, C, apex]} fill={tone.right} stroke={INK} />
      <path
        d={`M${dx - 7 * k},${dy - 1}v${-13 * k}a${7 * k},${7.5 * k} 0 0 1 ${14 * k},${3}v${13 * k}z`}
        fill={tone.door}
      />
      <Line a={apex} b={up(apex, 9 * k)} c={INK} w={1.2} />
      <Knob x={apex[0]} y={apex[1] - 11 * k} r={3} />
      {/* The sign board on its posts, facing front-right, with the provider's mark. */}
      <Line a={P(bi, j + w * 0.38)} b={up(P(bi, j + w * 0.38), 36 * k)} c="#000" w={1.9} />
      <Line a={P(bi, j + w * 0.72)} b={up(P(bi, j + w * 0.72), 30 * k)} c="#000" w={1.9} />
      <FaceR
        ip={bi}
        j0={j + w * 0.36}
        j1={j + w * 0.74}
        z0={14 * k}
        z1={40 * k}
        color={tone.board}
      />
      <g transform={`translate(${mx.toFixed(1)},${my.toFixed(1)}) matrix(1,-0.5,0,1,0,0)`}>
        <circle r={6.5 * k} fill={m.color} stroke={INK} strokeWidth={0.8} />
        <text
          y={3 * k}
          textAnchor="middle"
          fontSize={8 * k}
          fontWeight={800}
          fill="#fff"
          className="font-sans"
        >
          {m.mono}
        </text>
      </g>
    </>
  )
}

/**
 * The opportunities observatory (V50): a purple dome on a white drum, on a lavender platform, its telescope arm up and,
 * while there are opportunities to take, a white spark at the tip. `on` lights the spark. w × w tiles at (i, j).
 */
export function OpportunityArt({
  i,
  j,
  w = 1.53,
  on,
}: {
  i: number
  j: number
  w?: number
  on: boolean
}) {
  const ci = i + w / 2
  const cj = j + w / 2
  const [cx, cy] = up(P(ci, cj), 5 + 11)
  const rx = 0.49 * HW * 1.414
  const ry = 0.49 * HH * 1.414
  const tip: Pt = [cx - 17, cy - 33]
  return (
    <>
      <Foot i={i + 0.25} j={j + 0.08} w={w} d={w} fill={ART.shadow} />
      <Box i={i} j={j} w={w} d={w} h={5} color="#d2cfe8" right="#b8b4d6" top="#e4e2f3" />
      <Cyl ci={ci} cj={cj} r={0.49} h={11} z={5} color="#f2f3f5" top="#f2f3f5" />
      <path
        d={`M${cx - rx},${cy}A${rx},${ry * 2.2} 0 0 1 ${cx + rx},${cy}A${rx},${ry} 0 0 1 ${cx - rx},${cy}Z`}
        fill="#6c5ce7"
        stroke={INK}
      />
      <path
        d={`M${cx - rx * 0.55},${cy - ry * 1.4}a${rx * 0.35},${ry * 0.45} 0 0 1 ${rx * 0.45},${-ry * 0.5}`}
        stroke="#9a8ff2"
        strokeWidth={2.4}
        fill="none"
        strokeLinecap="round"
      />
      <Line a={[cx - 1, cy - ry * 1.6]} b={[cx + 9, cy - 30]} c={INK} w={5.4} />
      <Line a={[cx - 1, cy - ry * 1.6]} b={tip} c={INK} w={4.4} />
      <circle cx={tip[0]} cy={tip[1]} r={2.6} fill="#fff" stroke={INK} strokeWidth={1.4} />
      {on ? (
        <g>
          {(
            [
              [INK, 4.6],
              ['#fff', 2.4],
            ] as const
          ).map(([c, w]) => (
            <path
              key={c}
              d={`M${tip[0] + 2},${tip[1] - 9}l5,-9l4,7l5,-10l2,9l6,-4`}
              fill="none"
              stroke={c}
              strokeWidth={w}
              strokeLinejoin="round"
              strokeLinecap="round"
            />
          ))}
          <Loop>
            <animate attributeName="opacity" values="1;.4;1" dur="1.6s" repeatCount="indefinite" />
          </Loop>
        </g>
      ) : null}
    </>
  )
}

/**
 * The observability eye (V50), drawn at the origin standing on the ground: a black ball with one white pupil on a grey
 * drum and a tripod. `look` turns the pupil (px from the centre). The inside one walks the park; the dirt plot's
 * stands by its coding tools.
 */
export function EyeArt({ look = [-2, 2] }: { look?: readonly [number, number] }) {
  const rx = 0.39 * HW * 1.414
  const ry = 0.39 * HH * 1.414
  const legs = 18
  const drum = 17
  const by = -(legs + drum + 20)
  return (
    <>
      <ellipse cx={4} cy={1} rx={17} ry={7} fill={ART.shadow} />
      <g stroke="#6b717b" strokeWidth={2.4} strokeLinecap="round">
        <line x1={-3} y1={-legs} x2={-8} y2={0} />
        <line x1={3} y1={-legs} x2={8} y2={0} />
        <line x1={0} y1={-legs} x2={1} y2={-3} />
      </g>
      <path
        d={`M${-rx},${-legs - drum}V${-legs}A${rx},${ry} 0 0 0 ${rx},${-legs}V${-legs - drum}Z`}
        fill="#bfc3c9"
        stroke={INK}
      />
      <ellipse cx={0} cy={-legs - drum} rx={rx} ry={ry} fill="#d6d9de" stroke={INK} />
      <circle cx={0} cy={by} r={22} fill="#000" stroke={INK} />
      <circle cx={look[0]} cy={by + look[1]} r={4.2} fill="#fff" />
    </>
  )
}

/** The inside eye, walking its loop around the park (`points` in screen px, from its home), turning as it goes. */
export function WalkingEye({ points }: { points: readonly Pt[] }) {
  const [x0, y0] = points[0] ?? [0, 0]
  const path = `M${points.map((p) => `${(p[0] - x0).toFixed(1)},${(p[1] - y0).toFixed(1)}`).join('L')}Z`
  return (
    <g transform={`translate(${x0.toFixed(1)},${y0.toFixed(1)})`}>
      <g>
        <Loop>
          <animateMotion path={path} dur="26s" repeatCount="indefinite" />
        </Loop>
        <g>
          <EyeArt />
          <Loop>
            <animateTransform
              attributeName="transform"
              type="translate"
              values="0 0;0 -2.5;0 0"
              dur=".55s"
              repeatCount="indefinite"
            />
          </Loop>
        </g>
      </g>
    </g>
  )
}

/** The piggy bank: what optimizations saved. A coin waits in its slot once something was. Centred on its 1.25 tiles. */
export function PiggyArt({ i, j, full }: { i: number; j: number; full: boolean }) {
  const [x, y] = P(i + 0.62, j + 0.62)
  const by = y - 22
  return (
    <>
      <ellipse cx={x + 6} cy={y + 2} rx={30} ry={9} fill={ART.shadow} />
      {/* Back legs, then the body, then the front legs. */}
      <rect x={x + 9} y={by + 12} width={7} height={9} fill="#d9779d" stroke="#000" />
      <rect x={x - 6} y={by + 12} width={7} height={9} fill="#d9779d" stroke="#000" />
      <ellipse cx={x} cy={by} rx={27.5} ry={19} fill="#f6a8c6" stroke={INK} />
      <rect x={x - 14} y={by + 13} width={7} height={15} fill="#e98aae" stroke="#000" />
      <rect x={x + 14} y={by + 13} width={7} height={15} fill="#e98aae" stroke="#000" />
      <ellipse
        cx={x - 27}
        cy={by + 1}
        rx={5.5}
        ry={4.5}
        fill="#f390b4"
        stroke={INK}
        strokeWidth={0.9}
      />
      <circle cx={x - 18} cy={by - 6} r={2.2} fill={INK} />
      <circle cx={x - 23} cy={by - 8} r={2.2} fill={INK} />
      <path d={`M${x + 1},${by + 3}l11,-4l-3,10z`} fill="#ef93b6" stroke={INK} strokeWidth={0.9} />
      <path
        d={`M${x - 6},${by - 15}q12,-3 25,1`}
        stroke="#561630"
        strokeWidth={5}
        strokeLinecap="round"
        fill="none"
      />
      {full ? <circle cx={x + 4} cy={by - 22} r={6} fill={ART.coin} stroke={INK} /> : null}
    </>
  )
}

/* ---------- scenery ---------- */

/** A round tree on its trunk: [i, j, scale]; 1 is the reference's big tree, about 0.42 its small one. */
export function Tree({ t }: { t: readonly [number, number, number] }) {
  const S = world().skin.Tree
  if (S) return <S t={t} />
  const [i, j, s] = t
  const [x, y] = P(i, j)
  const r = 22.2 * s
  const trunk = 21 * s
  return (
    <>
      <ellipse cx={x + 12 * s} cy={y + 2} rx={21 * s} ry={9 * s} fill="rgba(0,0,0,.14)" />
      <rect
        x={x - 4.1 * s}
        y={y - trunk}
        width={8.3 * s}
        height={trunk}
        fill={ART.trunk}
        stroke={INK}
        strokeWidth={0.8}
      />
      <circle cx={x} cy={y - trunk - r * 0.7} r={r} fill={ART.leaf} stroke={INK} />
      <circle cx={x - r * 0.33} cy={y - trunk - r * 1.05} r={r * 0.45} fill={ART.leafHi} />
    </>
  )
}

/** A bush: two or three overlapping mounds. */
export function Bush({ b }: { b: readonly [number, number, number] }) {
  const S = world().skin.Bush
  if (S) return <S b={b} />
  const [i, j, s] = b
  const [x, y] = P(i, j)
  return (
    <>
      <ellipse cx={x + 3 * s} cy={y + 1} rx={20 * s} ry={7 * s} fill="rgba(0,0,0,.12)" />
      <ellipse
        cx={x - 7 * s}
        cy={y - 9 * s}
        rx={16 * s}
        ry={11.5 * s}
        fill={ART.leaf}
        stroke={INK}
      />
      <ellipse
        cx={x + 9 * s}
        cy={y - 7 * s}
        rx={13 * s}
        ry={9.5 * s}
        fill={ART.leaf}
        stroke={INK}
      />
    </>
  )
}

/** A grass tuft: three strokes. */
export function Tuft({ p }: { p: Pt }) {
  const S = world().skin.Tuft
  if (S) return <S p={p} />
  const [x, y] = p
  return (
    <path
      d={`M${x - 3.6},${y}l1.4,-6M${x},${y}l0.3,-6.6M${x + 3.4},${y}l-1.2,-5.6`}
      stroke={ART.tuft}
      strokeWidth={1.1}
      strokeLinecap="round"
      fill="none"
    />
  )
}

/* ---------- weather ---------- */

export function Smoke({ x, y, dark }: { x: number; y: number; dark: boolean }) {
  const c = dark ? '#4d4d52' : '#cfcfd4'
  const on = useContext(MotionOn)
  if (!on)
    return (
      <>
        <circle cx={x + 3} cy={y - 8} r={5} fill={c} opacity={0.75} />
        <circle cx={x + 7} cy={y - 18} r={7} fill={c} opacity={0.5} />
      </>
    )
  return (
    <>
      {[0, 0.85, 1.7].map((dl) => (
        <circle key={dl} cx={x} cy={y} r={dark ? 6 : 5} fill={c} opacity={0}>
          <animate
            attributeName="cy"
            values={`${y};${y - 34}`}
            dur="2.6s"
            begin={`${dl}s`}
            repeatCount="indefinite"
          />
          <animate
            attributeName="cx"
            values={`${x};${x + 10}`}
            dur="2.6s"
            begin={`${dl}s`}
            repeatCount="indefinite"
          />
          <animate
            attributeName="r"
            values={dark ? '3;10' : '2.5;8.5'}
            dur="2.6s"
            begin={`${dl}s`}
            repeatCount="indefinite"
          />
          <animate
            attributeName="opacity"
            values="0;.85;0"
            keyTimes="0;.2;1"
            dur="2.6s"
            begin={`${dl}s`}
            repeatCount="indefinite"
          />
        </circle>
      ))}
    </>
  )
}

export function Storm({ x, y, dark }: { x: number; y: number; dark: boolean }) {
  const c = dark ? '#5d6370' : '#b9bec8'
  return (
    <>
      <g>
        <circle cx={x - 15} cy={y} r={10} fill={c} />
        <circle cx={x} cy={y - 7} r={14} fill={c} />
        <circle cx={x + 15} cy={y} r={10} fill={c} />
        <rect x={x - 25} y={y - 2} width={50} height={11} rx={5.5} fill={c} />
        <Loop>
          <animateTransform
            attributeName="transform"
            type="translate"
            values="0 0;6 0;0 0"
            dur="10s"
            repeatCount="indefinite"
          />
        </Loop>
      </g>
      {[-2, -1, 0, 1, 2].map((k) => (
        <line
          key={`r${k}`}
          x1={x + k * 9}
          y1={y + 12}
          x2={x + k * 9 - 4}
          y2={y + 30}
          stroke={dark ? '#6f8fb3' : '#9db6d1'}
          strokeWidth={1.6}
          strokeDasharray="4 7"
        >
          <Loop>
            <animate
              attributeName="stroke-dashoffset"
              values="0;-22"
              dur=".7s"
              repeatCount="indefinite"
            />
          </Loop>
        </line>
      ))}
      {dark ? (
        <polygon
          points={`${x + 2},${y + 8} ${x - 6},${y + 24} ${x},${y + 24} ${x - 4},${y + 38} ${x + 9},${y + 19} ${x + 3},${y + 19} ${x + 7},${y + 8}`}
          fill="#ffd23f"
          stroke="#c99a00"
        >
          <Loop>
            <animate
              attributeName="opacity"
              values="0;0;1;1;0;0"
              keyTimes="0;.58;.6;.7;.72;1"
              dur="2.2s"
              repeatCount="indefinite"
            />
          </Loop>
        </polygon>
      ) : null}
    </>
  )
}

// The cactus is drawn by its own character engine: a chunk of its own, loaded when Kiko first shows.
const CactusMascot = deferred(() => import('./mascot/CactusMascot').then((m) => m.CactusMascot))

/** Kiko, the guide, in one of four moods: the cactus mascot (`mascot/CactusMascot.tsx`). */
export function KikoArt({ mood }: { mood: 'calm' | 'hand' | 'sign' | 'flare' }) {
  return <CactusMascot mood={mood} className="size-full overflow-visible" />
}
