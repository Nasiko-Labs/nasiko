/**
 * Foundry (steampunk): bronze deck plates for grass with rivets at the seams, rusted brick cliffs, brass walkways for
 * the paths, a coal yard for the coding tools, smokestacks for trees, turning gears for bushes, bolts for tufts, brick
 * workshops with a roof in the team's colour and a smoking chimney, and a brass clockwork hall with a verdigris dome
 * for the town hall. Big dim gears turn in the dark behind the island.
 */
import { useContext } from 'react'
import { Battery, Flag, MineShape, Smoke } from '../art'
import { CLIFF, MAIN, P, type Pt } from '../geo'
import type { Mine, Team } from '../model'
import { MotionOn } from '../motionContext'
import { ART, INK, shade } from '../palette'
import { Box, Cyl, FaceR, Foot, Gable } from '../solids'
import { blobPath, iso } from './paths'
import { Dome, UprightGear } from './shapes'

const BRICK = '#97503a'
const BRASS = '#c4913f'
const COPPER = '#c27a48'
const VERDIGRIS = '#5f9c86'
const AMBER = '#ffc96e'

/** A brick smokestack with copper bands, smoking. [i, j, scale]. */
export function Stack({ t }: { t: readonly [number, number, number] }) {
  const [i, j, s] = t
  const [x, y] = P(i, j)
  const h = 62 * s
  const r = 0.3 * Math.max(0.6, s)
  return (
    <>
      <ellipse cx={x + 9 * s} cy={y + 2} rx={16 * s} ry={6 * s} fill={ART.shadow} />
      <Cyl ci={i} cj={j} r={r} h={h} color={BRICK} top="#3a2219" />
      <Cyl ci={i} cj={j} r={r * 1.08} h={3.5} z={h * 0.3} color={COPPER} open />
      <Cyl ci={i} cj={j} r={r * 1.12} h={5} z={h - 5} color={COPPER} top="#2a1a14" />
      <Smoke x={x} y={y - h - 2} dark />
    </>
  )
}

/** A gear on a little iron stand, turning. [i, j, scale]. */
export function GearStand({ b }: { b: readonly [number, number, number] }) {
  const [i, j, s] = b
  const [x, y] = P(i, j)
  const r = 13 * s
  return (
    <>
      <ellipse cx={x + 6 * s} cy={y + 2} rx={18 * s} ry={6 * s} fill={ART.shadow} />
      <Box i={i - 0.18 * s} j={j - 0.1 * s} w={0.36 * s} d={0.2 * s} h={6 * s} color="#4f4740" />
      <UprightGear x={x - 2 * s} y={y - 6 * s - r} r={r} color={BRASS} />
      <UprightGear
        x={x + 14 * s}
        y={y - 6 * s - r * 0.3}
        r={r * 0.55}
        color={COPPER}
        teeth={6}
        reverse
        dur={5}
      />
    </>
  )
}

/** Two bolts in the deck. */
export function Bolts({ p }: { p: Pt }) {
  const [x, y] = p
  return (
    <>
      <ellipse cx={x - 3} cy={y} rx={2.1} ry={1.3} fill={ART.tuft} stroke={INK} strokeWidth={0.5} />
      <ellipse
        cx={x + 3}
        cy={y - 1.5}
        rx={2.1}
        ry={1.3}
        fill={ART.tuft}
        stroke={INK}
        strokeWidth={0.5}
      />
    </>
  )
}

/** A porthole on the front-left wall: a brass ring round warm light. */
function Porthole({ at, r }: { at: Pt; r: number }) {
  return (
    <g transform={`translate(${at[0].toFixed(1)},${at[1].toFixed(1)}) matrix(1,0.5,0,1,0,0)`}>
      <circle r={r} fill={AMBER} stroke={BRASS} strokeWidth={2} />
      <circle r={r + 1} fill="none" stroke={INK} strokeWidth={0.8} />
    </g>
  )
}

/** A brick workshop: a gable roof in the team's colour, a porthole, a smoking chimney at the back. */
export function Workshop({ t }: { t: Pick<Team, 'i' | 'j' | 'color' | 'budget' | 'pace'> }) {
  const { i, j } = t
  const s = 1.31
  const h = 19.3
  const b = t.budget
  const left = b && b.limit > 0 ? Math.max(0, 1 - b.spent / b.limit) : null
  const [chx, chy] = P(i + 0.3, j + 0.3)
  const chH = 34
  const front = P(i + s / 2, j + s)
  return (
    <>
      <Foot i={i + 0.22} j={j + 0.06} w={s} d={s} fill={ART.shadow} />
      <Box i={i} j={j} w={s} d={s} h={h} color={BRICK} right="#7b3f2c" top="#a85c44" />
      <Cyl ci={i + 0.3} cj={j + 0.3} r={0.13} h={chH} z={h} color="#6e3a2a" top="#2a1a14" />
      <Smoke x={chx} y={chy - h - chH - 2} dark />
      <Gable i={i} j={j} w={s} d={s} z={h} rh={15} color={t.color} wall={BRICK} />
      <Porthole at={[front[0], front[1] - 10]} r={4.6} />
      <FaceR ip={i + s} j0={j + 0.4} j1={j + 0.8} z0={0} z1={12} color="#2b211c" />
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

/** The clock on the hall's front: a cream face, brass ring, turning hands. */
function Clock({ at, r }: { at: Pt; r: number }) {
  const on = useContext(MotionOn)
  return (
    <g transform={`translate(${at[0].toFixed(1)},${at[1].toFixed(1)}) matrix(1,0.5,0,1,0,0)`}>
      <circle r={r + 2.4} fill={BRASS} stroke={INK} />
      <circle r={r} fill="#f4ead2" stroke={INK} strokeWidth={0.8} />
      {[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11].map((k) => (
        <circle
          key={k}
          cx={Math.cos((k * Math.PI) / 6) * r * 0.8}
          cy={Math.sin((k * Math.PI) / 6) * r * 0.8}
          r={0.7}
          fill={INK}
        />
      ))}
      <line
        x1={0}
        y1={0}
        x2={0}
        y2={-r * 0.75}
        stroke={INK}
        strokeWidth={1.2}
        strokeLinecap="round"
      >
        {on ? (
          <animateTransform
            attributeName="transform"
            type="rotate"
            values="0;360"
            dur="12s"
            repeatCount="indefinite"
          />
        ) : null}
      </line>
      <line x1={0} y1={0} x2={r * 0.45} y2={0} stroke={INK} strokeWidth={1.6} strokeLinecap="round">
        {on ? (
          <animateTransform
            attributeName="transform"
            type="rotate"
            values="0;360"
            dur="144s"
            repeatCount="indefinite"
          />
        ) : null}
      </line>
    </g>
  )
}

/** The town hall: a brass clockwork hall on an iron plinth, a verdigris dome, two stacks behind, a gear at its side. */
export function ClockHall({ i, j }: { i: number; j: number }) {
  const w = 3.7
  const wi = i + 0.35
  const wj = j + 0.35
  const base = 6
  const wall = 28.8
  const [cx, cy] = P(wi + 1.5, wj + 1.5)
  const [rx, ry] = iso(1.1)
  const drum = 8
  const domeTop = cy - base - wall - drum - 32
  const side = P(wi + 3, wj + 1.5)
  return (
    <>
      <Foot i={i + 0.45} j={j + 0.1} w={w} d={w} fill={ART.shadow} />
      <Box i={i} j={j} w={w} d={w} h={base} color="#5a4b3f" right="#463a30" top="#6a5a4c" />
      {(
        [
          [wi + 0.45, wj + 0.4, 78],
          [wi + 1.5, wj + 0.3, 66],
        ] as const
      ).map(([ci, cj, h]) => {
        const [sx, sy] = P(ci, cj)
        return (
          <g key={ci}>
            <Cyl ci={ci} cj={cj} r={0.24} h={h} z={base} color={BRICK} top="#2a1a14" />
            <Cyl ci={ci} cj={cj} r={0.27} h={5} z={base + h - 5} color={COPPER} top="#2a1a14" />
            <Smoke x={sx} y={sy - base - h - 2} dark />
          </g>
        )
      })}
      <Box
        i={wi}
        j={wj}
        w={3}
        d={3}
        h={wall}
        z={base}
        color={BRASS}
        right="#9c7232"
        top="#d0a35a"
      />
      <Box i={wi - 0.05} j={wj - 0.05} w={3.1} d={3.1} h={3} z={base + wall} color={COPPER} />
      <Cyl
        ci={wi + 1.5}
        cj={wj + 1.5}
        r={1.1}
        h={drum}
        z={base + wall + 3}
        color="#b0834a"
        top="#b0834a"
      />
      <Dome cx={cx} cy={cy - base - wall - 3 - drum} rx={rx} ry={ry} h={32} color={VERDIGRIS} />
      <Flag p={[cx, domeTop - 2]} color={COPPER} h={22} />
      <Clock at={[P(wi + 1.5, wj + 3)[0], P(wi + 1.5, wj + 3)[1] - base - 17]} r={9} />
      <UprightGear
        x={side[0] + 6}
        y={side[1] - base - 16}
        r={11}
        color={COPPER}
        plane="right"
        teeth={9}
      />
    </>
  )
}

/** Big dim gears and a haze behind the island. */
export function Works() {
  return (
    <g transform={`translate(0,${CLIFF})`} opacity={0.75}>
      <UprightGear x={-300} y={-40} r={120} color="#33291f" stroke={false} teeth={12} dur={60} />
      <UprightGear
        x={-120}
        y={-110}
        r={64}
        color="#2c231b"
        stroke={false}
        teeth={9}
        dur={32}
        reverse
      />
      <UprightGear
        x={420}
        y={-90}
        r={95}
        color="#33291f"
        stroke={false}
        teeth={11}
        dur={48}
        reverse
      />
      <UprightGear x={620} y={20} r={58} color="#2c231b" stroke={false} teeth={8} dur={28} />
    </g>
  )
}

/** Rivets at the deck plates' corners, oil stains, and a brass rail round the main deck. */
export function Deck() {
  let d = ''
  for (let a = 0; a <= MAIN.ni; a += 2)
    for (let c = 0; c <= MAIN.nj; c += 2) {
      const [x, y] = P(a, c)
      d += `M${(x - 1.6).toFixed(1)},${y.toFixed(1)}a1.6,1 0 1 0 3.2,0a1.6,1 0 1 0 -3.2,0`
    }
  return (
    <>
      <path d={d} fill={shade(ART.grass, 0.25)} opacity={0.8} />
      {(
        [
          [6, 4, 1.1, 1],
          [12, 10, 1.4, 2],
          [3, 13, 0.9, 3],
          [16, 15, 1.2, 4],
        ] as const
      ).map(([i, j, r, sd]) => (
        <path key={sd} d={blobPath(i, j, r, sd * 13, 0.4)} fill="#1c1410" opacity={0.18} />
      ))}
    </>
  )
}

/** The theme's mine, drawn by the village's own shape in this world's tones. */
export function FoundryMine({ m }: { m: Pick<Mine, 'i' | 'j' | 'w' | 'color' | 'mono'> }) {
  return (
    <MineShape
      m={m}
      tone={{ left: '#76604a', right: '#584634', door: '#140f0c', board: '#b08d57' }}
    />
  )
}
