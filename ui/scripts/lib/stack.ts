/**
 * The local infra stack as the live-contract scripts use it (plans/feat-live-contract.md §5, §6): Postgres and Redis
 * through `docker compose exec` on nasiko-cloud-rs's infra compose file, so no host Postgres or Redis tools are
 * needed. nasiko-cloud-rs is only run, never changed.
 */
import { spawnSync, type SpawnSyncOptions } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { EXIT, Failure } from './live.ts'

/** The only databases the scripts write, by purpose. Anything else is refused. */
export const DATABASES = {
  dev: 'nasiko_dev',
  contract: 'nasiko_contract',
  ee: 'nasiko_ee',
} as const
export type Database = (typeof DATABASES)[keyof typeof DATABASES]
/** The throwaway recording database: the only one the scripts ever drop. */
const THROWAWAY = DATABASES.contract

export function cloudRsPath(): string {
  return resolve(process.env.NASIKO_CLOUD_RS ?? '../nasiko-cloud-rs')
}

export function composeFile(repo = cloudRsPath()): string {
  const file = resolve(repo, 'oss/docker-compose.infra.yml')
  if (!existsSync(file))
    throw new Failure(
      'stack',
      `no infra compose file at ${file}`,
      EXIT.notReady,
      'export NASIKO_CLOUD_RS=/path/to/nasiko-cloud-rs',
    )
  return file
}

function run(cmd: string, args: string[], opts: SpawnSyncOptions = {}) {
  const r = spawnSync(cmd, args, { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, ...opts })
  if (r.error)
    throw new Failure(
      'stack',
      `${cmd} failed to start: ${r.error.message}`,
      EXIT.notReady,
      'docker info   (is Docker running?)',
    )
  return { status: r.status ?? 1, stdout: String(r.stdout ?? ''), stderr: String(r.stderr ?? '') }
}

/** `docker compose exec -T <service> <args…>`, optionally with stdin. */
function composeExec(service: string, args: string[], input?: string) {
  return run(
    'docker',
    ['compose', '-f', composeFile(), 'exec', '-T', service, ...args],
    input === undefined ? {} : { input },
  )
}

/** Run SQL in a database; throws with psql's stderr on failure. */
export function psql(db: Database, sql: string, opts: { tuples?: boolean } = {}): string {
  const args = [
    'psql',
    '-v',
    'ON_ERROR_STOP=1',
    '-q',
    '-U',
    'nasiko',
    ...(opts.tuples ? ['-tA'] : []),
    db,
  ]
  const r = composeExec('postgres', args, sql)
  if (r.status !== 0)
    throw new Failure(
      'psql',
      `psql on ${db} failed: ${r.stderr.trim().split('\n').slice(-3).join(' ')}`,
      EXIT.notReady,
    )
  return r.stdout
}

/** Drop and re-create the throwaway recording database. Refuses any other name. */
export function recreateThrowaway(name: string = THROWAWAY): void {
  if (name !== THROWAWAY)
    throw new Failure(
      'db',
      `refusing to drop ${name}; only ${THROWAWAY} is throwaway`,
      EXIT.notReady,
    )
  dropThrowaway(name)
  const c = composeExec('postgres', ['createdb', '-U', 'nasiko', name])
  if (c.status !== 0)
    throw new Failure('db', `createdb ${name} failed: ${c.stderr.trim()}`, EXIT.notReady)
}

export function dropThrowaway(name: string = THROWAWAY): void {
  if (name !== THROWAWAY)
    throw new Failure(
      'db',
      `refusing to drop ${name}; only ${THROWAWAY} is throwaway`,
      EXIT.notReady,
    )
  const d = composeExec('postgres', ['dropdb', '-U', 'nasiko', '--if-exists', '--force', name])
  if (d.status !== 0)
    throw new Failure('db', `dropdb ${name} failed: ${d.stderr.trim()}`, EXIT.notReady)
}

/**
 * Create a long-lived database if it doesn't exist yet (the EE database, plan §8). Never drops: `nasiko_ee` keeps its
 * seeded org and users between runs, and `seed-live --reset` removes seed rows through the server instead.
 */
export function ensureDatabase(name: Database): 'created' | 'exists' {
  if (name !== DATABASES.ee)
    throw new Failure(
      'db',
      `refusing to create ${name}; only ${DATABASES.ee} is created on demand`,
      EXIT.notReady,
    )
  const exists = composeExec('postgres', [
    'psql',
    '-U',
    'nasiko',
    '-tAc',
    `SELECT 1 FROM pg_database WHERE datname = '${name}'`,
    'postgres',
  ])
  if (exists.status !== 0)
    throw new Failure('db', `checking for ${name} failed: ${exists.stderr.trim()}`, EXIT.notReady)
  if (exists.stdout.trim() === '1') return 'exists'
  const c = composeExec('postgres', ['createdb', '-U', 'nasiko', name])
  if (c.status !== 0)
    throw new Failure('db', `createdb ${name} failed: ${c.stderr.trim()}`, EXIT.notReady)
  return 'created'
}

export function hasPgvector(db: Database): boolean {
  return (
    psql(db, "SELECT 1 FROM pg_available_extensions WHERE name = 'vector';", {
      tuples: true,
    }).trim() === '1'
  )
}

/** Flush one Redis logical DB (the contract server's, never DB 0). */
export function flushRedisDb(db: number): void {
  if (db === 0)
    throw new Failure('redis', 'refusing to flush Redis DB 0 (the dev stack)', EXIT.notReady)
  const r = composeExec('redis', ['redis-cli', '-n', String(db), 'FLUSHDB'])
  // A failed flush would leave the last run's lockout, rate-limit and revoked-token keys behind.
  if (r.status !== 0 || !/OK/.test(r.stdout))
    throw new Failure(
      'redis',
      `FLUSHDB on Redis DB ${db} failed: ${(r.stderr || r.stdout).trim()}`,
      EXIT.notReady,
      'docker compose -f $NASIKO_CLOUD_RS/oss/docker-compose.infra.yml up -d redis',
    )
}
