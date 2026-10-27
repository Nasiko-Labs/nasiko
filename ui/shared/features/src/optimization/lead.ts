/**
 * The /optimization lead (plans/feat-optimization-page.md R1A, R2A, R2E, R4B, R5A, R7C; eng C1, C2, C3, C5, C6): pure
 * rules for the answer heading, the five-number sentence, coverage, the trend, the chart's rows and the bar interval.
 * Tested in lead.test.ts. Strings come from copy.ts; numbers from here.
 */
import { fmtShortDay, fmtUtcTime } from '@nasiko/ui/lib/format'
import { savedOf, type Slice } from '@nasiko/app-core/slice'
import { MAX_BUCKETS } from '@nasiko/features/tokenops/series'
import { approx } from './logic'
import type { ContextSavings, SavingsPoint, SettingsChange } from './types'

/** C2: under this share of eligible requests reporting, the page states no percentage. */
const MIN_COVERAGE = 0.5
/** C6: last N recorded days against the N before, compared only with 2N recorded days in the window. */
export const TREND_DAYS = 7
/** C6: the savings share must move by more than this (relative) to be called a trend. */
const TREND_MIN_CHANGE = 0.1

const HOUR = 3_600_000
const DAY = 24 * HOUR

export interface Coverage {
  reports: number
  eligible: number
  /** reports ÷ eligible, 0–1. */
  share: number
  enough: boolean
}

/** C2: null when the server doesn't send `eligible_requests` (coverage unknown, nothing is said about it). */
export function coverageOf(
  s: Pick<ContextSavings, 'reports' | 'eligible_requests'>,
): Coverage | null {
  if (s.eligible_requests === undefined) return null
  const eligible = s.eligible_requests
  const share = eligible > 0 ? Math.min(1, s.reports / eligible) : 0
  return { reports: s.reports, eligible, share, enough: share >= MIN_COVERAGE }
}

/** One bucket's counts summed over several (a trend side, a week). */
function sumPoints(ps: readonly SavingsPoint[]): Omit<SavingsPoint, 'bucket_start'> {
  return {
    eligible_requests: ps.reduce((a, p) => a + p.eligible_requests, 0),
    reports: ps.reduce((a, p) => a + p.reports, 0),
    pool_tokens: ps.reduce((a, p) => a + p.pool_tokens, 0),
    sent_tokens: ps.reduce((a, p) => a + p.sent_tokens, 0),
  }
}

/** Saved ÷ without, 0–1 (never negative: a report can't send more than its pool). */
export function savedShare(pool: number, sent: number): number {
  return pool > 0 ? Math.max(0, pool - sent) / pool : 0
}

export type LeadState =
  | { kind: 'loading' }
  | { kind: 'error' }
  /** Today's server: no CX-V3 (R2B, the reason said once, here). */
  | { kind: 'absent' }
  /** R2E(2): the viewer can access no agents. */
  | { kind: 'no-agents' }
  /** R2E(1): `reports: 0`. */
  | { kind: 'no-reports' }
  /** C2: too few requests reported to state a percentage. */
  | { kind: 'low-coverage'; savings: ContextSavings; coverage: Coverage }
  | { kind: 'saving'; savings: ContextSavings; coverage: Coverage | null; share: number }

/**
 * C1: the heading comes from reported savings only (never from switch counts: selection trims history with Token
 * optimization off too). The reason the server can't say is given once, here (R2B), before any access state.
 */
export function leadState(input: {
  result: { kind: 'ready'; data: ContextSavings } | { kind: 'absent' } | undefined
  failed: boolean
  /** Agents the viewer can access, or null while unknown. */
  agentCount: number | null
}): LeadState {
  const { result } = input
  if (!result) return input.failed ? { kind: 'error' } : { kind: 'loading' }
  if (result.kind === 'absent') return { kind: 'absent' }
  if (input.agentCount === 0) return { kind: 'no-agents' }
  const s = result.data
  if (s.reports === 0) return { kind: 'no-reports' }
  const coverage = coverageOf(s)
  if (coverage && !coverage.enough) return { kind: 'low-coverage', savings: s, coverage }
  return { kind: 'saving', savings: s, coverage, share: savedShare(s.pool_tokens, s.sent_tokens) }
}

/**
 * `63.5M`, `420k`, `9,410`: the one token figure (the page and TokenOps' savings panel), three significant digits,
 * rounded before the unit is picked so 999,600 reads 1M, never 1,000k.
 */
export function tokensShort(n: number): string {
  const r = approx(n, 3)
  // M only once the k figure would reach 1,000 (950k is "950k", never "0.9M"); tenths by integer rounding, so no
  // float artefacts (1.45M reads 1.5M, review: adversarial).
  if (Math.round(r / 1e3) >= 1000) return `${String(Math.round(r / 1e5) / 10)}M`
  if (r >= 10_000) return `${Math.round(r / 1e3).toLocaleString('en-US')}k`
  return r.toLocaleString('en-US')
}

/** Whole percent, never "0%" for a real saving or "100%" for a partial one. */
export function pctOf(share: number): number {
  const p = Math.round(share * 100)
  if (share > 0 && p === 0) return 1
  if (share < 1 && p === 100) return 99
  return p
}

/** The saved cost, when the server priced any report (C3), else null. */
export function savedCost(
  s: Pick<ContextSavings, 'pool_cost_usd' | 'sent_cost_usd'>,
): number | null {
  return s.pool_cost_usd !== null && s.sent_cost_usd !== null
    ? Math.max(0, s.pool_cost_usd - s.sent_cost_usd)
    : null
}

// ─── trend (C6) ────────────────────────────────────────────────────────────────

/** A bucket counts as recorded when it has eligible requests and at least half of them reported (C2). */
function isRecorded(p: Pick<SavingsPoint, 'eligible_requests' | 'reports'>): boolean {
  return p.eligible_requests > 0 && p.reports / p.eligible_requests >= MIN_COVERAGE
}

export type Trend = { direction: 'up' | 'down'; from: number; to: number }

/**
 * C6: savings share (saved ÷ without) over the last 7 recorded days against the 7 before, only for daily buckets with
 * at least 14 recorded days, and mentioned only beyond a 10% relative change. Anything else is null: no sentence.
 */
export function trendOf(series: ContextSavings['series']): Trend | null {
  if (!series || series.bucket !== 'day') return null
  const recorded = series.points
    .filter(isRecorded)
    .sort((a, b) => a.bucket_start.localeCompare(b.bucket_start))
  if (recorded.length < TREND_DAYS * 2) return null
  const side = (ps: SavingsPoint[]) => {
    const t = sumPoints(ps)
    return {
      share: savedShare(t.pool_tokens, t.sent_tokens),
      pool: t.pool_tokens,
      covered: isRecorded(t),
    }
  }
  const prev = side(recorded.slice(-TREND_DAYS * 2, -TREND_DAYS))
  const last = side(recorded.slice(-TREND_DAYS))
  if (!prev.covered || !last.covered || prev.pool === 0 || last.pool === 0 || prev.share === 0)
    return null
  const change = (last.share - prev.share) / prev.share
  // A hair of tolerance: 0.55 vs 0.5 is exactly 10%, which float division puts a hair above.
  if (Math.abs(change) <= TREND_MIN_CHANGE + 1e-9) return null
  return { direction: change > 0 ? 'up' : 'down', from: prev.share, to: last.share }
}

/** CX-H: the newest change in the window, described, never as a cause (C6). */
export function latestChange(
  changes: readonly SettingsChange[] | undefined,
): SettingsChange | null {
  if (!changes?.length) return null
  return [...changes].sort((a, b) => b.at.localeCompare(a.at))[0] ?? null
}

// ─── chart rows (R5A) ──────────────────────────────────────────────────────────

/**
 * Oldest first, and at most TokenOps' bucket cap, newest kept: a custom range of centuries can't render hundreds of
 * thousands of bars (review: Codex).
 */
function newestBuckets(points: readonly SavingsPoint[]): SavingsPoint[] {
  return [...points]
    .sort((a, b) => a.bucket_start.localeCompare(b.bucket_start))
    .slice(-MAX_BUCKETS)
}

export interface TrendRow {
  iso: string
  sent: number
  saved: number
  pool: number
  /** The hatched "not recorded" slot: full height where the bucket isn't recorded, else 0. */
  notRecorded: number
  reports: number
  eligible: number
  recorded: boolean
}

/**
 * One row per bucket, stacked Sent → Saved (the full bar is without optimization). A bucket that isn't recorded
 * (before `recorded_since`, or under half its requests reported, C2) draws no figures, only a hatched slot as tall as
 * the tallest recorded bar, so it never moves the axis.
 */
export function trendRows(
  points: readonly SavingsPoint[],
  recordedSince: string | null = null,
): TrendRow[] {
  const sorted = newestBuckets(points)
  // A bucket with no requests at all is "not recorded" only before reporting began; after that it is a quiet bucket
  // with an empty bar (QA ISSUE-002: quiet night hours read "Not recorded").
  const quiet = (p: SavingsPoint) =>
    p.eligible_requests === 0 && !(recordedSince !== null && p.bucket_start < recordedSince)
  const tallest = Math.max(0, ...sorted.filter(isRecorded).map((p) => p.pool_tokens))
  const slot = tallest > 0 ? tallest : 1
  return sorted.map((p) => {
    const recorded = isRecorded(p) || quiet(p)
    return {
      iso: p.bucket_start,
      sent: recorded ? p.sent_tokens : 0,
      saved: recorded ? savedOf(p) : 0,
      pool: recorded ? p.pool_tokens : 0,
      notRecorded: recorded ? 0 : slot,
      reports: p.reports,
      eligible: p.eligible_requests,
      recorded,
    }
  })
}

// ─── bar interval (C5) ─────────────────────────────────────────────────────────

/** The chip's words for a slice: `Sep 29`, `Sep 29, 14:00 UTC`, `Sep 23 – Sep 29`. */
export function sliceLabel(s: Slice): string {
  if (s.size === 'hour') return `${fmtShortDay(s.from)}, ${fmtUtcTime(s.from)} UTC`
  if (s.size === 'day') return fmtShortDay(s.from)
  return `${fmtShortDay(s.from)} – ${fmtShortDay(new Date(Date.parse(s.to) - DAY))}`
}

/**
 * R6B: below 640 px the chart draws 7-day bars (the Table view keeps the daily rows). Weeks are counted back from the
 * newest day by date, so the last bar is the latest 7 days and only the first may be short; each week starts 6 days
 * before its newest day, so a click picks that week (C5, clipped to the window). Sums carry over, so coverage (C2) is
 * the week's.
 */
export function weeklyPoints(points: readonly SavingsPoint[]): SavingsPoint[] {
  const sorted = newestBuckets(points)
  const newest = sorted.at(-1)
  if (!newest) return []
  const last = Date.parse(newest.bucket_start)
  // By date, not by position, so a missing day never shifts older days into the wrong week (review: red team).
  const weeks = new Map<number, SavingsPoint[]>()
  for (const p of sorted) {
    const back = Math.floor((last - Date.parse(p.bucket_start)) / (7 * DAY))
    weeks.set(back, [...(weeks.get(back) ?? []), p])
  }
  return [...weeks.entries()]
    .sort((a, b) => b[0] - a[0])
    .map(([back, ps]) => ({
      bucket_start: new Date(last - (back * 7 + 6) * DAY).toISOString(),
      ...sumPoints(ps),
    }))
}
