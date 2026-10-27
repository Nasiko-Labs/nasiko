/**
 * Context optimization mocks (plans/feat-context-optimization.md §3, §4; the `optimization` MOCKABLE group).
 *
 * Merged routes, as nasiko-cloud-rs `development` 05f22246 `oss/server/src/context_selection.rs`: bare JSON
 * `{strategy}` / `{level}`, defaults `pacms` / `medium` (`for_user`), a value outside the enum is Axum's plain-text
 * 422 Json rejection. Proposed routes, answered by default so mock mode shows the whole design:
 * - CX-5: `enabled` on the strategy route (an `off` mode the server doesn't have yet);
 * - CX-T1: `GET /api/settings/context-tiers` with the env defaults (`oss/config/src/lib.rs`);
 * - CX-6: `POST /api/me/context-preview`, selection on the caller's latest chat at every tier.
 * `?mock=optimization-classic` answers as today's server: no `enabled`, and bare 404s for the proposed routes.
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type { ChatMessage, ChatSessionRow } from '@nasiko/features/chat/types'
import { tierProblems } from '@nasiko/features/optimization/logic'
import {
  LEVELS,
  SERVER_DEFAULT_TIERS,
  STRATEGIES,
  type FullTiers,
  type Level,
  type SettingsChange,
  type Strategy,
} from '@nasiko/features/optimization/types'

export interface OptimizationCtx {
  loggedIn(): boolean
  me(): { id: string; is_superuser: boolean }
  /** Today's server: no CX-5 / CX-T1 / CX-6 / CX-H. */
  classic(): boolean
  now(): number
  chats(): ChatSessionRow[]
  messages(sessionId: string): ChatMessage[]
  text(body: string, status: number): Response
  unauthorized(): Response
}

/** The env defaults (oss/config/src/lib.rs): the feature's own copy, so the page's fallback and the mock agree. */
export const TIER_DEFAULTS = SERVER_DEFAULT_TIERS

interface OptimizationState {
  strategy: Strategy
  level: Level
  enabled: boolean
}
let state: OptimizationState = { strategy: 'pacms', level: 'medium', enabled: true }
const TIERS_SINCE = '2026-09-01T00:00:00.000Z'
let tiers: FullTiers & { updated_at: string } = {
  ...structuredClone(SERVER_DEFAULT_TIERS),
  updated_at: TIERS_SINCE,
}
/** CX-H (proposed) settings history: this mock's changes, plus one seeded switch to PACMS 9 days before the first read. */
let history: SettingsChange[] | null = null
const historyAt = (now: number): SettingsChange[] =>
  (history ??= [
    {
      at: new Date(now - 9 * 86_400_000).toISOString(),
      field: 'strategy',
      from: 'topk',
      to: 'pacms',
    },
  ])
const record = (now: number, field: SettingsChange['field'], from: string, to: string) => {
  if (from !== to) historyAt(now).unshift({ at: new Date(now).toISOString(), field, from, to })
}
export const resetOptimizationMock = () => {
  state = { strategy: 'pacms', level: 'medium', enabled: true }
  tiers = { ...structuredClone(SERVER_DEFAULT_TIERS), updated_at: TIERS_SINCE }
  history = null
}
export const tiersMockState = () => tiers
export const optimizationMockState = () => state

/** pack_within_budget's estimate: chars / 4. */
const tokensOf = (m: ChatMessage) => Math.ceil(m.content.length / 4)

/**
 * An approximation of `fetch_for_user` without embeddings: the pool is the newest `pool_size` messages; PACMS always
 * keeps the newest `mandatory_recent`, then adds older ones while the budget holds; Top-K keeps K question–answer pairs;
 * Last-K the newest K messages.
 */
export function selectFrom(
  pool: readonly ChatMessage[],
  strategy: Strategy,
  level: Level,
  limits: FullTiers = tiers,
) {
  const newest = [...pool].reverse()
  let kept: ChatMessage[]
  if (strategy === 'pacms') {
    const budget = limits.pacms_budget[level]
    kept = newest.slice(0, limits.mandatory_recent)
    let used = kept.reduce((n, m) => n + tokensOf(m), 0)
    for (const m of newest.slice(limits.mandatory_recent)) {
      if (used + tokensOf(m) > budget) break
      kept.push(m)
      used += tokensOf(m)
    }
  } else {
    const k = limits.context_k[level]
    kept = newest.slice(0, strategy === 'topk' ? 2 * k : k)
  }
  return { messages: kept.length, tokens: kept.reduce((n, m) => n + tokensOf(m), 0) }
}

const STRATEGY_ENUM = STRATEGIES.map((s) => `\`${s}\``).join(', ')
const LEVEL_ENUM = LEVELS.map((s) => `\`${s}\``).join(', ')
/** Axum's Json rejection for a value outside the enum (422, plain text). Approximate: the real text adds
 *  ` at line 1 column N`, and a missing field reads "missing field `strategy`". */
const rejection = (field: string, value: unknown, expected: string) =>
  `Failed to deserialize the JSON body into the target type: ${field}: unknown variant \`${String(value)}\`, expected one of ${expected}`

/** The server's answer to a route it doesn't have (oss/server/src/lib.rs fallback; fixture errors.unknown-route). */
const noRoute = (request: Request) =>
  HttpResponse.json(
    {
      data: null,
      message: `no API route matches ${new URL(request.url).pathname}`,
      status_code: 404,
    },
    { status: 404 },
  )

export function optimizationHandlers(ctx: OptimizationCtx): HttpHandler[] {
  const strategyBody = () =>
    ctx.classic()
      ? { strategy: state.strategy }
      : { strategy: state.strategy, enabled: state.enabled }
  return [
    http.get('/api/me/context-strategy', () =>
      ctx.loggedIn() ? HttpResponse.json(strategyBody()) : ctx.unauthorized(),
    ),
    http.patch('/api/me/context-strategy', async ({ request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      const body = (await request.json().catch(() => null)) as {
        strategy?: unknown
        enabled?: unknown
      } | null
      const s = body?.strategy
      if (typeof s !== 'string' || !(STRATEGIES as readonly string[]).includes(s))
        return ctx.text(rejection('strategy', s, STRATEGY_ENUM), 422)
      record(ctx.now(), 'strategy', state.strategy, s)
      state = { ...state, strategy: s as Strategy }
      if (!ctx.classic() && typeof body?.enabled === 'boolean') {
        record(ctx.now(), 'enabled', String(state.enabled), String(body.enabled))
        state.enabled = body.enabled
      }
      return HttpResponse.json(strategyBody())
    }),
    http.get('/api/me/pacms-budget', () =>
      ctx.loggedIn() ? HttpResponse.json({ level: state.level }) : ctx.unauthorized(),
    ),
    http.patch('/api/me/pacms-budget', async ({ request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      const body = (await request.json().catch(() => null)) as { level?: unknown } | null
      const l = body?.level
      if (typeof l !== 'string' || !(LEVELS as readonly string[]).includes(l))
        return ctx.text(rejection('level', l, LEVEL_ENUM), 422)
      record(ctx.now(), 'level', state.level, l)
      state = { ...state, level: l as Level }
      return HttpResponse.json({ level: state.level })
    }),
    // CX-H (proposed): your setting changes in the window, newest first (`range` 24h|7d|30d|90d, default 30d, or
    // start_time/end_time). Descriptive only: the page never calls a change the cause of a trend (eng C6).
    http.get('/api/me/settings-history', ({ request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.classic()) return noRoute(request)
      const q = new URL(request.url).searchParams
      const now = ctx.now()
      const days = { '24h': 1, '7d': 7, '30d': 30, '90d': 90 }[q.get('range') ?? '30d'] ?? 30
      const start = q.get('start_time') ? Date.parse(q.get('start_time')!) : now - days * 86_400_000
      const end = q.get('end_time') ? Date.parse(q.get('end_time')!) : now
      return HttpResponse.json(
        historyAt(now).filter((c) => {
          const t = Date.parse(c.at)
          return t >= start && t <= end
        }),
      )
    }),
    http.get('/api/settings/context-tiers', ({ request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.classic()) return noRoute(request)
      return HttpResponse.json(tiers)
    }),
    // CX-T1 (proposed) write: superuser only, the whole set, the form's own rules.
    http.put('/api/settings/context-tiers', async ({ request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.classic()) return noRoute(request)
      if (!ctx.me().is_superuser) return ctx.text('requires superuser', 403)
      const raw = (await request.json().catch(() => null)) as
        (FullTiers & { expected_updated_at?: string }) | null
      if (!raw) return ctx.text('invalid JSON', 400)
      const { expected_updated_at: expected, ...body } = raw
      // The proposed guard (ledger V2), as budgets' `expected_updated_at`.
      if (expected !== tiers.updated_at) return ctx.text('tiers changed elsewhere', 409)
      if (Object.keys(tierProblems(body)).length)
        return ctx.text(
          'invalid tiers: whole numbers ≥ 1, low ≤ medium ≤ high, pool ≥ always-kept',
          400,
        )
      tiers = { ...structuredClone(body), updated_at: new Date().toISOString() }
      return HttpResponse.json(tiers)
    }),
    http.post('/api/me/context-preview', async ({ request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.classic()) return noRoute(request)
      const body = (await request.json().catch(() => null)) as { strategy?: unknown } | null
      const s = body?.strategy
      if (typeof s !== 'string' || !(STRATEGIES as readonly string[]).includes(s))
        return ctx.text(rejection('strategy', s, STRATEGY_ENUM), 422)
      const me = ctx.me().id
      const latest = ctx
        .chats()
        .filter((c) => !c.user_id || c.user_id === me)
        .sort((a, b) =>
          (b.updated_at ?? b.created_at).localeCompare(a.updated_at ?? a.created_at),
        )[0]
      const all = latest ? ctx.messages(latest.session_id) : []
      if (!latest || all.length === 0)
        return HttpResponse.json({ error: 'no chats yet', code: 'no_session' }, { status: 404 })
      // The saved tiers, as the server reads its own (review: Codex).
      const pool = all.slice(-tiers.pool_size)
      return HttpResponse.json({
        strategy: s,
        session_messages: all.length,
        pool: { messages: pool.length, tokens: pool.reduce((n, m) => n + tokensOf(m), 0) },
        tiers: Object.fromEntries(LEVELS.map((l) => [l, selectFrom(pool, s as Strategy, l)])),
      })
    }),
  ]
}
