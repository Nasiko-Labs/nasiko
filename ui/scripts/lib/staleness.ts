/**
 * Fixture staleness (plans/feat-live-contract.md §7.6): the committed live fixtures record the nasiko-cloud-rs commit
 * they came from (`server.sha`). When the sibling checkout has server changes since then, `npm test` says so once, with
 * both SHAs and the re-record command. It never fails a run, and it is silent when the checkout or the SHA is missing
 * (CI) or git doesn't answer within the timeout.
 */
import { spawnSync } from 'node:child_process'
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'

/** Distinct `server.sha` values across the committed fixtures, per edition. */
export function fixtureShas(liveDir: string): Map<string, Set<'oss' | 'ee'>> {
  const out = new Map<string, Set<'oss' | 'ee'>>()
  for (const edition of ['oss', 'ee'] as const) {
    const root = join(liveDir, edition)
    if (!existsSync(root)) continue
    const walk = (d: string) => {
      for (const f of readdirSync(d)) {
        const p = join(d, f)
        if (statSync(p).isDirectory()) walk(p)
        else if (p.endsWith('.json')) {
          const sha = (JSON.parse(readFileSync(p, 'utf8')) as { server?: { sha?: unknown } }).server
            ?.sha
          if (typeof sha === 'string' && /^[0-9a-f]{7,40}$/.test(sha))
            out.set(sha, (out.get(sha) ?? new Set()).add(edition))
        }
      }
    }
    walk(root)
  }
  return out
}

/**
 * The warning to print, or null. A SHA counts as stale only when `git diff --quiet <sha>..HEAD -- oss ee` exits 1
 * (server changes); an unknown SHA, a missing checkout, a timeout or any other exit stays silent.
 */
export function stalenessWarning(liveDir: string, repo: string, timeoutMs = 2_000): string | null {
  if (!existsSync(join(repo, '.git'))) return null
  const stale: string[] = []
  let head = ''
  for (const [sha, editions] of fixtureShas(liveDir)) {
    const r = spawnSync('git', ['-C', repo, 'diff', '--quiet', `${sha}..HEAD`, '--', 'oss', 'ee'], {
      timeout: timeoutMs,
      stdio: 'ignore',
    })
    if (r.status !== 1) continue
    head ||=
      spawnSync('git', ['-C', repo, 'rev-parse', '--short=8', 'HEAD'], {
        timeout: timeoutMs,
        encoding: 'utf8',
      }).stdout?.trim() ?? ''
    stale.push(`${sha.slice(0, 8)} (${[...editions].join(', ')})`)
  }
  if (!stale.length) return null
  return `live contract: nasiko-cloud-rs has server changes since the fixtures were recorded (fixtures at ${stale.join('; ')}, checkout at ${head || 'HEAD'}). Re-record: npm run record:live (EE: npm run record:live -- --edition ee). See docs/live-contract.md.`
}
