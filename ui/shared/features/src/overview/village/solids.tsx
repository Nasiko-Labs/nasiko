/**
 * The village's isometric solids (plans/feat-village-overview.md §4, V50): a box, hip and gable roofs, a wall face, a
 * cylinder, a footprint, a shadow and a line. Every building is made of these. As in the reference design, every face
 * is outlined in one dark ink and lit from the left: the front-left face in the base colour, the front-right one
 * darker, tops lighter. Plain SVG, no raw HTML; colours come from `palette.ts`.
 */
import { HH, HW, P, pts, up, type Pt } from './geo'
import { ART, INK, shade } from './palette'

export function Poly({
  p,
  fill,
  stroke,
  className,
  dash,
  width = 1,
}: {
  p: readonly Pt[]
  fill: string
  stroke?: string | null
  className?: string
  dash?: string
  width?: number
}) {
  return (
    <polygon
      points={pts(p)}
      fill={fill}
      stroke={stroke ?? undefined}
      strokeWidth={stroke ? width : undefined}
      strokeLinejoin="round"
      strokeDasharray={dash}
      className={className}
    />
  )
}

/** A box on tiles (i, j) of w × d, h px tall, standing z px up; `left`, `right` and `top` override the faces. */
export function Box({
  i,
  j,
  w,
  d,
  h,
  color,
  z = 0,
  top,
  left,
  right,
}: {
  i: number
  j: number
  w: number
  d: number
  h: number
  color: string
  z?: number
  top?: string
  left?: string
  right?: string
}) {
  const A = up(P(i, j), z)
  const B = up(P(i + w, j), z)
  const C = up(P(i + w, j + d), z)
  const D = up(P(i, j + d), z)
  return (
    <>
      <Poly p={[D, C, up(C, h), up(D, h)]} fill={left ?? color} stroke={INK} />
      <Poly p={[C, B, up(B, h), up(C, h)]} fill={right ?? shade(color, -0.15)} stroke={INK} />
      <Poly
        p={[up(A, h), up(B, h), up(C, h), up(D, h)]}
        fill={top ?? shade(color, 0.12)}
        stroke={INK}
      />
    </>
  )
}

/** A hip roof over a box's top, `rh` px high. */
export function Roof({
  i,
  j,
  w,
  d,
  z,
  rh,
  color,
  o = 0.14,
}: {
  i: number
  j: number
  w: number
  d: number
  z: number
  rh: number
  color: string
  o?: number
}) {
  const A = up(P(i - o, j - o), z)
  const B = up(P(i + w + o, j - o), z)
  const C = up(P(i + w + o, j + d + o), z)
  const D = up(P(i - o, j + d + o), z)
  const ap = up(P(i + w / 2, j + d / 2), z + rh)
  return (
    <>
      <Poly p={[A, B, ap]} fill={shade(color, 0.08)} stroke={INK} />
      <Poly p={[A, D, ap]} fill={shade(color, 0.13)} stroke={INK} />
      <Poly p={[D, C, ap]} fill={color} stroke={INK} />
      <Poly p={[C, B, ap]} fill={shade(color, -0.25)} stroke={INK} />
      <Line a={ap} b={C} c={INK} w={1} />
    </>
  )
}

/** A gable roof whose ridge runs along i. */
export function Gable({
  i,
  j,
  w,
  d,
  z,
  rh,
  color,
  wall,
  o = 0.12,
}: {
  i: number
  j: number
  w: number
  d: number
  z: number
  rh: number
  color: string
  wall: string
  o?: number
}) {
  const A = up(P(i - o, j - o), z)
  const B = up(P(i + w + o, j - o), z)
  const C = up(P(i + w + o, j + d + o), z)
  const D = up(P(i - o, j + d + o), z)
  const Ra = up(P(i - o, j + d / 2), z + rh)
  const Rb = up(P(i + w + o, j + d / 2), z + rh)
  const g = [up(P(i + w, j), z), up(P(i + w, j + d), z), up(P(i + w, j + d / 2), z + rh * 0.92)]
  return (
    <>
      <Poly p={[A, B, Rb, Ra]} fill={shade(color, 0.14)} stroke={INK} />
      <Poly p={g} fill={shade(wall, -0.15)} stroke={INK} />
      <Poly p={[D, C, Rb, Ra]} fill={color} stroke={INK} />
    </>
  )
}

/** A face on the front-left plane (constant j), from i0 to i1, z0 to z1 px. */
export function FaceL({
  i0,
  i1,
  jp,
  z0,
  z1,
  color,
}: {
  i0: number
  i1: number
  jp: number
  z0: number
  z1: number
  color: string
}) {
  return (
    <Poly
      p={[up(P(i0, jp), z0), up(P(i1, jp), z0), up(P(i1, jp), z1), up(P(i0, jp), z1)]}
      fill={color}
      stroke={INK}
    />
  )
}

/** A face on the front-right plane (constant i). */
export function FaceR({
  ip,
  j0,
  j1,
  z0,
  z1,
  color,
}: {
  ip: number
  j0: number
  j1: number
  z0: number
  z1: number
  color: string
}) {
  return (
    <Poly
      p={[up(P(ip, j0), z0), up(P(ip, j1), z0), up(P(ip, j1), z1), up(P(ip, j0), z1)]}
      fill={color}
      stroke={INK}
    />
  )
}

/**
 * An upright cylinder centred on tile point (ci, cj), radius r tiles, h px tall, standing z px up: its side in the base
 * colour with the reference's darker band on the right, a lighter top. `open` leaves the top off (for a liquid level
 * drawn over it).
 */
export function Cyl({
  ci,
  cj,
  r,
  h,
  color,
  z = 0,
  top,
  open = false,
}: {
  ci: number
  cj: number
  r: number
  h: number
  color: string
  z?: number
  top?: string
  open?: boolean
}) {
  const [cx, c0] = P(ci, cj)
  const cy = c0 - z
  const rx = r * HW * 1.414
  const ry = r * HH * 1.414
  const band = rx * 0.7
  return (
    <>
      <path
        d={`M${cx - rx},${cy - h}L${cx - rx},${cy}A${rx},${ry} 0 0 0 ${cx + rx},${cy}L${cx + rx},${cy - h}Z`}
        fill={color}
        stroke={INK}
      />
      <path
        d={`M${cx + band},${cy - h + ry * 0.7}L${cx + band},${cy + ry * 0.7}A${rx},${ry} 0 0 0 ${cx + rx},${cy}L${cx + rx},${cy - h}Z`}
        fill={ART.sideShade}
      />
      {open ? null : (
        <ellipse
          cx={cx}
          cy={cy - h}
          rx={rx}
          ry={ry}
          fill={top ?? shade(color, 0.14)}
          stroke={INK}
        />
      )}
    </>
  )
}

/** A flat footprint on the ground. */
export function Foot({
  i,
  j,
  w,
  d,
  fill,
  stroke,
  dash,
  width,
  className,
}: {
  i: number
  j: number
  w: number
  d: number
  fill: string
  stroke?: string
  dash?: string
  width?: number
  className?: string
}) {
  return (
    <Poly
      p={[P(i, j), P(i + w, j), P(i + w, j + d), P(i, j + d)]}
      fill={fill}
      stroke={stroke}
      dash={dash}
      width={width}
      className={className}
    />
  )
}

/** The reference's soft shadow, cast a little to the front-right. */
export function Shadow({ i, j, w, d }: { i: number; j: number; w: number; d: number }) {
  return <Foot i={i + 0.22} j={j + 0.06} w={w} d={d} fill={ART.shadow} />
}

export function Line({
  a,
  b,
  c,
  w = 1.5,
  dash,
}: {
  a: Pt
  b: Pt
  c: string
  w?: number
  dash?: string
}) {
  return (
    <line
      x1={a[0].toFixed(1)}
      y1={a[1].toFixed(1)}
      x2={b[0].toFixed(1)}
      y2={b[1].toFixed(1)}
      stroke={c}
      strokeWidth={w}
      strokeLinecap="round"
      strokeDasharray={dash}
    />
  )
}
