/**
 * Load (or remove) the lab seed in a local OpenRuntime Postgres (plans/feat-live-contract.md §6).
 *
 *   node scripts/seed-live.ts [--database nasiko_dev|nasiko_contract|nasiko_ee] [--edition oss|ee]
 *                             [--anchor <ISO>] [--otlp <url>] [--url <ee server>] [--reset]
 *
 * With no flags this is today's `npm run seed:live`: the SQL from scripts/seed-trace-usage.ts into nasiko_dev.
 * The throwaway contract database and EE's database also get the seed marker (the recorder reads it back).
 * nasiko_dev is refused for contract and EE runs; any other database name is refused outright.
 * `--otlp` also posts the last few days of seed traces as OTLP spans (scripts/lib/otlp.ts) to a private Tempo.
 * `--edition ee` then creates the seed org through the EE server's admin APIs (scripts/lib/eeOrg.ts), and `--reset`
 * removes it the same way before the SQL reset.
 */
import { spawnSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { generateSeed } from '../common/src/mocks/seed.ts'
import { ORG, apiClient, requireSeedMarker, resetOrg, seedOrg } from './lib/eeOrg.ts'
import { EXIT, Failure, login, requireLoopback } from './lib/live.ts'
import { otlpPayloads, seedTraceSessions, TRACE_WINDOW_DAYS } from './lib/otlp.ts'
import { DATABASES, type Database, psql } from './lib/stack.ts'

const HELP = `Load the lab seed into a local OpenRuntime Postgres (plans/feat-live-contract.md §6)

Usage: node scripts/seed-live.ts [options]

Options:
  --database <name>  nasiko_dev (default), nasiko_contract or nasiko_ee
  --edition <e>      oss (default) or ee; ee requires --database nasiko_ee
  --anchor <ISO>     Pin the seed's "now" (default: the moment it runs)
  --url <url>        The EE server for --edition ee (default http://127.0.0.1:9090, localhost only)
  --otlp <url>       Also post seed traces to this OTLP/HTTP endpoint (localhost; contract/EE databases; needs --anchor)
  --reset            Remove seed rows instead of loading them
  --help             Show this help

Environment: NASIKO_CLOUD_RS (default ../nasiko-cloud-rs) locates the infra compose file.
`

export interface SeedArgs {
  database: Database
  edition: 'oss' | 'ee'
  anchor?: string
  otlp?: string
  url?: string
  reset: boolean
}

/** The EE server's default address (scripts/ee-server.ts) and where the seed users' access secrets are kept. */
export const EE_DEFAULT_URL = 'http://127.0.0.1:9090'
export const CREDENTIALS_FILE = join(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  '.live',
  'credentials.json',
)

export function parseSeedArgs(argv: string[]): SeedArgs | 'help' {
  let values
  try {
    ;({ values } = parseArgs({
      args: argv,
      strict: true,
      options: {
        help: { type: 'boolean' },
        reset: { type: 'boolean' },
        database: { type: 'string' },
        edition: { type: 'string' },
        anchor: { type: 'string' },
        otlp: { type: 'string' },
        url: { type: 'string' },
      },
    }))
  } catch (err) {
    throw new Failure('args', `${(err as Error).message} (see --help)`, EXIT.usage)
  }
  if (values.help) return 'help'
  const edition = values.edition ?? 'oss'
  if (edition !== 'oss' && edition !== 'ee')
    throw new Failure('args', `--edition must be oss or ee, got "${edition}"`, EXIT.usage)
  const database = (values.database ?? (edition === 'ee' ? DATABASES.ee : DATABASES.dev)) as string
  if (!(Object.values(DATABASES) as string[]).includes(database)) {
    throw new Failure(
      'args',
      `refusing database "${database}": use nasiko_dev, nasiko_contract or nasiko_ee`,
      EXIT.usage,
    )
  }
  if (edition === 'ee' && database !== DATABASES.ee) {
    throw new Failure(
      'args',
      `--edition ee requires --database nasiko_ee, got ${database} (EE never touches nasiko_dev)`,
      EXIT.usage,
    )
  }
  if (values.anchor !== undefined && Number.isNaN(Date.parse(values.anchor)))
    throw new Failure(
      'args',
      `--anchor must be an ISO date/time, got "${values.anchor}"`,
      EXIT.usage,
    )
  if (values.otlp !== undefined) {
    if (database === DATABASES.dev)
      throw new Failure(
        'args',
        '--otlp is for the contract and EE databases only (seed spans would land in the dev Tempo for good)',
        EXIT.usage,
      )
    if (!values.anchor)
      throw new Failure(
        'args',
        '--otlp needs --anchor, so the spans match the SQL seed',
        EXIT.usage,
      )
    requireLoopback(values.otlp, '--otlp')
  }
  if (values.url !== undefined && edition !== 'ee')
    throw new Failure(
      'args',
      '--url is for --edition ee (the EE server the org is created through)',
      EXIT.usage,
    )
  const url = edition === 'ee' ? requireLoopback(values.url ?? EE_DEFAULT_URL, '--url') : undefined
  return {
    database: database as Database,
    edition,
    anchor: values.anchor,
    otlp: values.otlp,
    url,
    reset: !!values.reset,
  }
}

/** The seed-trace-usage.ts flags for a run: contract and EE databases get the marker. */
export function sqlFlags(a: SeedArgs): string[] {
  if (a.reset) return ['--reset']
  const flags = a.anchor ? ['--anchor', a.anchor] : []
  if (a.database !== DATABASES.dev) flags.push('--marker')
  return flags
}

export function seed(a: SeedArgs): void {
  const gen = spawnSync(
    process.execPath,
    [fileURLToPath(new URL('./seed-trace-usage.ts', import.meta.url)), ...sqlFlags(a)],
    { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 },
  )
  if (gen.status !== 0)
    throw new Failure('seed', `seed SQL generation failed: ${gen.stderr.trim()}`, EXIT.usage)
  psql(a.database, gen.stdout)
}

/** Post the seed's traces from the last TRACE_WINDOW_DAYS; returns the number of traces sent. */
export async function postTraces(otlp: string, anchor: string): Promise<number> {
  const seedData = generateSeed({ anchor: new Date(anchor) })
  const sessions = seedTraceSessions(seedData, Date.parse(anchor) - TRACE_WINDOW_DAYS * 86_400_000)
  for (const payload of otlpPayloads(seedData, sessions)) {
    const res = await fetch(new URL('/v1/traces', otlp), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
      signal: AbortSignal.timeout(30_000),
    })
    if (!res.ok)
      throw new Failure(
        'otlp',
        `OTLP POST to ${otlp} answered ${res.status}: ${(await res.text()).slice(0, 200)}`,
        EXIT.notReady,
        'npm run record:live -- --no-traces',
      )
  }
  return sessions.reduce((a, s) => a + s.traces.length, 0)
}

async function main() {
  let args: SeedArgs | 'help'
  try {
    args = parseSeedArgs(process.argv.slice(2))
  } catch (err) {
    console.error(`seed-live: ${(err as Error).message}`)
    process.exit((err as Failure).code ?? EXIT.usage)
  }
  if (args === 'help') {
    process.stdout.write(HELP)
    return
  }
  const eeApi = async () => {
    const admin = await login(
      args.url!,
      process.env.ADMIN_USERNAME ?? 'admin',
      process.env.ADMIN_PASSWORD ?? 'changeme',
    )
    return apiClient(args.url!, admin.token)
  }
  // EE: pin the anchor so the org step can prove the server holds THIS seed's marker before writing anything.
  if (args.edition === 'ee' && !args.reset && !args.anchor) args.anchor = new Date().toISOString()
  if (args.edition === 'ee' && args.reset) {
    const api = await eeApi()
    await requireSeedMarker(api)
    await resetOrg(api, CREDENTIALS_FILE)
  }
  seed(args)
  if (args.edition === 'ee' && !args.reset) {
    const agent0 = generateSeed({ anchor: new Date(args.anchor!) }).agents.find(
      (a) => !a.deleted,
    )!.id
    const api = await eeApi()
    await requireSeedMarker(api, new Date(args.anchor!).toISOString())
    const creds = await seedOrg(api, CREDENTIALS_FILE, agent0)
    console.log(
      `seed-live: EE org ready (${ORG.units.length} units, users ${Object.keys(creds).join(', ')}; secrets in .live/credentials.json)`,
    )
  }
  if (args.otlp)
    console.log(
      `seed-live: posted ${await postTraces(args.otlp, args.anchor!)} traces to ${args.otlp}`,
    )
  const what = args.reset
    ? 'removed seed rows from'
    : `seeded${args.database === DATABASES.dev ? '' : ' (with marker)'}`
  console.log(
    `seed-live: ${what} ${args.database}${args.edition === 'ee' ? ` (EE org via ${args.url})` : ''}`,
  )
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((err) => {
    const f = err instanceof Failure ? err : undefined
    console.error(
      `seed-live: FAILED at ${f?.stage ?? 'unexpected'}: ${(err as Error).message}${f?.fix ? `\n  fix: ${f.fix}` : ''}`,
    )
    process.exit(f?.code ?? 1)
  })
}
