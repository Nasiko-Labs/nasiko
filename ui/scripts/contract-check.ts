/**
 * The /ship contract hook (plans/feat-live-contract.md §11): `record:live --check` against an already running,
 * seed-only contract server, only when NASIKO_CONTRACT_URL names one. It never builds or starts a server.
 *
 *   NASIKO_CONTRACT_URL=http://127.0.0.1:8181 npm run contract:check
 *
 * Exit codes are record:live's: 1 (drift) blocks a ship, 2 (openapi fingerprint changed, no drift) warns, 3 means the
 * server isn't ready or holds non-seed data. Unset: prints one skip line and exits 0.
 */
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const url = process.env.NASIKO_CONTRACT_URL?.trim()
if (!url) {
  console.log(
    'contract:check: skipped (set NASIKO_CONTRACT_URL to a running seed-only contract server, e.g. npm run record:live -- --keep-db, then http://127.0.0.1:8181)',
  )
  process.exit(0)
}
const r = spawnSync(
  process.execPath,
  [fileURLToPath(new URL('./record-live.ts', import.meta.url)), '--check', '--reuse-server', url],
  { stdio: 'inherit' },
)
process.exit(r.status ?? 3)
