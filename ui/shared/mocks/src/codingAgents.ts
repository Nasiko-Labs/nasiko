/**
 * Coding-agent onboarding mock (contract v3, oss/server/src/coding_agent_onboarding.rs and the desktop app's
 * `/__nasiko/coding-agents/*`).
 *
 * - The server's sync runs (`/api/me/coding-agents/sync`, bare JSON, plain-text errors): what the browser shows.
 *   `?mock=agents-importing|sending|rejected` run a scripted client, so the history windows fill newest first, the
 *   queued turns drain (phase `sending`) and the import finishes while you watch; OpenCode ends with refused turns, and
 *   Retry (`POST …/sync/{device_id}/retry`, 404 for a device with no run) makes the client resend them.
 *   `agents-none` has no run yet, `agents-done` a finished one, `agents-absent` a server without the routes. Every
 *   variant but `agents-absent` is a viewer with no registered harness yet (handlers.ts `onboardingSeed`).
 * - The desktop app (`/__nasiko/coding-agents/state|start|decline|retry`): a 404 by default, as a browser gets.
 *   `?mock=desktop-unasked|running|failed|rejected` answer as the desktop app, with its local consent and run;
 *   `desktop-sending` is a finished import while the app delivers turns its hooks queued (the `backlog`, which every
 *   desktop state carries: the run's waiting and refused turns plus `queued`), so the queue is still being sent.
 *   Every desktop state also routes LLM calls (`/__nasiko/coding-agents/routing[/connect|/disconnect]`, `mockRouting`):
 *   a connect attaches its config to the viewer's harness row through the router mock, so the routing read follows.
 *
 * Tests start with no runs and no desktop, so the card stays hidden on every page test whose viewer has harnesses.
 */
import { http, HttpResponse, type HttpHandler } from 'msw'

type Consent = 'unasked' | 'granted' | 'declined'
type Phase =
  'awaiting_consent' | 'discovering' | 'connecting' | 'importing' | 'sending' | 'done' | 'failed'
type ImportStatus = 'waiting' | 'running' | 'done' | 'failed' | 'skipped'

export type MockSyncWindow = {
  label: string
  status: 'waiting' | 'running' | 'done'
  sessions: number
  turns: number
}

export type MockSyncAgent = {
  id: string
  name: string
  detected: boolean
  connected: boolean
  import: {
    status: ImportStatus
    done: number
    total: number
    sessions: number
    turns: number
    failed: number
    error: string | null
    pending: number
    rejected: number
    rejected_reasons: { reason: string; count: number }[]
  }
}

export type MockSyncRun = {
  device_id: string
  device_name: string
  client: 'desktop' | 'cli'
  phase: Phase
  agents: MockSyncAgent[]
  error: string | null
  started_at: string | null
  updated_at: string
  windows: MockSyncWindow[]
  retry_requested_at: string | null
}

/** The desktop app's local side: its consent, and its run (scripted, or a fixed one). */
export interface DesktopMockState {
  consent: Consent
  signedIn: boolean
  /** When the scripted client started (ms), or null. */
  scriptStart: number | null
  retryAt: number | null
  /** A fixed run in place of the script (the failed one). */
  run: MockLocalRun | null
  /** The next N starts answer 500, as a desktop that couldn't start. */
  failStarts: number
  /** Turns queued outside the run (the agents' hooks), waiting to be sent. */
  queued: number
  /** A background delivery pass is going now. */
  sending: boolean
  /** The routable harnesses (`/__nasiko/coding-agents/routing`, `coding_agents_api.rs` `RoutingStatus`). */
  routing: MockRoutedHarness[]
}

export type MockRoutedHarness = {
  id: string
  name: string
  detected: boolean
  connected_by: 'desktop' | 'cli' | null
  agent_id: string | null
  agent_name: string | null
  server_url: string | null
  mcp: boolean
  changed: boolean
}

/** Claude Code here and unrouted, Codex routed by the CLI, OpenCode not on this computer. */
export const mockRouting = (): MockRoutedHarness[] => {
  const row = (id: string, name: string, detected: boolean): MockRoutedHarness => ({
    id,
    name,
    detected,
    connected_by: null,
    agent_id: null,
    agent_name: null,
    server_url: null,
    mcp: false,
    changed: false,
  })
  return [
    row('claude', 'Claude Code', true),
    { ...row('codex', 'Codex', true), connected_by: 'cli', server_url: MOCK_SERVER_URL, mcp: true },
    row('opencode', 'OpenCode', false),
  ]
}

const MOCK_SERVER_URL = 'http://localhost:8080'

/** What attaching an LLM config to a harness's row did (handlers.ts, through the router mock). */
export interface MockRoutingAttach {
  agentId: string | null
  agentName: string | null
  provider: string | null
  model: string | null
}

export type MockLocalRun = Omit<
  MockSyncRun,
  'device_id' | 'device_name' | 'client' | 'retry_requested_at'
>

export interface CodingAgentsState {
  runs: MockSyncRun[]
  /** A server without the routes: every read and write is a bare 404. */
  absent: boolean
  /** Browser demo: when the scripted client started (ms), or null. */
  scriptStart: number | null
  /** When a Retry was asked of the scripted client (ms), or null. */
  retryAt: number | null
  /** The desktop app, or null in a browser (its routes are a 404). */
  desktop: DesktopMockState | null
}

/** `?mock=` states (page variants: they combine with the others). Mirrored by the Harnesses route's mock list. */
export const AGENTS_ONBOARDING_VARIANTS = [
  'agents-none',
  'agents-importing',
  'agents-sending',
  'agents-rejected',
  'agents-done',
  'agents-absent',
  'desktop-unasked',
  'desktop-running',
  'desktop-failed',
  'desktop-rejected',
  'desktop-sending',
] as const
export type AgentsOnboardingVariant = (typeof AGENTS_ONBOARDING_VARIANTS)[number]

const DEVICE_ID = '5eed0017000000000000000000000001'
const DEVICE_NAME = 'Example MacBook'
const DAY_MS = 86_400_000
const STARTED_AGO_MS = 60_000

const agent = (
  id: string,
  name: string,
  detected: boolean,
  imp: Partial<MockSyncAgent['import']> = {},
): MockSyncAgent => ({
  id,
  name,
  detected,
  connected: detected,
  import: {
    status: detected ? 'waiting' : 'skipped',
    done: 0,
    total: 0,
    sessions: 0,
    turns: 0,
    failed: 0,
    error: null,
    pending: 0,
    rejected: 0,
    rejected_reasons: [],
    ...imp,
  },
})

/** One run, built from its phase and agents (the device and timestamps are the mock's). */
export function mockRun(
  phase: Phase,
  agents: MockSyncAgent[],
  now: number,
  extra: Partial<MockSyncRun> = {},
): MockSyncRun {
  return {
    device_id: DEVICE_ID,
    device_name: DEVICE_NAME,
    client: 'desktop',
    phase,
    agents,
    error: null,
    started_at: phase === 'awaiting_consent' ? null : new Date(now - STARTED_AGO_MS).toISOString(),
    updated_at: new Date(now).toISOString(),
    windows: [],
    retry_requested_at: null,
    ...extra,
  }
}

/** The four agents mid-import: Claude Code importing, Codex done, OpenCode waiting, Cursor not on this computer. */
export function importingAgents(): MockSyncAgent[] {
  return [
    agent('claude', 'Claude Code', true, { status: 'running', done: 40, total: 123 }),
    agent('codex', 'Codex', true, {
      status: 'done',
      done: 12,
      total: 12,
      sessions: 12,
      turns: 58,
    }),
    agent('opencode', 'OpenCode', true, { status: 'waiting', total: 7 }),
    agent('cursor', 'Cursor', false),
  ]
}

/** The same agents once the import finished (one Claude Code session unreadable). */
export function doneAgents(): MockSyncAgent[] {
  return [
    agent('claude', 'Claude Code', true, {
      status: 'done',
      done: 123,
      total: 123,
      sessions: 98,
      turns: 753,
      failed: 1,
    }),
    agent('codex', 'Codex', true, {
      status: 'done',
      done: 12,
      total: 12,
      sessions: 12,
      turns: 58,
    }),
    agent('opencode', 'OpenCode', true, {
      status: 'done',
      done: 7,
      total: 7,
      sessions: 5,
      turns: 31,
    }),
    agent('cursor', 'Cursor', false),
  ]
}

/** OpenCode's refused turns: two reasons, as a client reports them. */
export const REJECTED_REASONS = [
  { reason: 'turn larger than 1 MB', count: 2 },
  { reason: 'missing timestamp', count: 1 },
]
const REJECTED_TOTAL = REJECTED_REASONS.reduce((n, r) => n + r.count, 0)

/** The finished agents with OpenCode's refused turns (and those turns not counted as imported). */
export function rejectedAgents(): MockSyncAgent[] {
  return doneAgents().map((a) =>
    a.id === 'opencode'
      ? {
          ...a,
          import: {
            ...a.import,
            turns: a.import.turns - REJECTED_TOTAL,
            rejected: REJECTED_TOTAL,
            rejected_reasons: REJECTED_REASONS,
          },
        }
      : a,
  )
}

/** The history a client reads, newest first: two recent windows, one per earlier day of the last month, then the rest.
 *  `done` windows are finished; the next one is running unless `running` is false; the rest wait. */
export function mockWindows(now: number, done: number, running = true): MockSyncWindow[] {
  const day = (k: number) =>
    new Date(now - k * DAY_MS).toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
      timeZone: 'UTC',
    })
  const labels = [
    'Last 8 hours',
    'Last 24 hours',
    ...Array.from({ length: MONTH_DAYS - 2 }, (_, i) => day(i + 2)),
    'Older',
  ]
  return labels.map((label, i) => {
    const sessions = i === 0 ? 12 : i === 1 ? 9 : label === 'Older' ? 30 : (i % 3) + 1
    return {
      label,
      status: i < done ? 'done' : i === done && running ? 'running' : 'waiting',
      sessions,
      turns: sessions * 6,
    }
  })
}
const MONTH_DAYS = 30
/** Windows in `mockWindows`: the last month's, plus "Older". */
const WINDOW_COUNT = MONTH_DAYS + 1

// The scripted client (browser demo): seconds after it starts. The last month takes the first 70% of the sessions,
// "Older" the rest; then the queued turns drain, `sendPerSecond` at a time; a Retry resends in `resend` seconds.
const SCRIPT = {
  discover: 3,
  connect: 5,
  import: 7,
  perSecond: 12,
  sendTotal: 48,
  sendPerSecond: 12,
  resend: 3,
} as const
const MONTH_SHARE = 0.7
const SCRIPT_TOTALS = { claude: 123, codex: 12, opencode: 7 } as const
const SCRIPT_SESSIONS = Object.values(SCRIPT_TOTALS).reduce((n, t) => n + t, 0)
const IMPORT_SECONDS = SCRIPT_SESSIONS / SCRIPT.perSecond
const SEND_SECONDS = SCRIPT.sendTotal / SCRIPT.sendPerSecond
/** An agent's import before it starts. */
const EMPTY = { status: 'waiting', done: 0, sessions: 0, turns: 0, failed: 0 } as const

/** Windows finished once `share` of the sessions are read. */
function windowsAt(share: number, now: number): MockSyncWindow[] {
  const month = Math.min(MONTH_DAYS, Math.floor((share / MONTH_SHARE) * MONTH_DAYS))
  return mockWindows(now, share >= 1 ? WINDOW_COUNT - 1 : month)
}

function importingRun(s: number, now: number): MockSyncRun {
  // Sessions are imported one agent after another, in catalog order.
  const read = Math.floor((s - SCRIPT.import) * SCRIPT.perSecond)
  let budget = read
  const agents = doneAgents().map((a): MockSyncAgent => {
    const total = SCRIPT_TOTALS[a.id as keyof typeof SCRIPT_TOTALS]
    if (total === undefined) return a
    const done = Math.min(total, budget)
    budget -= done
    if (done === total) return a
    const share = done / total
    return {
      ...a,
      import: {
        ...a.import,
        status: done ? 'running' : 'waiting',
        done,
        sessions: Math.floor(a.import.sessions * share),
        turns: Math.floor(a.import.turns * share),
        failed: 0,
      },
    }
  })
  return mockRun('importing', agents, now, {
    windows: windowsAt(Math.min(1, read / SCRIPT_SESSIONS), now),
  })
}

/** Every window read: the queued turns drain (Claude Code's), then the run is done with OpenCode's turns refused. */
function deliveringRun(s: number, now: number, retryAt: number | null): MockSyncRun {
  const sent = Math.floor((s - SCRIPT.import - IMPORT_SECONDS) * SCRIPT.sendPerSecond)
  const pending = Math.max(0, SCRIPT.sendTotal - sent)
  const retried = retryAt !== null
  const resending = retried && now - retryAt < SCRIPT.resend * 1000
  const agents = rejectedAgents().map((a): MockSyncAgent => {
    if (a.id === 'claude') return { ...a, import: { ...a.import, pending } }
    if (a.id !== 'opencode' || !retried) return a
    // A Retry re-queues the refused turns; they go through this time.
    const turns = a.import.turns + (resending ? 0 : REJECTED_TOTAL)
    return {
      ...a,
      import: {
        ...a.import,
        turns,
        pending: resending ? REJECTED_TOTAL : 0,
        rejected: 0,
        rejected_reasons: [],
      },
    }
  })
  const sending = agents.some((a) => a.import.pending > 0)
  return mockRun(sending ? 'sending' : 'done', agents, now, {
    windows: mockWindows(now, sending ? WINDOW_COUNT - 1 : WINDOW_COUNT),
    retry_requested_at: retried ? new Date(retryAt).toISOString() : null,
  })
}

function scriptedRun(elapsedMs: number, now: number, retryAt: number | null): MockSyncRun | null {
  const s = elapsedMs / 1000
  if (s < SCRIPT.discover) return null
  const before = (a: MockSyncAgent) => (a.detected ? { ...a.import, ...EMPTY } : a.import)
  if (s < SCRIPT.connect)
    return mockRun(
      'discovering',
      doneAgents().map((a) => ({ ...a, connected: false, import: before(a) })),
      now,
    )
  if (s < SCRIPT.import)
    return mockRun(
      'connecting',
      doneAgents().map((a) => ({ ...a, import: before(a) })),
      now,
    )
  if (s < SCRIPT.import + IMPORT_SECONDS) return importingRun(s, now)
  return deliveringRun(s, now, retryAt)
}

/** Seconds into the script for the `agents-sending` and `agents-rejected` states. */
const SENDING_AT = SCRIPT.import + IMPORT_SECONDS + 1
const REJECTED_AT = SCRIPT.import + IMPORT_SECONDS + SEND_SECONDS + 1

/** The script just past discovery's start, so a desktop start answers with a run at once. */
const DISCOVERING_AT = SCRIPT.discover

const desktopBase = (over: Partial<DesktopMockState> = {}): DesktopMockState => ({
  consent: 'granted',
  signedIn: true,
  scriptStart: null,
  retryAt: null,
  run: null,
  failStarts: 0,
  queued: 0,
  sending: false,
  routing: mockRouting(),
  ...over,
})

/** A run as the desktop holds it: without the device fields (it is this computer). */
function toLocal(run: MockSyncRun): MockLocalRun {
  const { device_id: _d, device_name: _n, client: _c, retry_requested_at: _r, ...local } = run
  return local
}

/** A desktop run that stopped: the user signed out mid-import. */
const failedLocalRun = (now: number): MockLocalRun =>
  toLocal(
    mockRun('failed', importingAgents(), now, {
      error: 'signed out of Nasiko',
      windows: mockWindows(now, 4, false),
    }),
  )

/** A desktop import that finished, every window in. */
export const doneLocalRun = (now: number): MockLocalRun =>
  toLocal(mockRun('done', doneAgents(), now, { windows: mockWindows(now, WINDOW_COUNT) }))

/** The state for a `?mock=` variant (null: the tests' default). */
export function buildCodingAgentsState(
  variant: AgentsOnboardingVariant | null,
  now: number,
): CodingAgentsState {
  const base: CodingAgentsState = {
    runs: [],
    absent: false,
    scriptStart: null,
    retryAt: null,
    desktop: null,
  }
  switch (variant) {
    case 'agents-importing':
      return { ...base, scriptStart: now - (SCRIPT.import + 2) * 1000 }
    case 'agents-sending':
      return { ...base, scriptStart: now - SENDING_AT * 1000 }
    case 'agents-rejected':
      return { ...base, scriptStart: now - REJECTED_AT * 1000 }
    case 'agents-done':
      return {
        ...base,
        runs: [mockRun('done', doneAgents(), now, { windows: mockWindows(now, WINDOW_COUNT) })],
      }
    case 'agents-absent':
      return { ...base, absent: true }
    case 'desktop-unasked':
      return { ...base, desktop: desktopBase({ consent: 'unasked' }) }
    case 'desktop-running':
      return {
        ...base,
        desktop: desktopBase({ scriptStart: now - (SCRIPT.import + 2) * 1000 }),
      }
    case 'desktop-failed':
      return { ...base, desktop: desktopBase({ run: failedLocalRun(now) }) }
    case 'desktop-rejected':
      return { ...base, desktop: desktopBase({ scriptStart: now - REJECTED_AT * 1000 }) }
    case 'desktop-sending':
      return {
        ...base,
        desktop: desktopBase({ run: doneLocalRun(now), queued: 499, sending: true }),
      }
    default:
      return base
  }
}

interface Ctx {
  state: () => CodingAgentsState
  now: () => number
  loggedIn: () => boolean
  unauthorized: () => Response
  text: (body: string, status: number) => Response
  /** Attaches `config` (null: detach) to the viewer's `harness` row, as the desktop's connect does; throws the
   *  router's refusal. */
  attachRouting: (harness: string, config: string | null) => MockRoutingAttach
}

function scriptedOf(st: CodingAgentsState, now: number): MockSyncRun | null {
  return st.scriptStart === null ? null : scriptedRun(now - st.scriptStart, now, st.retryAt)
}

function runsOf(st: CodingAgentsState, now: number): MockSyncRun[] {
  const scripted = scriptedOf(st, now)
  const runs = scripted
    ? [scripted, ...st.runs.filter((r) => r.device_id !== scripted.device_id)]
    : st.runs
  // The caller's rows updated in the last 24 h, newest first.
  return [...runs]
    .filter((r) => now - Date.parse(r.updated_at) <= DAY_MS)
    .sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at))
}

/** POST …/sync/{device_id}/retry: the scripted client resends on its own; a stored run just records the ask. */
function requestRetry(st: CodingAgentsState, deviceId: string, now: number): MockSyncRun | null {
  const stamp = new Date(now).toISOString()
  const scripted = scriptedOf(st, now)
  if (scripted?.device_id === deviceId) {
    st.retryAt = now
    return scriptedOf(st, now)
  }
  const stored = st.runs.find((r) => r.device_id === deviceId)
  if (!stored) return null
  const next = { ...stored, retry_requested_at: stamp, updated_at: stamp }
  st.runs = st.runs.map((r) => (r === stored ? next : r))
  return next
}

// ─── The desktop app ────────────────────────────────────────────────────────────────────────────────────────────────

function localRun(d: DesktopMockState, now: number): MockLocalRun | null {
  if (d.run) return d.run
  const run = d.scriptStart === null ? null : scriptedRun(now - d.scriptStart, now, d.retryAt)
  return run ? toLocal(run) : null
}

/** How long a turn waiting on a retry waits, as the desktop's own backoff might. */
const RETRY_IN_MS = 120_000

/** The desktop's queue (server.rs `backlog_json`): the run's waiting and refused turns plus what hooks queued. */
function desktopBacklog(d: DesktopMockState, run: MockLocalRun | null, now: number) {
  const sum = (key: 'pending' | 'rejected') =>
    (run?.agents ?? []).reduce((n, a) => n + a.import[key], 0)
  const pending = sum('pending') + d.queued
  const sending = d.sending || run?.phase === 'sending'
  return {
    pending,
    rejected: sum('rejected'),
    sending,
    next_attempt_at: pending > 0 && !sending ? new Date(now + RETRY_IN_MS).toISOString() : null,
  }
}

/** The desktop's own counts on its run (server.rs `run_json`): windows read and turns delivered. */
function withAppCounts(run: MockLocalRun) {
  const sum = (key: 'turns' | 'pending' | 'rejected') =>
    run.agents.reduce((n, a) => n + a.import[key], 0)
  return {
    ...run,
    windows_read: run.windows.filter((w) => w.status === 'done').length,
    delivered: Math.max(0, sum('turns') - sum('pending') - sum('rejected')),
  }
}

const desktopState = (d: DesktopMockState, now: number) => {
  const run = localRun(d, now)
  return {
    desktop: true,
    consent: d.consent,
    signed_in: d.signedIn,
    run: run && withAppCounts(run),
    backlog: desktopBacklog(d, run, now),
  }
}

/** Start (or start again): consent granted, and the scripted client begins at discovery, unless a run is active. */
function startDesktop(d: DesktopMockState, now: number) {
  d.consent = 'granted'
  const run = localRun(d, now)
  if (run && run.phase !== 'done' && run.phase !== 'failed') return
  d.run = null
  d.retryAt = null
  d.scriptStart = now - DISCOVERING_AT * 1000
}

function desktopHandlers(ctx: Ctx): HttpHandler[] {
  const BASE = '/__nasiko/coding-agents'
  // No desktop: the server's 404, as a browser gets for this path.
  const desktop = () => ctx.state().desktop
  const absent = () => new HttpResponse('Not Found', { status: 404 })
  return [
    http.get(`${BASE}/state`, () => {
      const d = desktop()
      return d ? HttpResponse.json(desktopState(d, ctx.now())) : absent()
    }),
    http.post(`${BASE}/start`, () => {
      const d = desktop()
      if (!d) return absent()
      if (!d.signedIn) return ctx.text('sign in to Nasiko first', 409)
      if (d.failStarts > 0) {
        d.failStarts -= 1
        return ctx.text('the desktop app could not start the import', 500)
      }
      startDesktop(d, ctx.now())
      return HttpResponse.json(desktopState(d, ctx.now()), { status: 202 })
    }),
    http.post(`${BASE}/decline`, () => {
      const d = desktop()
      if (!d) return absent()
      d.consent = 'declined'
      return HttpResponse.json(desktopState(d, ctx.now()))
    }),
    http.post(`${BASE}/retry`, () => {
      const d = desktop()
      if (!d) return absent()
      const now = ctx.now()
      // A failed run restarts; otherwise the refused turns are re-queued.
      if (localRun(d, now)?.phase === 'failed') startDesktop(d, now)
      else d.retryAt = now
      return HttpResponse.json(desktopState(d, now), { status: 202 })
    }),
    // LLM routing (`coding_agents_api.rs`): 409 plain text without a sign-in, 400/422 `{message}` refusals.
    http.get(`${BASE}/routing`, () => {
      const d = desktop()
      return d ? HttpResponse.json({ harnesses: d.routing }) : absent()
    }),
    http.post(`${BASE}/routing/connect`, async ({ request }) => {
      const d = desktop()
      if (!d) return absent()
      if (!d.signedIn) return ctx.text('Sign in to a Nasiko server in this window first.', 409)
      const body = (await request.json().catch(() => ({}))) as { agent?: string; config?: string }
      const row = d.routing.find((r) => r.id === body.agent)
      if (!row)
        return refuse(400, `'${body.agent}' is not a coding agent whose LLM calls can be routed`)
      if (!row.detected) return refuse(422, `${row.name} was not found on this computer`)
      let attached: MockRoutingAttach
      try {
        attached = ctx.attachRouting(row.id, body.config ?? null)
      } catch (e) {
        return refuse(422, e instanceof Error ? e.message : String(e))
      }
      const tookOver = row.connected_by === 'cli'
      Object.assign(row, {
        connected_by: 'desktop',
        agent_id: attached.agentId,
        agent_name: attached.agentName,
        server_url: MOCK_SERVER_URL,
        mcp: true,
        changed: false,
      })
      const notes = [
        ...(tookOver
          ? [
              `Took over ${row.name} from the Nasiko CLI; \`nasiko status ${row.id}\` now reports it disconnected.`,
            ]
          : []),
        `Restart ${row.name} so it picks up the change.`,
      ]
      return HttpResponse.json({
        harness: row,
        provider: attached.provider,
        model: attached.model,
        notes,
      })
    }),
    http.post(`${BASE}/routing/disconnect`, async ({ request }) => {
      const d = desktop()
      if (!d) return absent()
      const body = (await request.json().catch(() => ({}))) as { agent?: string }
      const row = d.routing.find((r) => r.id === body.agent)
      if (!row)
        return refuse(400, `'${body.agent}' is not a coding agent whose LLM calls can be routed`)
      const was = row.connected_by
      Object.assign(row, {
        connected_by: null,
        agent_id: null,
        agent_name: null,
        server_url: null,
        mcp: false,
        changed: false,
      })
      return HttpResponse.json({
        harness: row,
        notes: was ? [`Restart ${row.name} so it picks up the change.`] : [],
      })
    }),
  ]
}

const refuse = (status: number, message: string) => HttpResponse.json({ message }, { status })

export function codingAgentsHandlers(ctx: Ctx): HttpHandler[] {
  const gate = () => {
    if (!ctx.loggedIn()) return ctx.unauthorized()
    if (ctx.state().absent) return new HttpResponse(null, { status: 404 })
    return null
  }
  return [
    ...desktopHandlers(ctx),
    http.get('/api/me/coding-agents/sync', () => {
      const stop = gate()
      if (stop) return stop
      return HttpResponse.json({ runs: runsOf(ctx.state(), ctx.now()) })
    }),
    http.post('/api/me/coding-agents/sync/:deviceId/retry', ({ params }) => {
      const stop = gate()
      if (stop) return stop
      const run = requestRetry(ctx.state(), String(params.deviceId), ctx.now())
      return run ? HttpResponse.json(run) : ctx.text('no sync run for that device', 404)
    }),
  ]
}
