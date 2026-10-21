/**
 * Generate src/lib/api/schema.gen.ts from a running nasiko-server's OpenAPI document.
 *
 *   node scripts/gen-api.ts            (NASIKO_API_URL, default http://localhost:8080)
 *
 * The server's document reuses operationIds across paths (e.g. `list`, `create`,
 * `delete` in several modules), which makes openapi-typescript emit duplicate members
 * in `operations` and fail to type-check. Only the duplicates are renamed here
 * (`list` → `list_2`, …); schemas are untouched. Upstream fix recorded in
 * docs/designs/openruntime-server-recommendations.md.
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const base = process.env.NASIKO_API_URL ?? 'http://localhost:8080'
const res = await fetch(`${base}/api/openapi.json`)
if (!res.ok) {
  console.error(
    `GET ${base}/api/openapi.json → ${res.status}. Is nasiko-server running? (just run-stack in nasiko-cloud-rs)`,
  )
  process.exit(1)
}
const doc = (await res.json()) as {
  paths: Record<string, Record<string, { operationId?: string }>>
}

const seen = new Map<string, number>()
let renamed = 0
for (const item of Object.values(doc.paths)) {
  for (const op of Object.values(item)) {
    if (!op || typeof op !== 'object' || !op.operationId) continue
    const n = (seen.get(op.operationId) ?? 0) + 1
    seen.set(op.operationId, n)
    if (n > 1) {
      op.operationId = `${op.operationId}_${n}`
      renamed++
    }
  }
}

const dir = mkdtempSync(join(tmpdir(), 'ui-lab-openapi-'))
const file = join(dir, 'openapi.json')
writeFileSync(file, JSON.stringify(doc))
execFileSync(
  'npx',
  ['--yes', 'openapi-typescript@7.13.0', file, '-o', 'common/src/lib/api/schema.gen.ts'],
  { stdio: 'inherit' },
)
console.info(
  `[gen:api] ${Object.keys(doc.paths).length} paths; renamed ${renamed} duplicate operationId(s).`,
)
