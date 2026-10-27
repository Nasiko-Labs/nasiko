/**
 * The /optimization lead (plans/feat-optimization-page.md R1A, R1B, R2A, R2B, R2E, R4B, R5A, R7A; eng C1–C3, C5, C6):
 * a bento: a lead card (the page's one effect area) whose h2 is the answer, then the five-number sentence with the TokenOps link, coverage, cost
 * coverage, the trend and the latest settings change (described, never as a cause), a sent/saved bar, the four figures
 * beside it, then the chart in its card. It loads and
 * fails on its own (R2C); every number comes from reports, never from switch counts (C1).
 */
import { Link } from '@tanstack/react-router'
import { m } from 'motion/react'
import type { ReactNode } from 'react'
import { SpotlightCard } from '@nasiko/ui/composites/spotlight-card'
import { PanelError, PanelSkeleton } from '@nasiko/ui/composites/panel'
import { Reveal, RevealItem } from '@nasiko/ui/composites/reveal'
import { asButton } from '@nasiko/ui/primitives/button'
import { Card } from '@nasiko/ui/primitives/card'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import type { ResolvedWindow } from '@nasiko/features/tokenops/window'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { fmtMoney, fmtShortDay } from '@nasiko/ui/lib/format'
import { transitions } from '@nasiko/ui/lib/motion'
import { copy } from '../copy'
import {
  latestChange,
  pctOf,
  savedCost,
  tokensShort,
  trendOf,
  TREND_DAYS,
  type LeadState,
} from '../lead'
import { savedOf, type BucketSize } from '@nasiko/app-core/slice'
import type { SettingsChange } from '../types'
import { Outline } from './Outline'
import { SavingsTrend, type LabelledRow } from './SavingsTrend'
import { Without } from './Without'

/** The trend chart's own colours, so the bar and the chart read as one. */
const SENT = seriesAt(0)
const SAVED = seriesAt(1)

export function Lead({
  state,
  error,
  onRetry,
  win,
  superuser,
  rows,
  tableRows,
  unit,
  tableUnit,
  under,
  history,
  compression,
  onSelect,
}: {
  state: LeadState
  error: unknown
  onRetry: () => void
  win: Pick<ResolvedWindow, 'label' | 'start' | 'preset'> & { from?: string; to?: string }
  superuser: boolean
  rows: LabelledRow[]
  tableRows: LabelledRow[]
  unit: BucketSize
  tableUnit: BucketSize
  /** R6B: what sits under the lead's words on a phone (the Your settings link). */
  under?: ReactNode
  history: SettingsChange[] | undefined
  /** C1: history compression for your chats, or null when Needs attention says it (or it isn't known yet). */
  compression: string | null
  onSelect: (iso: string, size: BucketSize) => void
}) {
  const hourly = win.preset === '24h'
  const heading =
    state.kind === 'loading'
      ? null
      : state.kind === 'error'
        ? copy.lead.heading.error
        : state.kind === 'absent'
          ? copy.lead.heading.absent
          : state.kind === 'no-agents'
            ? copy.lead.heading.noAgents
            : state.kind === 'no-reports'
              ? copy.lead.heading.noReports
              : state.kind === 'low-coverage'
                ? copy.lead.heading.lowCoverage
                : state.share > 0
                  ? copy.lead.heading.saving(pctOf(state.share))
                  : copy.lead.heading.none

  const figures = state.kind === 'saving' || state.kind === 'low-coverage' ? state : null
  return (
    <section
      data-tour="optimization-lead"
      aria-labelledby="optimization-lead-h"
      aria-busy={state.kind === 'loading'}
      className="flex flex-col gap-3"
    >
      <Reveal className="grid gap-3 lg:grid-cols-12">
        <RevealItem className={figures ? 'lg:col-span-7' : 'lg:col-span-12'}>
          {/* The page's one effect area (CLAUDE.md "Aceternity effects"). */}
          <SpotlightCard className="h-full">
            <div className="relative flex flex-1 flex-col gap-3 p-5">
              <span className="text-xs text-muted-foreground">
                {copy.lead.meta(win.label, superuser)}
              </span>
              {heading ? (
                // The error panel below says what failed; the heading stays for navigation only (review: design).
                <h2
                  id="optimization-lead-h"
                  className={
                    state.kind === 'error'
                      ? 'sr-only'
                      : 'text-2xl font-semibold tracking-tight text-balance'
                  }
                >
                  {heading}
                </h2>
              ) : (
                <>
                  <h2 id="optimization-lead-h" className="sr-only">
                    {copy.lead.chart.loading}
                  </h2>
                  <Skeleton aria-hidden className="h-7 w-72" />
                </>
              )}
              <Body
                state={state}
                error={error}
                onRetry={onRetry}
                win={win}
                superuser={superuser}
                history={history}
              />
              {under}
              {compression && state.kind !== 'loading' && state.kind !== 'error' ? (
                <p className="m-0 text-xs text-muted-foreground">{compression}</p>
              ) : null}
              {figures ? (
                <SplitBar sent={figures.savings.sent_tokens} saved={savedOf(figures.savings)} />
              ) : null}
            </div>
          </SpotlightCard>
        </RevealItem>
        {figures ? (
          <dl
            aria-label={copy.lead.chart.summary(
              win.label,
              tokensShort(figures.savings.sent_tokens),
              tokensShort(savedOf(figures.savings)),
            )}
            className="grid grid-cols-2 gap-3 lg:col-span-5"
          >
            <Figure
              label={<Without label={copy.preview.without} />}
              value={`~${tokensShort(figures.savings.pool_tokens)}`}
            />
            <Figure
              label={copy.lead.chart.sent}
              value={`~${tokensShort(figures.savings.sent_tokens)}`}
            />
            <Figure
              label={copy.lead.chart.savedCol}
              value={`~${tokensShort(savedOf(figures.savings))}`}
              // C2: under half the requests reported, the page states no percentage.
              note={
                figures.kind === 'saving'
                  ? copy.lead.figures.fewer(pctOf(figures.share))
                  : undefined
              }
            />
            <Figure
              label={copy.lead.figures.notBilled}
              value={
                savedCost(figures.savings) === null
                  ? '—'
                  : `≈ ${fmtMoney(savedCost(figures.savings) ?? 0)}`
              }
              note={
                figures.coverage
                  ? copy.lead.figures.reported(pctOf(figures.coverage.share))
                  : undefined
              }
              meter={figures.coverage ? figures.coverage.share * 100 : undefined}
            />
          </dl>
        ) : null}
      </Reveal>
      {state.kind === 'saving' || state.kind === 'low-coverage' ? (
        <Card className="p-4">
          <SavingsTrend
            rows={rows}
            tableRows={tableRows}
            unit={unit}
            tableUnit={tableUnit}
            period={win.label}
            onSelect={onSelect}
          />
        </Card>
      ) : state.kind === 'loading' ? (
        <PanelSkeleton height={240} />
      ) : state.kind === 'error' ? null : (
        // R2B: the chart's place and shape, static (never read as loading), saying what will show here.
        <Outline>
          {state.kind === 'absent'
            ? copy.lead.chart.outline(hourly)
            : state.kind === 'no-reports'
              ? copy.lead.noReportsLine
              : copy.lead.noAgentsLine}
        </Outline>
      )}
    </section>
  )
}

/** Sent against saved for the window, one bar; the figures beside it carry the values in words. */
function SplitBar({ sent, saved }: { sent: number; saved: number }) {
  const total = sent + saved
  if (total <= 0) return null
  return (
    <div aria-hidden className="mt-auto flex h-2.5 w-full gap-0.5 overflow-hidden rounded-full">
      <div className="h-full flex-1 rounded-l-full" style={{ backgroundColor: SENT.fill }} />
      <m.div
        className="h-full rounded-r-full"
        style={{ backgroundColor: SAVED.fill }}
        initial={{ width: 0 }}
        animate={{ width: `${(saved / total) * 100}%` }}
        transition={transitions.disclosure}
      />
    </div>
  )
}

function Figure({
  label,
  value,
  note,
  meter,
}: {
  label: ReactNode
  value: string
  note?: string
  /** 0–100: a thin meter under the note. */
  meter?: number
}) {
  return (
    <RevealItem className="flex min-w-0 flex-col gap-1.5 rounded-xl border bg-card p-4 transition-colors hover:border-foreground/20">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="flex flex-col gap-1.5">
        <span className="font-mono text-2xl font-semibold tracking-tight tabular-nums">
          {value}
        </span>
        {note ? <span className="text-xs text-muted-foreground">{note}</span> : null}
        {meter !== undefined ? (
          <span aria-hidden className="h-1.5 w-full rounded-full bg-muted">
            <span
              className="block h-full rounded-full bg-chart-1-edge"
              style={{ width: `${Math.min(100, meter)}%` }}
            />
          </span>
        ) : null}
      </dd>
    </RevealItem>
  )
}

function Body({
  state,
  error,
  onRetry,
  win,
  superuser,
  history,
}: {
  state: LeadState
  error: unknown
  onRetry: () => void
  win: Pick<ResolvedWindow, 'start' | 'preset'> & { from?: string; to?: string }
  superuser: boolean
  history: SettingsChange[] | undefined
}) {
  if (state.kind === 'loading') return <Skeleton aria-hidden className="h-5 w-full max-w-xl" />
  if (state.kind === 'error')
    return <PanelError error={error} onRetry={onRetry} what={copy.lead.chart.what} />
  if (state.kind === 'absent')
    return <p className="m-0 text-sm text-muted-foreground">{copy.lead.absentLine}</p>
  if (state.kind === 'no-reports')
    return (
      <div>
        <Link
          to="/chat"
          {...asButton({ variant: 'outline', size: 'sm', className: 'pointer-coarse:min-h-11' })}
        >
          {copy.lead.startChat}
        </Link>
      </div>
    )
  if (state.kind === 'no-agents') return null
  const s = state.savings
  const coverage = state.coverage
  const cost = savedCost(s)
  const saved = savedOf(s)
  const trend = state.kind === 'saving' ? trendOf(s.series) : null
  const change = latestChange(history)
  const recordedLate =
    s.recorded_since && Date.parse(s.recorded_since) > win.start.getTime()
      ? copy.savings.coverage(fmtShortDay(s.recorded_since))
      : null
  const notes = [
    coverage ? copy.lead.basedOn(coverage.reports, coverage.eligible, pctOf(coverage.share)) : null,
    s.priced_reports !== undefined && cost !== null && s.priced_reports < s.reports
      ? copy.lead.costCovers(s.priced_reports, s.reports)
      : null,
    recordedLate,
    trend ? copy.lead.trend(trend.direction, pctOf(trend.from), pctOf(trend.to), TREND_DAYS) : null,
    change ? copy.lead.change(change.field, changeValue(change), fmtShortDay(change.at)) : null,
  ].filter(Boolean)
  return (
    <div className="flex flex-col gap-1">
      <p className="m-0 text-sm">
        {copy.lead.sentence(
          superuser,
          tokensShort(s.sent_tokens),
          tokensShort(s.pool_tokens),
          tokensShort(saved),
          cost !== null ? fmtMoney(cost) : null,
          state.kind === 'saving' ? pctOf(state.share) : null,
        )}{' '}
        <Link
          to="/tokenops"
          search={{ preset: win.preset, from: win.from, to: win.to }}
          className="underline underline-offset-4 hover:text-primary-text"
        >
          {copy.lead.tokenops}
        </Link>
      </p>
      {notes.length ? <p className="m-0 text-xs text-muted-foreground">{notes.join(' ')}</p> : null}
    </div>
  )
}

/** CX-H values are the server's enum words; show them as the settings form names them. */
function changeValue(c: SettingsChange): string {
  if (c.field === 'strategy')
    return Object.hasOwn(copy.strategies, c.to)
      ? copy.strategies[c.to as keyof typeof copy.strategies].name
      : c.to
  if (c.field === 'level')
    return c.to === 'low' || c.to === 'medium' || c.to === 'high' ? copy.level(c.to) : c.to
  return c.to
}
