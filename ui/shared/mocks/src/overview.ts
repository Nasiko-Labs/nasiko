/**
 * The Overview's proposed endpoints (docs/requirements/overview.md OV-1..OV-3), mocked over the other mocks' state so
 * every name on the map is one the rest of the app shows: the agents mock (seed agents, owners, statuses), the harness
 * seed (people, teams, coding harnesses and their sessions), the MCP mock (tools), the router mock (routers), the
 * workflows mock (workflows and their steps) and the TokenOps seed (spend, models).
 *
 * The problems, cause chains and opportunities are a fixed, hand-written set bound to those nodes by name, with dates
 * relative to "now", so the demo tells one story: a broken jira connection stalling support, an expired finance-ledger
 * sign-in, a GPU node down behind a router with no fallback, a slower Research Agent, and two teams heading over
 * budget. A problem whose node is missing (a variant, a test seed) is left out.
 *
 * State: OV-2 writes (owner, snooze, handled) and OV-3 pins, reset with the other mocks. Variants:
 * `overview-absent` (bare 404s, the page builds its fallback map), `overview-quiet` (no problems: the healthy state) and
 * `village-unlimited` (a paid plan: every village plot open).
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type { MockAgent } from './agents'
import type { Seed } from './seed'
import { ADMIN_ID, HARNESS_META, type HarnessSeed } from './seed-harness'

export interface OverviewMockCtx {
  loggedIn(): boolean
  me(): { id: string; is_superuser: boolean }
  now(): number
  absent(): boolean
  quiet(): boolean
  /** The workspace is on a paid plan (`village-unlimited`). */
  unlimited(): boolean
  agents(): readonly MockAgent[]
  harness(): HarnessSeed
  seed(): Seed
  /** Composio toolkits have no page of their own; MCP servers do. */
  connectors(): readonly {
    id: string
    name: string
    display_name: string | null
    toolkit: boolean
  }[]
  /** Connector ids an agent has enabled. */
  toolsOf(agentId: string): string[]
  routers(): readonly {
    id: string
    name: string
    provider: string
    model: string | null
    fallback: number
  }[]
  routerOf(agent: MockAgent): string | null
  workflows(): readonly { id: string; name: string; owner: string; steps: { agent_id: string }[] }[]
  unauthorized(): Response
}

const DAY = 86_400_000
// ponytail: the lab's projects mock seeds these; no projects feature here yet, so the village's two projects keep its ids.
const SEED_PROJECT_IDS = {
  ops: '5eed0015-0000-4000-8000-000000000001',
  cost: '5eed0015-0000-4000-8000-000000000002',
}
const day = (ms: number) => new Date(ms).toISOString().slice(0, 10)

interface ProblemState {
  owner_id: string | null
  snoozed_until: string | null
  handled_at: string | null
}
let problemState = new Map<string, ProblemState>()
let pins: string[] | null = null

export function resetOverviewMock() {
  problemState = new Map()
  pins = null
}

/** Brand marks the UI knows, by connector name (Composio toolkits and the seed's MCP servers). */
const BRANDS = ['github', 'gmail', 'slack', 'linear', 'jira', 'notion']
const brandOf = (name: string) =>
  name === 'finance-ledger'
    ? 'oracle'
    : (BRANDS.find((b) => name.toLowerCase().includes(b)) ?? null)
const PROVIDERS: Record<string, { name: string; brand: string | null }> = {
  anthropic: { name: 'Anthropic', brand: 'anthropic' },
  openai: { name: 'OpenAI', brand: 'openai' },
  google: { name: 'Google', brand: 'gemini' },
  custom: { name: 'Self-hosted', brand: null },
  cursor: { name: 'Cursor', brand: 'cursor' },
  windsurf: { name: 'Windsurf', brand: null },
}
const providerOf = (model: string) =>
  model.startsWith('claude')
    ? 'anthropic'
    : model.startsWith('gpt')
      ? 'openai'
      : model.startsWith('gemini')
        ? 'google'
        : model.startsWith('cursor')
          ? 'cursor'
          : model.startsWith('swe')
            ? 'windsurf'
            : 'custom'
const SHORT: Record<string, string> = {
  'gpt-4o': 'GPT-4o',
  'gpt-4o-mini': '4o mini',
  'claude-sonnet-4': 'Sonnet 4',
  'claude-opus-4': 'Opus 4',
  'gemini-2.0-flash': 'Flash',
  'custom-local': 'Llama',
  'gpt-5-codex': 'Codex',
  'cursor-small': 'Cursor',
  'swe-1': 'SWE-1',
}
export const modelId = (model: string) => `model:${model}`
const hash = (s: string) => [...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7)

export function buildEstate(ctx: OverviewMockCtx, range: string) {
  const now = ctx.now()
  const rangeDays = range === '7d' ? 7 : range === '90d' ? 90 : 30
  const since = now - rangeDays * DAY
  const days = Array.from({ length: 14 }, (_, i) => day(now - (13 - i) * DAY))
  const ago = (n: number) => days[Math.max(0, 13 - n)] ?? day(now - n * DAY)
  /** A day as people read it in a sentence ("Oct 12"). */
  const on = (n: number) =>
    new Date(now - n * DAY).toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
      timeZone: 'UTC',
    })
  const hs = ctx.harness()
  const seed = ctx.seed()
  const me = ctx.me().id

  // People and teams: the harness seed's non-root units are the teams; root-unit managers and the admin have none.
  const units = hs.units.filter((u) => u.parent_id && u.name !== 'IT Security')
  const unitIds = new Set(units.map((u) => u.id))
  const users = hs.users.filter((u) => u.is_active && !u.service_account)
  const groupOfUser = (u: (typeof users)[number]) =>
    u.unit_ids.find((id) => unitIds.has(id)) ?? null

  // Agents (not harnesses), as the catalog lists them.
  const all = ctx.agents().filter((a) => !a.deleted && !a.harness)
  const byRaw = new Map(all.map((a) => [a.name, a]))
  const A = (raw: string) => byRaw.get(`seed-${raw}`)?.id
  const traces = seed.traces.filter((t) => t.ts >= since && t.ts <= now)
  const spendBy = new Map<
    string,
    { spend: number; runs: number; models: Map<string, number>; lat: number[] }
  >()
  for (const t of traces) {
    const s = spendBy.get(t.agent_id) ?? {
      spend: 0,
      runs: 0,
      models: new Map<string, number>(),
      lat: [] as number[],
    }
    s.spend += t.cost_usd
    s.runs += 1
    s.models.set(t.model, (s.models.get(t.model) ?? 0) + t.cost_usd)
    if (s.lat.length < 400) s.lat.push(t.latency_ms)
    spendBy.set(t.agent_id, s)
  }
  const p95 = (xs: number[]) => {
    if (!xs.length) return null
    const s = [...xs].sort((a, b) => a - b)
    return s[Math.min(s.length - 1, Math.floor(s.length * 0.95))] ?? null
  }

  // Tools: the MCP page's servers and toolkits, MCP servers first (they are the ones that break).
  const tools = [...ctx.connectors()]
    .sort((a, b) => Number(a.toolkit) - Number(b.toolkit))
    .map((c) => ({
      id: c.id,
      name: c.name,
      brand: brandOf(c.name),
      connector_id: c.toolkit ? null : c.id,
      // Proposed (V-12): weather-api was turned off three days ago (the Map fades it); pdf-tools long ago (left out).
      disabled_at:
        c.name === 'weather-api'
          ? new Date(now - 3 * DAY).toISOString()
          : c.name === 'pdf-tools'
            ? new Date(now - 40 * DAY).toISOString()
            : null,
    }))
  const toolkits = tools.filter((t) => !t.connector_id)
  const toolByName = new Map(tools.map((t) => [t.name, t.id]))
  const T = (name: string) => toolByName.get(name)

  // Routers: the router mock's configs that have a model, then the one direct path.
  const routers = [
    ...ctx
      .routers()
      .filter((r) => r.model && PROVIDERS[providerOf(r.model)])
      .slice(0, 4)
      .map((r) => ({
        id: r.id,
        name: r.name,
        direct: false,
        note: r.fallback
          ? `${r.fallback} fallback model${r.fallback === 1 ? '' : 's'}`
          : 'No fallback',
      })),
    {
      id: 'router:direct',
      name: 'No router',
      direct: true,
      note: 'Coding harnesses call providers directly',
    },
  ]
  const routerIds = new Set(routers.map((r) => r.id))
  const R = (name: string) => routers.find((r) => r.name === name)?.id

  const agents = all.map((a, i) => {
    const s = spendBy.get(a.id)
    const models = [...(s?.models.entries() ?? [])].sort((x, y) => y[1] - x[1]).map(([m]) => m)
    const rid = ctx.routerOf(a)
    const own = ctx.toolsOf(a.id)
    const extra = toolkits.length ? [toolkits[i % toolkits.length]!.id] : []
    return {
      id: a.id,
      name: a.display_name || a.name,
      owner_id: a.owner_id,
      router_id:
        rid && routerIds.has(rid)
          ? rid
          : (routers[i % Math.max(1, routers.length - 1)]?.id ?? null),
      model_ids: (models.length ? models.slice(0, 2) : ['gpt-4o-mini']).map(modelId),
      tool_ids: [...new Set([...own, ...extra])],
      spend_usd: Math.round((s?.spend ?? 0) * 100) / 100,
      runs: s?.runs ?? 0,
      success_pct: s ? 92 + (hash(a.id) % 8) : null,
      p95_ms: p95(s?.lat ?? []),
      idle: ['stopped', 'registered'].includes(a.status),
    }
  })

  // Coding harnesses: one per developer and tool, with their sessions.
  const userById = new Map(users.map((u) => [u.id, u]))
  const first = (id: string) => userById.get(id)?.display_name.split(' ')[0] ?? 'someone'
  const sessionsBy = new Map<string, typeof hs.sessions>()
  for (const x of hs.sessions)
    if (x.ts <= now) sessionsBy.set(x.agent_id, [...(sessionsBy.get(x.agent_id) ?? []), x])
  const harnesses = hs.agents
    .filter((h) => !h.deleted && !h.spoofed)
    .map((h) => {
      const ss = (sessionsBy.get(h.id) ?? []).sort((a, b) => a.ts - b.ts).slice(-30)
      const big =
        [...ss.map((x) => x.tokens)].sort((a, b) => a - b)[Math.floor(ss.length * 0.85)] ?? Infinity
      const inRange = (sessionsBy.get(h.id) ?? []).filter((x) => x.ts >= since)
      const priced = inRange.filter((x) => x.unpriced_turns < x.turns)
      return {
        id: h.id,
        name: `${HARNESS_META[h.harness]?.name ?? h.harness} (${first(h.owner_id)})`,
        harness: h.harness,
        owner_id: h.owner_id,
        model_ids: (HARNESS_META[h.harness]?.models ?? []).map(modelId),
        tool_ids: [T('github'), h.harness === 'claude' ? T('linear') : T('slack')].filter(
          (x): x is string => !!x,
        ),
        spend_usd: priced.length
          ? Math.round(priced.reduce((n, x) => n + x.cost_usd, 0) * 100) / 100
          : null,
        sessions: ss.map((x) =>
          hash(x.session_id) % 19 === 0
            ? 'failed'
            : x.tokens >= big
              ? h.harness === 'codex'
                ? 'cost'
                : 'slow'
              : 'ok',
        ) as ('ok' | 'failed' | 'slow' | 'cost')[],
      }
    })
    .filter((h) => h.sessions.length > 0)
  const H = (owner: string, harness: string) =>
    harnesses.find((h) => h.owner_id === owner && h.harness === harness)?.id
  const userByName = new Map(users.map((u) => [u.username, u.id]))
  const P = (username: string) => userByName.get(username)

  // Models: the ones agents and harnesses use.
  const modelSpend = new Map<string, { spend: number; calls: number; lat: number[] }>()
  for (const t of traces) {
    const m = modelSpend.get(t.model) ?? { spend: 0, calls: 0, lat: [] as number[] }
    m.spend += t.cost_usd
    m.calls += 1
    if (m.lat.length < 400) m.lat.push(t.latency_ms)
    modelSpend.set(t.model, m)
  }
  const modelNames = [
    ...new Set([...agents.flatMap((a) => a.model_ids), ...harnesses.flatMap((h) => h.model_ids)]),
  ].map((id) => id.slice('model:'.length))
  const models = modelNames.map((m) => ({
    id: modelId(m),
    name: m,
    short: SHORT[m] ?? m,
    provider_id: `provider:${providerOf(m)}`,
    spend_usd: Math.round((modelSpend.get(m)?.spend ?? 0) * 100) / 100,
    calls: modelSpend.get(m)?.calls ?? 0,
    p95_ms: p95(modelSpend.get(m)?.lat ?? []),
  }))
  const totalModelSpend = models.reduce((n, m) => n + m.spend_usd, 0) || 1
  const providers = [...new Set(models.map((m) => m.provider_id))].map((id) => {
    const key = id.slice('provider:'.length)
    return {
      id,
      name: PROVIDERS[key]?.name ?? key,
      brand: PROVIDERS[key]?.brand ?? null,
      // Proposed (V-7): the org's Claude Code seats are on the same Anthropic account as its agents.
      billed_with_harnesses: key === 'anthropic',
      share_pct: Math.round(
        (models.filter((m) => m.provider_id === id).reduce((n, m) => n + m.spend_usd, 0) /
          totalModelSpend) *
          100,
      ),
    }
  })
  const M = (m: string) => (models.some((x) => x.id === modelId(m)) ? modelId(m) : undefined)

  // Workflows: the workflows mock's active ones.
  const workflows = ctx
    .workflows()
    .slice(0, 4)
    .map((w, i) => ({
      id: w.id,
      name: w.name,
      owner_id: w.owner,
      runs: 30 + (hash(w.id) % 9) * 23,
      steps: [
        ...w.steps.slice(0, 3).map((s) => ({
          kind: 'agent' as const,
          ref: s.agent_id,
          label: all.find((a) => a.id === s.agent_id)?.display_name ?? 'Agent',
        })),
        ...(i === 0 ? [{ kind: 'approval' as const, ref: null, label: 'Human approval' }] : []),
      ].slice(0, 4),
    }))
  const W = (i: number) => workflows[i]?.id

  // People's figures: their harness and agent spend, sessions and a segment.
  const groups = units.map((u, i) => ({
    id: u.id,
    name: u.name,
    budget: null as null | {
      limit_usd: number
      spent_usd: number
      projected_usd: number
      harness_usd: number
      hard: boolean
    },
    _i: i,
  }))
  const people = users.map((u) => {
    const own = harnesses.filter((h) => h.owner_id === u.id)
    const harnessSpend = own.reduce((n, h) => n + (h.spend_usd ?? 0), 0)
    const agentSpend = agents
      .filter((a) => a.owner_id === u.id)
      .reduce((n, a) => n + a.spend_usd, 0)
    const sessions = own.reduce((n, h) => n + h.sessions.length, 0) + (u.id === ADMIN_ID ? 120 : 0)
    const h = hash(u.id)
    return {
      id: u.id,
      name: u.display_name,
      group_id: groupOfUser(u),
      spend_usd: Math.round((harnessSpend + agentSpend) * 100) / 100,
      sessions,
      success_pct: sessions ? 88 + (h % 12) : null,
      avg_context_tokens: sessions ? 40_000 + (h % 9) * 30_000 : null,
      segment: (sessions < 20 ? 'light' : h % 7 === 0 ? 'expensive' : 'value') as
        'value' | 'expensive' | 'stuck' | 'light',
    }
  })
  // The people the broken agents block are stuck (the jira and finance-ledger chains end on them).
  const stuckIds = new Set([
    userByName.get('amara.mensah'),
    all.find((a) => a.name === 'seed-pricing-engine')?.owner_id,
    userByName.get('ravi.becker'),
  ])
  for (const p of people) if (stuckIds.has(p.id) && p.sessions) p.segment = 'stuck'
  // Budgets: each team's month so far against a limit; Platform and Research run over.
  const monthDay = new Date(now).getUTCDate()
  const monthLen = new Date(
    Date.UTC(new Date(now).getUTCFullYear(), new Date(now).getUTCMonth() + 1, 0),
  ).getUTCDate()
  for (const g of groups) {
    const members = people.filter((p) => p.group_id === g.id)
    const spent = Math.max(
      4,
      members.reduce((n, p) => n + p.spend_usd, 0) * (monthDay / Math.max(1, rangeDays)),
    )
    const harnessSpent =
      harnesses
        .filter((h) => members.some((m) => m.id === h.owner_id))
        .reduce((n, h) => n + (h.spend_usd ?? 0), 0) *
      (monthDay / Math.max(1, rangeDays))
    const projected = (spent / Math.max(1, monthDay)) * monthLen
    const over =
      g.name === 'Platform' ? 1.2 : g.name === 'Research' ? 1.1 : 0.62 + (hash(g.id) % 30) / 100
    g.budget = {
      limit_usd: Math.max(10, Math.round(projected / over)),
      spent_usd: Math.round(spent * 100) / 100,
      projected_usd: Math.round(projected * 100) / 100,
      harness_usd: Math.round(Math.min(spent, harnessSpent) * 100) / 100,
      hard: hash(g.id) % 2 === 0,
    }
  }
  const G = (name: string) => groups.find((g) => g.name === name)?.id

  // Mine: what the viewer owns and everything those touch.
  const mine = new Set<string>([me])
  const myPerson = people.find((p) => p.id === me)
  if (myPerson?.group_id) mine.add(myPerson.group_id)
  for (const x of [...agents, ...harnesses]) {
    if (x.owner_id !== me) continue
    mine.add(x.id)
    x.model_ids.forEach((m) => {
      mine.add(m)
      const pr = models.find((md) => md.id === m)?.provider_id
      if (pr) mine.add(pr)
    })
    x.tool_ids.forEach((t) => mine.add(t))
    if ('router_id' in x && x.router_id) mine.add(x.router_id)
    if ('harness' in x) mine.add('router:direct')
  }
  for (const w of workflows) if (w.owner_id === me) mine.add(w.id)

  // Problems, chains and opportunities: the demo's story, bound to the nodes above.
  const totalRuns = agents.reduce((n, a) => n + a.runs, 0)
  type Draft = {
    id: string
    kind: 'errors' | 'slow' | 'cost'
    also?: 'errors' | 'slow' | 'cost'
    tier: 'broken' | 'worse'
    title: string
    why: string
    metric: [
      label: string,
      current: number,
      target: number,
      direction: 'higher' | 'lower',
      unit: string,
      min: number,
      max: number,
    ]
    trend: number[]
    since: number
    confidence: 'high' | 'medium'
    evidence: string[]
    fixed: string
    action: [string, { page: string; id?: string | null; query?: Record<string, string> }]
    entities: (string | undefined)[]
    flow: string
    count: number
    chain?: string
    owner?: string
    score: number
  }
  const name = (id: string | undefined) =>
    (id ? all.find((a) => a.id === id)?.display_name : undefined) ?? ''
  const code = A('code-reviewer')
  const docs = A('doc-writer')
  const support = A('support-bot')
  const triage = A('triage-router')
  const pricing = A('pricing-engine')
  const research = A('research-agent')
  const sales = A('sales-assistant')
  const pricingOwner = all.find((a) => a.id === pricing)?.owner_id ?? undefined
  const drafts: Draft[] = [
    {
      id: 'ov-p1',
      kind: 'errors',
      tier: 'broken',
      title: `${name(code)} has crashed 3 times since ${on(2)}`,
      why: 'Pull requests wait for review while it restarts; each crash drops the run it was on.',
      metric: ['Replicas running', 0, 1, 'higher', '', 0, 1],
      trend: [1, 1, 1, 1, 1, 1, 1, 0, 0, 0],
      since: 2,
      confidence: 'high',
      evidence: [
        'Out of memory on the 4 GB container while loading large diffs',
        'Restart count 3 on the current deployment',
      ],
      fixed: 'Running and passing health checks for a day',
      action: ['Open the agent', { page: 'agent', id: code }],
      entities: [code, M('gpt-4o'), T('github')],
      flow: 'people-agents',
      count: 3,
      owner: ADMIN_ID,
      score: 148,
    },
    {
      id: 'ov-p2',
      kind: 'errors',
      tier: 'broken',
      title: `The jira-cloud tool fails 18% of calls`,
      why: `${name(support)} and ${name(triage)} depend on it, so escalations stall.`,
      metric: ['Tool call errors', 18, 1, 'lower', '%', 0, 25],
      trend: [0.4, 0.3, 0.5, 0.4, 0.6, 14, 19, 17, 18, 18],
      since: 2,
      confidence: 'high',
      evidence: [
        'HTTP 503 from the jira API since 11:20 two days ago',
        'Two agents retry automatically, which multiplies the calls',
      ],
      fixed: 'Under 1% for 24 hours',
      action: ['Open the MCP server', { page: 'connector', id: T('jira-cloud') }],
      entities: [T('jira-cloud'), support, triage, W(0), P('amara.mensah')],
      flow: 'agents-tools',
      count: 45,
      chain: 'ov-c1',
      score: 141,
    },
    {
      id: 'ov-p3',
      kind: 'errors',
      tier: 'broken',
      title: `${name(pricing)} fails 28% of runs`,
      why: 'Quotes are not going out: 41 failed runs since the finance-ledger sign-in expired.',
      metric: ['Success rate', 72, 95, 'higher', '%', 50, 100],
      trend: [96, 97, 96, 95, 96, 90, 79, 74, 72, 72],
      since: 4,
      confidence: 'high',
      evidence: [
        'Failures start an hour after the finance-ledger token expired',
        'Every failure is an auth error on the finance-ledger tool',
      ],
      fixed: 'Success above 95% for 3 days',
      action: ['See failed sessions', { page: 'sessions', query: { lane: 'failing' } }],
      entities: [pricing, T('finance-ledger'), pricingOwner, W(1)],
      flow: 'agents-tools',
      count: 41,
      chain: 'ov-c2',
      owner: pricingOwner,
      score: 146,
    },
    {
      id: 'ov-p4',
      kind: 'errors',
      tier: 'broken',
      title: `The ${SHORT['custom-local']} model fails 38% of calls`,
      why: `${routers.find((r) => r.id === R('local-models'))?.name ?? 'Its router'} has no fallback, so ${name(sales)} keeps sending traffic to it.`,
      metric: ['Call success', 62, 95, 'higher', '%', 40, 100],
      trend: [99, 99, 98, 99, 99, 99, 99, 63, 61, 62],
      since: 3,
      confidence: 'high',
      evidence: ['GPU node down on the self-hosted cluster', 'The router has no fallback model'],
      fixed: 'Above 95%, or traffic moved to another model',
      action: ['Add a fallback', { page: 'router' }],
      entities: [M('custom-local'), R('local-models'), sales],
      flow: 'agents-models',
      count: 38,
      chain: 'ov-c4',
      score: 139,
    },
    {
      id: 'ov-p5',
      kind: 'errors',
      tier: 'broken',
      title: `${name(docs)}'s last deploy failed`,
      why: 'The agent is down and no earlier version is running.',
      metric: ['Replicas running', 0, 1, 'higher', '', 0, 1],
      trend: [1, 1, 1, 1, 1, 1, 1, 1, 0, 0],
      since: 1,
      confidence: 'high',
      evidence: [
        'The image failed its health check on start',
        'The previous version was replaced, not kept',
      ],
      fixed: 'Running and passing health checks',
      action: ['Open its builds', { page: 'agent', id: docs, query: { tab: 'builds' } }],
      entities: [docs],
      flow: 'people-agents',
      count: 1,
      owner: ADMIN_ID,
      score: 132,
    },
    {
      id: 'ov-p6',
      kind: 'cost',
      tier: 'worse',
      title: 'Platform will finish the month at 120% of budget',
      why: 'Coding-harness spend doubled, and harness spend can only alert, not stop at the cap.',
      metric: ['Projected share of budget', 120, 100, 'lower', '%', 0, 140],
      trend: [60, 68, 72, 80, 88, 95, 104, 110, 116, 120],
      since: 10,
      confidence: 'high',
      evidence: [
        'Most of the overrun is Claude Code and Codex sessions',
        'Two developers carry 300k+ tokens per call',
      ],
      fixed: 'Projection under 100%',
      action: ['Open TokenOps', { page: 'tokenops' }],
      entities: [G('Platform'), H(P('kwame.rossi') ?? '', 'codex'), H(P('sam') ?? '', 'claude')],
      flow: 'people-harnesses',
      count: 1,
      score: 98,
    },
    {
      id: 'ov-p7',
      kind: 'cost',
      also: 'errors',
      tier: 'worse',
      title: `${name(research)} costs 2.4× more per successful run`,
      why: 'Most of the increase is retries, not more work: 31% of runs retry three times or more.',
      metric: ['Cost per successful run vs last month', 2.4, 1.2, 'lower', '×', 0, 3],
      trend: [1, 1, 1.1, 1, 1.2, 1.6, 1.9, 2.2, 2.3, 2.4],
      since: 8,
      confidence: 'medium',
      evidence: ['Retries per run rose from 0.3 to 2.1', 'Retries cluster on the docs-search step'],
      fixed: 'Back to last month’s cost per successful run',
      action: ['Inspect retries', { page: 'sessions', query: { sort: 'cost' } }],
      entities: [research, T('docs-search'), G('Research')],
      flow: 'agents-tools',
      count: 31,
      chain: 'ov-c3',
      score: 92,
    },
    {
      id: 'ov-p8',
      kind: 'cost',
      tier: 'worse',
      title: 'Research will finish the month at 110% of budget',
      why: `${name(research)}'s retries are costing 2.4× per successful run.`,
      metric: ['Projected share of budget', 110, 100, 'lower', '%', 0, 140],
      trend: [70, 74, 80, 85, 90, 96, 100, 104, 107, 110],
      since: 8,
      confidence: 'medium',
      evidence: ['Most of the extra spend traces to retried docs-search calls'],
      fixed: 'Projection under 100%',
      action: ['Open TokenOps', { page: 'tokenops' }],
      entities: [G('Research'), research],
      flow: 'people-agents',
      count: 1,
      chain: 'ov-c3',
      score: 90,
    },
    {
      id: 'ov-p9',
      kind: 'slow',
      tier: 'worse',
      title: `${name(research)} p95 is 7.2 s, over the 5 s target`,
      why: 'People wait 65% longer since its latest version started loading whole documents.',
      metric: ['p95 latency', 7.2, 5, 'lower', 's', 0, 9],
      trend: [4.3, 4.4, 4.2, 4.5, 4.4, 6.8, 7.1, 7, 7.3, 7.2],
      since: 12,
      confidence: 'high',
      evidence: [
        'A new version shipped the day it started',
        'Context per call went from 61k to 148k tokens',
      ],
      fixed: 'p95 under 5 s for 3 days',
      action: ['Open the agent', { page: 'agent', id: research }],
      entities: [research, M('claude-sonnet-4')],
      flow: 'agents-models',
      count: 1,
      chain: 'ov-c5',
      owner: ADMIN_ID,
      score: 86,
    },
    {
      id: 'ov-p10',
      kind: 'slow',
      also: 'errors',
      tier: 'worse',
      title: 'docs-search times out on 9% of calls',
      why: `This drives ${name(research)}'s retries and slows the workflows that use it.`,
      metric: ['Tool call timeouts', 9, 1, 'lower', '%', 0, 12],
      trend: [1, 1, 1.2, 1, 2, 4, 6, 8, 9, 9],
      since: 8,
      confidence: 'medium',
      evidence: ['Timeouts at the 30 s tool limit', 'They start the same day the retries rise'],
      fixed: 'Under 1% for 3 days',
      action: ['Open the MCP server', { page: 'connector', id: T('docs-search') }],
      entities: [T('docs-search'), research, W(2)],
      flow: 'agents-tools',
      count: 12,
      chain: 'ov-c3',
      score: 84,
    },
    {
      id: 'ov-p11',
      kind: 'cost',
      also: 'slow',
      tier: 'worse',
      title: `47% of ${harnesses.find((h) => h.id === H(ADMIN_ID, 'codex'))?.name ?? 'Codex'} calls run above 150k context`,
      why: 'Long calls cost about 2.4× as much and run 3× slower. Token optimization is off for this harness.',
      metric: ['Calls above 150k context', 47, 20, 'lower', '%', 0, 60],
      trend: [22, 25, 28, 31, 35, 38, 42, 44, 46, 47],
      since: 13,
      confidence: 'medium',
      evidence: [
        'p95 context is 419k tokens per call',
        'Unused tool definitions add 27k tokens to every call',
      ],
      fixed: 'Below 20% of calls',
      action: ['Turn on optimization', { page: 'optimization' }],
      entities: [H(ADMIN_ID, 'codex'), M('gpt-5-codex')],
      flow: 'harnesses-models',
      count: 47,
      owner: ADMIN_ID,
      score: 80,
    },
    {
      id: 'ov-p12',
      kind: 'slow',
      tier: 'worse',
      title: 'gpt-5-codex rate-limits 2.1% of calls',
      why: 'Codex sessions stall and retry during working hours.',
      metric: ['Calls rate limited', 2.1, 0.5, 'lower', '%', 0, 3],
      trend: [0.2, 0.3, 0.2, 0.4, 0.6, 0.9, 1.2, 1.6, 1.9, 2.1],
      since: 8,
      confidence: 'high',
      evidence: ['429 responses from OpenAI between 10:00 and 16:00'],
      fixed: 'Under 0.5% for a week',
      action: ['Review limits', { page: 'router' }],
      entities: [M('gpt-5-codex'), H(ADMIN_ID, 'codex')],
      flow: 'harnesses-models',
      count: 6,
      score: 70,
    },
    {
      id: 'ov-p13',
      kind: 'cost',
      tier: 'worse',
      title: `${name(triage)} costs 95% more than last week`,
      why: 'Same traffic, longer prompts: its system prompt doubled in its last edit.',
      metric: ['Spend vs last week', 195, 150, 'lower', '%', 0, 220],
      trend: [100, 104, 99, 110, 120, 150, 170, 185, 190, 195],
      since: 6,
      confidence: 'medium',
      evidence: [
        'Prompt tokens per call went from 3.1k to 6.4k',
        'Operations are flat week over week',
      ],
      fixed: 'Within 50% of last week',
      action: ['See its spend', { page: 'tokenops', query: { agent: triage ?? '' } }],
      entities: [triage, R('anthropic-default')],
      flow: 'agents-models',
      count: 1,
      score: 74,
    },
    {
      id: 'ov-p14',
      kind: 'cost',
      tier: 'worse',
      title: 'Harness spend skips every router, so caps can’t apply',
      why: 'Coding harnesses go straight from laptops to providers; budgets can only alert on it.',
      metric: ['Harness spend under a cap', 0, 100, 'higher', '%', 0, 100],
      trend: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
      since: 13,
      confidence: 'high',
      evidence: [
        `${harnesses.length} harnesses, ${new Set(harnesses.map((h) => h.owner_id)).size} developers`,
        'Hard caps need traffic to pass through a router',
      ],
      fixed: 'An allowance per developer',
      action: ['Open TokenOps', { page: 'tokenops' }],
      entities: ['router:direct', ...harnesses.slice(0, 3).map((h) => h.id)],
      flow: 'harnesses-models',
      count: harnesses.length,
      score: 66,
    },
  ]
  const nodeIds = new Set<string>([
    ...groups.map((g) => g.id),
    ...people.map((p) => p.id),
    ...workflows.map((w) => w.id),
    ...routers.map((r) => r.id),
    ...models.map((m) => m.id),
    ...providers.map((p) => p.id),
    ...agents.map((a) => a.id),
    ...tools.map((t) => t.id),
    ...harnesses.map((h) => h.id),
  ])
  const problems = ctx.quiet()
    ? []
    : drafts
        .filter((d) => d.entities[0] && nodeIds.has(d.entities[0]))
        .map((d) => {
          const entities = [
            ...new Set(d.entities.filter((x): x is string => !!x && nodeIds.has(x))),
          ]
          const st = problemState.get(d.id)
          const [label, current, target, direction, unit, min, max] = d.metric
          return {
            id: d.id,
            kind: d.kind,
            also: d.also ?? null,
            tier: d.tier,
            title: d.title,
            why: d.why,
            metric: { label, current, target, direction, unit, min, max },
            trend: d.trend,
            since: ago(d.since),
            confidence: d.confidence,
            evidence: d.evidence,
            fixed_when: d.fixed,
            action: { label: d.action[0], target: { id: null, query: null, ...d.action[1] } },
            entities,
            flow_id: d.flow,
            count: d.count,
            chain_id: d.chain ?? null,
            owner_id: st ? st.owner_id : (d.owner ?? null),
            snoozed_until: st?.snoozed_until ?? null,
            handled_at: st?.handled_at ?? null,
            score: d.score,
            // Mine: about something the viewer owns, or theirs to act on.
            mine:
              (!!entities[0] &&
                mine.has(entities[0]) &&
                !entities[0].startsWith('model:') &&
                !tools.some((t) => t.id === entities[0])) ||
              (st ? st.owner_id : (d.owner ?? null)) === me,
          }
        })
  const present = new Set(problems.map((p) => p.chain_id))
  const step = (entity: string | undefined, what: string, detail: string) =>
    entity && nodeIds.has(entity) ? [{ entity, what, detail }] : []
  const chains = [
    {
      id: 'ov-c1',
      title: 'Why support escalations are stalling',
      confidence: 'high' as const,
      steps: [
        ...step(T('jira-cloud'), 'jira-cloud returns 503s', `Since ${on(2)}, 18% of calls fail`),
        ...step(
          support,
          `${name(support)} retries and fails`,
          '45 failed calls with Triage Router',
        ),
        ...step(
          W(0),
          `${workflows[0]?.name ?? 'Ticket resolution'} waits on failed lookups`,
          'Approvals pile up',
        ),
        ...step(P('amara.mensah'), 'Amara Mensah is stuck', '76% session success'),
      ],
      fix: 'Reconnect jira-cloud, or pause retries until it recovers. Clears 2 problems.',
      clears: 2,
      action: {
        label: 'Open jira-cloud',
        target: { page: 'connector' as const, id: T('jira-cloud') ?? null, query: null },
      },
    },
    {
      id: 'ov-c2',
      title: `Why ${name(pricing)} is failing`,
      confidence: 'high' as const,
      steps: [
        ...step(
          T('finance-ledger'),
          'The finance-ledger sign-in expired',
          `On ${on(4)}; every call fails auth`,
        ),
        ...step(pricing, `${name(pricing)} fails 28% of runs`, '41 failed runs'),
        ...step(
          W(1),
          `${workflows[1]?.name ?? 'Quarterly report'} stalls at its first step`,
          '4 runs blocked',
        ),
        ...step(
          pricingOwner,
          `${people.find((p) => p.id === pricingOwner)?.name ?? 'Its owner'} is stuck`,
          '74% session success',
        ),
      ],
      fix: 'Sign in to finance-ledger again. That one action clears the problem and unblocks the workflow.',
      clears: 1,
      action: {
        label: 'Sign in again',
        target: { page: 'connector' as const, id: T('finance-ledger') ?? null, query: null },
      },
    },
    {
      id: 'ov-c3',
      title: 'Why Research is over budget',
      confidence: 'medium' as const,
      steps: [
        ...step(T('docs-search'), 'docs-search times out on 9% of calls', `Since ${on(8)}`),
        ...step(research, `${name(research)} retries the search step`, '2.1 retries per run'),
        ...step(G('Research'), 'Cost per successful run rises 2.4×', 'Research on pace for 110%'),
      ],
      fix: 'Raise the docs-search timeout or switch it to a faster index. Clears 3 problems.',
      clears: 3,
      action: {
        label: 'Open docs-search',
        target: { page: 'connector' as const, id: T('docs-search') ?? null, query: null },
      },
    },
    {
      id: 'ov-c4',
      title: `Why ${name(sales)} is failing`,
      confidence: 'high' as const,
      steps: [
        ...step(M('custom-local'), 'The self-hosted GPU node is down', '38% of calls fail'),
        ...step(
          R('local-models'),
          'local-models has no fallback',
          'It keeps routing to the same model',
        ),
        ...step(sales, `${name(sales)} fails 38% of runs`, `Since ${on(3)}`),
      ],
      fix: 'Add a fallback model on local-models. Clears the problem while the node is down.',
      clears: 1,
      action: {
        label: 'Open the LLM router',
        target: { page: 'router' as const, id: null, query: null },
      },
    },
    {
      id: 'ov-c5',
      title: `Why ${name(research)} got slower`,
      confidence: 'high' as const,
      steps: [
        ...step(research, 'Its new version loads whole documents', `Shipped ${on(12)}`),
        ...step(
          M('claude-sonnet-4'),
          'Each call carries 148k tokens, up from 61k',
          'Sonnet calls take longer',
        ),
        ...step(research, 'p95 latency reaches 7.2 s', 'Over the 5 s target'),
      ],
      fix: 'Roll back to the previous version, or trim the documents it retrieves.',
      clears: 1,
      action: {
        label: 'Open the agent',
        target: { page: 'agent' as const, id: research ?? null, query: null },
      },
    },
  ].filter((c) => present.has(c.id) && c.steps.length >= 2)
  const opportunities = ctx.quiet()
    ? []
    : [
        {
          id: 'ov-o1',
          title: 'Route simple traffic to a smaller model',
          detail: `82% of ${routers[0]?.name ?? 'the default router'}'s calls are simple but run on Sonnet.`,
          saving_usd: 6.4,
          method: 'Calls classified simple × (Sonnet price − Haiku price).',
          effort: 'One setting',
          entities: [routers[0]?.id],
          action: {
            label: 'Open the LLM router',
            target: { page: 'router', id: null, query: null },
          },
        },
        {
          id: 'ov-o2',
          title: 'Turn on token optimization for coding harnesses',
          detail: 'The heaviest callers carry 300k+ tokens per call with compression off.',
          saving_usd: 5.1,
          method: 'Billed input tokens × 38% median reduction × input price.',
          effort: 'Per developer',
          entities: harnesses
            .filter((h) => h.harness === 'codex')
            .slice(0, 3)
            .map((h) => h.id),
          action: { label: 'Review', target: { page: 'optimization', id: null, query: null } },
        },
        {
          id: 'ov-o3',
          title: 'Remove the unused weather-api tool',
          detail: 'Its tool definitions ride along on every call; none were used in 7 days.',
          saving_usd: 1.9,
          method: 'Tool-definition tokens × calls × input price.',
          effort: 'One setting',
          entities: [T('weather-api')],
          action: {
            label: 'Open weather-api',
            target: { page: 'connector', id: T('weather-api') ?? null, query: null },
          },
        },
        {
          id: 'ov-o4',
          title: `Fix the prompt-cache miss on ${name(support)}`,
          detail:
            'Its prompt now starts with a timestamp, so the cached prefix changes every call.',
          saving_usd: 1.2,
          method: '(94% − 61%) × cached tokens × (input − cache-read price).',
          effort: 'Prompt edit',
          entities: [support],
          action: {
            label: 'Open the agent',
            target: { page: 'agent', id: support ?? null, query: null },
          },
        },
      ]
        .map((o) => ({
          ...o,
          entities: o.entities.filter((x): x is string => !!x && nodeIds.has(x)),
        }))
        .filter((o) => o.entities.length)

  // Flows between the cards: volume over the range.
  const harnessCalls = harnesses.reduce((n, h) => n + h.sessions.length * 18, 0)
  const toolCalls = agents.reduce((n, a) => n + a.runs * a.tool_ids.length, 0)
  const flows = [
    {
      id: 'people-workflows',
      from: 'people',
      to: 'workflows',
      volume: workflows.reduce((n, w) => n + w.runs, 0),
      unit: 'workflow runs',
      spend_usd: null,
      direct: false,
    },
    {
      id: 'people-agents',
      from: 'people',
      to: 'agents',
      volume: totalRuns,
      unit: 'agent runs',
      spend_usd: null,
      direct: false,
    },
    {
      id: 'people-harnesses',
      from: 'people',
      to: 'harnesses',
      volume: harnesses.reduce((n, h) => n + h.sessions.length, 0),
      unit: 'harness sessions',
      spend_usd: null,
      direct: false,
    },
    {
      id: 'workflows-agents',
      from: 'workflows',
      to: 'agents',
      volume: workflows.reduce((n, w) => n + w.runs * w.steps.length, 0),
      unit: 'workflow steps',
      spend_usd: null,
      direct: false,
    },
    {
      id: 'agents-models',
      from: 'agents',
      to: 'models',
      volume: totalRuns * 3,
      unit: 'LLM calls',
      spend_usd: Math.round(agents.reduce((n, a) => n + a.spend_usd, 0) * 100) / 100,
      direct: false,
    },
    {
      id: 'harnesses-models',
      from: 'harnesses',
      to: 'models',
      volume: harnessCalls,
      unit: 'LLM calls',
      spend_usd: Math.round(harnesses.reduce((n, h) => n + (h.spend_usd ?? 0), 0) * 100) / 100,
      direct: true,
    },
    {
      id: 'agents-tools',
      from: 'agents',
      to: 'tools',
      volume: toolCalls,
      unit: 'tool calls',
      spend_usd: null,
      direct: false,
    },
    {
      id: 'harnesses-tools',
      from: 'harnesses',
      to: 'tools',
      volume: Math.round(harnessCalls * 0.4),
      unit: 'tool calls',
      spend_usd: null,
      direct: true,
    },
  ]

  const teamOfPerson = new Map(people.map((p) => [p.id, p.group_id]))
  const teamsOf = (id: string, owner: string | null) => {
    if (!groups.length) return []
    const own = owner ? teamOfPerson.get(owner) : null
    const h = hash(id)
    const picks = [own, groups[h % groups.length]!.id]
    if (h % 3 === 0) picks.push(groups[Math.floor(h / 8) % groups.length]!.id)
    return [...new Set(picks.filter((x): x is string => !!x))]
  }
  // Proposed (V-8): two projects own three of the workflows; Contract intake has none. Platform operations is the
  // org's; the cost review belongs to Platform.
  const projects = [
    {
      id: SEED_PROJECT_IDS.ops,
      name: 'Platform operations',
      owner_id: me,
      group_id: null,
      workflow_ids: [W(0), W(3)].filter((x): x is string => !!x),
    },
    {
      id: SEED_PROJECT_IDS.cost,
      name: 'Q4 cloud cost review',
      owner_id: me,
      group_id: G('Platform') ?? null,
      workflow_ids: [W(1)].filter((x): x is string => !!x),
    },
  ]
  const projectOf = new Map(projects.flatMap((p) => p.workflow_ids.map((w) => [w, p.id])))
  // Proposed (V-6): what each active person is on right now, by a stable hash: mostly their team's work, some at an
  // agent, a router or a project.
  const busy = agents.filter((a) => !a.idle)
  const routed = routers.filter((r) => !r.direct)
  const workingOn = (id: string, sessions: number) => {
    if (!sessions) return null
    const h = hash(`${id}:now`)
    const k = h % 20
    if (k < 10) return { kind: 'team' as const, id: teamOfPerson.get(id) ?? '' }
    if (k < 15 && busy.length) return { kind: 'agent' as const, id: busy[h % busy.length]!.id }
    if (k < 17 && routed.length)
      return { kind: 'router' as const, id: routed[h % routed.length]!.id }
    return { kind: 'project' as const, id: projects[h % projects.length]!.id }
  }
  const myAgents = agents.filter((a) => a.owner_id === me)
  const mySpend =
    myAgents.reduce((n, a) => n + a.spend_usd, 0) * 0.08 +
    harnesses.filter((h) => h.owner_id === me).reduce((n, h) => n + (h.spend_usd ?? 0), 0)
  // Proposed (V-10): what each agent's failed runs cost, its spend times its failure share; an agent a problem is about
  // fails as that problem says (Pricing Engine's success rate).
  const failing = new Map(
    problems.flatMap((p) =>
      p.entities[0] && p.metric.label === 'Success rate'
        ? [[p.entities[0], p.metric.current] as const]
        : [],
    ),
  )
  const failedSpend = (a: (typeof agents)[number]) => {
    const success = failing.get(a.id) ?? a.success_pct
    return success === null ? null : Math.round(a.spend_usd * (100 - success)) / 100
  }
  // Proposed (V-11): the org's spend per day, from the same traces and harness sessions as the figures above, split
  // by whether the person who spent it is in a team with a budget (every team has one here). Saved follows the agents'
  // spend at the savings mock's own rate ($91 saved on $740, aggregate.ts `savings`).
  const funded = new Set(groups.filter((g) => g.budget).map((g) => g.id))
  const fundedPerson = new Set(
    people.filter((p) => p.group_id && funded.has(p.group_id)).map((p) => p.id),
  )
  const ownerOfAgent = new Map(all.map((a) => [a.id, a.owner_id]))
  const ownerOfHarness = new Map(hs.agents.map((h) => [h.id, h.owner_id]))
  const daily = days.map(() => ({ budgeted_usd: 0, unbudgeted_usd: 0, uncapped_usd: 0, agents: 0 }))
  const dayIndex = new Map(days.map((d, i) => [d, i]))
  for (const t of seed.traces) {
    const row = daily[dayIndex.get(day(t.ts)) ?? -1]
    if (!row || t.ts > now) continue
    const owner = ownerOfAgent.get(t.agent_id)
    if (owner && fundedPerson.has(owner)) row.budgeted_usd += t.cost_usd
    else row.unbudgeted_usd += t.cost_usd
    row.agents += t.cost_usd
  }
  for (const x of hs.sessions) {
    const row = daily[dayIndex.get(day(x.ts)) ?? -1]
    if (!row || x.ts > now || x.unpriced_turns >= x.turns) continue
    const owner = ownerOfHarness.get(x.agent_id)
    if (owner && fundedPerson.has(owner)) {
      row.budgeted_usd += x.cost_usd
      row.uncapped_usd += x.cost_usd
    } else row.unbudgeted_usd += x.cost_usd
  }
  const cents = (v: number) => Math.round(v * 100) / 100
  return {
    as_of: new Date(now).toISOString(),
    // Proposed (V-9): the workspace's plan, which sets the village's open plots.
    plan: ctx.unlimited() ? 'team' : 'free',
    days,
    me,
    mine: [...mine],
    groups: groups.map(({ _i: _, ...g }) => g),
    people: people.map((p) => ({ ...p, working_on: workingOn(p.id, p.sessions) })),
    workflows: workflows.map((w) => ({ ...w, project_id: projectOf.get(w.id) ?? null })),
    projects,
    routers,
    providers,
    models,
    // Proposed (plans/feat-village-overview.md V-1): the teams whose people ran each agent in the range, so the
    // village knows which camps a troop walks to. The owner's team first, then one or two more by a stable hash.
    agents: agents.map((a) => ({
      ...a,
      team_ids: teamsOf(a.id, a.owner_id),
      failed_spend_usd: failedSpend(a),
    })),
    tools,
    harnesses,
    flows,
    problems,
    chains,
    opportunities,
    daily: daily.map((r) => ({
      budgeted_usd: cents(r.budgeted_usd),
      unbudgeted_usd: cents(r.unbudgeted_usd),
      uncapped_usd: cents(r.uncapped_usd),
      saved_usd: cents((r.agents * 91) / 740),
    })),
    allowance: {
      limit_usd: 16,
      spent_usd:
        Math.round(Math.min(15, mySpend * (monthDay / Math.max(1, rangeDays))) * 100) / 100 || 7.55,
      projected_usd: 17.9,
    },
  }
}

export function overviewHandlers(ctx: OverviewMockCtx): HttpHandler[] {
  const absent = () => new HttpResponse(null, { status: 404 })
  return [
    // OV-1 (proposed).
    http.get('/api/overview/estate', ({ request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.absent()) return absent()
      const range = new URL(request.url).searchParams.get('range') ?? '30d'
      return HttpResponse.json({ data: buildEstate(ctx, range) })
    }),
    // OV-2 (proposed): one change at a time; the problem as it now stands.
    http.patch('/api/overview/problems/:id', async ({ params, request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.absent()) return absent()
      const id = String(params.id)
      const body = (await request.json()) as {
        owner_id?: string | null
        snoozed_until?: string | null
        handled?: boolean
      }
      const current = buildEstate(ctx, '30d').problems.find((p) => p.id === id)
      if (!current)
        return new HttpResponse('no such problem', {
          status: 404,
          headers: { 'Content-Type': 'text/plain' },
        })
      const st: ProblemState = problemState.get(id) ?? {
        owner_id: current.owner_id,
        snoozed_until: current.snoozed_until,
        handled_at: current.handled_at,
      }
      if ('owner_id' in body) st.owner_id = body.owner_id ?? null
      if ('snoozed_until' in body) st.snoozed_until = body.snoozed_until ?? null
      if ('handled' in body) st.handled_at = body.handled ? new Date(ctx.now()).toISOString() : null
      problemState.set(id, st)
      const next = buildEstate(ctx, '30d').problems.find((p) => p.id === id)
      return HttpResponse.json({ data: next })
    }),
    // OV-3 (proposed): the viewer's pins, in order. `null` until they pin or unpin anything: the UI's defaults apply
    // (an empty list is a choice).
    http.get('/api/me/overview-pins', () => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.absent()) return absent()
      return HttpResponse.json({ pins })
    }),
    http.put('/api/me/overview-pins', async ({ request }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.absent()) return absent()
      const body = (await request.json()) as { pins?: unknown }
      if (!Array.isArray(body.pins) || body.pins.some((p) => typeof p !== 'string'))
        return new HttpResponse('pins must be a list of card ids', {
          status: 422,
          headers: { 'Content-Type': 'text/plain' },
        })
      pins = (body.pins as string[]).slice(0, 12)
      return HttpResponse.json({ pins })
    }),
  ]
}
