// The editions this checkout has, found by folder (docs/lab-vs-react-migration-review.md §10.6): an app dir with a
// vite.config.ts and a src/edition.ts, one to three levels down (`oss/app`, an edition's `<edition>/app`, a nested
// `<group>/<edition>/app`). No published file names a private edition, so the
// public OSS layout (without ee/) runs the same scripts.
import { existsSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { pathToFileURL } from 'node:url'

export interface EditionDir {
  /** The edition the app dir belongs to: its parent's name for `<edition>/app` ('oss' for `oss/app`), else its own. */
  id: string
  /** Relative to the repo root, e.g. 'oss/app'. */
  dir: string
}

const SKIP = new Set([
  'node_modules',
  'shared',
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
  const visit = (d: string, depth: number) => {
    for (const c of dirs(d)) {
      if (isApp(c)) found.push(c)
      else if (depth < 3) visit(c, depth + 1)
    }
  }
  visit(root, 1)
  return found
    .map((p) => relative(root, p))
    .map((dir) => ({ id: editionId(dir), dir }))
    .sort((x, y) => (x.id === 'oss' ? -1 : y.id === 'oss' ? 1 : x.id.localeCompare(y.id)))
}

/** An app dir is `<edition>/app`, the app of the edition its parent names; any other app dir is named for itself. */
export function editionId(dir: string): string {
  const parts = dir.split(/[/\\]/)
  const last = parts.at(-1)!
  return last === 'app' && parts.length > 1 ? parts.at(-2)! : last
}

/**
 * The module at `file` (relative to an app dir) from the first edition that ships one, or undefined when none does:
 * how a published script reaches an edition's feature without naming the edition.
 */
export async function importEditionModule<T>(root: string, file: string): Promise<T | undefined> {
  for (const { dir } of findEditions(root)) {
    const path = join(root, dir, file)
    if (existsSync(path)) return (await import(pathToFileURL(path).href)) as T
  }
  return undefined
}
