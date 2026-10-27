/**
 * Runtime mode, read from Vite env (see .env.example).
 *
 * VITE_NASIKO_API_MODE          mock | live    default: mock in dev, live in production builds
 * VITE_NASIKO_ALLOW_MOCK_BUILD  true           let a production build honour mock mode (deliberate demo builds only)
 * VITE_NASIKO_MOCK              comma list     in live mode, endpoints MSW still mocks (e.g. "top-traces")
 * VITE_NASIKO_LEGACY_UI_URL     http(s) URL    where trace links open (the existing nasiko UI)
 * VITE_NASIKO_WAITLIST_URL      http(s) URL    the Nasiko waitlist page the OSS app links to (default https://nasiko-waitlist.vercel.app);
 *                                              `off` (any non-URL) hides every waitlist link, as the desktop app's build does
 * NASIKO_API_URL                (vite.config.ts) proxy target for /api — server-side only
 */
type ApiMode = 'mock' | 'live'

/**
 * Keys of `handlerGroups` in src/mocks/handlers.ts — the unit of selective mocking. An edition's own mocks use the
 * same keys, so `harnesses` also brings that edition's harness identity routes (EE: org units and users/me).
 */
export const MOCKABLE = [
  'auth',
  'agents',
  'dashboard',
  'spend-timeseries',
  'spend-calendar',
  'providers',
  'top-traces',
  'savings',
  // Before observability and chat: it answers its own flows' traces and flow ids, and passes the rest on.
  'flows',
  'observability',
  'harnesses',
  'chat',
  'router',
  // Budgets (plans/feat-budget-burn.md eng D1, D2): R-L10 and the burn contract. `router` brings it along.
  'budgets',
  // Partial live for budgets (budgets.md §15): the real server where it serves a feature, the golden mock elsewhere.
  'budgets-fill',
  'deploy',
  'settings',
  'mcp',
  'workflows',
  'onboarding',
  // The free assessment form's POST (assessment.rs).
  'assessment',
  // Coding-agent sync progress and the desktop app's local routes (the Harnesses onboarding card).
  'coding-agents',
  'optimization',
  // The proposed agent event log (plans/feat-agent-centric.md PR1a): change markers on the agent hub.
  'events',
  // The Overview's proposed estate, problem state and pins (docs/requirements/overview.md OV-1..OV-3).
  'overview',
] as const
export type Mockable = (typeof MOCKABLE)[number]

export interface EnvConfig {
  mode: ApiMode
  /** Endpoints mocked while in live mode. Empty in mock mode (everything is mocked). */
  partialMocks: Mockable[]
  legacyUiUrl: string | null
  /** The waitlist page, tagged `ref=oss-app`; null hides every waitlist link. */
  waitlistUrl: string | null
}

type RawEnv = Record<string, string | boolean | undefined>

export function readEnv(
  env: RawEnv = import.meta.env,
  origin: string | undefined = globalThis.location?.origin,
): EnvConfig {
  const rawMode = String(env.VITE_NASIKO_API_MODE ?? '').trim()
  let mode: ApiMode = rawMode === 'live' || rawMode === 'mock' ? rawMode : env.DEV ? 'mock' : 'live'
  // A production build never serves fabricated data by accident: mock mode there needs an explicit opt-in.
  const mockAllowed = !env.PROD || String(env.VITE_NASIKO_ALLOW_MOCK_BUILD ?? '') === 'true'
  if (mode === 'mock' && !mockAllowed) mode = 'live'

  let partialMocks: Mockable[] =
    !mockAllowed || mode === 'mock'
      ? []
      : String(env.VITE_NASIKO_MOCK ?? '')
          .split(',')
          .map((s) => s.trim())
          // `auth` is never partially mocked: a fake session against a real server loops on 401s.
          .filter((s): s is Mockable => s !== 'auth' && (MOCKABLE as readonly string[]).includes(s))
  // Chat mocks read agent names and write sessions the observability pages list: mocking
  // chat alone would mix seed agents and chats with the live server's (plan §11, DX-M2).
  if (
    partialMocks.includes('chat') &&
    !(partialMocks.includes('agents') && partialMocks.includes('observability'))
  ) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "chat" needs "agents" and "observability" too; ignoring all three.',
    )
    partialMocks = partialMocks.filter(
      (s) => s !== 'chat' && s !== 'agents' && s !== 'observability',
    )
  }

  // Router mocks read seed agents and the seed catalog: without `agents` and `providers` they would route
  // real agents through seed configs. Only `router` is dropped, so a `providers` mock for TokenOps survives (eng #13).
  if (
    partialMocks.includes('router') &&
    !(partialMocks.includes('agents') && partialMocks.includes('providers'))
  ) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "router" needs "agents" and "providers" too; ignoring "router" (use VITE_NASIKO_MOCK=router,agents,providers).',
    )
    partialMocks = partialMocks.filter((s) => s !== 'router')
  }

  // budgets-fill answers budgets itself (live first, then the golden mock): it replaces `budgets`.
  if (partialMocks.includes('budgets-fill') && partialMocks.includes('budgets')) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "budgets-fill" already answers budgets (live first, then the mock); ignoring "budgets".',
    )
    partialMocks = partialMocks.filter((s) => s !== 'budgets')
  }
  // Budgets used to be part of the router group (plans/feat-budget-burn.md eng D2): `router` keeps mocking them.
  if (
    partialMocks.includes('router') &&
    !partialMocks.includes('budgets') &&
    !partialMocks.includes('budgets-fill')
  )
    partialMocks = [...partialMocks, 'budgets']
  // Budgets are counted from the seed agents' spend: without `agents` they would name agents the server doesn't have.
  if (partialMocks.includes('budgets') && !partialMocks.includes('agents')) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "budgets" needs "agents" too; ignoring "budgets" (use VITE_NASIKO_MOCK=budgets,agents).',
    )
    partialMocks = partialMocks.filter((s) => s !== 'budgets')
  }

  // Builds and uploads are derived from the seed agents: without `agents` the build rows would name agents the
  // real server doesn't have (plans/feat-deploy.md §9).
  if (partialMocks.includes('deploy') && !partialMocks.includes('agents')) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "deploy" needs "agents" too; ignoring "deploy" (use VITE_NASIKO_MOCK=deploy,agents).',
    )
    partialMocks = partialMocks.filter((s) => s !== 'deploy')
  }

  // MCP agent access reads the seed agents (plans/feat-mcp.md §8).
  if (partialMocks.includes('mcp') && !partialMocks.includes('agents')) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "mcp" needs "agents" too; ignoring "mcp" (use VITE_NASIKO_MOCK=mcp,agents).',
    )
    partialMocks = partialMocks.filter((s) => s !== 'mcp')
  }

  // Workflow steps name seed agents, and the step picker reads the agent list (plans/feat-workflows.md §8).
  if (partialMocks.includes('workflows') && !partialMocks.includes('agents')) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "workflows" needs "agents" too; ignoring "workflows" (use VITE_NASIKO_MOCK=workflows,agents).',
    )
    partialMocks = partialMocks.filter((s) => s !== 'workflows')
  }

  // Flows name seed agents and their traces carry seed agent ids (plans/feat-flows.md): without the agents mock the
  // lanes and Open agent links would point at agents the real server doesn't have.
  if (partialMocks.includes('flows') && !partialMocks.includes('agents')) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "flows" needs "agents" too; ignoring "flows" (use VITE_NASIKO_MOCK=flows,agents).',
    )
    partialMocks = partialMocks.filter((s) => s !== 'flows')
  }

  // The Optimization page's savings reads are built from the seed agents (their switches, names and chats), so without
  // the agents mock they would describe agents the real server doesn't have (plans/feat-optimization-page.md review).
  if (partialMocks.includes('optimization') && !partialMocks.includes('agents')) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "optimization" needs "agents" too; ignoring "optimization" (use VITE_NASIKO_MOCK=optimization,agents,observability; observability makes Biggest senders\' trace links open).',
    )
    partialMocks = partialMocks.filter((s) => s !== 'optimization')
  }

  // The event log describes the seed agents' changes (plans/feat-agent-centric.md, DX mocks): without the agents mock
  // its markers would sit on agents the real server doesn't have.
  if (partialMocks.includes('events') && !partialMocks.includes('agents')) {
    console.warn(
      '[ui-lab] VITE_NASIKO_MOCK: "events" needs "agents" too; ignoring "events" (use VITE_NASIKO_MOCK=events,agents).',
    )
    partialMocks = partialMocks.filter((s) => s !== 'events')
  }

  // Dev defaults to the local OSS server; a production build defaults to its own origin
  // (the nasiko server serves both the API and the legacy UI).
  const fallbackUi = env.DEV ? 'http://localhost:8080' : (origin ?? '')
  return {
    mode,
    partialMocks,
    legacyUiUrl: validHttpUrl(String(env.VITE_NASIKO_LEGACY_UI_URL || fallbackUi)),
    waitlistUrl: waitlistHref(String(env.VITE_NASIKO_WAITLIST_URL || WAITLIST_URL)),
  }
}

/** The Nasiko waitlist page, in dev servers and builds alike, unless VITE_NASIKO_WAITLIST_URL names another. */
const WAITLIST_URL = 'https://nasiko-waitlist.vercel.app'

/** An absolute http(s) URL with `ref=oss-app` added (its own query and hash kept), so the page can count sign-ups. */
function waitlistHref(value: string): string | null {
  const v = value.trim()
  if (!/^https?:\/\//i.test(v)) return null
  try {
    const u = new URL(v)
    u.searchParams.set('ref', 'oss-app')
    return u.href
  } catch {
    return null
  }
}

/** Accept only absolute http(s) URLs; anything else disables trace links rather than risk a bad href. */
export function validHttpUrl(value: string): string | null {
  const v = value.trim()
  if (!/^https?:\/\//i.test(v)) return null
  try {
    const u = new URL(v)
    return u.origin + u.pathname.replace(/\/$/, '')
  } catch {
    return null
  }
}

export const env = readEnv()

/**
 * A preview section may render (plans/feat-agent-centric.md eng, preview policy): true only in full mock mode or when
 * the feature's MOCKABLE group is mocked. It gates the section's queries and rendering, whatever the server answers,
 * so a preview never draws from an unexpected live response.
 */
export function isPreview(group: Mockable, config: Pick<EnvConfig, 'mode' | 'partialMocks'> = env) {
  // `router` implies `budgets` (eng D2), also for a config built by hand; `budgets-fill` previews budgets too.
  return (
    config.mode === 'mock' ||
    config.partialMocks.includes(group) ||
    (group === 'budgets' &&
      (config.partialMocks.includes('router') || config.partialMocks.includes('budgets-fill')))
  )
}
