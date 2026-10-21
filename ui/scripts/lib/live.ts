/**
 * Shared pieces for the scripts that talk to a live OpenRuntime server (plans/feat-live-contract.md §5):
 * the loopback guard, a staged failure that carries its exit code and fix, login (once, never retried:
 * the server locks accounts after repeated failures), the secret guard, the REST fixture scrub and the
 * recorder's error table. Imports nothing from src/.
 */

/** Recorder and seed exit codes (plan §5). The same table is printed by `--help`. */
export const EXIT = { ok: 0, drift: 1, stale: 2, notReady: 3, dirty: 4, usage: 64 } as const
export type ExitCode = (typeof EXIT)[keyof typeof EXIT]

export const EXIT_MEANING: Record<ExitCode, string> = {
  0: 'ok',
  1: 'drift',
  2: 'stale-fixtures',
  3: 'not-ready',
  4: 'dirty-server-tree',
  64: 'bad-arguments',
}

/** A failure at a named stage, with the exit code it maps to and the command that fixes it. */
export class Failure extends Error {
  readonly stage: string
  readonly code: ExitCode
  readonly fix?: string
  constructor(stage: string, message: string, code: ExitCode = EXIT.notReady, fix?: string) {
    super(message)
    this.stage = stage
    this.code = code
    this.fix = fix
  }
}

const LOOPBACK = new Set(['localhost', '127.0.0.1', '[::1]', '::1'])

/** True only for an http(s) URL whose hostname is exactly a loopback name (no suffix or prefix games). */
export function isLoopbackUrl(raw: string): boolean {
  let url: URL
  try {
    url = new URL(raw)
  } catch {
    return false
  }
  return (url.protocol === 'http:' || url.protocol === 'https:') && LOOPBACK.has(url.hostname)
}

/** Refuse any non-loopback target (plan §10.1). */
export function requireLoopback(raw: string, what: string): string {
  if (!isLoopbackUrl(raw))
    throw new Failure(
      'guard',
      `${what} ${raw} is not localhost`,
      EXIT.notReady,
      `use a localhost URL, e.g. http://localhost:8181`,
    )
  return raw.replace(/\/+$/, '')
}

/** Likely credentials; a fixture that matches is never written. */
export const SECRET =
  /eyJ[\w-]{10,}|\bsk-[\w-]{16,}|\bghp_\w{20,}|\bAKIA[0-9A-Z]{16}\b|Bearer\s+[\w.-]{16,}|-----BEGIN [A-Z ]*PRIVATE KEY-----/

/** Keys whose values are credentials, matched exactly (case-insensitive). Names like `api_key_secret_name` stay. */
const SECRET_KEYS = new Set([
  'token',
  'access_token',
  'refresh_token',
  'access_key',
  'access_secret',
  'password',
  'secret_value',
  'api_key',
  'authorization',
  'cookie',
  'set-cookie',
  'jwt',
])

const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi
/** Seed-owned ids keep their value: they are the same in the mock seed, so replay can use them. */
const SEED_ID = /^5eed[0-9a-f]{4}-/i

export interface ScrubOptions {
  /** Real value → placeholder, applied everywhere a string contains it (e.g. the live admin id). */
  replace?: Map<string, string>
  /** Dotted paths (`data[].id`, `sessions[].session_id`) whose non-seed values become `<path-N>` placeholders. */
  placeholders?: string[]
}

/**
 * Scrub a recorded body before it touches disk (plan §5): credential keys are replaced (their JSON type is
 * kept), known real values become placeholders, and declared server-generated id paths become stable
 * placeholders. Everything else, including counters such as `input_tokens`, is kept as recorded: the
 * recording database holds only seed data, so values are the seed's.
 */
export function scrubBody(body: unknown, opts: ScrubOptions = {}): unknown {
  const replace = opts.replace ?? new Map<string, string>()
  const declared = new Set(opts.placeholders ?? [])
  const byPath = new Map<string, Map<string, string>>()
  const placeFor = (path: string, value: string) => {
    let m = byPath.get(path)
    if (!m) byPath.set(path, (m = new Map()))
    if (!m.has(value)) m.set(value, `<${path.replace(/\[\]/g, '').split('.').pop()}-${m.size + 1}>`)
    return m.get(value)!
  }
  const walk = (v: unknown, path: string, key: string): unknown => {
    if (SECRET_KEYS.has(key.toLowerCase()))
      return typeof v === 'string'
        ? '<redacted>'
        : typeof v === 'number'
          ? 0
          : v === null
            ? null
            : '<redacted>'
    if (typeof v === 'string') {
      if (declared.has(path) && !SEED_ID.test(v)) return placeFor(path, v)
      let s = v
      for (const [real, fake] of replace) if (real) s = s.split(real).join(fake)
      return s
    }
    if (Array.isArray(v)) return v.map((x) => walk(x, `${path}[]`, key))
    if (v && typeof v === 'object')
      return Object.fromEntries(
        Object.entries(v).map(([k, x]) => [k, walk(x, path ? `${path}.${k}` : k, k)]),
      )
    return v
  }
  return walk(body, '', '')
}

/** The UUID-shaped values in a body that aren't seed ids or placeholders (the scrub test's backstop). */
export function foreignIds(body: unknown): string[] {
  const out = new Set<string>()
  const walk = (v: unknown) => {
    if (typeof v === 'string') {
      for (const m of v.match(UUID) ?? []) if (!SEED_ID.test(m)) out.add(m)
    } else if (Array.isArray(v)) {
      v.forEach(walk)
    } else if (v && typeof v === 'object') {
      Object.values(v).forEach(walk)
    }
  }
  walk(body)
  return [...out]
}

export interface LoginResult {
  token: string
  userId: string
  isSuperuser: boolean
}

/**
 * Log in once. A failed login is never retried: nasiko-server locks the account after repeated
 * failures (429 `account_locked` with `retry_after_secs`; a 401 carries `remaining_attempts`).
 */
export async function login(
  base: string,
  username: string,
  password: string,
  timeoutMs = 10_000,
): Promise<LoginResult> {
  let res: Response
  try {
    res = await fetch(`${base}/api/auth/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password }),
      signal: AbortSignal.timeout(timeoutMs),
    })
  } catch (err) {
    throw new Failure(
      'login',
      `login request to ${base} failed: ${(err as Error).message}`,
      EXIT.notReady,
      ERRORS.stackDown(base).fix,
    )
  }
  const text = await res.text()
  let body: Record<string, unknown> = {}
  try {
    body = text ? (JSON.parse(text) as Record<string, unknown>) : {}
  } catch {
    // Plain-text body: keep the text for the message.
  }
  if (res.status === 429 && body.code === 'account_locked') {
    const e = ERRORS.accountLocked(username, Number(body.retry_after_secs) || undefined)
    throw new Failure('login', e.problem, EXIT.notReady, e.fix)
  }
  if (res.status === 429) {
    const e = ERRORS.rateLimited()
    throw new Failure('login', e.problem, EXIT.notReady, e.fix)
  }
  if (!res.ok) {
    const left =
      typeof body.remaining_attempts === 'number'
        ? ` (${body.remaining_attempts} attempts left before the account locks)`
        : ''
    const e = ERRORS.loginFailed(username, res.status, left)
    throw new Failure('login', e.problem, EXIT.notReady, e.fix)
  }
  if (typeof body.token !== 'string')
    throw new Failure('login', 'login answered without a token', EXIT.notReady)
  return { token: body.token, userId: String(body.user_id ?? ''), isSuperuser: !!body.is_superuser }
}

export interface ErrorEntry {
  problem: string
  cause: string
  fix: string
}

/**
 * The recorder's error table (plan §5): problem, cause and the exact command to run. A unit test checks
 * that every entry's `fix` names a runnable command.
 */
export const ERRORS = {
  stackDown: (url: string): ErrorEntry => ({
    problem: `OpenRuntime is not reachable at ${url}`,
    cause: 'the server is not running, or is still starting',
    fix: 'npm run record:live   (it starts its own server; with --reuse-server, start that server first)',
  }),
  portInUse: (port: number): ErrorEntry => ({
    problem: `port ${port} is already in use`,
    cause: 'a previous recording is still running, or another process holds the port',
    fix: `lsof -nP -iTCP:${port} -sTCP:LISTEN   (stop that process, then re-run npm run record:live)`,
  }),
  locked: (path: string): ErrorEntry => ({
    problem: 'another recording is running',
    cause: `the lock file ${path} exists`,
    fix: `rm ${path}   (only if no npm run record:live is running)`,
  }),
  markerMissing: (edition: 'oss' | 'ee' = 'oss'): ErrorEntry => ({
    problem: 'the server has no seed marker (seed-marker-* llm config)',
    cause: 'the database was not seeded in contract mode',
    fix:
      edition === 'ee' ? 'npm run seed:ee' : 'node scripts/seed-live.ts --database nasiko_contract',
  }),
  nonSeed: (what: string, edition: 'oss' | 'ee' = 'oss'): ErrorEntry => ({
    problem: `this server holds non-seed data (${what})`,
    cause: 'recordings must come from a seed-only database, never a dev database',
    fix:
      edition === 'ee'
        ? 'node scripts/seed-live.ts --edition ee --reset && npm run seed:ee   (nasiko_ee must hold seed rows only)'
        : 'npm run record:live   (without --reuse-server, it records against a throwaway nasiko_contract)',
  }),
  dbRefused: (name: string): ErrorEntry => ({
    problem: `refusing database ${name}`,
    cause: 'contract recordings use nasiko_contract (OSS) or nasiko_ee (EE), never nasiko_dev',
    fix: 'node scripts/seed-live.ts --database nasiko_contract',
  }),
  dirty: (repo: string, files: number): ErrorEntry => ({
    problem: `${repo} has uncommitted changes (${files} files)`,
    cause: 'fixtures would claim a server SHA the running code does not match',
    fix: `git -C ${repo} stash   (or re-run with --allow-dirty)`,
  }),
  loginFailed: (user: string, status: number, left: string): ErrorEntry => ({
    problem: `login as ${user} failed: HTTP ${status}${left}`,
    cause: 'wrong ADMIN_USERNAME/ADMIN_PASSWORD for this server',
    fix: 'ADMIN_USERNAME=admin ADMIN_PASSWORD=<password> npm run record:live',
  }),
  accountLocked: (user: string, retryAfter?: number): ErrorEntry => ({
    problem: `account ${user} is locked after too many failed logins${retryAfter ? ` (retry in ${retryAfter} s)` : ''}`,
    cause: 'nasiko-server locks an account after repeated failures',
    fix: `sleep ${retryAfter ?? 60} && npm run record:live   (with the right ADMIN_PASSWORD)`,
  }),
  rateLimited: (): ErrorEntry => ({
    problem: 'login rate-limited',
    cause: 'nasiko-server allows 30 logins per minute across all users',
    fix: 'sleep 60 && npm run record:live',
  }),
  pgvector: (): ErrorEntry => ({
    problem: 'the infra Postgres has no pgvector extension',
    cause: 'nasiko-server migrations need CREATE EXTENSION vector',
    fix: 'docker compose -f $NASIKO_CLOUD_RS/oss/docker-compose.infra.yml up -d postgres   (the stack image ships pgvector)',
  }),
  tempo: (detail: string): ErrorEntry => ({
    problem: `the private Tempo did not start: ${detail}`,
    cause: 'docker is not running or the grafana/tempo image is missing',
    fix: 'docker pull grafana/tempo:2.6.1   (or re-run with --no-traces)',
  }),
  eeIsOss: (url: string): ErrorEntry => ({
    problem: `${url} is an OSS server, not EE (/api/org/units answered 404)`,
    cause: '--edition ee needs the EE server on nasiko_ee',
    fix: 'npm run ee:server   (then: npm run seed:ee && npm run record:live -- --edition ee)',
  }),
  unknownFeature: (name: string, valid: string[]): ErrorEntry => ({
    problem: `unknown feature "${name}"`,
    cause: `valid features: ${valid.join(', ')}`,
    fix: `npm run record:live -- --only ${valid[0] ?? '<feature>'}`,
  }),
  secret: (path: string): ErrorEntry => ({
    problem: `refusing to write a fixture: a secret-shaped value at ${path}`,
    cause: 'the scrub missed a credential',
    fix: 'add the key to SECRET_KEYS in scripts/lib/live.ts, then npm run record:live',
  }),
  serverExited: (log: string): ErrorEntry => ({
    problem: 'the contract server exited before it was healthy',
    cause: `see ${log}`,
    fix: `tail -50 ${log}`,
  }),
} satisfies Record<string, (...args: never[]) => ErrorEntry>

/** Where a secret-shaped value sits in a body, or null. */
export function findSecret(body: unknown, path = ''): string | null {
  if (typeof body === 'string') return SECRET.test(body) ? path || '(body)' : null
  if (Array.isArray(body)) {
    for (let i = 0; i < body.length; i++) {
      const hit = findSecret(body[i], `${path}[${i}]`)
      if (hit) return hit
    }
    return null
  }
  if (body && typeof body === 'object') {
    for (const [k, v] of Object.entries(body)) {
      const hit = findSecret(v, path ? `${path}.${k}` : k)
      if (hit) return hit
    }
  }
  return null
}
