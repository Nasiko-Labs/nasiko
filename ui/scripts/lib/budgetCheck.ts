/**
 * `contract:check -- --only budgets` (docs/requirements/budgets.md §15, plan feat-budget-burn X4): one request per 1.0
 * route against a server seeded with the golden scenario (`seed:live`, §14.8), each answered with a verdict:
 * `pass` (the UI's zod schema accepts the body), `fail` (the first mismatch) or `unavailable` (a bare 404: not built).
 *
 * Exit 0 when every endpoint of every capability the server lists passes; 1 on any failure, or when a listed
 * capability has an unavailable endpoint. Values are compared by the recorder's parity once a server has the routes
 * (record:live with the budgets entries), not here.
 */
import type { z } from 'zod'
import {
  alertListSchema,
  atRiskListSchema,
  breakdownResponseSchema,
  budgetListSchema,
  budgetResponseSchema,
  budgetSettingsSchema,
  burnResponseSchema,
  capabilitiesSchema,
  changesResponseSchema,
  hoursResponseSchema,
  priceListSchema,
  statusListSchema,
  traceListSchema,
  unbudgetedListSchema,
  versionListSchema,
} from '../../ee/web/src/features/budgets/types.ts'
import { SCENARIO_IDS as I } from '../../ee/web/src/mocks/seed-budgets.ts'

const DAY = 86_400_000
const day = (t: number) => new Date(t).toISOString().slice(0, 10)
/** Monday 00:00 UTC of `t`'s week. */
const monday = (t: number) => {
  const d = Math.floor(t / DAY) * DAY
  return d - ((new Date(d).getUTCDay() + 6) % 7) * DAY
}

interface BudgetCheck {
  /** The capability slug that covers it (§10.1). */
  feature: string
  path: (asOf: number) => string
  schema: z.ZodType
}

const BUDGET_CHECKS: BudgetCheck[] = [
  { feature: 'budgets', path: () => '/api/budgets', schema: budgetListSchema },
  { feature: 'budgets', path: () => '/api/budgets/status', schema: statusListSchema },
  { feature: 'budgets', path: () => '/api/budgets/alerts', schema: alertListSchema },
  { feature: 'versions', path: () => `/api/budgets/${I.bReview}`, schema: budgetResponseSchema },
  {
    feature: 'versions',
    path: () => `/api/budgets/${I.bReview}/versions`,
    schema: versionListSchema,
  },
  {
    feature: 'burn',
    path: () => '/api/budgets/burn?scope=org&include_previous=true',
    schema: burnResponseSchema,
  },
  {
    feature: 'burn',
    path: () => `/api/budgets/burn?scope=agent&id=${I.reviewer}`,
    schema: burnResponseSchema,
  },
  {
    feature: 'burn',
    path: () => `/api/budgets/burn?scope=owner&id=${I.dev}`,
    schema: burnResponseSchema,
  },
  { feature: 'org_budgets', path: () => `/api/budgets/${I.bOrg}`, schema: budgetResponseSchema },
  {
    feature: 'unit_budgets',
    path: () => `/api/budgets/burn?scope=unit&id=${I.platform}`,
    schema: burnResponseSchema,
  },
  {
    feature: 'quarter_periods',
    path: () => `/api/budgets/burn?scope=unit&id=${I.growth}`,
    schema: burnResponseSchema,
  },
  {
    feature: 'session_budgets',
    path: () => `/api/budgets/burn?scope=session&id=${I.s1}`,
    schema: burnResponseSchema,
  },
  {
    feature: 'contributor',
    path: () =>
      `/api/budgets/burn?scope=agent&id=${I.reviewer}&contributor=openai/gpt-4.1&by=model`,
    schema: burnResponseSchema,
  },
  {
    feature: 'breakdown',
    path: () => '/api/budgets/burn/breakdown?scope=org&by=agent',
    schema: breakdownResponseSchema,
  },
  {
    feature: 'unit_budgets',
    path: () => '/api/budgets/burn/breakdown?scope=org&by=unit',
    schema: breakdownResponseSchema,
  },
  {
    feature: 'changes',
    path: (t) => {
      // This week so far vs the same hours last week; at Monday 00:00 (an hour-floored anchor), last week vs the one
      // before, so the range is never empty.
      const fresh = t - monday(t) < 3_600_000
      const from = fresh ? monday(t) - 7 * DAY : monday(t)
      const to = fresh ? monday(t) : t
      const iso = (x: number) => new Date(x).toISOString()
      return `/api/budgets/burn/changes?scope=org&by=agent&from=${iso(from)}&to=${iso(to)}&compare_from=${iso(from - 7 * DAY)}&compare_to=${iso(to - 7 * DAY)}`
    },
    schema: changesResponseSchema,
  },
  {
    feature: 'hours',
    path: (t) => `/api/budgets/burn/hours?scope=org&date=${day(t)}`,
    schema: hoursResponseSchema,
  },
  {
    feature: 'traces',
    path: (t) => `/api/budgets/burn/traces?scope=agent&id=${I.reviewer}&date=${day(t - 5 * DAY)}`,
    schema: traceListSchema,
  },
  {
    feature: 'scope_status',
    path: () => '/api/budgets/status?scope=org',
    schema: statusListSchema,
  },
  { feature: 'at_risk', path: () => '/api/budgets/at-risk?scope=org', schema: atRiskListSchema },
  {
    feature: 'unbudgeted',
    path: () => '/api/budgets/unbudgeted?scope=org',
    schema: unbudgetedListSchema,
  },
  {
    feature: 'forecast_alerts',
    path: () => '/api/budgets/alerts?scope=org&kind=all',
    schema: alertListSchema,
  },
  { feature: 'prices', path: () => '/api/budgets/prices', schema: priceListSchema },
  { feature: 'team_identity', path: () => '/api/budgets/settings', schema: budgetSettingsSchema },
  // One route serves both settings capabilities: a server listing either must answer it.
  { feature: 'fiscal_year', path: () => '/api/budgets/settings', schema: budgetSettingsSchema },
]

export interface Verdict {
  feature: string
  path: string
  result: 'pass' | 'fail' | 'unavailable'
  detail?: string
}

/** A bare 404: an empty body or a body without the contract's `code` (a no-route answer). */
const isBare404 = (status: number, body: unknown) =>
  status === 404 && !(body && typeof body === 'object' && 'code' in body)

export function judge(
  feature: string,
  path: string,
  status: number,
  body: unknown,
  schema: z.ZodType,
): Verdict {
  if (isBare404(status, body)) return { feature, path, result: 'unavailable' }
  if (status !== 200)
    return {
      feature,
      path,
      result: 'fail',
      detail: `HTTP ${status}: ${JSON.stringify(body).slice(0, 160)}`,
    }
  const r = schema.safeParse(body)
  if (r.success) return { feature, path, result: 'pass' }
  const i = r.error.issues[0]!
  return { feature, path, result: 'fail', detail: `$.${i.path.join('.')}: ${i.message}` }
}

type Get = (path: string) => Promise<{ status: number; body: unknown }>

export async function checkBudgets(get: Get, now = Date.now()) {
  const caps = await get('/api/budgets/capabilities')
  const capsOk =
    !isBare404(caps.status, caps.body) && capabilitiesSchema.safeParse(caps.body).success
  const listed = capsOk ? (caps.body as { data: { features: string[] } }).data.features : null
  const verdicts: Verdict[] = [
    capsOk
      ? { feature: 'capabilities', path: '/api/budgets/capabilities', result: 'pass' }
      : judge(
          'capabilities',
          '/api/budgets/capabilities',
          caps.status,
          caps.body,
          capabilitiesSchema,
        ),
  ]
  // The server's own clock, when it answers a burn read, so the week and day paths match its data.
  const probe = await get('/api/budgets/burn?scope=org')
  // A malformed as_of is the burn check's failure to report, not a reason to stop: fall back to our own clock.
  const raw = (probe.body as { data?: { as_of?: unknown } } | null)?.data?.as_of
  const parsed = probe.status === 200 && typeof raw === 'string' ? Date.parse(raw) : NaN
  const asOf = Number.isFinite(parsed) ? parsed : now
  for (const c of BUDGET_CHECKS) {
    const path = c.path(asOf)
    const r = await get(path)
    verdicts.push(judge(c.feature, path, r.status, r.body, c.schema))
  }
  const failed = verdicts.some((v) => v.result === 'fail')
  const missing = verdicts.some((v) => v.result === 'unavailable' && listed?.includes(v.feature))
  return { verdicts, listed, code: failed || missing ? 1 : 0 }
}

export function report(r: Awaited<ReturnType<typeof checkBudgets>>): string {
  const lines = r.verdicts.map(
    (v) =>
      `${v.result.padEnd(11)} ${v.feature.padEnd(16)} ${v.path}${v.result === 'unavailable' && r.listed?.includes(v.feature) ? '  (listed in capabilities)' : ''}${v.detail ? `\n            ${v.detail}` : ''}`,
  )
  const n = (k: Verdict['result']) => r.verdicts.filter((v) => v.result === k).length
  lines.push(
    `contract:check budgets: ${n('pass')} pass, ${n('fail')} fail, ${n('unavailable')} unavailable; capabilities ${r.listed ? `[${r.listed.join(', ')}]` : 'absent'}; exit ${r.code}`,
  )
  return lines.join('\n')
}
