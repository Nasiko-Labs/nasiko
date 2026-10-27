export interface Vec {
  x: number
  y: number
}

const f = (n: number) => (Math.round(n * 100) / 100).toString()

/** Closed smooth path through points (uniform Catmull-Rom → cubic Bézier). */
export function smoothClosedPath(pts: Vec[]): string {
  const n = pts.length
  if (n < 3) return ''
  let d = `M${f(pts[0].x)},${f(pts[0].y)}`
  for (let i = 0; i < n; i++) {
    const p0 = pts[(i - 1 + n) % n]
    const p1 = pts[i]
    const p2 = pts[(i + 1) % n]
    const p3 = pts[(i + 2) % n]
    const c1x = p1.x + (p2.x - p0.x) / 6
    const c1y = p1.y + (p2.y - p0.y) / 6
    const c2x = p2.x - (p3.x - p1.x) / 6
    const c2y = p2.y - (p3.y - p1.y) / 6
    d += `C${f(c1x)},${f(c1y)} ${f(c2x)},${f(c2y)} ${f(p2.x)},${f(p2.y)}`
  }
  return d + 'Z'
}

/** Closed straight-edged path (used for clipped pupils and lid lines). */
export function polyPath(pts: Vec[]): string {
  if (pts.length < 3) return ''
  return 'M' + pts.map((p) => `${f(p.x)},${f(p.y)}`).join('L') + 'Z'
}

export function rotate(p: Vec, deg: number, about: Vec = { x: 0, y: 0 }): Vec {
  const a = (deg * Math.PI) / 180
  const c = Math.cos(a)
  const s = Math.sin(a)
  const dx = p.x - about.x
  const dy = p.y - about.y
  return { x: about.x + dx * c - dy * s, y: about.y + dx * s + dy * c }
}

export function transformPts(
  pts: Vec[],
  deg: number,
  tx: number,
  ty: number,
  sx = 1,
  sy = 1,
): Vec[] {
  const a = (deg * Math.PI) / 180
  const c = Math.cos(a)
  const s = Math.sin(a)
  return pts.map((p) => {
    const x = p.x * sx
    const y = p.y * sy
    return { x: tx + x * c - y * s, y: ty + x * s + y * c }
  })
}

/** Smooth maximum (k = softness in the same units as a and b). */
export function smax(a: number, b: number, k: number) {
  const d = a - b
  return (a + b + Math.sqrt(d * d + k * k)) / 2
}
export function smin(a: number, b: number, k: number) {
  return -smax(-a, -b, k)
}

export const lerp = (a: number, b: number, t: number) => a + (b - a) * t
export const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v)
export const smoothstep = (t: number) => {
  const x = clamp01(t)
  return x * x * (3 - 2 * x)
}

export interface Box {
  x0: number
  y0: number
  x1: number
  y1: number
}
export function bbox(pts: Vec[]): Box {
  let x0 = Infinity
  let y0 = Infinity
  let x1 = -Infinity
  let y1 = -Infinity
  for (const p of pts) {
    if (p.x < x0) x0 = p.x
    if (p.y < y0) y0 = p.y
    if (p.x > x1) x1 = p.x
    if (p.y > y1) y1 = p.y
  }
  return { x0, y0, x1, y1 }
}
export function unionBox(a: Box, b: Box): Box {
  return {
    x0: Math.min(a.x0, b.x0),
    y0: Math.min(a.y0, b.y0),
    x1: Math.max(a.x1, b.x1),
    y1: Math.max(a.y1, b.y1),
  }
}
