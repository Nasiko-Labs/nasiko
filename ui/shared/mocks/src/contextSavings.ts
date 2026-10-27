/**
 * Proposed context-savings reads for /optimization (plans/feat-optimization-page.md eng E1, C2–C4;
 * docs/designs/openruntime-context-recommendations.md CX-V3a/b/c), derived from the same seed traces as spend so the
 * numbers move with the window and the Agent filter. Pure: the handlers pass the viewer, titles and agent switches.
 *
 * - One report per request (C3): a seed trace is one dispatch/proxy request that ran selection. Reports exist from
 *   `RECORDED_DAYS` back, and one request in ten never reports, so coverage (C2) is below 100%. Only requests from
 *   the recording start count as eligible (before it no request could report; those days are "not recorded").
 * - Tokens count once per report, never per downstream call; cost is the report's tokens × the request's input price
 *   (`prompt_cost_usd / input_tokens`). A request with no price adds tokens but no cost (`priced_reports`).
 * - Top requests show the chat only for the viewer's own chats; a superuser sees all (C4).
 */
import {
  bucketStart,
  inWindow,
  matches,
  resolveAgent,
  MockHttpError,
  resolveServerWindow,
  type FilterParams,
} from './aggregate'
import type { Seed, SeedTrace } from './seed'

/** Reports began this many UTC days before now: a 30-day window shows coverage gaps, 7 days doesn't. */
export const RECORDED_DAYS = 14
const DAY = 86_400_000

/** FNV-1a: stable per id, so every read of a trace agrees. */
function hash(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  return h >>> 0
}

export const recordedSince = (now: number) => Math.floor(now / DAY) * DAY - RECORDED_DAYS * DAY

interface Report {
  reported: boolean
  pool: number
  sent: number
  dropped: number
  compressed: number
  /** Input price per token, or null when the request has none. */
  price: number | null
}

/** What one request's report would say (deterministic from the trace). */
export function reportOf(t: SeedTrace, now: number, compress: boolean): Report {
  const h = hash(t.trace_id)
  const reported = t.ts >= recordedSince(now) && h % 10 !== 0
  const factor = 1.8 + (h % 17) / 10
  const sent = t.input_tokens
  const pool = Math.round(sent * factor)
  const poolMsgs = 6 + (h % 30)
  const kept = Math.max(3, Math.round(poolMsgs / factor))
  return {
    reported,
    pool,
    sent,
    dropped: Math.max(0, poolMsgs - kept),
    compressed: compress ? Math.round(sent * 0.9) : 0,
    price: t.input_tokens > 0 && t.prompt_cost_usd > 0 ? t.prompt_cost_usd / t.input_tokens : null,
  }
}

const cents = (n: number) => Math.round(n * 100) / 100

interface Opts {
  now: number
  /** `agents.compress_enabled` by agent id now (the Token optimization switch, shown on rows). */
  compressOf: (agentId: string) => boolean
  /** Whether the agent's past requests were compressed (defaults to `compressOf`): reports are recorded at request time. */
  compressedOf?: (agentId: string) => boolean
  /** `?mock=optimization-no-reports`: requests ran, none reported. */
  noReports?: boolean
  /** ACL-scoped like the dashboard: the agents the viewer can access (all when absent). */
  visible?: (agentId: string) => boolean
}

function scoped(seed: Seed, p: FilterParams, now: number, visible?: (agentId: string) => boolean) {
  const agentName = resolveAgent(seed, p.agent_id)
  const w = resolveServerWindow(p, now)
  // Live agents only, as on the dashboard (review: api-contract).
  const deleted = new Set(seed.agents.filter((a) => a.deleted).map((a) => a.id))
  return {
    w,
    rows: seed.traces.filter(
      (t) =>
        !deleted.has(t.agent_id) &&
        (!visible || visible(t.agent_id)) &&
        inWindow(t, w.start, w.end) &&
        matches(t, agentName, p),
    ),
  }
}

function totals(rows: SeedTrace[], o: Opts) {
  let eligible = 0
  let reports = 0
  let priced = 0
  let pool = 0
  let sent = 0
  let poolCost = 0
  let sentCost = 0
  let dropped = 0
  let compressed = 0
  const since = recordedSince(o.now)
  for (const t of rows) {
    // Eligible = could have reported: requests from the recording start on (earlier ones are the hatched days).
    if (t.ts >= since) eligible += 1
    const r = reportOf(t, o.now, (o.compressedOf ?? o.compressOf)(t.agent_id))
    if (!r.reported || o.noReports) continue
    reports += 1
    pool += r.pool
    sent += r.sent
    dropped += r.dropped
    compressed += r.compressed
    if (r.price !== null) {
      priced += 1
      poolCost += r.pool * r.price
      sentCost += r.sent * r.price
    }
  }
  return { eligible, reports, priced, pool, sent, poolCost, sentCost, dropped, compressed }
}

/** CX-V3 (+ CX-V3a with `series`): the window's totals, coverage and, optionally, its buckets. */
export function contextSavings(seed: Seed, p: FilterParams & { series?: string | null }, o: Opts) {
  if (p.series && p.series !== 'daily' && p.series !== 'hourly')
    throw new MockHttpError(400, `invalid series '${p.series}'`)
  const { w, rows } = scoped(seed, p, o.now, o.visible)
  const t = totals(rows, o)
  const body = {
    eligible_requests: t.eligible,
    reports: t.reports,
    priced_reports: t.priced,
    pool_tokens: t.pool,
    sent_tokens: t.sent,
    pool_cost_usd: t.priced ? cents(t.poolCost) : null,
    sent_cost_usd: t.priced ? cents(t.sentCost) : null,
    messages_dropped: t.dropped,
    compressed_bytes: t.compressed,
    recorded_since: new Date(recordedSince(o.now)).toISOString(),
  }
  if (!p.series) return body
  // 24h is hourly (as spend); any other window daily, whatever was asked (the client asks for what it draws).
  const bucket = w.bucket
  const groups = new Map<number, SeedTrace[]>()
  for (let b = bucketStart(w.start, bucket); b < w.end; b += bucket === 'hour' ? 3_600_000 : DAY)
    groups.set(b, [])
  for (const r of rows) groups.get(bucketStart(r.ts, bucket))?.push(r)
  return {
    ...body,
    series: {
      bucket,
      points: [...groups.entries()].map(([start, g]) => {
        const s = totals(g, o)
        return {
          bucket_start: new Date(start).toISOString(),
          eligible_requests: s.eligible,
          reports: s.reports,
          pool_tokens: s.pool,
          sent_tokens: s.sent,
        }
      }),
    },
  }
}

/** CX-V3b: per agent, history volume first (eng E1; the page sorts by `pool_tokens`, R7B). */
export function contextSavingsAgents(
  seed: Seed,
  p: FilterParams,
  o: Opts & { nameOf: (agentId: string, raw: string) => string },
) {
  const { rows } = scoped(seed, p, o.now, o.visible)
  const by = new Map<string, SeedTrace[]>()
  for (const t of rows) {
    const g = by.get(t.agent_id)
    if (g) g.push(t)
    else by.set(t.agent_id, [t])
  }
  return [...by.entries()]
    .map(([id, g]) => {
      const s = totals(g, o)
      return {
        agent_id: id,
        name: o.nameOf(id, g[0]!.agent_name),
        requests: s.eligible,
        reports: s.reports,
        pool_tokens: s.pool,
        sent_tokens: s.sent,
        compress_enabled: o.compressOf(id),
      }
    })
    .sort((a, b) => b.pool_tokens - a.pool_tokens || a.name.localeCompare(b.name))
}

export interface Viewer {
  id: string
  superuser: boolean
}

/** CX-V3c: the requests that carried the most history; other users' chats are redacted (C4). */
export function topRequests(
  seed: Seed,
  p: FilterParams & { from?: string | null; to?: string | null; limit?: string | null },
  o: Opts & {
    viewer: Viewer
    /** The chat a request belongs to, as Sessions groups it (null: no chat, e.g. a workflow run). */
    sessionOf: (traceId: string) => { id: string; title: string } | null
    /** The chat's owner (mock rule in the handler). */
    ownerOf: (sessionId: string) => string
  },
) {
  const limit = p.limit ? Number(p.limit) : 10
  if (!Number.isInteger(limit) || limit < 1 || limit > 50)
    throw new MockHttpError(400, 'limit must be 1-50')
  const from = p.from ? Date.parse(p.from) : null
  const to = p.to ? Date.parse(p.to) : null
  if ((p.from && Number.isNaN(from)) || (p.to && Number.isNaN(to)))
    throw new MockHttpError(400, 'from/to must be RFC 3339 timestamps')
  const { rows } = scoped(seed, p, o.now, o.visible)
  return rows
    .filter((t) => (from === null || t.ts >= from) && (to === null || t.ts < to))
    .map((t) => ({ t, r: reportOf(t, o.now, (o.compressedOf ?? o.compressOf)(t.agent_id)) }))
    .filter(({ r }) => r.reported && !o.noReports)
    .sort((a, b) => b.r.pool - a.r.pool || a.t.trace_id.localeCompare(b.t.trace_id))
    .slice(0, limit)
    .map(({ t, r }) => {
      const chat = o.sessionOf(t.trace_id)
      const own = o.viewer.superuser || o.ownerOf(chat?.id ?? t.session_id) === o.viewer.id
      return {
        own,
        trace_id: own ? t.trace_id : null,
        span_id: null,
        session_id: own ? (chat?.id ?? null) : null,
        chat_title: own ? (chat?.title ?? null) : null,
        agent_id: t.agent_id,
        agent_name: t.agent_name,
        pool_tokens: r.pool,
        sent_tokens: r.sent,
        compress_enabled: o.compressOf(t.agent_id),
        started_at: t.started_at,
      }
    })
}
