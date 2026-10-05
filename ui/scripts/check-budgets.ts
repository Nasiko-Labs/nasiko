// Bundle budgets (plan §2, §8 Phase 4), from the Vite manifest, gzipped: the shell (the entry and its static
// imports, what every page loads first) ≤ 200 KB JS and ≤ 40 KB CSS; every lazily loaded chunk ≤ 120 KB.
// The mock worker (MSW + seed) only loads in mock mode and is reported, not budgeted.
// An edition may raise its own limits in `<app>/budgets.json` ({"shellJs": 205}, KB), next to its dist/.
// Usage: node scripts/check-budgets.ts [dist…]   (default: every edition's dist/, scripts/editions.ts)
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { findEditions } from './editions.ts'
import { gzipSync } from 'node:zlib'

type Chunk = { file: string; src?: string; isEntry?: boolean; imports?: string[]; css?: string[] }

const KB = 1024
export const BUDGET = { shellJs: 200 * KB, shellCss: 40 * KB, lazy: 120 * KB }
/** Chunks that never load in a live build (mock mode only). */
const MOCK_ONLY = /src\/mocks\/|node_modules\/msw\//

export function check(dist: string, log: (line: string) => void = console.log): boolean {
  const manifest = JSON.parse(readFileSync(join(dist, '.vite/manifest.json'), 'utf8')) as Record<
    string,
    Chunk
  >
  const gz = (file: string) => gzipSync(readFileSync(join(dist, file))).length
  const own = join(dist, '../budgets.json')
  const budget = { ...BUDGET }
  if (existsSync(own))
    for (const [k, kb] of Object.entries(
      JSON.parse(readFileSync(own, 'utf8')) as Record<string, number>,
    ))
      if (k in budget) budget[k as keyof typeof BUDGET] = kb * KB
  let ok = true
  const report = (label: string, size: number, max: number | null) => {
    const over = max !== null && size > max
    ok &&= !over
    log(
      `${over ? 'FAIL' : max === null ? 'mock' : 'ok  '} ${label}: ${(size / KB).toFixed(1)}${max === null ? '' : ` / ${max / KB}`} KB gz`,
    )
  }
  const shell = new Set<string>()
  const walk = (key: string) => {
    if (shell.has(key)) return
    shell.add(key)
    manifest[key]?.imports?.forEach(walk)
  }
  for (const [key, c] of Object.entries(manifest)) if (c.isEntry) walk(key)
  const chunks = [...shell].map((k) => manifest[k]!)
  report(
    'shell JS',
    chunks.reduce((n, c) => n + gz(c.file), 0),
    budget.shellJs,
  )
  const css = [...new Set(chunks.flatMap((c) => c.css ?? []))]
  report(
    'shell CSS',
    css.reduce((n, f) => n + gz(f), 0),
    budget.shellCss,
  )
  // Every lazy chunk is checked; the output lists the largest few and anything over budget.
  const lazy = Object.entries(manifest)
    .filter(([key, c]) => !shell.has(key) && c.file.endsWith('.js'))
    .map(([key, c]) => ({
      file: c.file,
      size: gz(c.file),
      max: MOCK_ONLY.test(key) ? null : budget.lazy,
    }))
    .sort((a, b) => b.size - a.size)
  lazy.forEach((c, i) => {
    if (i < 8 || (c.max !== null && c.size > c.max)) report(c.file, c.size, c.max)
  })
  log(`     ${lazy.length} lazy chunks checked`)
  // Two chunks that import each other evaluate one before the other's bindings exist: a CommonJS dep that rolldown
  // parks in a route chunk which imports `charts` broke every page ("m is not a function"). Any static cycle fails.
  const cycle = findCycle(manifest)
  if (cycle) {
    ok = false
    log(`FAIL chunk import cycle: ${cycle.map((k) => manifest[k]?.file ?? k).join(' → ')}`)
  }
  return ok
}

/** The first static-import cycle among the manifest's chunks, as the keys along it (first key repeated last). */
export function findCycle(manifest: Record<string, Pick<Chunk, 'imports'>>): string[] | null {
  const done = new Set<string>()
  const path: string[] = []
  const visit = (key: string): string[] | null => {
    const at = path.indexOf(key)
    if (at >= 0) return [...path.slice(at), key]
    if (done.has(key)) return null
    path.push(key)
    for (const next of manifest[key]?.imports ?? []) {
      const found = visit(next)
      if (found) return found
    }
    path.pop()
    done.add(key)
    return null
  }
  for (const key of Object.keys(manifest)) {
    const found = visit(key)
    if (found) return found
  }
  return null
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const root = join(dirname(fileURLToPath(import.meta.url)), '..')
  const dists =
    process.argv.length > 2
      ? process.argv.slice(2)
      : findEditions(root).map((e) => join(root, e.dir, 'dist'))
  let ok = true
  for (const dist of dists) {
    console.log(`== ${dist}`)
    ok = check(dist) && ok
  }
  if (!ok) process.exit(1)
}
