/**
 * Live contract recorder (plans/feat-live-contract.md §5): records every manifest endpoint from a seed-only
 * OpenRuntime server into scrubbed fixtures under src/test/__live__/.
 *
 *   npm run record:live [-- --only <feature>] [--reuse-server <url>] [--keep-db] [--no-traces] [--allow-dirty] [--check]
 *
 * By default it owns the whole run: a throwaway `nasiko_contract` database, a private Tempo, and a nasiko-server on
 * :8181 with an explicit environment (materializer and network syncs off, its own Redis DB), all torn down after.
 * The dev stack on :8080, nasiko_dev, the shared Tempo and MinIO are never written. It imports only the import-free
 * seed from src/ (for request params) and reads the manifest as data.
 */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'
import {
  closeSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { connect } from 'node:net'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { generateSeed } from '../common/src/mocks/seed.ts'
import { MCP_CONNECTORS, mcpConnectorId } from '../common/src/mocks/seed-mcp.ts'
import {
  ADMIN_ID,
  adminHarnessAgents,
  generateHarnessSeed,
  harnessTurns,
} from '../common/src/mocks/seed-harness.ts'
import {
  CAPTURE_OFF_AGENT,
  generateSpans,
  observabilityData,
} from '../common/src/mocks/spanBuilder.ts'
import { readCredentials } from './lib/eeOrg.ts'
import {
  ERRORS,
  EXIT,
  EXIT_MEANING,
  type ExitCode,
  Failure,
  findSecret,
  login,
  requireLoopback,
  scrubBody,
} from './lib/live.ts'
import { CREDENTIALS_FILE, EE_DEFAULT_URL } from './seed-live.ts'
import { readEeBuild } from './lib/eeOrg.ts'
import { compareResponses, describe, type Difference } from './lib/shape.ts'
import {
  cloudRsPath,
  composeFile,
  DATABASES,
  dropThrowaway,
  flushRedisDb,
  hasPgvector,
  psql,
  recreateThrowaway,
} from './lib/stack.ts'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const LIVE_DIR = join(ROOT, '.live')
const FIXTURES = join(ROOT, 'common/src/test/__live__')
const MANIFEST = join(FIXTURES, 'manifest.json')
const TEMPO_CONFIG = join(ROOT, 'scripts/live/tempo.yaml')
export const FIXTURE_VERSION = 1

export const PORTS = { server: 8181, tempoQuery: 13200, tempoOtlp: 14318 } as const
const TEMPO_IMAGE = 'grafana/tempo:2.6.1'
const TEMPO_NAME = 'nasiko-ui-lab-tempo'
const REDIS_DB = 15
const TIMEOUT = { request: 60_000, health: 120_000, tempo: 30_000, traces: 60_000 }

const HELP = `Live contract recorder (plans/feat-live-contract.md §5)

Usage: npm run record:live [-- options]

Options:
  --only <feature>       Record one feature: FEATURES
  --reuse-server <url>   Record against an already running server (localhost only; must hold seed-only data)
  --keep-db              Keep the contract server, database and Tempo running afterwards
  --no-traces            Don't start the private Tempo (observability routes then report "not configured")
  --allow-dirty          Record even if nasiko-cloud-rs has uncommitted changes
  --check                Record and compare shapes with the committed fixtures; writes nothing
  --edition ee           Record the running EE server (npm run ee:server; default --reuse-server http://127.0.0.1:9090)
  --help                 Show this help

Environment:
  NASIKO_CLOUD_RS        nasiko-cloud-rs checkout (default ../nasiko-cloud-rs)
  ADMIN_USERNAME/ADMIN_PASSWORD   the contract server's admin (default admin / changeme)

Exit codes: 0 ok · 1 drift · 2 fingerprint changed only (--check) · 3 not ready or refused · 4 dirty nasiko-cloud-rs · 64 bad arguments
`

// ── Manifest ─────────────────────────────────────────────────────────────────

export interface Endpoint {
  id: string
  feature: string
  method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  path: string
  query?: Record<string, string>
  /** Who asks: the admin (default), nobody, or the EE seed member `seed-ee-member` (plan §8). */
  auth?: 'admin' | 'none' | 'member'
  editions?: ('oss' | 'ee')[]
  window?: 'hour' | 'day'
  /** Needs the seed's traces in Tempo; recorded as `source: "no-trace-data"` when they never became searchable. */
  traces?: boolean
  map_paths?: string[]
  /** Compare values too (both sides derive from the same seed); `ignore` lists time-dependent paths. */
  values?: boolean | { ignore?: string[]; tolerance?: number }
  placeholders?: string[]
  allow?: { path: string; reason: string; since: string }[]
  absent?: boolean
  /**
   * Replay against this path instead (tokens allowed), when the mock has no row matching the live seed's: e.g. chat
   * messages, where the live seed's chats are harness turns and the mock's are hand-made (`replay_reason` required).
   */
  replay_path?: string
  replay_reason?: string
  /** Recorded as a server shape but never replayed against the mocks (reason required). */
  no_replay?: string
  hand_written?: string
  excluded?: string
}
export interface Manifest {
  strict: boolean
  endpoints: Endpoint[]
}

export function loadManifest(file = MANIFEST): Manifest {
  return JSON.parse(readFileSync(file, 'utf8')) as Manifest
}

/** Endpoints the recorder calls for an edition (and feature): not excluded, not hand-written. */
export function recordable(m: Manifest, edition: 'oss' | 'ee', only?: string): Endpoint[] {
  return m.endpoints.filter(
    (e) =>
      !e.excluded &&
      !e.hand_written &&
      (e.editions ?? ['oss']).includes(edition) &&
      (!only || e.feature === only),
  )
}

// ── Request params from the seed ─────────────────────────────────────────────

export interface TokenContext {
  anchor: Date
  agent0: string
  agentDeleted: string
  agentRegistered: string
  adminId: string
  /** The newest seed chat session, its costliest trace and that trace's root span; the coding_agent.turn trace. */
  session0: string
  trace0: string
  span0: string
  traceCoding: string
  /** The newest seeded harness chat session (it has chat_messages rows; seed-trace-usage.ts writes them). */
  chatSession0: string
}

/**
 * The seed agent the Agents mock shows as registered (never deployed), like every live seed agent: src/mocks/agents.ts
 * `statusFor`. The mock deploys the others for the demo, so their card, deployment and resources can't match live.
 * A parity test checks the mock still agrees.
 */
export const REGISTERED_AGENT_INDEX = 11

/** `adminId` is the signed-in admin: the live one when recording, the mock's seed admin when replaying. */
export function tokenContext(anchor: Date, adminId: string = ADMIN_ID): TokenContext {
  const seed = generateSeed({ anchor })
  const agent0 = seed.agents.find((a) => !a.deleted)?.id
  const agentDeleted = seed.agents.find((a) => a.deleted)?.id
  const agentRegistered = seed.agents[REGISTERED_AGENT_INDEX]
  if (!agent0 || !agentDeleted || !agentRegistered || agentRegistered.deleted)
    throw new Failure(
      'seed',
      'the seed needs a live, a deleted and a registered agent',
      EXIT.notReady,
    )
  const obs = observabilityData(seed)
  // The capture-off agent's spans carry no content, so its session would record a different span shape.
  const newest = obs.sessions.find((s) => s.agent.name !== CAPTURE_OFF_AGENT)
  const trace0 = newest?.traces.reduce((a, t) => (t.cost_usd > a.cost_usd ? t : a))
  const span0 = trace0 && generateSpans(seed, trace0.trace_id).find((s) => s.parentHex === null)
  if (!newest || !trace0 || !span0 || !obs.codingTraceId)
    throw new Failure(
      'seed',
      'the seed needs a chat session with traces and a coding_agent.turn trace',
      EXIT.notReady,
    )
  // The same harness seed and turn filter as seed-trace-usage.ts, so the session exists live.
  const hs = generateHarnessSeed({ anchor })
  const turns = harnessTurns(hs, adminHarnessAgents(hs), seed.spikeDate)
  const chatSession0 = turns.reduce<{ id: string; at: number } | null>(
    (a, t) => (!a || t.ts > a.at ? { id: t.session_id, at: t.ts } : a),
    null,
  )?.id
  if (!chatSession0)
    throw new Failure('seed', 'the harness seed has no admin chat session', EXIT.notReady)
  return {
    anchor,
    agent0,
    agentDeleted,
    agentRegistered: agentRegistered.id,
    adminId,
    session0: newest.session_id,
    trace0: trace0.trace_id,
    span0: span0.hex,
    traceCoding: obs.codingTraceId,
    chatSession0,
  }
}

const DAY = 86_400_000
const monthOf = (d: Date) => d.toISOString().slice(0, 7)

/** `{token}` values in manifest paths and queries (plan §4). */
export function tokens(ctx: TokenContext): Record<string, string> {
  const a = ctx.anchor
  const prevMonth = new Date(Date.UTC(a.getUTCFullYear(), a.getUTCMonth() - 1, 1))
  return {
    agent0: ctx.agent0,
    agentDeleted: ctx.agentDeleted,
    agentRegistered: ctx.agentRegistered,
    session0: ctx.session0,
    trace0: ctx.trace0,
    span0: ctx.span0,
    traceCoding: ctx.traceCoding,
    chatSession0: ctx.chatSession0,
    // session/list searches Tempo from start_time to now, one lookup per row: a day keeps it quick (plan C3).
    sessionsStart: new Date(a.getTime() - DAY).toISOString(),
    adminId: ctx.adminId,
    anchorMonth: monthOf(a),
    prevMonth: monthOf(prevMonth),
    prevDay: new Date(a.getTime() - DAY).toISOString().slice(0, 10),
    prevWindowStart: new Date(a.getTime() - 60 * DAY).toISOString(),
    prevWindowEnd: new Date(a.getTime() - 30 * DAY).toISOString(),
    // MCP servers (plans/feat-mcp.md §8): fixed ids from the shared seed, the same in the live seed and the mock.
    mcpDocs: mcpId('docs-search'),
    mcpOauth: mcpId('jira-cloud'),
    mcpBearer: mcpId('weather-api'),
    mcpUpload: mcpId('pdf-tools'),
    mcpFailed: mcpId('sql-runner'),
    mcpUnknown: mcpConnectorId(999),
  }
}

const mcpId = (name: string) => {
  const c = MCP_CONNECTORS.find((x) => x.name === name && x.live)
  if (!c) throw new Failure('seed', `the MCP seed has no live server named ${name}`, EXIT.notReady)
  return mcpConnectorId(c.n)
}

export function fill(template: string, t: Record<string, string>): string {
  return template.replace(/\{(\w+)\}/g, (m, k: string) => {
    if (!(k in t)) throw new Failure('manifest', `unknown token ${m} in "${template}"`, EXIT.usage)
    return t[k]!
  })
}

export function requestFor(
  e: Endpoint,
  t: Record<string, string>,
): { path: string; query: Record<string, string> } {
  const query = Object.fromEntries(Object.entries(e.query ?? {}).map(([k, v]) => [k, fill(v, t)]))
  return { path: fill(e.path, t), query }
}

/** True when a request window crossed an hour or a UTC day (plan §5: re-issue once). */
export function crossed(start: Date, end: Date, unit: 'hour' | 'day'): boolean {
  const key = (d: Date) =>
    unit === 'hour' ? d.toISOString().slice(0, 13) : d.toISOString().slice(0, 10)
  return key(start) !== key(end)
}

// ── Environment for the contract server ──────────────────────────────────────

/** KEY=VALUE lines of an env file (comments and blanks skipped, surrounding quotes removed). */
export function parseEnvFile(text: string): Record<string, string> {
  const out: Record<string, string> = {}
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    const m = /^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$/.exec(line)
    if (!m) continue
    out[m[1]!] = m[2]!.trim().replace(/^(['"])(.*)\1$/, '$2')
  }
  return out
}

export const SECRET_ENV = new Set([
  'JWT_SECRET',
  'SECRETS_ENCRYPTION_KEY',
  'ADMIN_PASSWORD',
  'S3_SECRET_KEY',
  'AGENT_JWT_SECRET',
])
/** Variables the contract server must never inherit (plan §5). */
export const UNSET_ENV = [
  'CODING_AGENT_OTLP_ENDPOINT',
  'OPENAI_API_KEY',
  'ANTHROPIC_API_KEY',
  'OTEL_EXPORTER_OTLP_ENDPOINT',
  'OTEL_COLLECTOR_ENDPOINT',
]

export function contractOverrides(opts: {
  traces: boolean
  admin: { username: string; password: string }
}): Record<string, string> {
  return {
    CP_BIND: `127.0.0.1:${PORTS.server}`,
    SEED_AGENTS: '',
    TRACE_USAGE_SYNC_SECS: '0',
    MODEL_PRICING_SYNC_ENABLED: 'false',
    MODEL_CATALOG_SYNC_ENABLED: 'false',
    // The hours meter watches the host's container runtime, so it would count this machine's real agent containers
    // (state.rs container_hours_poll_secs, ea233d20); the seed's instance sessions supply container hours instead.
    CONTAINER_HOURS_POLL_SECS: '0',
    REDIS_URL: `redis://localhost:6379/${REDIS_DB}`,
    TEMPO_URL: opts.traces ? `http://localhost:${PORTS.tempoQuery}` : '',
    // Not the dev stack's Loki (localhost:3100): a seed agent id someone deployed in nasiko_dev would pull its real logs
    // into committed fixtures. Observability needs both URLs set, so this points at a port nothing listens on; Loki
    // reads fail soft. The agent runtime stays the example's (local): `simulated` changes what /resources answers
    // (503), and nothing deploys anyway (the seed guard keeps seed agents without images).
    LOKI_URL: opts.traces ? 'http://127.0.0.1:9' : '',
    JWT_SECRET: randomBytes(32).toString('hex'),
    SECRETS_ENCRYPTION_KEY: randomBytes(32).toString('base64'),
    ADMIN_USERNAME: opts.admin.username,
    ADMIN_PASSWORD: opts.admin.password,
  }
}

/** `url` pointing at another database on the same server (the example's credentials are kept, never written here). */
export function withDatabase(url: string, database: string): string {
  const u = new URL(url)
  u.pathname = `/${database}`
  return u.toString()
}

/**
 * The server's environment: the example file, then the overrides, minus UNSET_ENV; PATH/HOME from this process.
 * DATABASE_URL is the example's with the database swapped for `database` (the throwaway contract database by default).
 */
export function contractEnv(
  example: Record<string, string>,
  overrides: Record<string, string>,
  database: string = DATABASES.contract,
): Record<string, string> {
  if (!example.DATABASE_URL)
    throw new Failure(
      'env',
      'the server .env.example has no DATABASE_URL',
      EXIT.notReady,
      'ls $NASIKO_CLOUD_RS/oss/server/.env.example',
    )
  const env: Record<string, string> = {
    ...example,
    ...overrides,
    DATABASE_URL: withDatabase(example.DATABASE_URL, database),
  }
  for (const k of UNSET_ENV) delete env[k]
  for (const k of ['PATH', 'HOME', 'TMPDIR']) if (process.env[k]) env[k] = process.env[k]!
  return env
}

export function describeOverrides(overrides: Record<string, string>): string {
  return Object.entries(overrides)
    .map(([k, v]) => `${k}=${SECRET_ENV.has(k) ? '<redacted>' : v || "''"}`)
    .join(' ')
}

// ── Args ─────────────────────────────────────────────────────────────────────

export interface RecordArgs {
  only?: string
  reuse?: string
  keepDb: boolean
  traces: boolean
  allowDirty: boolean
  check: boolean
  edition: 'oss' | 'ee'
}

export function parseRecordArgs(argv: string[], features: string[]): RecordArgs | 'help' {
  let values
  try {
    ;({ values } = parseArgs({
      args: argv,
      strict: true,
      options: {
        help: { type: 'boolean' },
        only: { type: 'string' },
        'reuse-server': { type: 'string' },
        'keep-db': { type: 'boolean' },
        'no-traces': { type: 'boolean' },
        'allow-dirty': { type: 'boolean' },
        check: { type: 'boolean' },
        edition: { type: 'string' },
      },
    }))
  } catch (err) {
    throw new Failure('args', `${(err as Error).message} (see --help)`, EXIT.usage)
  }
  if (values.help) return 'help'
  if (values.only && !features.includes(values.only)) {
    const e = ERRORS.unknownFeature(values.only, features)
    throw new Failure('args', `${e.problem}; ${e.cause}`, EXIT.usage, e.fix)
  }
  const edition = values.edition ?? 'oss'
  if (edition !== 'oss' && edition !== 'ee')
    throw new Failure('args', `--edition must be oss or ee, got "${edition}"`, EXIT.usage)
  return {
    only: values.only,
    // EE runs against the long-lived EE server (scripts/ee-server.ts); the recorder never starts or stops it.
    reuse: values['reuse-server']
      ? requireLoopback(values['reuse-server'], '--reuse-server')
      : edition === 'ee'
        ? EE_DEFAULT_URL
        : undefined,
    keepDb: !!values['keep-db'],
    traces: !values['no-traces'],
    allowDirty: !!values['allow-dirty'],
    check: !!values.check,
    edition,
  }
}

// ── Lifecycle pieces ─────────────────────────────────────────────────────────

function portInUse(port: number): Promise<boolean> {
  return new Promise((done) => {
    const s = connect({ host: '127.0.0.1', port })
    s.once('connect', () => {
      s.destroy()
      done(true)
    })
    s.once('error', () => done(false))
  })
}

function git(repo: string, args: string[]): string {
  const r = spawnSync('git', ['-C', repo, ...args], { encoding: 'utf8' })
  if (r.status !== 0)
    throw new Failure(
      'git',
      `git ${args.join(' ')} failed in ${repo}: ${r.stderr.trim()}`,
      EXIT.notReady,
      `ls ${repo}   (set NASIKO_CLOUD_RS)`,
    )
  return r.stdout.trim()
}

function takeLock(liveDir: string): () => void {
  mkdirSync(liveDir, { recursive: true })
  const file = join(liveDir, 'record.lock')
  if (existsSync(file)) {
    const pid = Number(readFileSync(file, 'utf8'))
    let alive = false
    try {
      if (pid) {
        process.kill(pid, 0)
        alive = true
      }
    } catch {
      // Stale lock from a crashed run.
    }
    if (alive) {
      const e = ERRORS.locked(file)
      throw new Failure('lock', `${e.problem}: ${e.cause}`, EXIT.notReady, e.fix)
    }
    rmSync(file)
  }
  const fd = openSync(file, 'wx')
  writeFileSync(fd, String(process.pid))
  closeSync(fd)
  return () => rmSync(file, { force: true })
}

async function waitFor(
  what: string,
  check: () => Promise<boolean>,
  timeoutMs: number,
  abort?: () => string | null,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const why = abort?.()
    if (why) throw new Failure(what, why, EXIT.notReady)
    if (await check().catch(() => false)) return
    if (Date.now() > deadline)
      throw new Failure(what, `${what} not ready after ${timeoutMs / 1000} s`, EXIT.notReady)
    await new Promise((r) => setTimeout(r, 500))
  }
}

const ok = async (url: string, expect?: string) => {
  const res = await fetch(url, { signal: AbortSignal.timeout(3000) })
  return res.ok && (expect === undefined || (await res.text()).trim() === expect)
}

// ── Recording ────────────────────────────────────────────────────────────────

export interface Fixture {
  fixture_version: number
  id: string
  recorded_at: string
  window: { start: string; end: string }
  anchor: string
  edition: 'oss' | 'ee'
  identity: 'admin' | 'none' | 'seed-ee-member'
  server: { sha: string; dirty: boolean; openapi_sha256: string; binary_mtime: string | null }
  request: { method: string; path: string; query: Record<string, string> }
  status: number
  content_type: string
  body: unknown
  source?: 'no-trace-data' | 'hand-written'
}

// ── --check (plan §5) ────────────────────────────────────────────────────────

export interface CheckResult {
  id: string
  drift: Difference[]
  info: Difference[]
  fingerprintChanged: boolean
}

/**
 * A fresh recording against the committed fixture: live against live, so any status, type or removed-field change is
 * drift. A field only the fresh recording has is additive (information), and so is a null-only difference (the seed's
 * data decides whether a nullable field was ever null). Values aren't compared: they move with the anchor, and the
 * parity test already checks them against the mocks.
 */
export function checkFixture(
  committed: Pick<Fixture, 'status' | 'content_type' | 'body' | 'server'> | undefined,
  fresh: Fixture,
  e: Pick<Endpoint, 'map_paths'>,
): CheckResult {
  if (!committed)
    return {
      id: fresh.id,
      drift: [
        { path: '$', kind: 'missing-in-mock', live: 'recorded', mock: '(no committed fixture)' },
      ],
      info: [],
      fingerprintChanged: false,
    }
  const diffs = compareResponses(fresh, committed, e.map_paths)
  const additive = (d: Difference) => d.kind === 'missing-in-mock' || d.kind === 'nullability'
  return {
    id: fresh.id,
    drift: diffs.filter((d) => !additive(d)),
    info: diffs.filter(additive),
    fingerprintChanged: committed.server.openapi_sha256 !== fresh.server.openapi_sha256,
  }
}

/** Exit code and printed lines for a --check run: 1 on any drift (even when the fingerprint changed too), 2 on a fingerprint change alone. */
export function checkReport(results: readonly CheckResult[]): {
  code: ExitCode
  lines: string[]
  summary: string
} {
  // In a committed-vs-fresh comparison, "live" is the fresh recording and "mock" the committed fixture.
  const say = (d: Difference) =>
    describe(d).replace(' live ', ' now ').replace(', mock ', ', committed ')
  const lines: string[] = []
  for (const r of results) {
    for (const d of r.drift)
      lines.push(
        `record-live: DRIFT ${r.id} ${d.kind === 'missing-in-mock' && d.mock === '(no committed fixture)' ? '$: no committed fixture' : say(d)}`,
      )
    for (const d of r.info)
      lines.push(
        `record-live: info  ${r.id} ${d.kind === 'missing-in-mock' ? `${d.path}: new field (${d.live})` : say(d)}`,
      )
  }
  const drifted = results.filter((r) => r.drift.length).length
  const stale = results.some((r) => r.fingerprintChanged)
  if (drifted)
    return {
      code: EXIT.drift,
      lines,
      summary: `${drifted} of ${results.length} endpoints drifted; re-record with npm run record:live after fixing the mocks`,
    }
  if (stale)
    return {
      code: EXIT.stale,
      lines,
      summary: `openapi sha changed; no drift in ${results.length} endpoints`,
    }
  return {
    code: EXIT.ok,
    lines,
    summary: `${results.length} endpoints match the committed fixtures`,
  }
}

async function fetchOnce(
  base: string,
  method: string,
  path: string,
  query: Record<string, string>,
  token?: string,
) {
  const qs = new URLSearchParams(query).toString()
  const start = new Date()
  const res = await fetch(`${base}${path}${qs ? `?${qs}` : ''}`, {
    method,
    headers: token
      ? { Authorization: `Bearer ${token}`, Accept: 'application/json' }
      : { Accept: 'application/json' },
    signal: AbortSignal.timeout(TIMEOUT.request),
  })
  const text = await res.text()
  const end = new Date()
  const contentType = (res.headers.get('content-type') ?? '').split(';')[0]!.trim()
  let body: unknown = text
  if (contentType.includes('json') && text) {
    try {
      body = JSON.parse(text)
    } catch {
      // Keep the text: a JSON content type with a non-JSON body is itself worth recording.
    }
  }
  return { status: res.status, contentType, body, start, end }
}

/** Seed-only verification (plan §5): everything the server lists must be seed-owned. */
async function verifySeedOnly(
  base: string,
  token: string,
  adminName: string,
  edition: 'oss' | 'ee',
): Promise<string> {
  // Fail closed: every list must answer 200 in its known shape, and every page is read (catalog/routes.rs clamps
  // agents to 100 per page; chat sessions page by cursor). An error or an unknown shape refuses the recording.
  const refuse = (what: string) => {
    const e = ERRORS.nonSeed(`could not verify ${what}`, edition)
    return new Failure('verify', e.problem, EXIT.notReady, e.fix)
  }
  const get = async (p: string, what: string) => {
    const r = await fetchOnce(base, 'GET', p, {}, token)
    if (r.status !== 200) throw refuse(`${what} (HTTP ${r.status})`)
    return r.body as unknown
  }
  const cfgBody = (await get('/api/llm-configs', 'llm configs')) as { data?: unknown }
  if (!Array.isArray(cfgBody.data)) throw refuse('llm configs')
  const cfg = cfgBody.data as { id: string; name: string }[]
  const marker = cfg.find((c) => c.name.startsWith('seed-marker-'))
  if (!marker) {
    const e = ERRORS.markerMissing(edition)
    throw new Failure('verify', e.problem, EXIT.notReady, e.fix)
  }
  const bad: string[] = []
  for (const c of cfg) if (!c.id.startsWith('5eed')) bad.push(`llm_config ${c.name}`)
  for (let offset = 0; ; offset += 100) {
    const page = await get(`/api/agents?limit=100&offset=${offset}`, 'agents')
    if (!Array.isArray(page)) throw refuse('agents')
    for (const a of page as { id: string; name: string }[])
      if (!a.id.startsWith('5eed')) bad.push(`agent ${a.name}`)
    if (page.length < 100 || offset >= 10_000) break
  }
  const users = (await get('/api/users?limit=1000', 'users')) as { data?: unknown } | unknown[]
  const list = Array.isArray(users) ? users : users.data
  if (!Array.isArray(list)) throw refuse('users')
  for (const u of list as { username: string }[])
    if (u.username !== adminName && !u.username.startsWith('seed-')) bad.push(`user ${u.username}`)
  let cursor: string | null = null
  for (let n = 0; n < 200; n++) {
    const q: string = cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''
    const chats = (await get(`/api/chat/sessions?limit=100${q}`, 'chat sessions')) as {
      data?: unknown
      has_more?: boolean
      next_cursor?: string | null
    }
    if (!Array.isArray(chats.data)) throw refuse('chat sessions')
    for (const c of chats.data as { session_id?: string; id?: string }[]) {
      const id = c.session_id ?? c.id ?? ''
      if (!id.startsWith('5eed')) bad.push(`chat ${id}`)
    }
    if (!chats.has_more || !chats.next_cursor) break
    cursor = chats.next_cursor
  }
  if (bad.length) {
    const e = ERRORS.nonSeed(`${bad.length} rows, e.g. ${bad.slice(0, 3).join(', ')}`, edition)
    throw new Failure('verify', e.problem, EXIT.notReady, e.fix)
  }
  return marker.name.slice('seed-marker-'.length)
}

// ── Main ─────────────────────────────────────────────────────────────────────

/** Where a run reads the manifest and writes fixtures and its lock; tests point these at a temp dir. */
export interface RecordPaths {
  fixtures: string
  manifest: string
  liveDir: string
}
const DEFAULT_PATHS: RecordPaths = { fixtures: FIXTURES, manifest: MANIFEST, liveDir: LIVE_DIR }

export async function main(
  argv = process.argv.slice(2),
  paths: RecordPaths = DEFAULT_PATHS,
): Promise<{ code: ExitCode; summary: string }> {
  const manifest = loadManifest(paths.manifest)
  const features = [...new Set(manifest.endpoints.map((e) => e.feature))]
  const parsed = parseRecordArgs(argv, features)
  if (parsed === 'help') {
    process.stdout.write(HELP.replace('FEATURES', features.join(', ')))
    return { code: EXIT.ok, summary: 'help' }
  }
  const args = parsed
  const repo = cloudRsPath()
  const admin = {
    username: process.env.ADMIN_USERNAME ?? 'admin',
    password: process.env.ADMIN_PASSWORD ?? 'changeme',
  }
  const base = args.reuse ?? `http://127.0.0.1:${PORTS.server}`
  const overrides = contractOverrides({ traces: args.traces, admin })
  console.log(
    `record-live: url=${base} (${args.reuse ? 'flag --reuse-server' : 'own server'}) db=${args.reuse ? '(server-owned; verified seed-only)' : DATABASES.contract} edition=${args.edition} NASIKO_CLOUD_RS=${repo} (${process.env.NASIKO_CLOUD_RS ? 'env' : 'default'})${args.reuse ? '' : `\n  overrides: ${describeOverrides(overrides)}`}`,
  )

  const cleanups: (() => void | Promise<void>)[] = []
  const teardown = async () => {
    while (cleanups.length) {
      try {
        await cleanups.pop()!()
      } catch (err) {
        console.error(`record-live: cleanup: ${(err as Error).message}`)
      }
    }
  }
  const onSignal = (sig: NodeJS.Signals) => {
    console.error(`record-live: ${sig}, tearing down`)
    void teardown().finally(() => process.exit(130))
  }
  process.once('SIGINT', onSignal)
  process.once('SIGTERM', onSignal)

  try {
    cleanups.push(takeLock(paths.liveDir))
    // Fingerprint inputs and the dirty check come first: a dirty tree never starts a server.
    const head = git(repo, ['rev-parse', 'HEAD'])
    // The EE server is long-lived and was built when it started: stamp the commit it was built from, not HEAD.
    const eeBuilt = args.edition === 'ee' ? readEeBuild() : null
    const sha = eeBuilt ?? head
    if (eeBuilt && eeBuilt !== head)
      console.log(
        `record-live: the EE server was built at ${eeBuilt.slice(0, 8)}, the checkout is at ${head.slice(0, 8)}; restart npm run ee:server to record HEAD`,
      )
    const dirtyFiles = git(repo, ['status', '--porcelain', '--untracked-files=no'])
      .split('\n')
      .filter(Boolean)
    if (dirtyFiles.length && !args.allowDirty) {
      const e = ERRORS.dirty(repo, dirtyFiles.length)
      throw new Failure('dirty', e.problem, EXIT.dirty, e.fix)
    }

    let child: ChildProcess | undefined
    let childExit: string | null = null
    const binary = join(repo, 'target/debug/nasiko-server')
    if (!args.reuse) {
      composeFile(repo)
      for (const port of [
        PORTS.server,
        ...(args.traces ? [PORTS.tempoQuery, PORTS.tempoOtlp] : []),
      ]) {
        if (await portInUse(port)) {
          const e = ERRORS.portInUse(port)
          throw new Failure('ports', e.problem, EXIT.notReady, e.fix)
        }
      }
      if (!existsSync(binary))
        console.log('record-live: building nasiko-server (the first build takes several minutes)')
      const build = spawnSync('cargo', ['build', '-p', 'nasiko-server'], {
        cwd: repo,
        stdio: ['ignore', 'ignore', 'pipe'],
        encoding: 'utf8',
      })
      if (build.status !== 0)
        throw new Failure(
          'build',
          `cargo build -p nasiko-server failed: ${String(build.stderr).trim().split('\n').slice(-5).join(' | ')}`,
          EXIT.notReady,
          `cd ${repo} && cargo build -p nasiko-server`,
        )

      recreateThrowaway()
      if (!args.keepDb) cleanups.push(() => dropThrowaway())
      if (!hasPgvector(DATABASES.contract)) {
        const e = ERRORS.pgvector()
        throw new Failure('db', e.problem, EXIT.notReady, e.fix)
      }

      if (args.traces) {
        spawnSync('docker', ['rm', '-f', TEMPO_NAME], { stdio: 'ignore' })
        const t = spawnSync(
          'docker',
          [
            'run',
            '-d',
            '--rm',
            '--name',
            TEMPO_NAME,
            '--tmpfs',
            '/var/tempo:mode=1777',
            '-p',
            `127.0.0.1:${PORTS.tempoQuery}:3200`,
            '-p',
            `127.0.0.1:${PORTS.tempoOtlp}:4318`,
            '-v',
            `${TEMPO_CONFIG}:/etc/tempo.yaml:ro`,
            TEMPO_IMAGE,
            '-config.file=/etc/tempo.yaml',
          ],
          { encoding: 'utf8' },
        )
        if (t.status !== 0) {
          const e = ERRORS.tempo(t.stderr.trim())
          throw new Failure('tempo', e.problem, EXIT.notReady, e.fix)
        }
        if (!args.keepDb)
          cleanups.push(() => {
            spawnSync('docker', ['rm', '-f', TEMPO_NAME], { stdio: 'ignore' })
          })
        await waitFor(
          'tempo',
          () => ok(`http://127.0.0.1:${PORTS.tempoQuery}/ready`),
          TIMEOUT.tempo,
        )
      }

      flushRedisDb(REDIS_DB)
      // --keep-db keeps the server running, so it keeps its Redis state too.
      if (!args.keepDb) cleanups.push(() => flushRedisDb(REDIS_DB))

      const env = contractEnv(
        parseEnvFile(readFileSync(join(repo, 'oss/server/.env.example'), 'utf8')),
        overrides,
      )
      mkdirSync(paths.liveDir, { recursive: true })
      writeFileSync(
        join(paths.liveDir, 'contract.env'),
        Object.entries(env)
          .map(([k, v]) => `${k}=${SECRET_ENV.has(k) ? '<redacted>' : v}`)
          .join('\n') + '\n',
      )
      const log = join(paths.liveDir, 'contract-server.log')
      const out = openSync(log, 'w')
      const cwd = mkdtempSync(join(tmpdir(), 'nasiko-contract-'))
      child = spawn(binary, [], { cwd, env, stdio: ['ignore', out, out] })
      child.once('exit', (code, sig) => {
        childExit = `server exited (${sig ?? code})`
      })
      // A missing binary (another CARGO_TARGET_DIR) fails the spawn: end the health wait, so teardown still runs.
      child.once('error', (err) => {
        childExit = `server failed to start: ${err.message}`
      })
      if (!args.keepDb) {
        cleanups.push(async () => {
          if (!child || childExit) return
          child.kill('SIGTERM')
          const deadline = Date.now() + 10_000
          while (!childExit && Date.now() < deadline) await new Promise((r) => setTimeout(r, 200))
          if (!childExit) child.kill('SIGKILL')
        })
        cleanups.push(() => rmSync(cwd, { recursive: true, force: true }))
      }
      await waitFor(
        'server',
        () => ok(`${base}/health`, 'ok'),
        TIMEOUT.health,
        () => (childExit ? `${ERRORS.serverExited(log).problem} (${childExit}); see ${log}` : null),
      )
      // The admin is bootstrapped just after the listener starts; the seed needs it.
      await waitFor(
        'admin',
        async () =>
          psql(DATABASES.contract, 'SELECT count(*) FROM users WHERE is_superuser;', {
            tuples: true,
          }).trim() !== '0',
        30_000,
      )

      const anchor = new Date().toISOString()
      const otlp = args.traces ? ['--otlp', `http://127.0.0.1:${PORTS.tempoOtlp}`] : []
      const s = spawnSync(
        process.execPath,
        [
          join(ROOT, 'scripts/seed-live.ts'),
          '--database',
          DATABASES.contract,
          '--anchor',
          anchor,
          ...otlp,
        ],
        { encoding: 'utf8' },
      )
      if (s.status !== 0)
        throw new Failure(
          'seed',
          `seed failed: ${(s.stderr || s.stdout).trim()}`,
          EXIT.notReady,
          `node scripts/seed-live.ts --database ${DATABASES.contract}`,
        )
    }

    const session = await login(base, admin.username, admin.password)
    const anchorIso = await verifySeedOnly(base, session.token, admin.username, args.edition)
    const anchor = new Date(anchorIso)
    if (Number.isNaN(anchor.getTime()))
      throw new Failure(
        'verify',
        `seed marker has no valid anchor: ${anchorIso}`,
        EXIT.notReady,
        'node scripts/seed-live.ts --database nasiko_contract',
      )
    const openapi = await fetchOnce(base, 'GET', '/api/openapi.json', {}, session.token)
    const server = {
      sha,
      dirty: dirtyFiles.length > 0,
      openapi_sha256: createHash('sha256')
        .update(typeof openapi.body === 'string' ? openapi.body : JSON.stringify(openapi.body))
        .digest('hex'),
      binary_mtime: existsSync(binary) && !args.reuse ? statSync(binary).mtime.toISOString() : null,
    }
    const me = (await fetchOnce(base, 'GET', '/api/me', {}, session.token)).body as { sub?: string }
    const replace = new Map<string, string>([[session.token, '<token>']])
    if (me.sub) replace.set(me.sub, '<admin-id>')
    if (session.userId) replace.set(session.userId, '<admin-id>')
    // The live admin's email appears in harness display names ("Codex (admin@localhost)"); map it to the mock admin's,
    // like the admin id, so values compare.
    const usersMe = (await fetchOnce(base, 'GET', '/api/users/me', {}, session.token)).body as {
      email?: string | null
    }
    const mockAdminEmail = generateHarnessSeed().users.find((u) => u.id === ADMIN_ID)?.email
    if (usersMe.email && mockAdminEmail && usersMe.email !== mockAdminEmail)
      replace.set(usersMe.email, mockAdminEmail)

    // EE (plan §8): refuse an OSS server, sign in the seed member, and keep every org id and secret out of fixtures.
    let member: Awaited<ReturnType<typeof login>> | null = null
    if (args.edition === 'ee') {
      if ((await fetchOnce(base, 'GET', '/api/org/units', {}, session.token)).status === 404) {
        const e = ERRORS.eeIsOss(base)
        throw new Failure('preflight', e.problem, EXIT.notReady, e.fix)
      }
      const creds = readCredentials(CREDENTIALS_FILE)
      const m = creds['seed-ee-member']
      if (!m)
        throw new Failure(
          'preflight',
          'no stored secret for seed-ee-member',
          EXIT.notReady,
          'npm run seed:ee',
        )
      member = await login(base, 'seed-ee-member', m.access_secret)
      replace.set(member.token, '<token>')
      for (const [name, c] of Object.entries(creds)) {
        replace.set(c.id, `<user-${name}>`)
        replace.set(c.access_secret, '<secret>')
        replace.set(c.access_key, '<access-key>')
      }
      const units = (await fetchOnce(base, 'GET', '/api/org/units', {}, session.token)).body as {
        data?: { id: string; name: string }[]
      }
      for (const u of units.data ?? []) replace.set(u.id, `<unit-${u.name}>`)
    }
    const t = tokens(tokenContext(anchor, me.sub ?? session.userId))
    // Traces are ready once the server finds the newest seed session through Tempo search. Fetching a trace by id
    // works as soon as Tempo has it, but search only once the trace is idle and cut into a block (about 20 s at
    // tempo.yaml's settings), and session list and detail use search. Otherwise trace-backed entries record as
    // no-trace-data.
    let tracesReady = false
    if (args.traces && recordable(manifest, args.edition, args.only).some((e) => e.traces)) {
      const deadline = Date.now() + TIMEOUT.traces
      while (!tracesReady && Date.now() < deadline) {
        tracesReady =
          (
            await fetchOnce(
              base,
              'GET',
              `/api/observability/session/${t.session0}`,
              {},
              session.token,
            )
          ).status === 200
        if (!tracesReady) await new Promise((r) => setTimeout(r, 2_000))
      }
      if (!tracesReady)
        console.log(
          `record-live: traces not searchable after ${TIMEOUT.traces / 1000} s; trace-backed fixtures record as no-trace-data (the observability contract is unverified). ${ERRORS.tempo('').fix}`,
        )
    }
    const endpoints = recordable(manifest, args.edition, args.only)
    const staging = mkdtempSync(join(paths.liveDir, 'out-'))
    cleanups.push(() => rmSync(staging, { recursive: true, force: true }))
    const written: string[] = []
    const checks: CheckResult[] = []
    for (const e of endpoints) {
      const req = requestFor(e, t)
      const token =
        e.auth === 'none' ? undefined : e.auth === 'member' ? member?.token : session.token
      if (e.auth === 'member' && !token)
        throw new Failure('manifest', `${e.id}: member identity is EE-only`, EXIT.usage)
      let r = await fetchOnce(base, e.method, req.path, req.query, token)
      if (e.window && crossed(r.start, r.end, e.window)) {
        r = await fetchOnce(base, e.method, req.path, req.query, token)
        if (crossed(r.start, r.end, e.window))
          throw new Failure(
            'record',
            `${e.id}: the request crossed a ${e.window} boundary twice`,
            EXIT.notReady,
            'npm run record:live',
          )
      }
      const fixture: Fixture = {
        fixture_version: FIXTURE_VERSION,
        id: e.id,
        recorded_at: r.start.toISOString(),
        window: { start: r.start.toISOString(), end: r.end.toISOString() },
        anchor: anchor.toISOString(),
        edition: args.edition,
        identity: e.auth === 'none' ? 'none' : e.auth === 'member' ? 'seed-ee-member' : 'admin',
        server,
        request: { method: e.method, path: e.path, query: e.query ?? {} },
        status: r.status,
        content_type: r.contentType,
        body: scrubBody(r.body, { replace, placeholders: e.placeholders }),
        ...(e.traces && !tracesReady ? { source: 'no-trace-data' as const } : {}),
      }
      const text = `${JSON.stringify(fixture, null, 2)}\n`
      const secretAt = findSecret(fixture.body)
      if (secretAt || text.includes(session.token)) {
        const x = ERRORS.secret(secretAt ?? 'token')
        throw new Failure('scrub', x.problem, EXIT.notReady, x.fix)
      }
      const rel = join(args.edition, e.feature, `${e.id}.json`)
      if (args.check) {
        const file = join(paths.fixtures, rel)
        checks.push(
          checkFixture(
            existsSync(file) ? (JSON.parse(readFileSync(file, 'utf8')) as Fixture) : undefined,
            fixture,
            e,
          ),
        )
        console.log(`record-live: ${e.id.padEnd(34)} ${String(r.status).padEnd(4)} checked`)
        continue
      }
      mkdirSync(dirname(join(staging, rel)), { recursive: true })
      writeFileSync(join(staging, rel), text)
      written.push(rel)
      console.log(
        `record-live: ${e.id.padEnd(34)} ${String(r.status).padEnd(4)} ${String(text.length).padStart(7)} B`,
      )
    }

    if (args.check) {
      const report = checkReport(checks)
      for (const line of report.lines) console.log(line)
      return { code: report.code, summary: report.summary }
    }
    // All or nothing: only now do fixtures replace the committed ones.
    for (const rel of written) {
      mkdirSync(dirname(join(paths.fixtures, rel)), { recursive: true })
      renameSync(join(staging, rel), join(paths.fixtures, rel))
    }
    const keep = new Set(written)
    const recordedFeatures = new Set(endpoints.map((e) => e.feature))
    let removed = 0
    for (const feature of recordedFeatures) {
      const dir = join(paths.fixtures, args.edition, feature)
      for (const f of existsSync(dir) ? readdirSync(dir) : []) {
        const rel = join(args.edition, feature, f)
        if (keep.has(rel) || !f.endsWith('.json')) continue
        const existing = JSON.parse(
          readFileSync(join(paths.fixtures, rel), 'utf8'),
        ) as Partial<Fixture>
        if (existing.source === 'hand-written') continue
        rmSync(join(paths.fixtures, rel))
        removed++
      }
    }
    return {
      code: EXIT.ok,
      summary: `${written.length} fixtures written${removed ? `, ${removed} stale removed` : ''} (anchor ${anchor.toISOString()}, server ${sha.slice(0, 8)})`,
    }
  } finally {
    process.off('SIGINT', onSignal)
    process.off('SIGTERM', onSignal)
    await teardown()
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().then(
    ({ code, summary }) => {
      if (summary !== 'help')
        console.log(`record-live: exit ${code} ${EXIT_MEANING[code]} (${summary})`)
      process.exit(code)
    },
    (err) => {
      const f = err instanceof Failure ? err : undefined
      const code = f?.code ?? EXIT.notReady
      console.error(
        `record-live: FAILED at ${f?.stage ?? 'unexpected'}: ${(err as Error).message}${f?.fix ? `\n  fix: ${f.fix}` : ''}`,
      )
      console.error(`record-live: exit ${code} ${EXIT_MEANING[code]}`)
      process.exit(code)
    },
  )
}
