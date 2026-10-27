/**
 * Plain geometry the world themes share: a seeded random for scenery that should look scattered but stay put between
 * renders, ground radii, wobbly blob outlines and a gear's outline. The drawings using them are in `shapes.tsx`.
 */
import { HH, HW, P, type Pt } from '../geo'

/** A small deterministic random, so scattered scenery is the same on every render. */
export function seeded(seed: number) {
  let s = seed >>> 0 || 1
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0
    return s / 4294967296
  }
}

/** Screen radii of a circle on the ground, `r` tiles across. */
export const iso = (r: number) => [r * HW * 1.414, r * HH * 1.414] as const

/** A wobbly closed shape round tile point (i, j), `r` tiles across: ground patches, shores, holes. */
export function blobPath(
  i: number,
  j: number,
  r: number,
  seed: number,
  wobble = 0.28,
  n = 11,
): string {
  const [cx, cy] = P(i, j)
  const [rx, ry] = iso(r)
  return blobAt(cx, cy, rx, ry, seed, wobble, n)
}

/** The same in screen px: centre (cx, cy), radii rx × ry. */
export function blobAt(
  cx: number,
  cy: number,
  rx: number,
  ry: number,
  seed: number,
  wobble = 0.28,
  n = 11,
): string {
  const rnd = seeded(seed)
  const pts: Pt[] = []
  for (let k = 0; k < n; k++) {
    const a = (k / n) * Math.PI * 2
    const f = 1 - wobble / 2 + rnd() * wobble
    pts.push([cx + Math.cos(a) * rx * f, cy + Math.sin(a) * ry * f])
  }
  // Smooth through the midpoints.
  const mid = (a: Pt, b: Pt): Pt => [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2]
  let d = ''
  for (const [k, a] of pts.entries()) {
    const b = pts[(k + 1) % n] ?? a
    const m = mid(a, b)
    if (k === 0)
      d += `M${mid(pts.at(-1) ?? a, a)
        .map((v) => v.toFixed(1))
        .join(',')}`
    d += `Q${a[0].toFixed(1)},${a[1].toFixed(1)} ${m[0].toFixed(1)},${m[1].toFixed(1)}`
  }
  return `${d}Z`
}

/** A gear's outline, centred on 0,0: `teeth` teeth, outer radius r. */
export function gearPath(r: number, teeth = 8, depth = 0.22): string {
  const ri = r * (1 - depth)
  const n = teeth * 4
  let d = ''
  for (let k = 0; k < n; k++) {
    const a = (k / n) * Math.PI * 2
    const rr = k % 4 < 2 ? r : ri
    const x = Math.cos(a) * rr
    const y = Math.sin(a) * rr
    d += `${k ? 'L' : 'M'}${x.toFixed(2)},${y.toFixed(2)}`
  }
  return `${d}Z`
}
