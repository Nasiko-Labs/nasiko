/**
 * The proposed agent event log (AH-3 in docs/designs/openruntime-agent-hub-recommendations.md; plans/feat-agent-centric.md
 * PR1a), mocked under the `events` MOCKABLE group: `GET /api/observability/agent/{id}/events?start_time&end_time&limit`, newest
 * first, in the `{data}` envelope, `{events, truncated}`. No server has it, so the hub reads it only while this group
 * is a preview (`isPreview('events')`), and `?mock=events-absent` answers the bare 404 a server without it gives.
 *
 * The log is derived from the agents mock (deterministic, anchored to the mock clock): every live agent gets the
 * changes the server doesn't record today (restarts, routing, optimization, the Token optimization switch, MCP rules,
 * budget crossings), spread over the last four weeks so 7d and 30d windows both show some. Harnesses get only the
 * kinds that apply to them. Ids are `5eed0016-*`. Summaries state what changed and never why (Product Principle 1).
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type { AgentEvent } from '@nasiko/features/hub/types'
import type { MockAgent } from './agents'

const DAY = 86_400_000
const HOUR = 3_600_000
const pad = (n: number) => String(n).padStart(12, '0')
const eventId = (n: number) => `5eed0016-0000-4000-8000-${pad(n)}`

interface Template {
  kind: AgentEvent['kind']
  /** Days back from now, and the hour of that UTC day. */
  days: number
  hour: number
  actor: 'owner' | null
  summary: string
  before?: string
  after?: string
  harness?: boolean
}

const TEMPLATES: Template[] = [
  { kind: 'restart', days: 2, hour: 9, actor: 'owner', summary: 'Restarted' },
  {
    kind: 'route_change',
    days: 6,
    hour: 14,
    actor: 'owner',
    summary: 'Routing switched to gpt-4.1-mini',
    before: 'gpt-4o',
    after: 'gpt-4.1-mini',
  },
  {
    kind: 'compress_change',
    days: 11,
    hour: 10,
    actor: 'owner',
    summary: 'Token optimization turned on',
    before: 'off',
    after: 'on',
    harness: true,
  },
  {
    kind: 'mcp_rule',
    days: 15,
    hour: 16,
    actor: 'owner',
    summary: 'MCP tool github.create_issue set to Ask',
    before: 'Allow',
    after: 'Ask',
    harness: true,
  },
  {
    kind: 'optimization_change',
    days: 19,
    hour: 11,
    actor: 'owner',
    summary: 'History selection set to PACMS, medium budget',
    before: 'Last K',
    after: 'PACMS · medium',
  },
  {
    kind: 'budget_warning',
    days: 4,
    hour: 18,
    actor: null,
    summary: 'Spend passed 80% of its monthly budget',
  },
]

/** The agent the mock stops on budget (the busiest one in the budgets mock is index 0, `mocks/budgets.ts`). */
const STOPPED_INDEX = 0

function eventsFor(agent: MockAgent, index: number, now: number): AgentEvent[] {
  const harness = !!agent.harness
  const created = Date.parse(agent.created_at)
  const out: AgentEvent[] = []
  TEMPLATES.forEach((t, k) => {
    if (harness && !t.harness) return
    // Agents differ by a day or two, so the fleet's changes don't all land on the same buckets.
    const back = t.days + (index % 3)
    const dayStart = Math.floor((now - back * DAY) / DAY) * DAY
    const at = dayStart + ((t.hour + index) % 24) * HOUR
    // Nothing is recorded before the agent existed or after now (QA ISSUE-002).
    if (at > now || at < created) return
    out.push({
      id: eventId(index * 100 + k),
      kind: t.kind,
      at: new Date(at).toISOString(),
      actor: t.actor === 'owner' ? 'admin' : null,
      summary: t.summary,
      before: t.before ?? null,
      after: t.after ?? null,
    })
  })
  if (!harness && index === STOPPED_INDEX) {
    const at = Math.floor((now - DAY) / DAY) * DAY + 21 * HOUR
    if (at <= now && at >= created)
      out.push({
        id: eventId(index * 100 + 99),
        kind: 'budget_stop',
        at: new Date(at).toISOString(),
        actor: null,
        summary: 'Calls stopped at its monthly budget limit',
        before: null,
        after: null,
      })
  }
  return out.sort((a, b) => b.at.localeCompare(a.at))
}

export interface EventsContext {
  loggedIn: () => boolean
  now: () => number
  agents: () => MockAgent[]
  /** `?mock=events-absent` (a server without the log) and `hub-markers-fail` (every marker read fails). */
  hasVariant: (v: 'events-absent' | 'hub-markers-fail') => boolean
  unauthorized: () => Response
  text: (body: string, status: number) => Response
}

export function eventHandlers(ctx: EventsContext): HttpHandler[] {
  return [
    http.get('/api/observability/agent/:id/events', ({ request, params }) => {
      if (!ctx.loggedIn()) return ctx.unauthorized()
      if (ctx.hasVariant('events-absent')) return new HttpResponse(null, { status: 404 })
      if (ctx.hasVariant('hub-markers-fail')) return ctx.text('internal error', 500)
      const all = ctx.agents()
      const index = all.findIndex((a) => a.id === String(params.id) && !a.deleted)
      const agent = all[index]
      if (!agent) return ctx.text('agent not found', 404)
      const url = new URL(request.url)
      const now = ctx.now()
      const from = Date.parse(url.searchParams.get('start_time') ?? '') || now - 30 * DAY
      const to = Date.parse(url.searchParams.get('end_time') ?? '') || now
      const limit = Math.max(1, Math.min(500, Number(url.searchParams.get('limit')) || 200))
      const inWindow = eventsFor(agent, index, now).filter((e) => {
        const t = Date.parse(e.at)
        return t >= from && t < to
      })
      return HttpResponse.json({
        data: { events: inWindow.slice(0, limit), truncated: inWindow.length > limit },
        status_code: 200,
        message: 'ok',
      })
    }),
  ]
}
