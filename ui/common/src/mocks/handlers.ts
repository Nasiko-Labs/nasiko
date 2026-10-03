/**
 * MSW handlers — every mocked endpoint derives from one seed (plan D4, A19).
 *
 * Adding a mocked endpoint: derive its data from `seed` in aggregate.ts, add a handler
 * here under the right `Mockable` group, and it applies to the browser worker and
 * Vitest (msw/node) automatically.
 *
 * Finops 200s use the `{data, status_code, message}` envelope; 400/404/500 are plain
 * text, like the real server.
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type { components } from '@/lib/api/schema.gen'
import { mockEntries as parseMockList } from '@/features/chat/search'
import { SPEND_LIMIT } from '@/features/router/tuning'
import { secretNameProblem } from '@/features/settings/logic'
import { PASSWORD_CODES, passwordProblem } from '@/lib/password'
import {
  budgetAlerts,
  budgetStatus,
  buildBudgetState,
  createBudget,
  deleteBudget,
  listBudgets,
  updateBudget,
  type BudgetMockStore,
} from './budgets'
import type { Mockable } from '@/lib/env'
import {
  dashboard,
  dayDrilldown,
  MockHttpError,
  savings,
  spendCalendar,
  spendTimeseries,
  topTraces,
  withHarnessTurns,
} from './aggregate'
import {
  agentLogs,
  agentsList,
  observabilityData,
  sessionDetail,
  sessionList,
  spanDetail,
  traceDetail,
} from './observability'
import { generateSeed, SEED_MODELS, type Seed } from './seed'
import {
  fleetUnpriced,
  myAgentDashboardRows,
  resolveWin,
  usage,
  UsageHttpError,
} from './harnessUsage'
import {
  ADMIN_ID,
  DEFAULT_PERSONA,
  generateHarnessSeed,
  type HarnessSeed,
  type HUser,
} from './seed-harness'
import {
  buildAgentsState,
  detailBody,
  listRow,
  liveStatus,
  logLines,
  type AgentsState,
  type MockAgent,
} from './agents'
import { CHAT_SCENARIOS, type ChatScenario } from './chat'
import {
  buildRouterState,
  createConfig,
  createCustom,
  validateDialect,
  deleteConfig,
  deleteCustom,
  listConfigs,
  listCustom,
  listSecrets,
  deleteSecret,
  patchRouting,
  touchSecret,
  upsertSecret,
  resolveRouting,
  setDefault,
  syncCustom,
  updateConfig,
  updateCustom,
  usageByAgent,
  type RouterState,
} from './router'
import { chatHandlers, chatMockRows, resetChatMock } from './chatStore'
import {
  advanceBuild,
  buildDeployState,
  buildStatusAt,
  completeImport,
  mockRepos,
  queueUpload,
  readMultipart,
  recordAt,
  settleUploads,
  uploadAt,
  type DeployState,
  type MockBuild,
} from './deploy'
import { buildMcpState, mcpHandlers, type McpState } from './mcp'
import { buildWorkflowsState, workflowHandlers, type WorkflowsState } from './workflows'
import { checkZip, MAX_ZIP_BYTES } from '@/features/deploy/zipcheck'
import { nameProblem } from '@/features/deploy/name'
import { parseVersion } from '@/features/deploy/version'
import { DEFAULT_REGISTRY, parseReference } from '@/features/deploy/registry'
import type { BuildStatus, UploadPipelineStatus } from '@/features/deploy/types'
import { PERSONAS, type Persona } from '@/features/onboarding/types'

// Built on first use, so mock mode doesn't block first paint on seed generation.
let seed: Seed | null = null
const getSeed = (): Seed => (seed ??= generateSeed())
let nowFn: () => number = () => Date.now()
let loggedIn = true
/** Browser mock only: remembers sign-in across reloads, like the server's cookie (QA ISSUE-003). */
let persistLogin: ((loggedIn: boolean) => void) | null = null
const setLoggedIn = (value: boolean) => {
  loggedIn = value
  persistLogin?.(value)
}

/**
 * Chat page states (v1c DX1): unlike stream scenarios they combine, `?mock=waiting,many-chats`.
 * `CHAT_PAGE_VARIANT_KEYS` in src/features/chat/scenarioKeys.ts mirrors this list (a test checks it).
 * - `no-agents`: every agent reads as stopped, so Chat has nothing running to offer.
 * - `many-chats`: the opt-in 60-row rail seed (P-5), for the date groups and Load more.
 * - `many-recorded`: 60 recorded sessions in the last hour, crossing the 50-row page (§5.7, DS12).
 * - `probe-500`: the Sessions → Open chat probe fails with a 500 (§5.10).
 * - `waiting`: the opt-in pending requests (§5.11); `pending-fail` fails every poll, `pending-flaky` answers
 *   once, fails the next 3 polls, then works again (DX3).
 */
/** `/api/agents/<segment>` routes that aren't an agent id (`agents/mod.rs` router). */
const AGENTS_STATIC = new Set(['uploads', 'deployments'])

export const CHAT_PAGE_VARIANTS = [
  'no-agents',
  'many-chats',
  'many-recorded',
  'probe-500',
  'waiting',
  'pending-fail',
  'pending-flaky',
] as const

/**
 * LLM router page states (plan §6). They combine like the chat page variants (`?mock=router-legacy,router-repin-fail`).
 * `ROUTER_VARIANT_KEYS` in src/features/router/scenarioKeys.ts mirrors this list (a test checks it).
 * - `router-empty`: no configs (every agent on no config); `router-409`: delete answers "attached to 3 agent(s)";
 * - `router-custom-down`: custom-provider test and sync fail; `router-catalog-fail`: the provider catalog 500s;
 * - `router-secrets-fail` / `router-no-secrets`: the secret list fails / is empty;
 * - `router-repin-fail`: a pin-only PATCH fails with a 500 (the second step of attach → re-pin);
 * - `router-usage-fail` / `router-usage-full`: by-agent usage fails / returns 1,000 rows;
 * - `router-legacy`: configs made elsewhere (openrouter, a missing saved key, a deleted custom provider, off-catalog models).
 * - `router-budgets-empty` / `router-budgets-fail`: no budgets / every /api/budgets read answers 500 (R2).
 */
export const ROUTER_PAGE_VARIANTS = [
  'router-empty',
  'router-409',
  'router-custom-down',
  'router-catalog-fail',
  'router-secrets-fail',
  'router-no-secrets',
  'router-repin-fail',
  'router-usage-fail',
  'router-usage-full',
  'router-legacy',
  'router-budgets-empty',
  'router-budgets-fail',
] as const
/** Page variants combine (read through hasVariant) instead of taking the single degraded-state slot. */
const PAGE_VARIANTS: readonly string[] = [...CHAT_PAGE_VARIANTS, ...ROUTER_PAGE_VARIANTS]

/**
 * Degraded states for manual QA and tests (mock mode only). The browser reads `?mock=`
 * on every request, so switching the URL is enough; tests call configureMocks({ variant }).
 * `?mock=` takes a comma list (v1c DX1): the first entry that names one of these is the variant.
 */
export const MOCK_VARIANTS = [
  'tempo-down',
  'empty',
  'trace-503',
  'trace-500',
  'scan-fail',
  // Harnesses page (plan §7): each forces one state the page must handle. The OSS build is the OSS case, and the EE
  // build's mocks answer as its own server does, so neither edition is a variant (docs/lab-vs-react-migration-review.md
  // §10.5).
  'usage-404',
  // A coded 404 below the landing: served by the EE layer's mocks (the OSS endpoint only ever answers "self").
  'drill-404',
  'prev-fail',
  'usage-500',
  'all-unpriced',
  'no-activity',
  // App shell (plans/feat-app-shell.md eng D4): /health fails like a stopped server.
  'server-down',
  // App shell sign out: logout gets the server's fail-closed 401 (review, api-contract).
  'logout-unavailable',
  // Deploy (plans/feat-deploy.md §9): every build read answers `{available:false}` (a caller without deploy rights, EE);
  // `builds-absent` answers /api/builds with a bare 404, like a server that predates it; `deploy-build-fails` accepts an
  // upload and then fails its build.
  'deploy-no-rights',
  'builds-absent',
  'deploy-build-fails',
  // GitHub (plans/feat-deploy.md §4.2): no OAuth app on the server, the viewer not connected yet, or no repositories.
  'deploy-github-unconfigured',
  'deploy-github-disconnected',
  'deploy-github-no-repos',
  // Registry import (§4.3): turned off on the server, or imported but the deploy didn't start (container_name null).
  'deploy-registry-disabled',
  'deploy-registry-not-running',
  // MCP servers (plans/feat-mcp.md §8): no servers or toolkits at all; the toolkit list fails alone; an upload's build fails.
  'mcp-empty',
  'mcp-toolkits-fail',
  'mcp-upload-fails',
  // Workflows (plans/feat-workflows.md §8): no workflows or runs; answers as OSS `main` (no drafts, promote, metrics);
  // the planner has no API key (503) or fails (422).
  'workflows-empty',
  'workflows-classic',
  'workflows-no-key',
  'workflows-planner-fails',
  // Onboarding (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §5): a server without
  // /api/me/onboarding (bare 404), or a user who already picked a persona.
  'onboarding-absent',
  'onboarding-done',
  ...CHAT_PAGE_VARIANTS,
  ...ROUTER_PAGE_VARIANTS,
] as const
export type MockVariant = (typeof MOCK_VARIANTS)[number]
let fixedVariant: MockVariant | null = null
/** The mock `me`'s superuser flag (v1c DX2): the seed admin is one; tests override it, the browser uses `?superuser=0`. */
let fixedSuperuser: boolean | null = null
function superuser(): boolean {
  if (fixedSuperuser !== null) return fixedSuperuser
  if (personaLocked) return true
  return new URLSearchParams(globalThis.location?.search ?? '').get('superuser') !== '0'
}

/** The `?mock=` comma list (v1c DX1). Partial-live mode (VITE_NASIKO_MOCK) reads none of it: `?mock=` must not
 *  force states that mix seed and real-server data (e.g. usage-404 would run the fallback against real finops rows). */
function mockEntries(): string[] {
  if (personaLocked) return []
  return parseMockList(
    new URLSearchParams(globalThis.location?.search ?? '').get('mock') ?? undefined,
  )
}

function variant(): MockVariant | null {
  if (fixedVariant) return fixedVariant
  // Page variants (chat, router) combine (read through hasVariant), so they never take the single degraded-state slot:
  // `?mock=waiting,server-down` still takes the server down.
  return (
    (mockEntries().find(
      (e) => (MOCK_VARIANTS as readonly string[]).includes(e) && !PAGE_VARIANTS.includes(e),
    ) as MockVariant | undefined) ?? null
  )
}

/** A page variant (chat or router) is on: the test's fixed variants, or any entry of the comma list. */
function hasVariant(v: MockVariant): boolean {
  return fixedVariant === v || fixedPageVariants.has(v) || mockEntries().includes(v)
}
/** Tests: page variants that combine (`configureMocks({ chatVariants, routerVariants })`). */
let fixedPageVariants = new Set<string>()

// Harnesses: its own seed (T1) and the viewer persona (`?as=<seed-username>`, mock only).
let harnessSeed: HarnessSeed | null = null
const getHarnessSeed = (): HarnessSeed =>
  (harnessSeed ??= generateHarnessSeed({ anchor: new Date(nowFn()) }))
let fixedPersona: string | null = null
let personaLocked = false

/** The viewer: `?as=` on every request (the switcher replaces the URL), unless pinned. */
function viewer(): HUser {
  const hs = getHarnessSeed()
  const name =
    personaLocked || fixedPersona
      ? (fixedPersona ?? DEFAULT_PERSONA)
      : (new URLSearchParams(globalThis.location?.search ?? '').get('as') ?? DEFAULT_PERSONA)
  return (
    hs.users.find((u) => u.username === name) ??
    hs.users.find((u) => u.username === DEFAULT_PERSONA)!
  )
}

/** Who is signed in: what /api/me and login report. */
export interface SessionUser {
  id: string
  username: string
  is_superuser: boolean
}
/** An edition's mocks may keep their own session (`configureMocks({ session })`); null: the viewer. */
let editionSession: (() => SessionUser) | null = null
function sessionUser(): SessionUser {
  if (editionSession) return editionSession()
  const v = viewer()
  // The seed admin's flag follows `?superuser=0` / configureMocks({ superuser }) (v1c DX2).
  return {
    id: v.id,
    username: v.username,
    is_superuser: v.id === ADMIN_ID ? superuser() : v.is_superuser,
  }
}

/** Seed variants that change the data itself (memoised per variant). */
const variantSeeds = new Map<string, HarnessSeed>()
function harnessData(): HarnessSeed {
  const base = getHarnessSeed()
  const v = variant()
  if (v !== 'all-unpriced' && v !== 'no-activity') return base
  const hit = variantSeeds.get(v)
  if (hit && hit.anchor === base.anchor) return hit
  const next =
    v === 'no-activity'
      ? { ...base, sessions: [] }
      : {
          ...base,
          sessions: base.sessions.map((x) => ({ ...x, unpriced_turns: x.turns, cost_usd: 0 })),
        }
  variantSeeds.set(v, next)
  return next
}

// Agents pages: mutable state derived from both seeds, rebuilt on reset (plan §8).
let agentsState: AgentsState | null = null
const getAgents = (): AgentsState => {
  agentsState ??= buildAgentsState(getSeed(), getHarnessSeed(), nowFn())
  // Uploads that finished since the last read change their agent rows (plans/feat-deploy.md §9).
  if (deployState?.pending.length) settleUploads(deployState, agentsState, nowFn())
  return agentsState
}
/** Finops endpoints see the harness turns too (aggregate.ts withHarnessTurns); memoised per seed pair. */
let finops: { seed: Seed; hs: HarnessSeed; merged: Seed } | null = null
const getFinopsSeed = (): Seed => {
  const base = getSeed()
  const hs = getHarnessSeed()
  if (!finops || finops.seed !== base || finops.hs !== hs)
    finops = { seed: base, hs, merged: withHarnessTurns(base, hs) }
  return finops.merged
}
// Deploy: builds and uploads derived from the agents state, rebuilt with it (plans/feat-deploy.md §9).
let deployState: DeployState | null = null
const getDeploy = (): DeployState => (deployState ??= buildDeployState(getAgents(), nowFn()))
/** The deploy mock's live state, for tests that move a build (the browser's clock moves the demo builds). */
export const deployMockState = () => ({
  state: getDeploy(),
  find: (id: string) => getDeploy().builds.find((b) => b.record.id === id),
  advance: (id: string, build: BuildStatus | null, upload: UploadPipelineStatus | null) => {
    const b = getDeploy().builds.find((x) => x.record.id === id)
    if (!b) throw new Error(`no mock build ${id}`)
    advanceBuild(b, nowFn(), build, upload)
  },
  /** The viewer's GitHub connection (tests complete the OAuth popup with this). */
  setGithubConnected: (connected: boolean) => {
    getDeploy().github.connected = connected
  },
  setAgentStatus: (agentId: string, status: string) => {
    const a = getAgents().agents.find((x) => x.id === agentId)
    if (!a) throw new Error(`no mock agent ${agentId}`)
    a.status = status
  },
})
// LLM router: configs and routing derived from the agents state, rebuilt with it (plan §6).
let routerState: RouterState | null = null
// MCP servers (plans/feat-mcp.md §8): built from the agents state, rebuilt with it.
let mcpState: McpState | null = null
const getMcp = (): McpState =>
  (mcpState ??= buildMcpState(
    getAgents(),
    ADMIN_ID,
    getSeed()
      .agents.filter((a) => !a.deleted)
      .map((a) => a.id),
    nowFn(),
  ))
/** The MCP mock's live state, for tests. */
export const mcpMockState = (): McpState => getMcp()
// Workflows (plans/feat-workflows.md §8): built from the agents state, rebuilt with it.
let workflowsState: WorkflowsState | null = null
let workflowsKey = ''
function getWorkflows(): WorkflowsState {
  const key = String(hasVariant('workflows-empty'))
  if (!workflowsState || workflowsKey !== key) {
    workflowsState = buildWorkflowsState(getAgents(), getSeed(), ADMIN_ID, nowFn(), {
      empty: hasVariant('workflows-empty'),
    })
    workflowsKey = key
  }
  return workflowsState
}
/** The workflows mock's live state, for tests. */
export const workflowsMockState = (): WorkflowsState => getWorkflows()
// Settings (plans/feat-settings.md): the singleton row, and the password the mock last set (null: any is current).
let settingsRow: Record<string, unknown> | null = null
// Secret values the mock was sent (the router state keeps names only); a seed secret reads as a fake key.
let secretValues = new Map<string, string>()
let mockPassword: string | null = null
// context_selection.rs: the viewer's chat-context preferences, at the migrations' defaults (0038, 0039).
const CHAT_CONTEXT_DEFAULTS = { strategy: 'pacms', level: 'medium' }
let chatContext = { ...CHAT_CONTEXT_DEFAULTS }
/** settings.rs `get_settings` with no row: its hard-coded defaults (not the env's, ST-1). */
const SETTINGS_DEFAULTS = {
  router_model: 'deepseek-v4-pro',
  default_provider: 'openai',
  max_flow_depth: 5,
  max_flow_fan_out: 20,
  max_flow_tokens: 100_000,
  flow_timeout_secs: 120,
  registry_url: null,
  catalog_tabs: null,
}
// R2 budgets (proposed R-L10): built from this month's seed spend, rebuilt with the router state.
let budgetState: BudgetMockStore | null = null
let budgetKey = ''
function getBudgets(): BudgetMockStore {
  const key = String(hasVariant('router-budgets-empty'))
  if (!budgetState || budgetKey !== key) {
    budgetState = buildBudgetState(getSeed(), getAgents().agents, ADMIN_ID, nowFn(), {
      empty: hasVariant('router-budgets-empty'),
    })
    budgetKey = key
  }
  return budgetState
}
/** Partial-live mode: the live user's id, learned from their `owner=` agent list (the mock has no session). */
let liveOwner: string | null = null
let routerKey = ''
function getRouter(): RouterState {
  const key = `${hasVariant('router-empty')}:${hasVariant('router-legacy')}`
  if (!routerState || routerKey !== key) {
    routerState = buildRouterState(getAgents().agents, nowFn(), {
      empty: hasVariant('router-empty'),
      legacy: hasVariant('router-legacy'),
    })
    routerKey = key
  }
  return routerState
}

/** The budgets mock's live state, for tests. */
export const budgetMockState = (): BudgetMockStore => getBudgets()

/** The router mock's live state, for tests that change it under the page (plan §8 ordering test). */
export const routerMockState = (): RouterState => getRouter()

/** The caller's onboarding row (nasiko-cloud-rs 41f776ae `onboarding.rs`). Tests start completed, so every page renders
 *  as before; the browser bootstrap starts a first-time user and keeps the row across reloads (browser.ts). */
export interface MockOnboarding {
  persona: Persona | null
  completed: boolean
}
const ONBOARDED: MockOnboarding = { persona: 'developer', completed: true }
let onboardingRow: MockOnboarding = ONBOARDED
let persistOnboarding: ((row: MockOnboarding) => void) | null = null

/** An edition's own mock state, dropped with the core's (EE: the SSO settings and SCIM tokens). */
const editionResets = new Set<() => void>()
export const onMockReset = (fn: () => void) => void editionResets.add(fn)

/** Drop agent, router and chat mutations (tests call this between cases). */
export function resetAgentsMock() {
  for (const fn of editionResets) fn()
  agentsState = null
  deployState = null
  routerState = null
  mcpState = null
  workflowsState = null
  budgetState = null
  settingsRow = null
  secretValues = new Map()
  mockPassword = null
  chatContext = { ...CHAT_CONTEXT_DEFAULTS }
  onboardingRow = ONBOARDED
  resetChatMock()
  // The chat knobs tests can turn (v1c DX3).
  fixedSuperuser = null
  fixedPageVariants = new Set()
}

/** Tests pin time and data; the browser uses the defaults. */
export function configureMocks(
  opts: {
    seed?: Seed
    now?: () => number
    loggedIn?: boolean
    /** Called whenever the mock session starts or ends (the browser bootstrap stores it). */
    persistLogin?: ((loggedIn: boolean) => void) | null
    variant?: MockVariant | null
    harnessSeed?: HarnessSeed
    /** A seed username; null returns to `?as=` / the default. */
    persona?: string | null
    /** Partial-mock live mode: the live user always maps onto the seed admin (N19). */
    lockPersona?: boolean
    /** The mock `me`'s superuser flag (v1c DX2); null returns to `?superuser=` / the default (true). */
    superuser?: boolean | null
    /** Chat page variants that combine, e.g. `['waiting', 'pending-flaky']` (v1c DX1). */
    chatVariants?: readonly (typeof CHAT_PAGE_VARIANTS)[number][]
    /** Router page variants that combine, e.g. `['router-legacy']` (plan §6). */
    routerVariants?: readonly (typeof ROUTER_PAGE_VARIANTS)[number][]
    /** An edition's session (EE: the seed admin whatever the persona); null returns to the viewer. */
    session?: (() => SessionUser) | null
    /** The caller's onboarding row, e.g. `{ persona: null, completed: false }` for a first-time user. */
    onboarding?: MockOnboarding
    /** Called whenever a PATCH changes the onboarding row (the browser bootstrap stores it). */
    persistOnboarding?: ((row: MockOnboarding) => void) | null
  } = {},
) {
  if (opts.onboarding) onboardingRow = opts.onboarding
  if (opts.persistOnboarding !== undefined) persistOnboarding = opts.persistOnboarding
  if (opts.session !== undefined) editionSession = opts.session
  if (opts.superuser !== undefined) fixedSuperuser = opts.superuser
  if (opts.chatVariants !== undefined)
    fixedPageVariants = new Set([
      ...opts.chatVariants,
      ...[...fixedPageVariants].filter((v) =>
        (ROUTER_PAGE_VARIANTS as readonly string[]).includes(v),
      ),
    ])
  if (opts.routerVariants !== undefined) {
    fixedPageVariants = new Set([
      ...[...fixedPageVariants].filter((v) =>
        (CHAT_PAGE_VARIANTS as readonly string[]).includes(v),
      ),
      ...opts.routerVariants,
    ])
    routerState = null
    budgetState = null
  }
  if (opts.seed) seed = opts.seed
  if (opts.now) nowFn = opts.now
  if (opts.loggedIn !== undefined) loggedIn = opts.loggedIn
  if (opts.persistLogin !== undefined) persistLogin = opts.persistLogin
  if (opts.variant !== undefined) fixedVariant = opts.variant
  if (opts.harnessSeed) {
    harnessSeed = opts.harnessSeed
    variantSeeds.clear()
  }
  if (opts.seed || opts.harnessSeed || opts.now) {
    agentsState = null
    deployState = null
    routerState = null
    mcpState = null
    workflowsState = null
    budgetState = null
  }
  if (opts.persona !== undefined) fixedPersona = opts.persona
  if (opts.lockPersona !== undefined) {
    personaLocked = opts.lockPersona
    liveOwner = null
  }
}

function envelope<T>(data: T) {
  return HttpResponse.json({ data, status_code: 200, message: 'ok' })
}

function params(request: Request): Record<string, string | null> {
  const url = new URL(request.url)
  return Object.fromEntries([...url.searchParams.entries()])
}

/** Run an aggregation, mapping MockHttpError to the server's plain-text error responses. */
function respond(fn: () => unknown) {
  if (!loggedIn) return unauthorized()
  try {
    return envelope(fn())
  } catch (err) {
    if (err instanceof MockHttpError)
      return new HttpResponse(err.message, {
        status: err.status,
        headers: { 'Content-Type': 'text/plain' },
      })
    return new HttpResponse('internal error', {
      status: 500,
      headers: { 'Content-Type': 'text/plain' },
    })
  }
}

type S = components['schemas']
const FINOPS = '/api/observability/finops'
const OBS = '/api/observability'

/** auth/middleware.rs `AuthRejection::into_response`: every auth 401 has this JSON envelope. */
const authRejection = (message: string) =>
  HttpResponse.json({ data: null, status_code: 401, message }, { status: 401 })
const text = (body: string, status: number) =>
  new HttpResponse(body, { status, headers: { 'Content-Type': 'text/plain' } })
/** No cookie: `require_auth` → `validate_bearer`'s reason. */
const unauthorized = () => authRejection('missing or invalid token')

/** Session/trace/span answers are `{data: …}` (no status_code/message) and plain-text errors. */
function traceStore(fn: () => Response): Response {
  if (!loggedIn) return unauthorized()
  const v = variant()
  if (v === 'trace-503')
    return text('observability backend not configured (set TEMPO_URL and LOKI_URL)', 503)
  if (v === 'trace-500') return text('internal error', 500)
  return fn()
}

function sseStream(lines: string[], closeMessage: string): ReadableStream<Uint8Array> {
  const enc = new TextEncoder()
  return new ReadableStream({
    start(controller) {
      for (const l of lines) controller.enqueue(enc.encode(`data: ${l}\n\n`))
      controller.enqueue(enc.encode(`event: close\ndata: ${closeMessage}\n\n`))
      controller.close()
    },
  })
}

export const handlerGroups: Record<Mockable, HttpHandler[]> = {
  // nasiko-cloud-rs 41f776ae `onboarding.rs`: bare JSON; a persona outside the enum is Axum's 422 Json rejection.
  onboarding: [
    http.get('/api/me/onboarding', () => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'onboarding-absent') return new HttpResponse(null, { status: 404 })
      const row = variant() === 'onboarding-done' ? ONBOARDED : onboardingRow
      return HttpResponse.json({ is_first_time_user: !row.completed, persona: row.persona })
    }),
    http.patch('/api/me/onboarding', async ({ request }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'onboarding-absent') return new HttpResponse(null, { status: 404 })
      const body = (await request.json().catch(() => null)) as { persona?: unknown } | null
      const persona = body?.persona
      if (typeof persona !== 'string' || !(PERSONAS as readonly string[]).includes(persona))
        return text(
          `Failed to deserialize the JSON body into the target type: persona: unknown variant \`${String(persona)}\`, expected one of ${PERSONAS.map((p) => `\`${p}\``).join(', ')}`,
          422,
        )
      onboardingRow = { persona: persona as Persona, completed: true }
      persistOnboarding?.(onboardingRow)
      return HttpResponse.json({ is_first_time_user: false, persona })
    }),
  ],
  auth: [
    http.get('/api/me', () => {
      if (!loggedIn) return unauthorized()
      // The session is the viewer: OSS gates /api/users/me behind an admin, so the Harnesses page falls back to these
      // claims for anyone else. An edition's mocks may keep another session (EE: the seed admin, whatever the persona).
      const v = sessionUser()
      return HttpResponse.json({ sub: v.id, username: v.username, is_superuser: v.is_superuser })
    }),
    http.post('/api/auth/login', async ({ request }) => {
      const body = (await request.json().catch(() => ({}))) as {
        username?: string
        password?: string
      }
      if (!body.username || !body.password)
        return HttpResponse.json({ error: 'invalid credentials' }, { status: 401 })
      setLoggedIn(true)
      // auth/login.rs: `user_id` is the id the JWT carries as `sub`, so it equals what /api/me reports next.
      const v = sessionUser()
      return HttpResponse.json({
        token: 'mock',
        user_id: v.id,
        username: v.username,
        is_superuser: v.is_superuser,
        expires_in: 604800,
      })
    }),
    // Logout sits behind `require_auth` (lib.rs), so its 401s are the AuthRejection JSON envelope
    // (auth/middleware.rs @ cb3aaf0c): with no session, "missing or invalid token".
    // `?mock=logout-unavailable` is the fail-closed 401 when the server can't check the token
    // (database down): nothing is revoked (docs/designs/openruntime-app-shell-recommendations.md).
    http.post('/api/auth/logout', () => {
      if (variant() === 'logout-unavailable') return authRejection('token validation unavailable')
      if (!loggedIn) return authRejection('missing or invalid token')
      setLoggedIn(false)
      return new HttpResponse(null, { status: 204 })
    }),
    // auth/login.rs `change_password` (43833316): policy 400s, then 403 on a wrong current password (never 401);
    // 200 re-issues the session. The mock's login takes any password, so any current one works until a change.
    http.post('/api/auth/change-password', async ({ request }) => {
      if (!loggedIn) return unauthorized()
      const body = (await request.json().catch(() => ({}))) as {
        current_password?: string
        new_password?: string
      }
      const next = body.new_password ?? ''
      const coded = (status: number, code: string, error: string) =>
        HttpResponse.json({ error, code }, { status })
      const failed = passwordProblem(next)
      if (failed) return coded(400, PASSWORD_CODES[failed], 'password must meet the policy')
      if (next === body.current_password)
        return coded(400, 'password_unchanged', 'new password must differ from the current one')
      if (mockPassword !== null && body.current_password !== mockPassword)
        return coded(403, 'current_password_incorrect', 'current password is incorrect')
      mockPassword = next
      return HttpResponse.json({ data: { token: 'mock', expires_in: 604800 } })
    }),
  ],
  agents: [
    // catalog/routes.rs list: bare array, owner must be a UUID (else a plain-text 400), limit 1–100.
    http.get('/api/agents', ({ request }) => {
      if (!loggedIn) return unauthorized()
      const p = params(request)
      if (p.owner && !UUID_RE.test(p.owner))
        return text('Failed to deserialize query string: owner: UUID parsing failed', 400)
      const limit = Math.min(100, Math.max(1, Number.parseInt(p.limit ?? '', 10) || 50))
      const offset = Math.max(0, Number.parseInt(p.offset ?? '', 10) || 0)
      const now = nowFn()
      // Partial-live mode: the live user is the seed admin (N19), so their `owner=` list is the admin's seed agents,
      // answered with their real id as owner (the router preview's "Your agents" and its owner checks; the real
      // `me.sub` never matches a seed owner).
      const asLive = personaLocked && !!p.owner
      if (asLive) liveOwner = p.owner!
      const owner = asLive ? ADMIN_ID : p.owner
      const rows = getAgents()
        .agents.filter((a) => !a.deleted && (!owner || a.owner_id === owner))
        .sort((a, b) => b.created_at.localeCompare(a.created_at))
      // `?mock=no-agents` (v1c): nothing is running, so Chat has no agent to offer.
      const stopped = hasVariant('no-agents')
      return HttpResponse.json(
        rows.slice(offset, offset + limit).map((a) => ({
          ...listRow(a, now),
          ...(stopped ? { status: 'stopped' } : {}),
          ...(asLive ? { owner_id: p.owner } : {}),
        })),
      )
    }),
    http.get('/api/agents/:id', ({ params: prm }) => {
      // Static routes beside `/{id}` win on the server (axum): leave them to their own handlers (deploy group).
      if (AGENTS_STATIC.has(String(prm.id))) return undefined
      if (!loggedIn) return unauthorized()
      const a = findAgent(String(prm.id))
      // get_one: a bare 404 with an empty body for unknown and deleted agents.
      if (!a) return new HttpResponse(null, { status: 404 })
      const body = detailBody(a, canManage(a))
      // Partial-live mode: the seed admin's agents are the live user's (as in the `owner=` list above).
      return envelope(
        personaLocked && liveOwner && a.owner_id === ADMIN_ID
          ? { ...body, owner_id: liveOwner }
          : body,
      )
    }),
    http.put('/api/agents/:id', async ({ params: prm, request }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      const body = (await request.json().catch(() => ({}))) as {
        display_name?: string
        description?: string
        metadata?: Record<string, unknown>
        compress_enabled?: boolean
        minimal_code_enabled?: boolean
      }
      // COALESCE per field (catalog/routes.rs update); `metadata` replaces the whole column.
      if (typeof body.display_name === 'string') a.display_name = body.display_name
      if (typeof body.description === 'string') a.description = body.description
      if (body.metadata && typeof body.metadata === 'object') a.metadata = body.metadata
      if (typeof body.compress_enabled === 'boolean') a.compress_enabled = body.compress_enabled
      if (typeof body.minimal_code_enabled === 'boolean')
        a.minimal_code_enabled = body.minimal_code_enabled
      a.updated_at = new Date(nowFn()).toISOString()
      return HttpResponse.json(listRow(a, nowFn()))
    }),
    http.delete('/api/agents/:id', ({ params: prm }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      a.deleted = true
      // The server destroys by UUID and by name, and Docker destroy is idempotent: both count.
      return HttpResponse.json({
        deleted: true,
        agent_id: a.id,
        containers_stopped: 2,
        runtime_errors: [],
      })
    }),
    http.get('/api/agents/:id/deployment', ({ params: prm }) => {
      if (!loggedIn) return unauthorized()
      const a = findAgent(String(prm.id))
      if (!a) return new HttpResponse(null, { status: 404 })
      const s = liveStatus(a, nowFn())
      if (!a.deployment || s === 'stopped' || s === 'registered')
        return new HttpResponse(null, { status: 404 })
      return HttpResponse.json(a.deployment)
    }),
    http.get('/api/agents/:id/versions', ({ params: prm }) => {
      if (!loggedIn) return unauthorized()
      const a = findAgent(String(prm.id))
      if (!a) return new HttpResponse(null, { status: 404 })
      liveStatus(a, nowFn())
      return HttpResponse.json({
        data: a.versions,
        status_code: 200,
        message: 'version history retrieved successfully',
      })
    }),
    http.post('/api/agents/:id/rollback', async ({ params: prm, request }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      const body = (await request.json().catch(() => ({}))) as { target_version?: string }
      const target = a.versions.find((v) => v.version === body.target_version)
      if (!target) return text(`version ${body.target_version ?? ''} not found`, 404)
      // update.rs:791-797. No in-progress check server-side: a second roll back just queues (the worker serializes).
      if (!target.can_rollback)
        return text(`version ${target.version} is not rollback-eligible`, 400)
      const build = `5eed0006-0000-4000-8000-${String(nowFn() % 1e12).padStart(12, '0')}`
      a.rollback = { at: nowFn(), to: target.version, build }
      return HttpResponse.json(
        {
          agent_id: a.id,
          build_id: build,
          rolled_back_from: a.version,
          rolled_back_to: target.version,
          status: 'queued',
        },
        { status: 202 },
      )
    }),
    // admin/routes.rs: stop/start answer 200 with an empty body, restart answers the redeploy's
    // ContainerStatus as JSON (568-583); start on a missing container → 500 "internal error".
    http.post('/api/containers/:id/:action', ({ params: prm }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      const action = String(prm.action)
      const s = liveStatus(a, nowFn())
      if (action === 'start' || action === 'restart') {
        if (!a.deployment && s === 'registered') return text('internal error', 500)
        a.status = 'running'
      } else if (action === 'stop') {
        a.status = 'stopped'
      } else {
        return new HttpResponse(null, { status: 404 })
      }
      a.updated_at = new Date(nowFn()).toISOString()
      if (action === 'restart')
        return HttpResponse.json({
          id: a.id,
          state: 'running',
          endpoint: `http://localhost:${9100 + getAgents().agents.indexOf(a)}`,
        })
      return new HttpResponse(null, { status: 200 })
    }),
    http.get('/api/agents/:id/secrets', ({ params: prm }) => {
      const a = managed(String(prm.id))
      return a instanceof Response ? a : HttpResponse.json(a.secrets)
    }),
    http.post('/api/agents/:id/secrets', async ({ params: prm, request }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      const body = (await request.json().catch(() => ({}))) as { name?: string; value?: string }
      if (!body.name || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(body.name))
        return text('invalid secret name', 422)
      if (!body.value) return text('value must not be empty', 422)
      a.secrets = [
        ...a.secrets.filter((x) => x.name !== body.name),
        { name: body.name, updated_at: null },
      ]
      return new HttpResponse(null, { status: 201 })
    }),
    http.delete('/api/agents/:id/secrets/:name', ({ params: prm }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      a.secrets = a.secrets.filter((x) => x.name !== String(prm.name))
      return new HttpResponse(null, { status: 204 })
    }),
    http.get('/api/agents/:id/grants', ({ params: prm }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      return HttpResponse.json({
        agent_id: a.id,
        is_public: a.is_public,
        user_grants: a.userGrants,
        agent_acl: a.agentAcl,
      })
    }),
    // EE-only paths (visibility, who can reach the agent, path-shaped grant writes): on OSS they reach the agent
    // proxy, which has no running agent here (recorded 503 at ea233d20).
    ...(
      [
        ['get', '/api/agents/:id/visibility'],
        ['get', '/api/agents/:id/users'],
        ['post', '/api/agents/:id/grants/users/:uid'],
        ['post', '/api/agents/:id/grants/agents/:tid'],
      ] as const
    ).map(([method, path]) => http[method](path, () => new HttpResponse(null, { status: 503 }))),
    http.post('/api/agents/:id/grants/public', ({ params: prm }) =>
      grant(String(prm.id), (a) => {
        a.is_public = true
      }),
    ),
    http.delete('/api/agents/:id/grants/public', ({ params: prm }) =>
      grant(String(prm.id), (a) => {
        a.is_public = false
      }),
    ),
    http.get('/api/agents/:id/grants/users', ({ params: prm }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      const users = getAgents().users
      // LEFT JOIN users (agents/grants.rs:241): a deleted user's grant has username null.
      return HttpResponse.json(
        a.userGrants.map((uid) => ({
          user_id: uid,
          username: users.find((u) => u.id === uid)?.username ?? null,
        })),
      )
    }),
    http.post('/api/agents/:id/grants/users', async ({ params: prm, request }) => {
      const body = (await request.json().catch(() => ({}))) as { user_id?: string }
      if (!body.user_id || !getAgents().users.some((u) => u.id === body.user_id))
        return text('user not found', 404)
      return grant(
        String(prm.id),
        (a) => {
          if (!a.userGrants.includes(body.user_id!)) a.userGrants = [...a.userGrants, body.user_id!]
        },
        201,
      )
    }),
    http.delete('/api/agents/:id/grants/users/:uid', ({ params: prm }) =>
      grant(String(prm.id), (a) => {
        a.userGrants = a.userGrants.filter((u) => u !== String(prm.uid))
      }),
    ),
    http.get('/api/agents/:id/grants/agents', ({ params: prm }) => {
      const a = managed(String(prm.id))
      if (a instanceof Response) return a
      const all = getAgents().agents
      // LEFT JOIN agents: a deleted target has target_name null.
      return HttpResponse.json(
        a.agentAcl.map((tid) => ({
          target_agent_id: tid,
          target_name: all.find((x) => x.id === tid && !x.deleted)?.name ?? null,
        })),
      )
    }),
    http.post('/api/agents/:id/grants/agents', async ({ params: prm, request }) => {
      const body = (await request.json().catch(() => ({}))) as { agent_id?: string }
      if (!body.agent_id || !findAgent(body.agent_id)) return text('agent not found', 404)
      return grant(
        String(prm.id),
        (a) => {
          if (!a.agentAcl.includes(body.agent_id!)) a.agentAcl = [...a.agentAcl, body.agent_id!]
        },
        201,
      )
    }),
    http.delete('/api/agents/:id/grants/agents/:tid', ({ params: prm }) =>
      grant(String(prm.id), (a) => {
        a.agentAcl = a.agentAcl.filter((t) => t !== String(prm.tid))
      }),
    ),
    // The agent proxy: only a running agent answers its own card.
    http.get('/api/agents/:id/.well-known/agent-card.json', ({ params: prm }) => {
      if (!loggedIn) return unauthorized()
      const a = findAgent(String(prm.id))
      if (!a) return new HttpResponse(null, { status: 404 })
      // The proxy has no upstream for an agent that isn't deployed: an empty 503 (recorded at ea233d20).
      if (liveStatus(a, nowFn()) !== 'running') return new HttpResponse(null, { status: 503 })
      return HttpResponse.json({
        name: a.display_name,
        description: a.description,
        version: a.version,
        protocolVersion: '0.3.0',
        capabilities: a.capabilities,
        skills: a.skills,
        defaultInputModes: ['text'],
        defaultOutputModes: ['text'],
      })
    }),
    http.get(`${OBS}/agent/:ref/stats`, ({ params: prm }) => {
      if (!loggedIn) return unauthorized()
      const a = findAgent(String(prm.ref))
      if (!a) return text(`Agent '${String(prm.ref)}' not found`, 404)
      const i = getAgents().agents.indexOf(a)
      // observability/service.rs get_agent_stats at ea233d20: a Phoenix-style project with {cost} buckets.
      const cost = a.harness ? 0 : 0.35 + i * 0.12
      const bucket = (c: number) => ({ cost: c })
      return envelope({
        project: {
          id: a.name,
          trace_count: a.harness ? 0 : 40 + i * 7,
          cost_summary: {
            total: bucket(cost),
            prompt: bucket(cost * 0.4),
            completion: bucket(cost * 0.6),
            cache_read: bucket(0),
            cache_creation: bucket(0),
          },
          latency_ms_p50: 820 + i * 10,
          latency_ms_p99: 4100 + i * 40,
          span_annotation_names: [],
          document_evaluation_names: [],
        },
      })
    }),
    http.get(`${OBS}/agent/:ref/resources`, ({ params: prm }) => {
      if (!loggedIn) return unauthorized()
      const a = findAgent(String(prm.ref))
      if (!a) return text(`Agent '${String(prm.ref)}' not found`, 404)
      const running = liveStatus(a, nowFn()) === 'running'
      // resources.rs at ea233d20 answers a bare {data} (no message or status_code).
      return HttpResponse.json({
        data: {
          agent_id: a.id,
          agent_name: a.name,
          usage: running
            ? { cpu_percent: 3.2, memory_usage_bytes: 182_000_000, memory_limit_bytes: 536_870_912 }
            : null,
          collected_at: new Date(nowFn()).toISOString(),
        },
      })
    }),
    // Only the agent pages' calls (they always send limit/level); bare calls keep the observability mock.
    http.get(`${OBS}/agents/:ref/logs`, ({ params: prm, request }) => {
      if (!loggedIn) return unauthorized()
      if (
        !new URL(request.url).searchParams.has('limit') &&
        !new URL(request.url).searchParams.has('level')
      )
        return undefined
      const a = findAgent(String(prm.ref))
      if (!a) return text(`Agent '${String(prm.ref)}' not found`, 404)
      const p = params(request)
      return HttpResponse.json(logLines(a, nowFn(), p.level, Math.min(500, Number(p.limit) || 200)))
    }),
    http.get('/api/search/users', ({ request }) => {
      if (!loggedIn) return unauthorized()
      const q = (params(request).q ?? '').trim().toLowerCase()
      if (q.length < 2) return text('q must be at least 2 characters', 400)
      const hits = getAgents()
        .users.filter(
          (u) => u.username.toLowerCase().includes(q) || u.display_name.toLowerCase().includes(q),
        )
        .slice(0, 50)
      // search.rs at ea233d20: each hit carries email, role and a relevance score.
      return HttpResponse.json({
        data: hits.map((u) => ({
          id: u.id,
          username: u.username,
          display_name: u.display_name,
          email: u.email,
          role: u.role,
          score: u.username.toLowerCase().startsWith(q) ? 270 : 90,
        })),
        query: q,
        total_matches: hits.length,
        showing: hits.length,
      })
    }),
    http.get('/api/users', () => {
      if (!loggedIn) return unauthorized()
      const users = getAgents().users
      // users.rs list at ea233d20: the full user row.
      return HttpResponse.json({
        data: users.map((u) => ({
          id: u.id,
          username: u.username,
          email: u.email,
          display_name: u.display_name,
          is_superuser: u.is_superuser,
          is_active: u.is_active,
          role: u.role,
          department_id: null,
          team_id: null,
          created_at: getHarnessSeed().anchor,
          last_login: u.id === ADMIN_ID ? new Date(nowFn()).toISOString() : null,
        })),
        total: users.length,
      })
    }),
  ],
  dashboard: [
    http.get(`${FINOPS}/dashboard`, ({ request }) => {
      const p = params(request)
      // `my_agent=true` (Harnesses live fallback, handler.rs:497) answers from the harness seed for
      // the viewer's own agents; TokenOps never sends it, so its numbers are untouched.
      if (p.my_agent === 'true') return respond(() => myAgentDashboard(p))
      return respond(() => dashboard(getFinopsSeed(), p, nowFn()))
    }),
  ],
  'spend-timeseries': [
    http.get(`${FINOPS}/spend-timeseries`, ({ request }) =>
      respond(() => spendTimeseries(getFinopsSeed(), params(request), nowFn())),
    ),
  ],
  'spend-calendar': [
    http.get(`${FINOPS}/spend-calendar/day`, ({ request }) => {
      const p = params(request)
      return respond(() => dayDrilldown(getFinopsSeed(), p.date, p))
    }),
    http.get(`${FINOPS}/spend-calendar`, ({ request }) => {
      const p = params(request)
      return respond(() => spendCalendar(getFinopsSeed(), p.month, p, nowFn()))
    }),
  ],
  providers: [
    // llm_router/providers.rs list_providers: served ∪ priced models, grouped by provider; a custom provider's
    // group carries its id and display name. Unpriced models have null prices and `pricing_available: false`.
    http.get('/api/llm-router/providers', () => {
      if (hasVariant('router-catalog-fail')) return text('internal error', 500)
      const byProvider = new Map<string, S['ModelEntry'][]>()
      for (const m of SEED_MODELS) {
        const priced = m.inPerM > 0 || m.outPerM > 0
        // ModelEntry (llm_router/providers.rs, ea233d20) carries cache prices, notes and the pricing row's validity.
        const row = (notes: string, daysAgo: number): S['ModelEntry'] => ({
          model: m.model,
          input_price_per_1m: priced ? m.inPerM : null,
          output_price_per_1m: priced ? m.outPerM : null,
          cache_creation_price_per_1m: null,
          cache_read_price_per_1m: null,
          currency: priced ? 'USD' : null,
          notes: priced ? notes : null,
          effective_from: priced ? new Date(nowFn() - daysAgo * 86_400_000).toISOString() : null,
          effective_until: null,
          pricing_available: priced,
        })
        const rows = [row(m.model, 1)]
        // The server returns two open pricing rows for some models (a boot-seed row and a named one); mirror one.
        if (m.model === 'gemini-2.0-flash') rows.push(row('boot seed (static list)', 12))
        byProvider.set(m.provider, [...(byProvider.get(m.provider) ?? []), ...rows])
      }
      const custom = new Map(listCustom(getRouter()).map((c) => [c.label, c]))
      return HttpResponse.json({
        data: [...byProvider.entries()].map(([provider, models]) => {
          const c = custom.get(provider)
          return c
            ? { provider, provider_id: c.id, display_name: c.display_name, models }
            : { provider, models }
        }),
        status_code: 200,
        message: 'ok',
      })
    }),
  ],
  'top-traces': [
    http.get(`${FINOPS}/top-traces`, ({ request }) =>
      respond(() => topTraces(getFinopsSeed(), params(request), nowFn())),
    ),
  ],
  savings: [http.get(`${FINOPS}/savings`, () => respond(() => savings()))],
  observability: [
    // `owner=` lists are the Harnesses live fallback's (harnesses group): pass them through, so
    // VITE_NASIKO_MOCK=observability alone never answers them from the harness seed.
    http.get('/api/agents', ({ request }) => {
      if (!loggedIn) return unauthorized()
      const p = params(request)
      if (p.owner) return undefined
      return HttpResponse.json(agentsList(getSeed(), p))
    }),
    http.get(`${OBS}/session/list`, ({ request }) => {
      if (!loggedIn) return unauthorized()
      const p = params(request)
      const v = variant()
      if (v === 'scan-fail' && Number(p.offset ?? 0) >= 100) return text('internal error', 500)
      const body = sessionList(getSeed(), p, nowFn(), v === 'tempo-down')
      if (v === 'empty')
        body.data = {
          ...body.data,
          sessions: [],
          successful_agents: 0,
          pagination: { end_cursor: null, has_next_page: false },
        }
      return HttpResponse.json(body)
    }),
    http.get(`${OBS}/session/:sessionId`, ({ params: p }) =>
      traceStore(() => {
        const id = String(p.sessionId)
        const data = observabilityData(getSeed())
        // A chat from the chat mock is a session too (live, a chat's id IS its session id): answer
        // with the seeded showcase session, re-keyed, so View session / Open full trace resolve.
        const chat = chatMockRows().find((r) => r.session_id === id && r.user_id === ADMIN_ID)
        const showcase = chat ? data.sessions.find((x) => x.showcase) : undefined
        const s =
          data.byId.get(id) ??
          (showcase ? { ...showcase, session_id: id, title: chat!.title } : undefined)
        // The server answers 404 for unknown AND inaccessible sessions (reveal no existence).
        return s
          ? HttpResponse.json({ data: { session: sessionDetail(getSeed(), s) } })
          : text(`session '${id}' not found`, 404)
      }),
    ),
    http.get(`${OBS}/trace/:traceId`, ({ params: p }) =>
      traceStore(() => {
        const t = traceDetail(getSeed(), String(p.traceId))
        return t
          ? HttpResponse.json({ data: { trace: t } })
          : text(`trace '${String(p.traceId)}' not found`, 404)
      }),
    ),
    http.get(`${OBS}/span/:traceId/:spanId`, ({ params: p }) =>
      traceStore(() => {
        const s = spanDetail(getSeed(), String(p.traceId), String(p.spanId))
        return s
          ? HttpResponse.json({ data: { span: s } })
          : text(`span '${String(p.spanId)}' in trace '${String(p.traceId)}' not found`, 404)
      }),
    ),
    http.get(`${OBS}/agents/:agentRef/logs/stream`, ({ params: p }) => {
      if (!loggedIn) return unauthorized()
      const lines = agentLogs(getSeed(), String(p.agentRef), nowFn())
      if (!lines) return text(`Agent '${String(p.agentRef)}' not found`, 404)
      return new HttpResponse(
        sseStream(
          lines.map((l) => JSON.stringify(l)),
          'stream timeout — reconnect to continue',
        ),
        { headers: { 'Content-Type': 'text/event-stream' } },
      )
    }),
    http.get(`${OBS}/agents/:agentRef/logs`, ({ params: p }) => {
      if (!loggedIn) return unauthorized()
      const lines = agentLogs(getSeed(), String(p.agentRef), nowFn())
      return lines ? HttpResponse.json(lines) : text(`Agent '${String(p.agentRef)}' not found`, 404)
    }),
  ],
  harnesses: [
    http.get('/api/agents', ({ request }) => {
      if (!loggedIn) return unauthorized()
      const p = params(request)
      // `owner=<uuid>` (catalog/routes.rs): the harness seed's agents for that owner, tags/metadata like
      // the CLI's; any other list is the observability group's. Paged like catalog/routes.rs.
      if (!p.owner) return undefined
      const limit = Math.min(100, Math.max(1, Number(p.limit) || 100))
      const offset = Math.max(0, Number(p.offset) || 0)
      return HttpResponse.json(harnessAgentsList(p.owner).slice(offset, offset + limit))
    }),
    http.get('/api/agents/:id', ({ params: p }) => {
      if (AGENTS_STATIC.has(String(p.id))) return undefined
      if (!loggedIn) return unauthorized()
      const agent = getHarnessSeed().agents.find((a) => a.id === String(p.id) && !a.deleted)
      // Partial-live mode: anything but a seed harness agent is the real server's.
      if (!agent && personaLocked) return undefined
      // catalog/routes.rs get_one: a bare 404 with an empty body.
      if (!agent) return new HttpResponse(null, { status: 404 })
      return envelope(harnessAgentDetail(agent))
    }),
    // GET /api/chat/sessions?agent_id=<harness>: the harness page's per-agent lists (own-only,
    // keyset-paged, as chat/routes.rs). Every other list belongs to the `chat` group (or, in
    // partial-live mode without it, the real server).
    http.get('/api/chat/sessions', ({ request }) => {
      if (!loggedIn) return unauthorized()
      const p = params(request)
      if (!getHarnessSeed().agents.some((a) => a.id === p.agent_id)) return undefined
      const limit = Math.min(100, Math.max(1, Number(p.limit) || 50))
      return HttpResponse.json(chatSessions(limit, p.agent_id ?? undefined))
    }),
    http.get(`${OBS}/coding-agents/usage`, ({ request }) => {
      if (!loggedIn) return unauthorized()
      const v = variant()
      // The endpoint doesn't exist on a real server yet: an uncoded 404 means "absent" (plan §4).
      // Body as nasiko-server's api_not_found fallback (oss/server/src/lib.rs) sends it.
      if (v === 'usage-404' || v === 'prev-fail') {
        return HttpResponse.json(
          {
            data: null,
            status_code: 404,
            message: `no API route matches ${new URL(request.url).pathname}`,
          },
          { status: 404 },
        )
      }
      if (v === 'usage-500')
        return HttpResponse.json(
          { error: 'usage rollup failed', code: 'internal' },
          { status: 500 },
        )
      return usageResponse(() => usage(harnessData(), params(request), viewer(), nowFn()))
    }),
    // OSS: the users router needs an admin (auth/rbac.rs require_user_manager at ea233d20), so anyone else gets a 403
    // and the Harnesses page reads the /api/me claims instead.
    http.get('/api/users/me', () => {
      if (!loggedIn) return unauthorized()
      const v = viewer()
      if (!v.is_superuser) return text('requires admin role', 403)
      const hs = getHarnessSeed()
      // users.rs at ea233d20 includes the EE org placement, null on OSS.
      return HttpResponse.json({
        id: v.id,
        username: v.username,
        email: v.email,
        display_name: v.display_name,
        is_superuser: v.is_superuser,
        is_active: v.is_active,
        role: v.role,
        created_at: hs.anchor,
        last_login: hs.anchor,
        department_id: null,
        team_id: null,
      })
    }),
  ],
  router: [
    // llm_configs.rs: the caller's own configs, `{data, status_code, message}` on success, plain-text errors.
    http.get('/api/llm-configs', () => routed(() => envelope(listConfigs(getRouter(), ADMIN_ID)))),
    http.post('/api/llm-configs', async ({ request }) => {
      const body = await request.json().catch(() => ({}))
      return routed(() =>
        HttpResponse.json(
          {
            data: createConfig(getRouter(), ADMIN_ID, body as never, nowFn()),
            status_code: 201,
            message: 'LLM config created successfully',
          },
          { status: 201 },
        ),
      )
    }),
    http.patch('/api/llm-configs/:id', async ({ params: prm, request }) => {
      const body = await request.json().catch(() => ({}))
      return routed(() =>
        envelope(updateConfig(getRouter(), String(prm.id), ADMIN_ID, body as never, nowFn())),
      )
    }),
    http.delete('/api/llm-configs/:id', ({ params: prm }) =>
      routed(() => {
        const liveAgents = new Set(
          getAgents()
            .agents.filter((a) => !a.deleted)
            .map((a) => a.id),
        )
        deleteConfig(
          getRouter(),
          String(prm.id),
          ADMIN_ID,
          liveAgents,
          hasVariant('router-409') ? 3 : undefined,
        )
        return HttpResponse.json({
          data: null,
          status_code: 200,
          message: 'LLM config deleted successfully',
        })
      }),
    ),
    http.post('/api/llm-configs/:id/default', ({ params: prm }) =>
      routed(() => envelope(setDefault(getRouter(), String(prm.id), ADMIN_ID, true))),
    ),
    http.delete('/api/llm-configs/:id/default', ({ params: prm }) =>
      routed(() => envelope(setDefault(getRouter(), String(prm.id), ADMIN_ID, false))),
    ),
    // agents/llm_config.rs: owner or superuser; 404 "agent not found" (plain text) for unknown agents.
    http.get('/api/agents/:id/llm-config', ({ params: prm }) =>
      routed(() => envelope(resolveRouting(getRouter(), routingAgent(String(prm.id))))),
    ),
    http.patch('/api/agents/:id/llm-config', async ({ params: prm, request }) => {
      const body = (await request.json().catch(() => ({}))) as Record<string, unknown>
      return routed(() =>
        envelope(
          patchRouting(getRouter(), routingAgent(String(prm.id)), body, {
            failPin:
              hasVariant('router-repin-fail') &&
              !('llm_config_id' in body) &&
              'pinned_model' in body,
          }),
        ),
      )
    }),
    // secrets/routes.rs list_secrets: names and timestamps only.
    http.get('/api/secrets', () =>
      routed(() => {
        if (hasVariant('router-secrets-fail')) throw new MockHttpError(500, 'internal error')
        return envelope(hasVariant('router-no-secrets') ? [] : listSecrets(getRouter(), ADMIN_ID))
      }),
    ),
    // `create_secret`: 422 plain text on a bad name, else an upsert (201, the entry in the envelope).
    http.post('/api/secrets', async ({ request }) => {
      const body = (await request.json().catch(() => ({}))) as { name?: string; value?: string }
      return routed(() => {
        const name = body.name ?? ''
        if (secretNameProblem(name))
          throw new MockHttpError(422, 'secret name may only contain [A-Z0-9_]')
        const entry = upsertSecret(getRouter(), ADMIN_ID, name, nowFn())
        secretValues.set(name, body.value ?? '')
        return HttpResponse.json(
          { data: entry, status_code: 201, message: 'Secret created successfully' },
          { status: 201 },
        )
      })
    }),
    // `get_secret`: the caller's own value, decrypted; an empty 404 when there's no such name.
    http.get('/api/secrets/:name', ({ params: prm }) =>
      routed(() => {
        const name = String(prm.name)
        if (!listSecrets(getRouter(), ADMIN_ID).some((x) => x.name === name))
          return new HttpResponse(null, { status: 404 })
        const value = secretValues.get(name) ?? `sk-mock-${name.toLowerCase().replace(/_/g, '-')}`
        return envelope({ name, value })
      }),
    ),
    // `update_secret` / `delete_secret`: `data: null` on success, an empty 404 when the caller has no such name.
    http.put('/api/secrets/:name', async ({ params: prm, request }) => {
      const body = (await request.json().catch(() => ({}))) as { value?: string }
      return routed(() => {
        if (!touchSecret(getRouter(), ADMIN_ID, String(prm.name), nowFn()))
          return new HttpResponse(null, { status: 404 })
        secretValues.set(String(prm.name), body.value ?? '')
        return HttpResponse.json({
          data: null,
          status_code: 200,
          message: 'Secret updated successfully',
        })
      })
    }),
    http.delete('/api/secrets/:name', ({ params: prm }) =>
      routed(() => {
        const name = String(prm.name)
        if (!deleteSecret(getRouter(), ADMIN_ID, name))
          return new HttpResponse(null, { status: 404 })
        secretValues.delete(name)
        return HttpResponse.json({
          data: null,
          status_code: 200,
          message: 'Secret deleted successfully',
        })
      }),
    ),
    http.get('/api/model-registry', () => routed(() => envelope(getRouter().registry))),
    // R2 budgets: the proposed R-L10 contract (docs/designs/openruntime-llm-router-recommendations.md). No server yet.
    http.get('/api/budgets', () =>
      routed(() => {
        if (hasVariant('router-budgets-fail')) throw new MockHttpError(500, 'internal error')
        return envelope(listBudgets(getBudgets(), ADMIN_ID))
      }),
    ),
    http.get('/api/budgets/status', () =>
      routed(() => {
        if (hasVariant('router-budgets-fail')) throw new MockHttpError(500, 'internal error')
        return HttpResponse.json(
          budgetStatus(getBudgets(), getSeed(), getAgents().agents, ADMIN_ID, nowFn()),
        )
      }),
    ),
    http.get('/api/budgets/alerts', () =>
      routed(() => {
        if (hasVariant('router-budgets-fail')) throw new MockHttpError(500, 'internal error')
        return envelope(
          budgetAlerts(getBudgets(), getSeed(), getAgents().agents, ADMIN_ID, nowFn()),
        )
      }),
    ),
    http.post('/api/budgets', async ({ request }) => {
      const body = await request.json().catch(() => ({}))
      const agentOwner = (id: string) =>
        getAgents().agents.find((a) => !a.deleted && a.id === id)?.owner_id
      return routed(() =>
        HttpResponse.json(
          {
            data: createBudget(
              getBudgets(),
              ADMIN_ID,
              body as never,
              agentOwner,
              superuser(),
              nowFn(),
            ),
            status_code: 201,
            message: 'Budget created',
          },
          { status: 201 },
        ),
      )
    }),
    http.put('/api/budgets/:id', async ({ params: prm, request }) => {
      const body = await request.json().catch(() => ({}))
      return routed(() =>
        envelope(updateBudget(getBudgets(), String(prm.id), ADMIN_ID, body as never, nowFn())),
      )
    }),
    http.delete('/api/budgets/:id', ({ params: prm }) =>
      routed(() => {
        deleteBudget(getBudgets(), String(prm.id), ADMIN_ID)
        return HttpResponse.json({ data: null, status_code: 200, message: 'Budget deleted' })
      }),
    ),
    // usage/routes.rs by_agent: `crate::Paginated` is `{data, total}` with total = the page length.
    http.get('/api/usage/by-agent', ({ request }) =>
      routed(() => {
        if (hasVariant('router-usage-fail')) throw new MockHttpError(500, 'internal error')
        const p = params(request)
        const limit = Number(p.limit) || 50
        const offset = Number(p.offset) || 0
        let rows = usageByAgent(
          getSeed(),
          getAgents().agents,
          ADMIN_ID,
          Number(p.days) || 30,
          nowFn(),
        )
        if (hasVariant('router-usage-full')) {
          rows = [
            ...rows,
            ...Array.from({ length: SPEND_LIMIT }, (_, i) => ({
              agent_id: null,
              agent_name: null,
              request_count: 1,
              total_input_tokens: 10,
              total_output_tokens: 5,
              total_tokens: 15 - (i % 5),
              total_cost_usd: 0.0001,
              avg_latency_ms: 900,
            })),
          ]
        }
        const page = rows.slice(offset, offset + limit)
        return HttpResponse.json({ data: page, total: page.length })
      }),
    ),
    // custom_providers.rs: list for anyone (never the key), writes superuser-only (require_superuser: 403).
    http.get('/api/custom-providers', () => routed(() => envelope(listCustom(getRouter())))),
    http.post('/api/custom-providers', async ({ request }) => {
      const body = await request.json().catch(() => ({}))
      return routed(
        () =>
          HttpResponse.json(
            {
              data: createCustom(getRouter(), body as never, nowFn()),
              status_code: 201,
              message: 'Custom provider created',
            },
            { status: 201 },
          ),
        true,
      )
    }),
    http.post('/api/custom-providers/test', async ({ request }) => {
      const body = (await request.json().catch(() => ({}))) as {
        base_url?: string
        api_key?: string
        kind?: string
        api_version?: string
        model?: string
      }
      return routed(() => {
        validateDialect(body.kind, body.api_version)
        if (!body.base_url?.trim() || !body.api_key?.trim())
          throw new MockHttpError(400, 'base_url and api_key are required')
        if (hasVariant('router-custom-down'))
          return envelope({ chat_ok: false, chat_error: 'connection refused', models: [] })
        return envelope({ chat_ok: !!body.model, models: ['custom-local', 'custom-large'] })
      }, true)
    }),
    http.patch('/api/custom-providers/:id', async ({ params: prm, request }) => {
      const body = await request.json().catch(() => ({}))
      return routed(() => envelope(updateCustom(getRouter(), String(prm.id), body as never)), true)
    }),
    http.delete('/api/custom-providers/:id', ({ params: prm }) =>
      routed(() => envelope(deleteCustom(getRouter(), String(prm.id))), true),
    ),
    http.post('/api/custom-providers/:id/sync', ({ params: prm }) =>
      routed(
        () =>
          envelope(
            syncCustom(getRouter(), String(prm.id), nowFn(), hasVariant('router-custom-down')),
          ),
        true,
      ),
    ),
  ],
  // Filled in below: chatHandlers() needs the seed getters defined in this file.
  deploy: deployHandlers(),
  mcp: mcpHandlers({
    loggedIn: () => loggedIn,
    me: () => sessionUser(),
    users: () => getAgents().users,
    agents: getAgents,
    now: () => nowFn(),
    hasVariant: (v) => hasVariant(v as MockVariant),
    state: getMcp,
  }),
  // Before chat: a run's requests share /api/hitl/:id, and chat's handlers answer 404 for ids they don't own.
  workflows: workflowHandlers({
    loggedIn: () => loggedIn,
    me: () => sessionUser(),
    agents: getAgents,
    // acl.rs can_access_agent: owner, public, a user grant, or a superuser.
    canAccess: (id) => {
      const a = getAgents().agents.find((x) => x.id === id)
      const me = sessionUser()
      return (
        !!a &&
        (me.is_superuser || a.owner_id === me.id || a.is_public || a.userGrants.includes(me.id))
      )
    },
    now: () => nowFn(),
    hasVariant: (v) => hasVariant(v as MockVariant),
    state: getWorkflows,
  }),
  chat: [],
  // Settings (plans/feat-settings.md); EE stacks /api/settings/oidc and the SCIM tokens on top.
  settings: [
    // settings.rs at 43833316: a bare object; with no row yet, its hard-coded defaults.
    http.get('/api/settings', () =>
      loggedIn ? HttpResponse.json(settingsRow ?? SETTINGS_DEFAULTS) : unauthorized(),
    ),
    // `update_settings`: superuser-only, and it writes EVERY column, so a field left out becomes null (ST-2).
    http.put('/api/settings', async ({ request }) => {
      if (!loggedIn) return unauthorized()
      if (!sessionUser().is_superuser) return text('requires superuser', 403)
      const body = (await request.json().catch(() => ({}))) as Record<string, unknown>
      settingsRow = Object.fromEntries(
        Object.keys(SETTINGS_DEFAULTS).map((k) => [k, body[k] ?? null]),
      )
      return HttpResponse.json(settingsRow)
    }),
    // context_selection.rs (nasiko-cloud-rs 1a305a63): bare objects, any signed-in user, the caller's own. A value
    // outside the enum never reaches the handler: Axum's Json extractor answers a plain-text 422.
    ...(
      [
        ['/api/me/context-strategy', 'strategy', ['pacms', 'topk', 'lastk']],
        ['/api/me/pacms-budget', 'level', ['low', 'medium', 'high']],
      ] as const
    ).flatMap(([path, key, values]) => [
      http.get(path, () =>
        loggedIn ? HttpResponse.json({ [key]: chatContext[key] }) : unauthorized(),
      ),
      http.patch(path, async ({ request }) => {
        if (!loggedIn) return unauthorized()
        const v = ((await request.json().catch(() => ({}))) as Record<string, unknown>)[key]
        if (typeof v !== 'string' || !(values as readonly string[]).includes(v))
          return text(
            `Failed to deserialize the JSON body into the target type: ${key}: unknown variant`,
            422,
          )
        chatContext[key] = v
        return HttpResponse.json({ [key]: v })
      }),
    ]),
  ],
}

// ── Agents helpers ─────────────────────────────────────────────────────────────

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** UUID or exact name (get_one resolves both); deleted agents are gone. */
function findAgent(ref: string): MockAgent | undefined {
  return getAgents().agents.find((a) => !a.deleted && (a.id === ref || a.name === ref))
}

/** acl.rs can_manage_agent: owner or superuser. The mock viewer is the seed admin (superuser) by default. */
function canManage(a: MockAgent): boolean {
  const me = viewer()
  return me.is_superuser || a.owner_id === me.id
}

/** The agent, or the response a manage-only route sends instead. */
function managed(ref: string): MockAgent | Response {
  if (!loggedIn) return unauthorized()
  const a = findAgent(ref)
  if (!a) return new HttpResponse(null, { status: 404 })
  if (!canManage(a)) return new HttpResponse(null, { status: 403 })
  return a
}

function grant(ref: string, apply: (a: MockAgent) => void, status = 204): Response {
  const a = managed(ref)
  if (a instanceof Response) return a
  apply(a)
  return new HttpResponse(null, { status })
}

// ── Router helpers ─────────────────────────────────────────────────────────────

/** Run a router handler, mapping thrown errors to the server's responses (plain text, or JSON when the error carries a body). */
function routed(fn: () => Response, superuserOnly = false): Response {
  if (!loggedIn) return unauthorized()
  if (superuserOnly && !superuser()) return text('requires superuser', 403)
  try {
    return fn()
  } catch (err) {
    if (err instanceof MockHttpError)
      return err.body === undefined
        ? text(err.message, err.status)
        : HttpResponse.json(err.body as never, { status: err.status })
    if (err instanceof Response) return err
    return text('internal error', 500)
  }
}

/** agents/llm_config.rs `agent_owner_or_reject`: 404 unknown, 403 "not the agent owner" unless owner or superuser. */
function routingAgent(ref: string): MockAgent {
  const a = getAgents().agents.find((x) => !x.deleted && x.id === ref)
  if (!a) throw new MockHttpError(404, 'agent not found')
  if (a.owner_id !== ADMIN_ID && !superuser()) throw new MockHttpError(403, 'not the agent owner')
  return a
}

// ── Harnesses live-fallback helpers (existing endpoints, harness seed) ─────────

function myAgentDashboard(p: Record<string, string | null>) {
  const now = nowFn()
  let win: { start: number; end: number }
  try {
    win = resolveWin(p, now)
  } catch (err) {
    // finops answers a bad window with a plain-text 400 (handler.rs resolve_range_params).
    throw new MockHttpError(400, err instanceof Error ? err.message : 'invalid window')
  }
  const { start, end } = win
  // prev-fail: the previous-window call (ending well before now) fails, the current one works.
  if (variant() === 'prev-fail' && end < now - 60_000)
    throw new MockHttpError(500, 'internal error')
  const hs = harnessData()
  const me = viewer()
  const agents = myAgentDashboardRows(hs, me, start, end)
  const total = agents.reduce((n, a) => n + a.total_cost, 0)
  const ops = agents.reduce((n, a) => n + a.operations, 0)
  const kpi = (current: number) => ({ current, previous: 0, change_pct: null })
  return {
    summary: {
      total_cost: total,
      total_operations: ops,
      operations_last_24h: 0,
      average_cost: ops ? total / ops : 0,
      active_agents: agents.length,
      total_agents: agents.length,
      total_container_hours: 0,
      unpriced_calls: fleetUnpriced(hs, start, end),
      estimated_cost: 0,
      unknown_confidence_calls: ops,
    },
    agents,
    token_usage: {
      total_tokens: agents.reduce((n, a) => n + a.total_tokens, 0),
      prompt_tokens: 0,
      completion_tokens: 0,
      cache_read_tokens: 0,
      cache_creation_tokens: 0,
      avg_tokens_per_operation: 0,
    },
    kpis: {
      total_spend: kpi(total),
      total_tokens: kpi(0),
      cost_per_operation: kpi(0),
      avg_latency_ms: kpi(0),
      total_agents: kpi(agents.length),
      active_agents: kpi(agents.length),
      total_operations: kpi(ops),
      total_tool_calls: kpi(0),
      latency_p95_ms: kpi(0),
      latency_p99_ms: kpi(0),
    },
    attributions: { by_agent: [], by_workflow: [] },
  }
}

function harnessAgentsList(owner: string) {
  return getHarnessSeed()
    .agents.filter((a) => a.owner_id === owner && !a.deleted)
    .map((a) => ({
      id: a.id,
      name: a.name,
      display_name: a.display_name,
      version: '1.0.0',
      status: 'registered',
      owner_id: a.owner_id,
      created_at: getHarnessSeed().anchor,
      updated_at: getHarnessSeed().anchor,
      capabilities: {},
      security_schemes: {},
      default_input_modes: ['text'],
      default_output_modes: ['text'],
      preferred_transport: 'JSONRPC',
      protocol_version: '0.3.0',
      skills: [],
      // CLI-registered rows carry these (catalog/routes.rs); the spoof claims a harness without an integration id.
      tags: ['local', 'coding-agent'],
      metadata: {
        source: 'nasiko-cli-integration',
        integration_id: a.spoofed ? 'claude' : a.harness,
      },
    }))
}

function harnessAgentDetail(a: HarnessSeed['agents'][number]) {
  return {
    id: a.id,
    name: a.name,
    display_name: a.display_name,
    owner_id: a.owner_id,
    description: '',
    created_at: getHarnessSeed().anchor,
    capabilities: {},
    can_manage: true,
    coding_agent_integration_id: a.spoofed ? null : a.harness,
    is_coding_agent: !a.spoofed,
  }
}

/** Own-only, newest first (ORDER BY updated_at DESC), optionally one agent (`?agent_id=`), as chat/routes.rs. */
function chatSessions(limit: number, agentId?: string) {
  const hs = harnessData()
  const me = viewer()
  const rows = hs.sessions
    .filter((x) => x.user_id === me.id && (!agentId || x.agent_id === agentId))
    .map((x) => ({
      x,
      updated: new Date(Date.parse(x.started_at) + x.turns * 60_000).toISOString(),
    }))
    .sort((a, b) => b.updated.localeCompare(a.updated))
    .slice(0, limit + 1)
  const has_more = rows.length > limit
  const agent = (id: string) => hs.agents.find((a) => a.id === id)
  return {
    // updated_at = the last turn (chat/routes.rs orders by it), so it differs from created_at.
    data: rows.slice(0, limit).map(({ x, updated }) => ({
      session_id: x.session_id,
      user_id: x.user_id,
      agent_id: x.agent_id,
      agent_url: `/api/agents/${x.agent_id}`,
      title: `${agent(x.agent_id)?.display_name ?? 'Session'}`,
      created_at: x.started_at,
      updated_at: updated,
      agent_name: agent(x.agent_id)?.name ?? null,
      is_coding_agent: true,
      last_message: null,
      message_count: x.turns * 2,
      trace_count: x.turns,
      total_tokens: x.tokens,
      latency_p50_ms: null,
    })),
    has_more,
    next_cursor: has_more ? 'more' : null,
    prev_cursor: null,
  }
}

const CHAT_SCENARIO_NAMES = Object.keys(CHAT_SCENARIOS) as ChatScenario[]

/** `?mock=<chat scenario>` in full mock mode only (the partial-live rule, as `variant()`). */
function chatScenario(): ChatScenario | null {
  return (
    (mockEntries().find((e) => (CHAT_SCENARIO_NAMES as string[]).includes(e)) as
      ChatScenario | undefined) ?? null
  )
}

handlerGroups.chat = chatHandlers({
  loggedIn: () => loggedIn,
  userId: () => ADMIN_ID,
  now: () => nowFn(),
  agents: () =>
    getAgents()
      .agents.filter((a) => !a.deleted && !a.tags.includes('coding-agent'))
      .map((a) => ({
        id: a.id,
        name: a.name,
        running: a.status === 'running' && !hasVariant('no-agents'),
      })),
  scenario: chatScenario,
  traceId: () => observabilityData(getSeed()).showcaseTraceId || null,
  username: () => 'admin',
  harness: () => {
    const h = getAgents().agents.find(
      (a) => !a.deleted && a.tags.includes('coding-agent') && a.owner_id === ADMIN_ID,
    )
    return h ? { id: h.id, name: h.name } : null
  },
  hasVariant: (v) =>
    (CHAT_PAGE_VARIANTS as readonly string[]).includes(v) && hasVariant(v as MockVariant),
  superuser,
})

/**
 * Deploy group (plans/feat-deploy.md §9). ACL as the server: a superuser sees every build and upload, anyone else only
 * their own agents' (`list_all_builds`, `list_upload_status`); single-build reads aren't owner-checked (D-5).
 */
function deployHandlers(): HttpHandler[] {
  const noRights = () => HttpResponse.json({ available: false })
  const githubConnected = () =>
    getDeploy().github.connected ?? variant() !== 'deploy-github-disconnected'
  const visible = (b: MockBuild) => superuser() || b.ownerId === ADMIN_ID
  return [
    // upload.rs upload_and_deploy: validates like the server (the zip check is the same rules the page runs), then
    // 202 with a queued build. The version comes from `version_tag`, else the zip (AgentCard.json → pyproject → Cargo).
    http.post('/api/agents/upload', async ({ request }) => {
      if (!loggedIn) return unauthorized()
      const fd = await readMultipart(request).catch(() => null)
      const name = String(fd?.get('name') ?? fd?.get('agent_name') ?? '')
      if (!name) return text('name is required', 400)
      const problem = nameProblem(name)
      if (problem) return text(`invalid name: ${problem}`, 400)
      const file = fd?.get('file') ?? fd?.get('source')
      if (!(file instanceof Blob)) return text('source zip is required', 400)
      if (file.size > MAX_ZIP_BYTES) return text('upload exceeds 100 MiB', 413)
      const check = await checkZip(file)
      if (check.items.dockerfile.state === 'fail') {
        return text(
          check.items.dockerfile.detail?.includes('FROM')
            ? 'Dockerfile has no FROM instruction'
            : 'no Dockerfile found in root of zip',
          400,
        )
      }
      if (check.items.entrypoint.state === 'fail')
        return text(
          'no Python entrypoint found (main.py, src/main.py, __main__.py, or src/__main__.py)',
          400,
        )
      const version = String(fd?.get('version_tag') ?? '') || check.version || ''
      if (!parseVersion(version))
        return text(
          'version_tag is required and must be in x.y.z format (or declare it in AgentCard.json, pyproject.toml or Cargo.toml)',
          400,
        )
      const agents = getAgents()
      const existing = agents.agents.find((a) => a.name === name && !a.deleted)
      if (existing?.versions.some((v) => v.version === version))
        return text(
          `version ${version} already exists in this agent's history — choose a new version`,
          409,
        )
      const ids = queueUpload(getDeploy(), agents, {
        name,
        version,
        ownerId: ADMIN_ID,
        now: nowFn(),
        fails: variant() === 'deploy-build-fails' ? 'upload and deploy failed' : null,
      })
      return HttpResponse.json(
        {
          data: {
            success: true,
            agent_name: name,
            agent_id: ids.agentId,
            build_id: ids.buildId,
            status: 'queued',
            capabilities_generated: false,
            orchestration_triggered: false,
            validation_errors: [],
          },
          status_code: 202,
          message: `Upload accepted; build ${ids.buildId} queued`,
        },
        { status: 202 },
      )
    }),
    // ── GitHub (github.rs) ──
    http.get('/api/auth/github/status', () =>
      HttpResponse.json({ configured: variant() !== 'deploy-github-unconfigured' }),
    ),
    http.get('/api/github/user', () => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-github-unconfigured')
        return HttpResponse.json({ connected: false, configured: false })
      return githubConnected()
        ? HttpResponse.json({ connected: true, valid: true, login: getDeploy().github.login })
        : HttpResponse.json({ connected: false, valid: false })
    }),
    http.get('/api/auth/github/token', () => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-github-unconfigured')
        return HttpResponse.json(
          { success: false, message: 'GitHub OAuth not configured', status: 'disconnected' },
          { status: 503 },
        )
      return githubConnected()
        ? HttpResponse.json({
            success: true,
            message: 'GitHub token is valid',
            status: 'connected',
            username: getDeploy().github.login,
          })
        : HttpResponse.json(
            { success: false, message: 'GitHub not connected', status: 'disconnected' },
            { status: 202 },
          )
    }),
    // The popup can't do real OAuth in mock mode: it opens a blank page and the mock connects a moment later.
    http.get('/api/github/login', () => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-github-unconfigured')
        return HttpResponse.json({ error: 'GitHub OAuth not configured' }, { status: 503 })
      const d = getDeploy()
      // Tests complete the popup themselves (`deployMockState().setGithubConnected`).
      if (import.meta.env.MODE !== 'test')
        setTimeout(() => {
          d.github.connected = true
        }, 1_500)
      return HttpResponse.json({ auth_url: 'about:blank#openruntime-mock-github-oauth' })
    }),
    http.get('/api/github/repositories', () => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-github-unconfigured')
        return text('GitHub OAuth not configured', 404)
      if (!githubConnected())
        return text('GitHub not connected — visit /agents.html?view=import to connect', 403)
      const repos =
        variant() === 'deploy-github-no-repos'
          ? []
          : mockRepos(getAgents(), nowFn()).map(({ agentName: _a, ...r }) => r)
      return HttpResponse.json({ repositories: repos, total: repos.length })
    }),
    http.delete('/api/github/logout', () => {
      if (!loggedIn) return unauthorized()
      getDeploy().github.connected = false
      return HttpResponse.json({ message: 'GitHub credentials cleared' })
    }),
    // github.rs github_clone: validates, then 202 with the upload (= build) id. The repo's declared version is the
    // existing agent's current one, so cloning an existing agent without `version_override` fails later with
    // VERSION_CONFLICT (the build page offers "Deploy as vX"); a new agent starts at 0.1.0.
    http.post('/api/github/clone', async ({ request }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-github-unconfigured')
        return text('GitHub OAuth not configured', 503)
      if (!githubConnected())
        return text('GitHub not connected — visit /agents.html?view=import to connect', 403)
      const body = (await request.json().catch(() => ({}))) as {
        repository_full_name?: string
        branch?: string
        agent_name?: string
        version_override?: string
      }
      const repo = body.repository_full_name ?? ''
      const branch = body.branch ?? 'main'
      if (!/^[\w.-]+\/[\w.-]+$/.test(repo))
        return text(`invalid request: invalid repository name '${repo}'`, 422)
      if (!/^[\w./-]+$/.test(branch) || branch.includes('..'))
        return text(`invalid request: invalid branch name '${branch}'`, 422)
      if (body.version_override && !parseVersion(body.version_override))
        return text(
          `invalid version_override ${body.version_override}: must be in x.y.z format, e.g. 1.2.3`,
          422,
        )
      const name = body.agent_name ?? repo.split('/').pop()!
      const problem = nameProblem(name)
      if (problem) return text(`invalid agent name: ${problem}`, 400)
      const agents = getAgents()
      const existing = agents.agents.find((a) => a.name === name && !a.deleted)
      const declared = existing?.version ?? '0.1.0'
      const version = body.version_override ?? declared
      const clash = !!existing && existing.versions.some((v) => v.version === version)
      const next = declared.replace(/(\d+)$/, (d) => String(Number(d) + 1))
      const ids = queueUpload(getDeploy(), agents, {
        // github_clone inserts the build without github_url, and nothing writes commit_hash (D-12).
        name,
        version,
        ownerId: ADMIN_ID,
        now: nowFn(),
        fails: clash
          ? `VERSION_CONFLICT:${version}:${next}:${name} version ${version} already exists and versions are immutable`
          : variant() === 'deploy-build-fails'
            ? 'clone and deploy failed'
            : null,
      })
      return HttpResponse.json(
        {
          success: true,
          message: `Agent '${name}' clone queued`,
          agent_name: name,
          upload_id: ids.buildId,
        },
        { status: 202 },
      )
    }),
    // catalog/import.rs import_registry: synchronous (the mock takes a few seconds in the browser, none in tests).
    // Allowed hosts: registry.nasiko.dev and localhost:5000. A `<host>/nasiko/...` reference is an agent package (built);
    // anything else is a plain image (pulled, no build). The name is the last path part, the version the x.y.z tag.
    http.post('/api/import/registry', async ({ request }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-registry-disabled')
        return text(
          'registry import is disabled — set REGISTRY_IMPORT_ALLOWED_HOSTS to enable it',
          403,
        ) // unreachable on OSS 2d6178e4 (D-7): the built-in host is always allowed
      const { reference = '' } = (await request.json().catch(() => ({}))) as { reference?: string }
      // import_registry splits the host at the first `/`: a scheme becomes the host `https:` and fails the allow list.
      const scheme = /^(https?):\/\//.exec(reference.trim())
      if (scheme) return text(`registry host '${scheme[1]}' is not in the allowed list`, 422)
      const ref = parseReference(reference)
      if (!ref) return text('invalid reference: expected registry.host/owner/name[:tag]', 400)
      if (ref.host !== DEFAULT_REGISTRY && ref.host !== 'localhost:5000')
        return text(`registry host '${ref.host}' is not in the allowed list`, 422)
      const name = ref.repo.split('/').pop()!
      const built = ref.repo.startsWith('nasiko/')
      // A pulled image keeps its tag (one leading `v` stripped) and has no version-history check; only the build path
      // (`build_and_deploy`) refuses a version it has seen.
      const version = built
        ? parseVersion(ref.tag)
          ? ref.tag
          : '1.0.0'
        : ref.tag.replace(/^v/, '')
      const agents = getAgents()
      const existing = agents.agents.find((a) => a.name === name && !a.deleted)
      if (built && existing?.versions.some((v) => v.version === version))
        return text(
          `version ${version} already exists in this agent's history — choose a new version`,
          409,
        )
      if (import.meta.env.MODE !== 'test') await new Promise((r) => setTimeout(r, 6_000))
      const running = variant() !== 'deploy-registry-not-running'
      const ids = completeImport(getDeploy(), agents, {
        name,
        version,
        image: `${ref.host}/${ref.repo}:${ref.tag}`,
        ownerId: ADMIN_ID,
        now: nowFn(),
        built,
        running,
      })
      return HttpResponse.json(
        {
          agent_id: ids.agentId,
          build_id: ids.buildId,
          container_name: running ? `agent-${name}` : null,
          status: 'success',
        },
        { status: 201 },
      )
    }),
    http.get('/api/builds', ({ request }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'builds-absent') return new HttpResponse(null, { status: 404 })
      if (variant() === 'deploy-no-rights') return noRights()
      const p = params(request)
      const limit = Math.max(1, Number(p.limit ?? 20) || 20)
      const offset = Math.max(0, Number(p.offset ?? 0) || 0)
      const q = (p.q ?? '').toLowerCase()
      const now = nowFn()
      const rows = getDeploy()
        .builds.filter(visible)
        .map((b) => ({ b, r: recordAt(b, now) }))
        .filter(
          (x): x is { b: MockBuild; r: NonNullable<ReturnType<typeof recordAt>> } =>
            !!x.r && x.r.created_at <= new Date(now).toISOString(),
        )
        .filter(({ r }) => !p.status || r.status === p.status)
        .filter(
          ({ b, r }) =>
            !q || b.agentName.toLowerCase().includes(q) || r.version_tag.toLowerCase().includes(q),
        )
        .sort((x, y) => y.r.created_at.localeCompare(x.r.created_at))
        .slice(offset, offset + limit)
        .map(({ r }) => r)
      return HttpResponse.json({ data: rows, total: rows.length })
    }),
    http.get('/api/builds/agent/:agentId', ({ params: p }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-no-rights') return noRights()
      const now = nowFn()
      // list_builds: the agent's owner or a superuser, whether or not it has builds.
      const agent = getAgents().agents.find((a) => a.id === String(p.agentId))
      if (!superuser() && agent && agent.owner_id !== ADMIN_ID) return noRights()
      const mine = getDeploy().builds.filter((b) => b.record.agent_id === String(p.agentId))
      const rows = mine
        .map((b) => recordAt(b, now))
        .filter((r) => !!r)
        .sort((x, y) => y!.created_at.localeCompare(x!.created_at))
        .slice(0, 20)
      return HttpResponse.json(rows)
    }),
    http.get('/api/builds/:id', ({ params: p }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-no-rights') return noRights()
      const b = getDeploy().builds.find((x) => x.record.id === String(p.id))
      const r = b ? recordAt(b, nowFn()) : null
      return r ? HttpResponse.json(r) : new HttpResponse(null, { status: 404 })
    }),
    http.get('/api/agents/uploads', ({ request }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-no-rights') return noRights()
      const p = params(request)
      const limit = Math.min(100, Math.max(1, Number(p.limit ?? 10) || 10))
      const offset = Math.max(0, Number(p.offset ?? 0) || 0)
      const now = nowFn()
      const data = getDeploy()
        .builds.filter((b) => b.hasUpload && visible(b) && b.timeline[0]!.at <= now)
        .sort((x, y) => y.timeline[0]!.at - x.timeline[0]!.at)
        .slice(offset, offset + limit)
        .map((b) => uploadAt(b, now))
      return HttpResponse.json({
        data,
        status_code: 200,
        message: `Retrieved ${data.length} upload records`,
      })
    }),
    http.get('/api/agents/uploads/:id', ({ params: p }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-no-rights') return noRights()
      const b = getDeploy().builds.find((x) => x.record.id === String(p.id))
      const row = b && visible(b) ? uploadAt(b, nowFn()) : null
      return row ? HttpResponse.json(row) : new HttpResponse(null, { status: 404 })
    }),
    // upload.rs deploy_status_sse: the current status, then only changes (the server polls its DB every 3 s; the mock
    // every 100 ms), closing after success or failed; an unknown id gets one `not_found` and closes.
    http.get('/api/agents/deploys/:id/stream', ({ params: p, request }) => {
      if (!loggedIn) return unauthorized()
      if (variant() === 'deploy-no-rights') return noRights()
      const id = String(p.id)
      const enc = new TextEncoder()
      const frame = (o: unknown) => enc.encode(`data: ${JSON.stringify(o)}\n\n`)
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          let last: BuildStatus | null = null
          let timer: ReturnType<typeof setInterval> | null = null
          const close = () => {
            if (timer) clearInterval(timer)
            try {
              controller.close()
            } catch {
              /* already closed */
            }
          }
          const tick = () => {
            const b = getDeploy().builds.find((x) => x.record.id === id)
            const s = b ? buildStatusAt(b, nowFn()) : null
            if (!s) {
              controller.enqueue(frame({ status: 'not_found' }))
              return close()
            }
            if (s !== last) {
              last = s
              controller.enqueue(frame({ status: s, build_id: id }))
            }
            if (s === 'success' || s === 'failed') close()
          }
          tick()
          if (last && last !== 'success' && last !== 'failed') timer = setInterval(tick, 100)
          request.signal.addEventListener('abort', close, { once: true })
        },
      })
      return new HttpResponse(stream, { headers: { 'Content-Type': 'text/event-stream' } })
    }),
  ]
}

/**
 * An edition's own mock handlers by MOCKABLE group (`@edition/mocks`; docs/lab-vs-react-migration-review.md §10.5).
 * They answer before every core handler; one that returns nothing falls through to the core's.
 */
export type EditionHandlers = Partial<Record<Mockable, HttpHandler[]>>

export function handlersFor(
  groups: readonly Mockable[],
  edition: EditionHandlers = {},
): HttpHandler[] {
  return [...groups.flatMap((g) => edition[g] ?? []), ...groups.flatMap((g) => handlerGroups[g])]
}

/**
 * GET /health (public). Full mock mode only: it isn't a MOCKABLE key, so partial-live mode asks the
 * real server (eng D4). The body is plain-text `ok`, as nasiko-server `lib.rs` `health` (cb3aaf0c).
 * `?mock=server-down` fails it as a network error, as a stopped server would.
 */
const healthHandlers: HttpHandler[] = [
  http.get('/health', () =>
    variant() === 'server-down' ? HttpResponse.error() : HttpResponse.text('ok'),
  ),
]

export const allHandlers: HttpHandler[] = [
  ...Object.values(handlerGroups).flat(),
  ...healthHandlers,
]

/** The usage endpoint's answer: the `{data}` envelope, or its coded API_CONVENTIONS error. */
function usageResponse(fn: () => unknown): Response {
  try {
    return HttpResponse.json({ data: fn() })
  } catch (err) {
    if (err instanceof UsageHttpError)
      return HttpResponse.json({ error: err.message, code: err.code }, { status: err.status })
    throw err
  }
}

/** What an edition's mocks build on: the core mock's live state and response helpers (never copies of them). */
export const mockCtx = {
  loggedIn: () => loggedIn,
  now: () => nowFn(),
  variant,
  superuser,
  viewer,
  harnessSeed: getHarnessSeed,
  /** The harness seed as the current variant changes it (`all-unpriced`, `no-activity`). */
  harnessData,
  agents: getAgents,
  mcp: getMcp,
  findAgent,
  managed,
  grant,
  params,
  text,
  envelope,
  unauthorized,
  usageResponse,
}
