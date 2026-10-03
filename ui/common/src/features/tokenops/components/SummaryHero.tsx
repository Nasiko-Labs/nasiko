/**
 * TokenOps executive summary (approved mockup D1: variant C's sentence, variant A's
 * figures and meter), laid out as one card: the narrative on the left, a 2×2 figures grid on
 * the right. The narrative is the hero; the figures support it and never repeat a whole sentence. The spike clause links to that day's sessions and is a shared
 * `layoutId` source for the Sessions header morph.
 */
// Budgets hidden: Link served only the Budgets link below.
// import { Link } from '@tanstack/react-router'
import { m } from 'motion/react'
import type { ReactNode } from 'react'
import { Card } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { morphId } from '@/features/observability/tuning'
import { cn } from '@/lib/utils'
import { spikeSentence, type Spike } from '@/features/narrative/tokenops'
import { describeDelta, POLARITY, TONE_CLASS } from '@/lib/delta'
import { FORECAST_MIN_DAYS, type MonthSummary } from '../forecast'
import { fmtCostPerOp, fmtInt, fmtMoney, fmtMoneyFloor, fmtShortDay } from '@/lib/format'
import type { FinopsKpis } from '../types'
import { PanelError } from '@/components/shared/panel'
import { Delta } from '@/components/shared/delta'

export function SummaryHero({
  sentences,
  spike,
  spikeLink,
  windowLabel,
  kpis,
  totalAgents,
  compare,
  unpriced,
  month,
  loading,
  error,
  onRetry,
}: {
  sentences: string[]
  spike: Spike | null
  /** Wraps the spike link text in a link to that day's sessions. */
  spikeLink: (children: ReactNode) => ReactNode
  windowLabel: string
  kpis: FinopsKpis | undefined
  totalAgents: number | undefined
  compare: boolean
  unpriced: boolean
  month: MonthSummary | null
  loading: boolean
  error: unknown
  onRetry: () => void
}) {
  if (error) return <PanelError error={error} onRetry={onRetry} what="the summary" />
  const changePct = kpis?.total_spend.change_pct
  return (
    <Card
      asChild
      className="gap-0 overflow-hidden p-0 lg:grid lg:grid-cols-[minmax(0,1.25fr)_minmax(0,1fr)]"
    >
      <section aria-labelledby="summary-title">
        <h2 id="summary-title" className="sr-only">
          Summary
        </h2>
        <div className="flex flex-col justify-center gap-4 p-5">
          <Badge variant="outline" className="border-primary/30 bg-primary/10 text-primary-text">
            <span>{windowLabel}</span>
            <span aria-hidden>·</span>
            <span>{compare ? 'vs the period before' : 'Compare is off'}</span>
          </Badge>
          {loading ? (
            <div aria-busy="true" className="flex flex-col gap-2">
              <span className="sr-only">Loading summary</span>
              <Skeleton className="h-6 w-4/5" />
              <Skeleton className="h-6 w-2/5" />
            </div>
          ) : (
            <p className="max-w-[62ch] text-xl leading-8 text-foreground">
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

        <dl className="grid grid-cols-2 gap-px border-t border-border bg-border lg:border-t-0 lg:border-l">
          <Figure
            label="Total spend"
            value={kpis ? fmtMoneyFloor(kpis.total_spend.current, unpriced) : '—'}
          >
            {kpis && compare ? (
              <Delta
                changePct={changePct}
                polarity={POLARITY.spend}
                current={kpis.total_spend.current}
              />
            ) : null}
          </Figure>
          <Figure
            label="Month-end at pace"
            /* Budgets hidden: no server support for /api/budgets yet (R-L10). Restore when it lands.
            aside={
              // LLM router R2 (A3): budgets use router-metered token_usage, not this page's trace_usage, so only a link.
              <Link
                to="/router"
                hash="router-budgets"
                aria-label="Budgets are on the LLM router page"
                title="Budgets are on the LLM router page"
                className="text-xs font-medium text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11"
              >
                Budgets <span aria-hidden>→</span>
              </Link>
            }
            */
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
          </Figure>
          <Figure
            label="Cost / operation"
            value={kpis ? fmtCostPerOp(kpis.cost_per_operation.current) : '—'}
          >
            {kpis && compare ? (
              <Delta
                changePct={kpis.cost_per_operation.change_pct}
                polarity={POLARITY.costPerOp}
                current={kpis.cost_per_operation.current}
              />
            ) : null}
          </Figure>
          <Figure
            label="Active agents"
            value={
              kpis ? (
                <>
                  {fmtInt(kpis.active_agents.current)}
                  {totalAgents !== undefined ? (
                    <span className="ml-1.5 text-sm font-normal text-muted-foreground">
                      of {fmtInt(totalAgents)}
                    </span>
                  ) : null}
                </>
              ) : (
                '—'
              )
            }
          />
        </dl>
      </section>
    </Card>
  )
}

/** Money in bold and the "N% more/less" change in its tone; the words carry the direction, so colour is never alone. */
function emphasize(text: string, changePct: number | null | undefined): ReactNode[] {
  return text.split(/(\$[\d,]+(?:\.\d+)?|\d+% (?:more|less))/).map((part, i) =>
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

function Figure({
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
    // A `dl` group: only `dt`/`dd` may sit in it, so the aside (a link) lives in the term's row.
    <div className="flex min-w-0 flex-col gap-1 bg-card p-4">
      <dt className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span>{label}</span>
        {aside}
      </dt>
      <dd className="flex flex-col gap-2">
        <span className="text-xl font-semibold tabular-nums sm:text-2xl">{value}</span>
        {children}
      </dd>
    </div>
  )
}

/** Month to date against the forecast band (LlamaIndex credits meter). */
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
        <div
          className={cn(
            'absolute inset-y-0 left-0 rounded-full',
            over ? 'bg-warning' : 'bg-primary',
          )}
          style={{ width: pct(month.mtd) }}
        />
        {low !== null ? (
          <span
            className="absolute -inset-y-1 w-px bg-foreground/50"
            style={{ left: pct(low) }}
            title={`Forecast low ${fmtMoney(low)}`}
          />
        ) : null}
        {high !== null ? (
          <span
            className="absolute -inset-y-1 w-px bg-foreground/50"
            style={{ left: `calc(${pct(high)} - 1px)` }}
            title={`Forecast high ${fmtMoney(high)}`}
          />
        ) : null}
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
