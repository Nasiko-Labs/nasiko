/**
 * TokenOps hero: one bento that answers "what did we spend, and is it normal" before anything is opened.
 *
 *   ┌ lead: the narrative over the window's spend curve ┬ Total spend ┬ Month-end ┐
 *   │ (the page's one effect area: Aceternity's grid    ├ Cost / op   ┼ Ops       ┤
 *   │  background + a pointer spotlight)                ├ p95         ┼ Agents    ┤
 *   ├ secondary metrics: tokens · tool calls · p50 · p99 ─────────────────────────┤
 *   └ savings strip (children) ───────────────────────────────────────────────────┘
 *
 * The narrative is the answer; the tiles back it and never repeat a whole sentence. The spike clause links to that
 * day's sessions and is a shared `layoutId` source for the day panel and the Sessions header morph.
 */
import { area, curveMonotoneX, line } from 'd3-shape'
import { m } from 'motion/react'
import type { ReactNode } from 'react'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { SpotlightCard } from '@nasiko/ui/composites/spotlight-card'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Delta } from '@nasiko/ui/composites/delta'
import { PanelError } from '@nasiko/ui/composites/panel'
import { morphId } from '@nasiko/features/observability/tuning'
import { spikeSentence, type Spike } from '@nasiko/features/narrative/tokenops'
import { describeDelta, POLARITY, TONE_CLASS, type Polarity } from '@nasiko/ui/lib/delta'
import {
  fmtCostPerOp,
  fmtInt,
  fmtLatency,
  fmtMoney,
  fmtMoneyFloor,
  fmtShortDay,
  fmtTokens,
} from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { FORECAST_MIN_DAYS, type MonthSummary } from '../forecast'
import type { TimelinePoint } from '../series'
import type { FinopsDashboardData, KpiValue } from '../types'
import { Reveal, RevealItem } from '@nasiko/ui/composites/reveal'

export function SummaryHero({
  sentences,
  spike,
  spikeLink,
  windowLabel,
  compareLabel,
  data,
  points,
  compare,
  unpriced,
  month,
  loading,
  error,
  onRetry,
  children,
}: {
  sentences: string[]
  spike: Spike | null
  /** Wraps the spike link text in a link to that day's sessions. */
  spikeLink: (children: ReactNode) => ReactNode
  windowLabel: string
  /** What Δ compares against, when "the period before" alone would mislead (This month). */
  compareLabel?: string
  data: FinopsDashboardData | undefined
  /** The window's buckets, drawn as the lead's spend curve. */
  points: TimelinePoint[]
  compare: boolean
  unpriced: boolean
  month: MonthSummary | null
  loading: boolean
  error: unknown
  onRetry: () => void
  /** The savings strip, the bento's last row. */
  children?: ReactNode
}) {
  // Budgets are Enterprise only: the link is the layer's, and an OSS build has neither it nor the route.
  const OpenBudgets = useSlots().budgets.OpenLink
  if (error) return <PanelError error={error} onRetry={onRetry} what="the summary" />
  const kpis = data?.kpis
  const delta = (k: KpiValue | undefined, polarity: Polarity) =>
    k && compare ? <Delta changePct={k.change_pct} polarity={polarity} current={k.current} /> : null
  return (
    <section data-tour="tokenops-summary" aria-labelledby="summary-title">
      <h2 id="summary-title" className="sr-only">
        Summary
      </h2>
      <Reveal className="grid gap-3 lg:grid-cols-12">
        <RevealItem className="lg:col-span-7">
          <Lead
            windowLabel={windowLabel}
            compareLabel={compareLabel}
            compare={compare}
            loading={loading}
            sentences={sentences}
            spike={spike}
            spikeLink={spikeLink}
            unpriced={unpriced}
            changePct={kpis?.total_spend.change_pct}
            points={points}
          />
        </RevealItem>

        <dl
          aria-label={`Key metrics, ${windowLabel}`}
          className="grid grid-cols-2 gap-3 lg:col-span-5"
        >
          <Tile
            label="Total spend"
            value={kpis ? fmtMoneyFloor(kpis.total_spend.current, unpriced) : '—'}
            aside={delta(kpis?.total_spend, POLARITY.spend)}
          />
          <Tile
            label="Month-end at pace"
            aside={
              // Budgets count their own populations, not this page's trace_usage, so only a link — and only where
              // the edition has budgets and the server serves them (the layer's own probe).
              OpenBudgets ? <OpenBudgets /> : null
            }
            value={
              month?.show && month.low !== null && month.high !== null ? (
                <>
                  {fmtMoney(month.low)}–<wbr />
                  {fmtMoney(month.high)}
                </>
              ) : (
                '—'
              )
            }
          >
            <MonthMeter month={month} />
          </Tile>
          <Tile
            label="Cost / operation"
            value={kpis ? fmtCostPerOp(kpis.cost_per_operation.current) : '—'}
            aside={delta(kpis?.cost_per_operation, POLARITY.costPerOp)}
          />
          <Tile
            label="Operations"
            value={kpis ? fmtInt(kpis.total_operations.current) : '—'}
            aside={delta(kpis?.total_operations, POLARITY.operations)}
          />
          <Tile
            label="p95 latency"
            value={kpis ? fmtLatency(kpis.latency_p95_ms.current) : '—'}
            aside={delta(kpis?.latency_p95_ms, POLARITY.p95)}
          />
          <Tile
            label="Active agents"
            value={
              kpis ? (
                <>
                  {fmtInt(kpis.active_agents.current)}
                  {data ? (
                    <span className="ml-1.5 text-sm font-normal text-muted-foreground">
                      of {fmtInt(data.summary.total_agents)}
                    </span>
                  ) : null}
                </>
              ) : (
                '—'
              )
            }
          >
            {kpis && data && data.summary.total_agents > 0 ? (
              <Meter pct={(kpis.active_agents.current / data.summary.total_agents) * 100} />
            ) : null}
          </Tile>
        </dl>

        {kpis ? (
          <RevealItem className="lg:col-span-12">
            <dl
              aria-label={`More metrics, ${windowLabel}`}
              className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border sm:grid-cols-4"
            >
              <Stat label="Tokens" value={fmtTokens(kpis.total_tokens.current)}>
                {delta(kpis.total_tokens, POLARITY.tokens)}
              </Stat>
              <Stat label="Tool calls" value={fmtInt(kpis.total_tool_calls.current)}>
                {delta(kpis.total_tool_calls, POLARITY.toolCalls)}
              </Stat>
              <Stat label="p50 latency" value={fmtLatency(kpis.avg_latency_ms.current)}>
                {delta(kpis.avg_latency_ms, POLARITY.latency)}
              </Stat>
              <Stat label="p99 latency" value={fmtLatency(kpis.latency_p99_ms.current)}>
                {delta(kpis.latency_p99_ms, POLARITY.p99)}
              </Stat>
            </dl>
          </RevealItem>
        ) : null}

        {children ? <RevealItem className="lg:col-span-12">{children}</RevealItem> : null}
      </Reveal>
    </section>
  )
}

/** The narrative card, over the window's spend curve. Its backdrop is the page's one effect area. */
function Lead({
  windowLabel,
  compareLabel,
  compare,
  loading,
  sentences,
  spike,
  spikeLink,
  unpriced,
  changePct,
  points,
}: {
  windowLabel: string
  compareLabel?: string
  compare: boolean
  loading: boolean
  sentences: string[]
  spike: Spike | null
  spikeLink: (children: ReactNode) => ReactNode
  unpriced: boolean
  changePct: number | null | undefined
  points: TimelinePoint[]
}) {
  return (
    <SpotlightCard className="h-full min-h-64">
      <div className="relative flex flex-1 flex-col gap-4 p-5 pb-0">
        <Badge variant="outline" className="border-primary/30 bg-primary/10 text-primary-text">
          <span>{windowLabel}</span>
          <span aria-hidden>·</span>
          <span>{compare ? `vs ${compareLabel ?? 'the period before'}` : 'Compare is off'}</span>
        </Badge>
        {loading ? (
          <div aria-busy="true" className="flex flex-col gap-2">
            <span className="sr-only">Loading summary</span>
            <Skeleton className="h-6 w-4/5" />
            <Skeleton className="h-6 w-2/5" />
          </div>
        ) : (
          <p className="max-w-[60ch] text-xl leading-8 text-balance text-foreground">
            <span data-testid="summary-narrative">
              {emphasize(
                sentences.filter((s) => !spike || !s.startsWith('Spend peaked')).join(' '),
                changePct,
              )}
            </span>
            {spike ? (
              <>
                {' '}
                <m.span layoutId={morphId(`day-header-${spike.date}`)} className="inline">
                  {spikeSentence(spike)}
                </m.span>
                .
              </>
            ) : null}
          </p>
        )}
        {spike || unpriced ? (
          <p className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-muted-foreground">
            {spike
              ? spikeLink(
                  <>
                    See sessions for {fmtShortDay(spike.date)} <span aria-hidden>→</span>
                  </>,
                )
              : null}
            {unpriced ? <span>Some calls are unpriced, so totals are a floor</span> : null}
          </p>
        ) : null}
      </div>
      <SpendCurve points={points} spikeIso={spike?.date} />
    </SpotlightCard>
  )
}

const CURVE_W = 600
const CURVE_H = 72

/**
 * The window's spend as a filled curve along the card's foot, with the peak marked. Decorative here (`aria-hidden`):
 * the Spend over time chart below carries the same buckets with axes, a tooltip and a Table view.
 */
function SpendCurve({ points, spikeIso }: { points: TimelinePoint[]; spikeIso?: string }) {
  if (points.length < 2) return <div className="h-6" />
  const max = Math.max(...points.map((p) => p.spend), 0)
  if (max <= 0) return <div className="h-6" />
  const x = (i: number) => (i / (points.length - 1)) * CURVE_W
  const y = (v: number) => CURVE_H - 4 - (v / max) * (CURVE_H - 12)
  const fill = area<TimelinePoint>()
    .x((_, i) => x(i))
    .y0(CURVE_H)
    .y1((p) => y(p.spend))
    .curve(curveMonotoneX)(points)
  const stroke = line<TimelinePoint>()
    .x((_, i) => x(i))
    .y((p) => y(p.spend))
    .curve(curveMonotoneX)(points)
  const peakIndex = points.findIndex((p) =>
    spikeIso ? p.iso.startsWith(spikeIso) : p.spend === max,
  )
  const peak = points[peakIndex]
  return (
    <svg
      aria-hidden
      viewBox={`0 0 ${CURVE_W} ${CURVE_H}`}
      preserveAspectRatio="none"
      className="relative mt-4 block h-18 w-full"
    >
      <defs>
        <linearGradient id="lead-curve" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0%" stopColor="var(--chart-1)" stopOpacity={0.35} />
          <stop offset="100%" stopColor="var(--chart-1)" stopOpacity={0} />
        </linearGradient>
      </defs>
      <m.path
        d={fill ?? undefined}
        fill="url(#lead-curve)"
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        transition={{ duration: 0.6 }}
      />
      <m.path
        d={stroke ?? undefined}
        fill="none"
        stroke="var(--chart-1-edge)"
        strokeWidth={1.5}
        vectorEffect="non-scaling-stroke"
        initial={{ pathLength: 0 }}
        animate={{ pathLength: 1 }}
        transition={{ duration: 0.9, ease: 'easeOut' }}
      />
      {peak ? (
        <line
          x1={x(peakIndex)}
          x2={x(peakIndex)}
          y1={y(peak.spend)}
          y2={CURVE_H}
          stroke="var(--chart-1-edge)"
          strokeDasharray="2 3"
          vectorEffect="non-scaling-stroke"
        />
      ) : null}
    </svg>
  )
}

/** Money in bold and the "N% more/less" (or "N× …") change in its tone; the words carry the direction, so colour is never alone. */
function emphasize(text: string, changePct: number | null | undefined): ReactNode[] {
  return text.split(/(\$[\d,]+(?:\.\d+)?|\d+% (?:more|less)|[\d,]+× as much as)/).map((part, i) =>
    i % 2 === 0 ? (
      part
    ) : part.startsWith('$') ? (
      // eslint-disable-next-line @eslint-react/no-array-index-key -- a piece of one split sentence: parts can repeat, position is identity
      <strong key={i} className="font-semibold">
        {part}
      </strong>
    ) : (
      <span
        // eslint-disable-next-line @eslint-react/no-array-index-key -- a piece of one split sentence: parts can repeat, position is identity
        key={i}
        className={cn('font-medium', TONE_CLASS[describeDelta(changePct, POLARITY.spend).tone])}
      >
        {part}
      </span>
    ),
  )
}

function Tile({
  label,
  aside,
  value,
  children,
}: {
  label: string
  aside?: ReactNode
  value: ReactNode
  children?: ReactNode
}) {
  return (
    // A `dl` group: only `dt`/`dd` may sit in it, so the aside (a delta) lives in the term's row.
    <RevealItem className="flex min-w-0 flex-col gap-1.5 rounded-xl border bg-card p-4 transition-colors hover:border-foreground/20">
      <dt className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span>{label}</span>
        {aside}
      </dt>
      <dd className="flex flex-col gap-2">
        <span className="text-xl font-semibold tracking-tight tabular-nums sm:text-2xl">
          {value}
        </span>
        {children}
      </dd>
    </RevealItem>
  )
}

function Stat({ label, value, children }: { label: string; value: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 items-baseline justify-between gap-2 bg-card px-4 py-2.5">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="flex items-baseline gap-2">
        <span className="font-mono text-sm font-medium tabular-nums">{value}</span>
        {children}
      </dd>
    </div>
  )
}

function Meter({ pct }: { pct: number }) {
  return (
    <div className="h-1.5 w-full rounded-full bg-muted" aria-hidden>
      <div
        className="h-full rounded-full bg-chart-1-edge"
        style={{ width: `${Math.min(100, pct)}%` }}
      />
    </div>
  )
}

/** Month to date against the forecast band. */
function MonthMeter({ month }: { month: MonthSummary | null }) {
  if (!month) return <Skeleton className="h-1.5 w-full" aria-label="Loading month" />
  const high = month.show && month.high ? month.high : null
  const low = month.show && month.low ? month.low : null
  const over = high !== null && month.mtd > high
  const scale = high ? Math.max(high, month.mtd) : Math.max(month.mtd, 1)
  const pct = (v: number) => `${Math.min(100, (v / scale) * 100)}%`
  return (
    <div className="flex flex-col gap-1.5" role="group" aria-label="Month progress">
      <div className="relative h-1.5 w-full rounded-full bg-muted" aria-hidden>
        {low !== null && high !== null ? (
          // The forecast band itself, so the range reads as a range.
          <div
            className="absolute inset-y-0 rounded-full bg-foreground/10"
            style={{ left: pct(low), right: `calc(100% - ${pct(high)})` }}
          />
        ) : null}
        <div
          className={cn(
            'absolute inset-y-0 left-0 rounded-full',
            over ? 'bg-warning' : 'bg-primary',
          )}
          style={{ width: pct(month.mtd) }}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        <span className="font-medium text-foreground tabular-nums">{fmtMoney(month.mtd)}</span>{' '}
        spent this month
        {over ? (
          <span className="font-medium text-warning"> · over forecast</span>
        ) : high === null ? (
          <>
            {' '}
            ·{' '}
            {month.elapsedDays < 1
              ? 'forecast starts tomorrow'
              : `forecast appears after ${FORECAST_MIN_DAYS} days`}
          </>
        ) : null}
      </p>
    </div>
  )
}
