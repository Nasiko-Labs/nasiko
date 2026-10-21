// The editions this checkout has, found by folder (docs/lab-vs-react-migration-review.md §10.6): an app dir with a
// vite.config.ts and a src/edition.ts, one or two levels down. No published file names a private edition, so the
// public OSS layout (without ee/) runs the same scripts.
import { existsSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

export interface EditionDir {
  /** 'oss', 'ee', … (the app dir's first segment). */
  id: string
  /** Relative to the repo root, e.g. 'oss'. */
  dir: string
}

const SKIP = new Set([
  'node_modules',
  'common',
  'dist',
  'scripts',
  'e2e',
  'docs',
  'plans',
  '.git',
  '.claude',
])

export function findEditions(root: string): EditionDir[] {
  const isApp = (d: string) =>
    existsSync(join(d, 'vite.config.ts')) && existsSync(join(d, 'src/edition.ts'))
  const dirs = (d: string) =>
    readdirSync(d)
      .filter((f) => !SKIP.has(f) && !f.startsWith('.'))
      .map((f) => join(d, f))
      .filter((p) => statSync(p).isDirectory())
  const found: string[] = []
  for (const a of dirs(root)) {
    if (isApp(a)) found.push(a)
    else for (const b of dirs(a)) if (isApp(b)) found.push(b)
  }
  return found
    .map((p) => relative(root, p))
    .map((dir) => ({ id: dir.split(/[/\\]/)[0]!, dir }))
    .sort((x, y) => (x.id === 'oss' ? -1 : y.id === 'oss' ? 1 : x.id.localeCompare(y.id)))
}
