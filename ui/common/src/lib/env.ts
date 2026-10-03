/**
 * Runtime mode, read from Vite env (see .env.example).
 *
 * VITE_NASIKO_API_MODE          mock | live    default: mock in dev, live in production builds
 * VITE_NASIKO_ALLOW_MOCK_BUILD  true           let a production build honour mock mode (deliberate demo builds only)
 * VITE_NASIKO_MOCK              comma list     in live mode, endpoints MSW still mocks (e.g. "top-traces")
 * VITE_NASIKO_LEGACY_UI_URL     http(s) URL    where trace links open (the existing nasiko UI)
 * VITE_NASIKO_WAITLIST_URL      http(s) URL    the Nasiko waitlist page the OSS app links to (default https://nasiko-waitlist.vercel.app)
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
  'observability',
  'harnesses',
  'chat',
  'router',
  'deploy',
  'settings',
  'mcp',
  'workflows',
  'onboarding',
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
