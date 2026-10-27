/**
 * Desert (reference: a flat-shaded mesa island on a teal sea): pale sand for grass with peach and orange patches,
 * faceted grey mesas with brown caps for trees, capped boulders for bushes, pebbles for tufts, sandstone houses with
 * a dome in the team's colour, a domed sandstone keep for the town hall, and a sand shore round the island with
 * pebbles and holes through to the sea.
 */
import { Battery, Flag, MineShape } from '../art'
import { CLIFF, P, type Pt } from '../geo'
import type { Mine, Team } from '../model'
import { ART, INK, shade } from '../palette'
import { Box, Cyl, FaceL, FaceR, Foot, Poly } from '../solids'
import { blobAt, blobPath, iso, seeded } from './paths'
import { Dome } from './shapes'

const SEA = '#3b5c60'
const SHORE = '#e2d5bb'
const PEACH = '#f2ae86'
const ORANGE = '#e9965f'
const STONE = '#e4cca2'
const TERRACOTTA = '#cf6a40'
const ROCK_L = '#8e9299'
const ROCK_R = '#6d7179'
const ROCK_FACET = '#9fa3aa'
const CAP = '#7b5b44'

/**
 * A mesa: a faceted rock column with a flat brown cap, `w` px half-wide and `h` px tall, standing on (x, y).
 * Its faces taper a little and carry lighter facets, like the reference's.
 */
function Mesa({ x, y, w, h }: { x: number; y: number; w: number; h: number }) {
  const tw = w * 0.86
  const front = w * 0.5
  const L: Pt[] = [
    [x - w, y],
    [x, y + front],
    [x, y + front * 0.86 - h],
    [x - tw, y - h],
  ]
  const R: Pt[] = [
    [x, y + front],
    [x + w, y],
    [x + tw, y - h],
    [x, y + front * 0.86 - h],
  ]
  const top = y - h
  return (
    <>
      <ellipse
        cx={x + w * 0.7}
        cy={y + front * 0.6}
        rx={w * 1.35}
        ry={w * 0.55}
        fill={ART.shadow}
      />
      <Poly p={L} fill={ROCK_L} stroke={INK} />
      <Poly
        p={[
          [x - w * 0.62, y - h * 0.12],
          [x - w * 0.18, y + front * 0.55 - h * 0.1],
          [x - w * 0.26, y - h * 0.62],
          [x - w * 0.7, y - h * 0.7],
        ]}
        fill={ROCK_FACET}
      />
      <Poly p={R} fill={ROCK_R} stroke={INK} />
      <Poly
        p={[
          [x + w * 0.24, y + front * 0.55 - h * 0.3],
          [x + w * 0.78, y - h * 0.2],
          [x + w * 0.72, y - h * 0.78],
          [x + w * 0.3, y - h * 0.62],
        ]}
        fill={shade(ROCK_R, 0.1)}
      />
      <Poly
        p={[
          [x - tw, top],
          [x - tw * 0.4, top - front * 0.62],
          [x + tw * 0.45, top - front * 0.6],
          [x + tw, top],
          [x + tw * 0.3, top + front * 0.82],
          [x - tw * 0.5, top + front * 0.6],
        ]}
        fill={CAP}
        stroke={INK}
      />
      <ellipse
        cx={x}
        cy={top + front * 0.1}
        rx={tw * 0.55}
        ry={front * 0.42}
        fill={shade(CAP, 0.12)}
      />
    </>
  )
}

export function MesaTree({ t }: { t: readonly [number, number, number] }) {
  const [i, j, s] = t
  const [x, y] = P(i, j)
  return <Mesa x={x} y={y} w={18 * s} h={58 * s} />
}

export function Boulder({ b }: { b: readonly [number, number, number] }) {
  const [i, j, s] = b
  const [x, y] = P(i, j)
  return (
    <>
      <Mesa x={x - 6 * s} y={y} w={12 * s} h={13 * s} />
      <Mesa x={x + 11 * s} y={y + 4 * s} w={7 * s} h={7 * s} />
    </>
  )
}

export function Pebbles({ p }: { p: Pt }) {
  const [x, y] = p
  return (
    <>
      <ellipse
        cx={x - 2.5}
        cy={y - 1}
        rx={2.6}
        ry={1.7}
        fill="#a39d93"
        stroke={INK}
        strokeWidth={0.5}
      />
      <ellipse
        cx={x + 2.4}
        cy={y}
        rx={1.7}
        ry={1.2}
        fill="#f2ede4"
        stroke={INK}
        strokeWidth={0.5}
      />
    </>
  )
}

/** A sandstone house: flat roof with corner merlons, a dome in the team's colour, an awning over the door. */
export function SandHouse({ t }: { t: Pick<Team, 'i' | 'j' | 'color' | 'budget' | 'pace'> }) {
  const { i, j } = t
  const s = 1.31
  const h = 19.3
  const b = t.budget
  const left = b && b.limit > 0 ? Math.max(0, 1 - b.spent / b.limit) : null
  const [cx, cy] = P(i + s / 2, j + s / 2)
  const [rx, ry] = iso(0.44)
  const merlon = (mi: number, mj: number) => (
    <Box key={`${mi},${mj}`} i={mi} j={mj} w={0.2} d={0.2} h={5} z={h} color={STONE} />
  )
  return (
    <>
      <Foot i={i + 0.22} j={j + 0.06} w={s} d={s} fill={ART.shadow} />
      <Box i={i} j={j} w={s} d={s} h={h} color={STONE} right="#cbb085" top="#efdcba" />
      <FaceL i0={i + 0.42} i1={i + 0.82} jp={j + s} z0={0} z1={11} color="#5b3c29" />
      <Poly
        p={[
          [P(i + 0.34, j + s)[0], P(i + 0.34, j + s)[1] - 14],
          [P(i + 0.9, j + s)[0], P(i + 0.9, j + s)[1] - 14],
          [P(i + 0.9, j + s + 0.28)[0], P(i + 0.9, j + s + 0.28)[1] - 9],
          [P(i + 0.34, j + s + 0.28)[0], P(i + 0.34, j + s + 0.28)[1] - 9],
        ]}
        fill={t.color}
        stroke={INK}
      />
      <FaceR ip={i + s} j0={j + 0.45} j1={j + 0.78} z0={9} z1={15} color="#3f2c20" />
      {merlon(i, j)}
      <Dome cx={cx} cy={cy - h} rx={rx} ry={ry} h={15} color={t.color} />
      {merlon(i + s - 0.2, j)}
      {merlon(i, j + s - 0.2)}
      {merlon(i + s - 0.2, j + s - 0.2)}
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

/** A round tower with a small terracotta dome. */
function Tower({ ci, cj, z, h }: { ci: number; cj: number; z: number; h: number }) {
  const [cx, cy] = P(ci, cj)
  const [rx, ry] = iso(0.42)
  return (
    <>
      <Cyl ci={ci} cj={cj} r={0.42} h={h} z={z} color={STONE} top="#d6bb8f" />
      <Dome cx={cx} cy={cy - z - h} rx={rx * 1.05} ry={ry * 1.05} h={16} color={TERRACOTTA} />
    </>
  )
}

/** The town hall: a sandstone keep with a great terracotta dome and four round towers. */
export function Keep({ i, j }: { i: number; j: number }) {
  const w = 3.7
  const wi = i + 0.35
  const wj = j + 0.35
  const base = 6
  const wall = 28.8
  const [cx, cy] = P(wi + 1.5, wj + 1.5)
  const [rx, ry] = iso(1.05)
  const drum = 9
  const top = cy - base - wall - drum
  return (
    <>
      <Foot i={i + 0.45} j={j + 0.1} w={w} d={w} fill={ART.shadow} />
      <Box i={i} j={j} w={w} d={w} h={base} color="#d6c09a" right="#bba47b" top="#e5d3b0" />
      <Tower ci={wi} cj={wj} z={base} h={44} />
      <Box
        i={wi}
        j={wj}
        w={3}
        d={3}
        h={wall}
        z={base}
        color={STONE}
        right="#cbb085"
        top="#efdcba"
      />
      {[0.4, 2.1].map((k) => (
        <FaceL
          key={k}
          i0={wi + k}
          i1={wi + k + 0.42}
          jp={wj + 3}
          z0={base + 10}
          z1={base + 22}
          color="#5b3c29"
        />
      ))}
      <FaceL i0={wi + 1.2} i1={wi + 1.8} jp={wj + 3} z0={base} z1={base + 18} color="#6b4630" />
      {[0.7, 1.9].map((k) => (
        <FaceR
          key={k}
          ip={wi + 3}
          j0={wj + 3 - k - 0.4}
          j1={wj + 3 - k}
          z0={base + 10}
          z1={base + 22}
          color="#4a3324"
        />
      ))}
      <Cyl
        ci={wi + 1.5}
        cj={wj + 1.5}
        r={1.05}
        h={drum}
        z={base + wall}
        color="#e8d3ab"
        top="#e8d3ab"
      />
      <Dome cx={cx} cy={cy - base - wall - drum} rx={rx} ry={ry} h={34} color={TERRACOTTA} />
      <Flag p={[cx, top - 32]} color="#e5513b" h={22} />
      <Tower ci={wi} cj={wj + 3} z={base} h={44} />
      <Tower ci={wi + 3} cj={wj} z={base} h={44} />
      <Tower ci={wi + 3} cj={wj + 3} z={base} h={44} />
    </>
  )
}

/** Is a screen point over the island (with its cliffs)? Inverts the iso projection. */
function onIsland(x: number, y: number, pad = 0.6) {
  for (const dy of [0, CLIFF]) {
    const a = (x / 24 + (y - dy) / 12) / 2
    const c = ((y - dy) / 12 - x / 24) / 2
    if (a > -pad && a < 19 + pad && c > -pad && c < 18 + pad) return true
    if (a > 19 - pad && a < 27 + pad && c > -pad && c < 8 + pad) return true
  }
  return false
}

/** The shore round the island: a sand blob with a lobe, holes through to the sea, pebbles, and mesas behind. */
export function Shore() {
  const shapes = [
    blobAt(110, 250, 640, 330, 3, 0.16, 16),
    blobAt(-250, 520, 190, 95, 5, 0.3, 10),
    blobAt(560, 520, 150, 70, 8, 0.3, 10),
  ]
  const holes = [
    [-420, 370, 60, 26, 2],
    [690, 270, 46, 22, 4],
    [-330, 70, 50, 22, 6],
  ] as const
  const rnd = seeded(17)
  const pebbles: [number, number, number, boolean][] = []
  while (pebbles.length < 70) {
    const a = rnd() * Math.PI * 2
    const r = 0.55 + rnd() * 0.45
    const x = 110 + Math.cos(a) * 640 * r
    const y = 250 + Math.sin(a) * 330 * r
    if (!onIsland(x, y)) pebbles.push([x, y, 1.4 + rnd() * 2.6, rnd() > 0.6])
  }
  const mesas: [number, number, number][] = [
    [-3.4, 4, 1.1],
    [-2.6, 9.5, 0.7],
    [-3.8, 14, 0.9],
    [5, -3.2, 1.25],
    [11, -2.6, 0.75],
    [17, -3.6, 1],
    [24, -2.8, 0.8],
  ]
  return (
    <>
      {shapes.map((d) => (
        <path key={`o${d}`} d={d} fill={SHORE} stroke={INK} strokeWidth={2.4} />
      ))}
      {shapes.map((d) => (
        <path key={`f${d}`} d={d} fill={SHORE} />
      ))}
      {holes.map(([x, y, rx, ry, sd]) => (
        <path
          key={sd}
          d={blobAt(x, y, rx, ry, sd, 0.3, 9)}
          fill={SEA}
          stroke={INK}
          strokeWidth={1.2}
        />
      ))}
      {pebbles.map(([x, y, r, light]) => (
        <ellipse
          key={`${x},${y}`}
          cx={x}
          cy={y}
          rx={r}
          ry={r * 0.7}
          fill={light ? '#f4efe6' : '#9b958b'}
          stroke={INK}
          strokeWidth={0.5}
        />
      ))}
      <g transform={`translate(0,${CLIFF})`}>
        {mesas.map(([i, j, s]) => (
          <MesaTree key={`${i},${j}`} t={[i, j, s]} />
        ))}
      </g>
    </>
  )
}

/** The peach and orange patches on the island's sand, with darker speckles. */
export function Patches() {
  const spots = [
    [3, 3, 2.2, 1],
    [9.5, 1.8, 1.6, 2],
    [15, 6, 2.5, 3],
    [7, 10, 1.9, 4],
    [13.5, 13.5, 2.6, 5],
    [3, 15, 1.8, 6],
    [17.5, 15.5, 1.5, 7],
    [10, 16.4, 1.3, 8],
    [22.5, 5, 2.1, 9],
    [25.5, 1.5, 1.2, 10],
  ] as const
  return (
    <>
      {spots.map(([i, j, r, sd]) => (
        <g key={sd}>
          <path d={blobPath(i, j, r, sd * 3, 0.34)} fill={PEACH} opacity={0.92} />
          <path
            d={blobPath(i + 0.15, j - 0.1, r * 0.62, sd * 5, 0.42)}
            fill={ORANGE}
            opacity={0.9}
          />
          <path
            d={blobPath(i - 0.35, j + 0.3, r * 0.2, sd * 7, 0.5, 7)}
            fill="#c97848"
            opacity={0.85}
          />
        </g>
      ))}
    </>
  )
}

/** The theme's mine, drawn by the village's own shape in this world's tones. */
export function DesertMine({ m }: { m: Pick<Mine, 'i' | 'j' | 'w' | 'color' | 'mono'> }) {
  return (
    <MineShape
      m={m}
      tone={{ left: '#ead9b8', right: '#cbb893', door: '#5b3c29', board: '#c4a982' }}
    />
  )
}
