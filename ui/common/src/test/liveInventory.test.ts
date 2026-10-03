/**
 * Inventory (plans/feat-live-contract.md §7.4): the manifest is valid against its schema, and the server paths the
 * app calls are listed against it. Report-only while the manifest has `"strict": false`: unmatched paths are printed,
 * not failed (regex scanning cries wolf; plan decision "inventory scanner").
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import { extractPaths, uncovered } from '../../../scripts/lib/inventory.ts'
import { loadManifest } from '../../../scripts/record-live.ts'

const ROOT = join(__dirname, '..', '..', '..')
const SRC = join(ROOT, 'common/src')
const manifest = loadManifest()
const schema = JSON.parse(
  readFileSync(join(__dirname, '__live__', 'manifest.schema.json'), 'utf8'),
) as Record<string, unknown>

/** App source only: no tests, mocks, fixtures or generated types. */
function sources(dir: string, out: string[] = []): string[] {
  for (const f of readdirSync(dir)) {
    const p = join(dir, f)
    if (statSync(p).isDirectory()) {
      if (!['mocks', 'test', '__fixtures__', '__live__'].includes(f)) sources(p, out)
    } else if (
      /\.tsx?$/.test(f) &&
      !/\.test\.tsx?$/.test(f) &&
      f !== 'schema.gen.ts' &&
      f !== 'routeTree.gen.ts'
    )
      out.push(p)
  }
  return out
}

describe('manifest', () => {
  it('is valid against manifest.schema.json', () => {
    const { $schema: _s, $id: _i, ...rest } = schema
    const r = z.fromJSONSchema(rest as never).safeParse(manifest)
    expect(
      r.success ? [] : r.error.issues.slice(0, 5).map((i) => `${i.path.join('.')}: ${i.message}`),
    ).toEqual([])
  })
  it('rejects an entry with an unknown key', () => {
    const { $schema: _s, $id: _i, ...rest } = schema
    const bad = { ...manifest, endpoints: [{ ...manifest.endpoints[0]!, nope: true }] }
    expect(z.fromJSONSchema(rest as never).safeParse(bad).success).toBe(false)
  })
})

describe('extractPaths', () => {
  it('finds literals, templates and same-file constants, dropping queries and comments', () => {
    const src = [
      "const OBS = '/api/observability'",
      "apiFetch('/api/agents?limit=1')",
      'apiFetch(`/api/agents/${id}/grants`)',
      'apiFetch(`${OBS}/agent/${id}/stats`)',
      "// apiFetch('/api/commented-out')",
      "fetch('/health')",
      "const x = 'not/a/path'",
    ].join('\n')
    expect(extractPaths(src)).toEqual([
      '/api/agents',
      '/api/agents/{}/grants',
      '/api/observability/agent/{}/stats',
      '/health',
    ])
  })
  it('matches source paths to manifest tokens', () => {
    expect(
      uncovered(
        ['/api/agents/{}/grants', '/api/agents/{}/secrets', '/api/agents'],
        ['/api/agents/{agent0}/grants', '/api/agents'],
      ),
    ).toEqual(['/api/agents/{}/secrets'])
    expect(uncovered(['/api/flows/{}'], ['/api/flows/00000000000000000000000000000000'])).toEqual(
      [],
    )
  })
})

describe('inventory', () => {
  const files = sources(SRC)
  const byPath = new Map<string, string[]>()
  for (const f of files)
    for (const p of extractPaths(readFileSync(f, 'utf8')))
      byPath.set(p, [...(byPath.get(p) ?? []), relative(ROOT, f)])
  const missing = uncovered(
    [...byPath.keys()].sort(),
    manifest.endpoints.map((e) => e.path),
  )

  it('scans the app source and finds the paths the manifest records', () => {
    expect(files.length).toBeGreaterThan(50)
    expect([...byPath.keys()]).toEqual(
      expect.arrayContaining([
        '/api/agents',
        '/api/observability/finops/dashboard',
        '/api/llm-configs',
      ]),
    )
  })
  it(
    manifest.strict
      ? 'every called path has a manifest entry'
      : 'lists called paths without a manifest entry (report-only)',
    () => {
      if (!manifest.strict) {
        if (missing.length)
          console.info(
            `live inventory: ${missing.length} called paths have no manifest entry (report-only; POST/PUT/DELETE routes are expected here):\n${missing.map((p) => `  ${p}  (${byPath.get(p)!.join(', ')})`).join('\n')}`,
          )
        return
      }
      expect(missing).toEqual([])
    },
  )
})
