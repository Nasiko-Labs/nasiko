/**
 * Shape comparison for live-contract parity (plans/feat-live-contract.md §7.1). Pure: shared by the parity test and
 * the recorder's `--check`, so it lives beside the recorder and imports nothing from src/.
 *
 * A body flattens to paths (`data[].llm_config.tier1_model`, `$` for the root) with the JSON types seen there.
 * Rules:
 * - an empty array on either side matches any element shape (nothing below it is compared);
 * - a key missing from some objects gets the type `absent`, so absent and null stay different;
 * - `map_paths` are objects keyed by data (dates, names): their keys collapse to `*`;
 * - string bodies (plain-text errors, `/health`) compare by content type, status and emptiness, never by value;
 * - a difference only in whether `null` was seen is kind `nullability`: mock and live data can differ there (the router
 *   and model catalog mocks are seeded apart from the live seed), and the type contract test (§7.2) decides whether the
 *   wire type allows null. Parity reports nullability differences without failing on them.
 */

type JsonType = 'string' | 'number' | 'boolean' | 'null' | 'object' | 'array' | 'absent'

export interface Shape {
  /** path → types seen at that path */
  types: Map<string, Set<JsonType>>
  /** array paths that held no elements */
  emptyArrays: Set<string>
}

const typeOf = (v: unknown): JsonType =>
  v === null
    ? 'null'
    : Array.isArray(v)
      ? 'array'
      : typeof v === 'object'
        ? 'object'
        : (typeof v as JsonType)

export function shapeOf(body: unknown, mapPaths: readonly string[] = []): Shape {
  const maps = new Set(mapPaths)
  const types = new Map<string, Set<JsonType>>()
  const emptyArrays = new Set<string>()
  const add = (path: string, t: JsonType) => {
    let s = types.get(path)
    if (!s) types.set(path, (s = new Set()))
    s.add(t)
  }
  const walk = (v: unknown, path: string) => {
    const t = typeOf(v)
    add(path, t)
    if (t === 'array') {
      const arr = v as unknown[]
      if (!arr.length) emptyArrays.add(path)
      const el = `${path}[]`
      const objs = arr.filter((x) => typeOf(x) === 'object') as Record<string, unknown>[]
      // Keys present in only some elements are optional: record `absent` for the others.
      const keys = new Set(objs.flatMap((o) => Object.keys(o)))
      for (const x of arr) walk(x, el)
      if (!maps.has(el))
        for (const k of keys) for (const o of objs) if (!(k in o)) add(`${el}.${k}`, 'absent')
    } else if (t === 'object') {
      const o = v as Record<string, unknown>
      const keyed = maps.has(path)
      for (const [k, x] of Object.entries(o)) walk(x, `${path}.${keyed ? '*' : k}`)
    }
  }
  walk(body, '$')
  return { types, emptyArrays }
}

export interface Difference {
  path: string
  kind:
    | 'missing-in-mock'
    | 'extra-in-mock'
    | 'type'
    | 'nullability'
    | 'value'
    | 'status'
    | 'content-type'
    | 'body-kind'
    | 'emptiness'
  live: string
  mock: string
}

const fmt = (s: Set<JsonType> | undefined) => (s ? [...s].sort().join('|') : '(none)')

/** Is `path` below an array that one side saw empty? Then nothing under it is compared. */
function underEmpty(path: string, a: Shape, b: Shape): boolean {
  for (const e of [...a.emptyArrays, ...b.emptyArrays]) if (path.startsWith(`${e}[]`)) return true
  return false
}

export function compareShapes(
  live: unknown,
  mock: unknown,
  mapPaths: readonly string[] = [],
): Difference[] {
  const a = shapeOf(live, mapPaths)
  const b = shapeOf(mock, mapPaths)
  const out: Difference[] = []
  for (const [path, lt] of a.types) {
    if (underEmpty(path, a, b)) continue
    const mt = b.types.get(path)
    if (!mt) {
      // A missing key is reported once, at its highest missing path.
      const parent = path.replace(/(\.[^.[\]]+|\[\])$/, '')
      if (parent !== path && !b.types.has(parent)) continue
      out.push({ path, kind: 'missing-in-mock', live: fmt(lt), mock: '(none)' })
      continue
    }
    if (fmt(lt) !== fmt(mt)) {
      const bare = (x: Set<JsonType>) => fmt(new Set([...x].filter((t) => t !== 'null')))
      // Only null seen on one side carries no type information: the other side's type can't be contradicted by it.
      const onlyNull = (x: Set<JsonType>) => x.size === 1 && x.has('null')
      out.push({
        path,
        kind: bare(lt) === bare(mt) || onlyNull(lt) || onlyNull(mt) ? 'nullability' : 'type',
        live: fmt(lt),
        mock: fmt(mt),
      })
    }
  }
  for (const [path, mt] of b.types) {
    if (a.types.has(path) || underEmpty(path, a, b)) continue
    const parent = path.replace(/(\.[^.[\]]+|\[\])$/, '')
    if (parent !== path && !a.types.has(parent)) continue
    out.push({ path, kind: 'extra-in-mock', live: '(none)', mock: fmt(mt) })
  }
  return out.sort((x, y) => x.path.localeCompare(y.path))
}

export interface Recorded {
  status: number
  content_type: string
  body: unknown
}

const kindOf = (ct: string) =>
  ct.includes('json') ? 'json' : ct.startsWith('text/') ? 'text' : ct || 'none'

/** Compare a replayed mock response with a recorded fixture: status, content type, then body shape. */
export function compareResponses(
  live: Recorded,
  mock: Recorded,
  mapPaths: readonly string[] = [],
): Difference[] {
  const out: Difference[] = []
  if (live.status !== mock.status)
    out.push({ path: '$', kind: 'status', live: String(live.status), mock: String(mock.status) })
  const lk = kindOf(live.content_type)
  const mk = kindOf(mock.content_type)
  // An empty body has no content type worth comparing (a bare 404 with no body).
  const lEmpty = live.body === '' || live.body === null || live.body === undefined
  const mEmpty = mock.body === '' || mock.body === null || mock.body === undefined
  if (lEmpty !== mEmpty)
    return [
      ...out,
      {
        path: '$',
        kind: 'emptiness',
        live: lEmpty ? 'empty' : 'body',
        mock: mEmpty ? 'empty' : 'body',
      },
    ]
  if (lEmpty) return out
  if (lk !== mk) out.push({ path: '$', kind: 'content-type', live: lk, mock: mk })
  const lString = typeof live.body === 'string'
  const mString = typeof mock.body === 'string'
  if (lString || mString) {
    if (lString !== mString)
      out.push({
        path: '$',
        kind: 'body-kind',
        live: lString ? 'text' : 'json',
        mock: mString ? 'text' : 'json',
      })
    return out
  }
  return [...out, ...compareShapes(live.body, mock.body, mapPaths)]
}

export interface Allow {
  path: string
  reason: string
  since: string
}

/**
 * Split differences into unexplained ones and ones an allowlist entry explains; an allowlist entry that explains
 * nothing is stale and reported, so the list can't rot.
 */
export function applyAllowlist(
  diffs: Difference[],
  allow: readonly Allow[] = [],
): { unexplained: Difference[]; stale: Allow[] } {
  const used = new Set<Allow>()
  const unexplained = diffs.filter((d) => {
    const hit = allow.find(
      (a) =>
        d.path === a.path || d.path.startsWith(`${a.path}.`) || d.path.startsWith(`${a.path}[]`),
    )
    if (hit) used.add(hit)
    return !hit
  })
  return { unexplained, stale: allow.filter((a) => !used.has(a)) }
}

export function describe(d: Difference): string {
  switch (d.kind) {
    case 'missing-in-mock':
      return `${d.path}: live ${d.live}, mock has no such field`
    case 'extra-in-mock':
      return `${d.path}: mock ${d.mock}, live has no such field`
    default:
      return `${d.path}: ${d.kind} live ${d.live}, mock ${d.mock}`
  }
}

// ── Values (plan §7.1: value rules on seed rows) ─────────────────────────────

export interface ValueRules {
  /** Paths (`$.data.points[].p95_latency_ms`, `[]` matches any index) whose values aren't compared. */
  ignore?: readonly string[]
  /**
   * Tolerance for numbers, relative to the larger magnitude (absolute below 1). Default 1e-6: the server sums
   * numeric in SQL and the mock in floats, and container hours carry sub-second timestamp rounding over thousands of hours.
   */
  tolerance?: number
}

const clip = (v: unknown) => {
  const s = JSON.stringify(v)
  return s.length > 80 ? `${s.slice(0, 77)}...` : s
}

/**
 * Compare values where live and mock derive from the same seed. Arrays compare by index (order matters to the UI),
 * numbers within the tolerance. Only the first `limit` differences are returned.
 */
export function compareValues(
  live: unknown,
  mock: unknown,
  rules: ValueRules = {},
  limit = 25,
): Difference[] {
  const tol = rules.tolerance ?? 1e-6
  // The envelope's `message` is human text the UI never reads; its presence and type are compared as shape.
  const ignore = ['$.message', ...(rules.ignore ?? [])]
  const out: Difference[] = []
  // `$.a[3].b` matches the ignore path `$.a[].b` (and anything below it).
  const skip = (path: string) => {
    const norm = path.replace(/\[\d+\]/g, '[]')
    return ignore.some((p) => norm === p || norm.startsWith(`${p}.`) || norm.startsWith(`${p}[`))
  }
  const walk = (a: unknown, b: unknown, path: string) => {
    if (out.length >= limit || skip(path)) return
    if (typeof a === 'number' && typeof b === 'number') {
      if (Math.abs(a - b) > tol * Math.max(1, Math.abs(a), Math.abs(b)))
        out.push({ path, kind: 'value', live: clip(a), mock: clip(b) })
      return
    }
    if (Array.isArray(a) && Array.isArray(b)) {
      if (a.length !== b.length)
        out.push({
          path: `${path}.length`,
          kind: 'value',
          live: String(a.length),
          mock: String(b.length),
        })
      for (let i = 0; i < Math.min(a.length, b.length); i++) walk(a[i], b[i], `${path}[${i}]`)
      return
    }
    if (a && b && typeof a === 'object' && typeof b === 'object') {
      for (const k of new Set([...Object.keys(a), ...Object.keys(b)]))
        walk((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k], `${path}.${k}`)
      return
    }
    if (a !== b && !(a === undefined || b === undefined))
      out.push({ path, kind: 'value', live: clip(a), mock: clip(b) })
  }
  walk(live, mock, '$')
  return out
}
