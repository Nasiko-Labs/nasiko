/**
 * Tundra (reference: a snowy steppe camp): snow for grass, icy rock cliffs, trodden earth for the paths and the coding
 * tools' yard, felt yurts with blue domes for the team houses and a three-yurt camp for the town hall, snowy pines,
 * snow-capped rocks, and more pines and rocks on the snowfield round the island.
 */
import { Battery, Flag, MineShape } from '../art'
import { CLIFF, P, type Pt } from '../geo'
import type { Mine, Team } from '../model'
import { ART, INK, shade } from '../palette'
import { Cyl, Poly } from '../solids'
import { blobPath, iso, seeded } from './paths'
import { Dome } from './shapes'

export const SNOW = '#f4f7fa'
const FELT = '#ebe0c6'
const BLUE = '#3f7cc4'

/** A yurt centred on tile point (ci, cj): felt walls with a coloured band, a dome roof, a door on the front. */
function Yurt({
  ci,
  cj,
  r,
  wall,
  dome,
  roof,
  band = roof,
  flag,
}: {
  ci: number
  cj: number
  r: number
  wall: number
  dome: number
  roof: string
  band?: string
  flag?: string
}) {
  const [cx, cy] = P(ci, cj)
  const [rx, ry] = iso(r)
  const dx = cx - rx * 0.42
  const dy = cy + ry * 0.9
  const dh = wall * 0.72
  return (
    <>
      <ellipse cx={cx + rx * 0.3} cy={cy + 2} rx={rx * 1.2} ry={ry * 1.15} fill={ART.shadow} />
      <Cyl ci={ci} cj={cj} r={r} h={wall} color={FELT} top={FELT} />
      <Cyl ci={ci} cj={cj} r={r} h={Math.max(2.5, wall * 0.2)} z={wall * 0.62} color={band} open />
      <path
        d={`M${dx - rx * 0.13},${dy}v${-dh}h${rx * 0.26}v${dh}z`}
        fill="#4a3528"
        stroke={INK}
        strokeWidth={0.8}
      />
      <Dome cx={cx} cy={cy - wall} rx={rx * 1.04} ry={ry * 1.04} h={dome} color={roof} />
      <ellipse
        cx={cx}
        cy={cy - wall - dome * 0.94}
        rx={rx * 0.24}
        ry={ry * 0.24}
        fill={FELT}
        stroke={INK}
        strokeWidth={0.8}
      />
      {flag ? <Flag p={[cx, cy - wall - dome * 0.94]} color={flag} h={24} /> : null}
    </>
  )
}

export function TeamYurt({ t }: { t: Pick<Team, 'i' | 'j' | 'color' | 'budget' | 'pace'> }) {
  const { i, j } = t
  const b = t.budget
  const left = b && b.limit > 0 ? Math.max(0, 1 - b.spent / b.limit) : null
  return (
    <>
      <Yurt ci={i + 0.66} cj={j + 0.66} r={0.64} wall={13} dome={16} roof={t.color} band={BLUE} />
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

/** The town hall: the camp's great yurt between two small ones, on trodden earth. */
export function Camp({ i, j }: { i: number; j: number }) {
  const w = 3.7
  return (
    <>
      <path
        d={blobPath(i + w / 2, j + w / 2, 2.35, 7, 0.22)}
        fill={ART.dirt}
        stroke={INK}
        strokeWidth={0.8}
      />
      <path d={blobPath(i + w / 2, j + w / 2, 1.7, 11, 0.3)} fill={ART.dirtTile} />
      <Yurt ci={i + 3.1} cj={j + 0.55} r={0.55} wall={12} dome={13} roof={BLUE} />
      <Yurt ci={i + 0.55} cj={j + 3.1} r={0.55} wall={12} dome={13} roof={BLUE} />
      <Yurt
        ci={i + w / 2}
        cj={j + w / 2}
        r={1.32}
        wall={24}
        dome={32}
        roof={BLUE}
        band="#2f5f9e"
        flag="#e5513b"
      />
    </>
  )
}

/** A snowy pine: three tiers, lit on the left, snow on each tip. [i, j, scale]. */
export function Pine({ t }: { t: readonly [number, number, number] }) {
  const [i, j, s] = t
  const [x, y] = P(i, j)
  const leaf = ART.leaf
  return (
    <>
      <ellipse cx={x + 10 * s} cy={y + 2} rx={18 * s} ry={7 * s} fill={ART.shadow} />
      <rect
        x={x - 3 * s}
        y={y - 9 * s}
        width={6 * s}
        height={10 * s}
        fill={ART.trunk}
        stroke={INK}
        strokeWidth={0.8}
      />
      {[0, 1, 2].map((k) => {
        const yb = y - 6 * s - k * 15 * s
        const hw = (19 - k * 4.6) * s
        const th = 25 * s
        const tip: Pt = [x, yb - th]
        return (
          <g key={k}>
            <Poly p={[[x - hw, yb], [x, yb + 3 * s], tip]} fill={leaf} />
            <Poly p={[[x, yb + 3 * s], [x + hw, yb], tip]} fill={shade(leaf, -0.28)} />
            <Poly p={[[x - hw, yb], [x, yb + 3 * s], [x + hw, yb], tip]} fill="none" stroke={INK} />
            <Poly
              p={[
                [x - hw * 0.42, yb - th * 0.58],
                tip,
                [x + hw * 0.42, yb - th * 0.58],
                [x + hw * 0.16, yb - th * 0.5],
                [x, yb - th * 0.58],
                [x - hw * 0.2, yb - th * 0.48],
              ]}
              fill={SNOW}
              stroke={INK}
              width={0.6}
            />
          </g>
        )
      })}
    </>
  )
}

/** Snow-capped rocks: a big one and a small one. [i, j, scale]. */
export function SnowRock({ b }: { b: readonly [number, number, number] }) {
  const [i, j, s] = b
  const [x0, y0] = P(i, j)
  const rock = (x: number, y: number, k: number) => {
    const q = (a: number, c: number): Pt => [x + a * k, y + c * k]
    return (
      <>
        <Poly
          p={[q(-16, 0), q(-13, -10), q(-4, -16), q(8, -14), q(16, -4), q(13, 2), q(0, 4)]}
          fill="#6f7884"
          stroke={INK}
        />
        <Poly p={[q(-4, -16), q(8, -14), q(16, -4), q(13, 2), q(0, 4), q(2, -8)]} fill="#59616c" />
        <Poly
          p={[q(-13, -10), q(-4, -16), q(8, -14), q(13, -8), q(4, -9), q(-3, -7), q(-9, -5)]}
          fill={SNOW}
          stroke={INK}
          width={0.6}
        />
      </>
    )
  }
  return (
    <>
      <ellipse cx={x0 + 4 * s} cy={y0 + 2} rx={22 * s} ry={7 * s} fill={ART.shadow} />
      {rock(x0, y0, s)}
      {rock(x0 + 15 * s, y0 + 4 * s, s * 0.55)}
    </>
  )
}

/** Dry grass poking through the snow. */
export function DryGrass({ p }: { p: Pt }) {
  const [x, y] = p
  return (
    <path
      d={`M${x - 3.6},${y}l1.4,-6M${x},${y}l0.3,-7M${x + 3.4},${y}l-1.2,-5.6M${x + 1.6},${y}l2.6,-4`}
      stroke={ART.tuft}
      strokeWidth={1.1}
      strokeLinecap="round"
      fill="none"
    />
  )
}

/** The snowfield round the island: pines and rocks behind it, drifts beside it. Drawn at the cliffs' foot. */
export function Snowfield() {
  const rnd = seeded(41)
  const pines: [number, number, number][] = []
  for (let k = 0; k < 16; k++) pines.push([-1.5 - rnd() * 4, rnd() * 17, 0.55 + rnd() * 0.5])
  for (let k = 0; k < 14; k++) pines.push([rnd() * 26, -1.5 - rnd() * 4, 0.55 + rnd() * 0.5])
  pines.sort((a, b) => a[0] + a[1] - (b[0] + b[1]))
  return (
    <g transform={`translate(0,${CLIFF})`}>
      {(
        [
          [-3, 6, 3.2, 3],
          [8, -3, 3, 5],
          [22, -3, 2.6, 9],
        ] as const
      ).map(([i, j, r, sd]) => (
        <path key={sd} d={blobPath(i, j, r, sd, 0.35)} fill="#e3e9ef" />
      ))}
      {pines.map((t, k) =>
        k % 5 === 2 ? (
          <SnowRock key={`${t[0]},${t[1]}`} b={[t[0], t[1], t[2] * 0.9]} />
        ) : (
          <Pine key={`${t[0]},${t[1]}`} t={t} />
        ),
      )}
    </g>
  )
}

/** Drifts and bare patches on the island's snow. */
export function Drifts() {
  return (
    <>
      {(
        [
          [3, 3, 1.6, 1],
          [14, 3, 1.4, 2],
          [16, 9, 1.8, 3],
          [2, 16, 1.5, 4],
          [9, 15.5, 1.2, 5],
          [23, 6, 1.5, 6],
        ] as const
      ).map(([i, j, r, sd]) => (
        <path key={sd} d={blobPath(i, j, r, sd, 0.4)} fill="#dfe6ee" opacity={0.85} />
      ))}
    </>
  )
}

/** The theme's mine, drawn by the village's own shape in this world's tones. */
export function TundraMine({ m }: { m: Pick<Mine, 'i' | 'j' | 'w' | 'color' | 'mono'> }) {
  return (
    <MineShape
      m={m}
      tone={{ left: '#cdbd9c', right: '#ab9a7a', door: '#4a3528', board: '#d8ccb4' }}
    />
  )
}
