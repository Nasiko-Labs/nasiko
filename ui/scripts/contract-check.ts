/**
 * The /ship contract hook (plans/feat-live-contract.md §11): `record:live --check` against an already running,
 * seed-only contract server, only when NASIKO_CONTRACT_URL names one. It never builds or starts a server.
 *
 *   NASIKO_CONTRACT_URL=http://127.0.0.1:8181 npm run contract:check [-- --only <feature>]
 *
 * Arguments are forwarded to record:live (`--only` included). Exit codes are record:live's: 1 (drift) blocks a ship,
 * 2 (openapi fingerprint changed, no drift) warns, 3 means the server isn't ready or holds non-seed data. Unset: one
 * skip line and exit 0, unless `--only` names a feature, which can't be checked without a server (exit 3, never a
 * skip that reads as a pass; plan feat-budget-burn X4).
 *
 * `--only budgets` checks the proposed budgets 1.0 routes instead (docs/requirements/budgets.md §15): one verdict per
 * route, pass / fail / unavailable, against the UI's zod schemas. Budgets belong to an edition, so the checks come
 * from the edition that ships `scripts/lib/budgetCheck.ts`; a checkout with no such edition exits 3.
 */
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { importEditionModule } from './editions.ts'
import { Failure, login, requireLoopback } from './lib/live.ts'

type Get = (path: string) => Promise<{ status: number; body: unknown }>
interface BudgetCheckModule {
  checkBudgets(get: Get): Promise<{ code: number }>
  report(result: { code: number }): string
}

const args = process.argv.slice(2)
const onlyAt = args.indexOf('--only')
const only = onlyAt >= 0 ? args[onlyAt + 1] : undefined
const url = process.env.NASIKO_CONTRACT_URL?.trim()

if (!url) {
  if (only) {
    console.error(
      `contract:check: --only ${only} needs a server: set NASIKO_CONTRACT_URL to a running seed-only contract server (npm run record:live -- --keep-db, then http://127.0.0.1:8181)`,
    )
    process.exit(3)
  }
  console.log(
    'contract:check: skipped (set NASIKO_CONTRACT_URL to a running seed-only contract server, e.g. npm run record:live -- --keep-db, then http://127.0.0.1:8181)',
  )
  process.exit(0)
}

if (only === 'budgets') {
  const budgets = await importEditionModule<BudgetCheckModule>(
    fileURLToPath(new URL('..', import.meta.url)),
    'scripts/lib/budgetCheck.ts',
  )
  if (!budgets) {
    console.error('contract:check: --only budgets: no edition in this checkout has budgets')
    process.exit(3)
  }
  try {
    const base = requireLoopback(url, 'NASIKO_CONTRACT_URL').replace(/\/$/, '')
    const { token } = await login(
      base,
      process.env.ADMIN_USERNAME ?? 'admin',
      process.env.ADMIN_PASSWORD ?? 'changeme',
    )
    const get = async (path: string) => {
      const r = await fetch(`${base}${path}`, {
        headers: { Authorization: `Bearer ${token}`, Accept: 'application/json' },
        signal: AbortSignal.timeout(30_000),
      })
      const text = await r.text()
      let body: unknown = text
      try {
        body = text ? JSON.parse(text) : null
      } catch {
        // A plain-text body stays text.
      }
      return { status: r.status, body }
    }
    const result = await budgets.checkBudgets(get)
    console.log(budgets.report(result))
    process.exit(result.code)
  } catch (err) {
    if (err instanceof Failure) {
      console.error(`contract:check: ${err.message}${err.fix ? `\n  fix: ${err.fix}` : ''}`)
      process.exit(err.code)
    }
    console.error(`contract:check: ${(err as Error).message}`)
    process.exit(3)
  }
}

const r = spawnSync(
  process.execPath,
  [
    fileURLToPath(new URL('./record-live.ts', import.meta.url)),
    '--check',
    '--reuse-server',
    url,
    ...args,
  ],
  { stdio: 'inherit' },
)
process.exit(r.status ?? 3)
