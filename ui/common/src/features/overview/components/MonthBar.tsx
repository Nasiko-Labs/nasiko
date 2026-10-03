/**
 * The summary card's month bar, under the headline: fleet month-to-date (the fleet-wide calendar, eng review R2) against
 * last month's total, with the forecast band once it shows (TokenOps' MonthMeter, plus last month). The bar is
 * decorative: the lines above and below it carry every number.
 */
import { Skeleton } from '@/components/ui/skeleton'
import { FORECAST_MIN_DAYS } from '@/features/tokenops/forecast'
import { fmtMoney, fmtMoneyShort } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { Spend } from '../api'
import { copy } from '../copy'
import { SourceFailed } from './Card'

export function MonthBar({ spend }: { spend: Spend }) {
  const { summary: m } = spend
  if (!m) {
    return spend.error ? (
      <div data-testid="overview-month">
        <SourceFailed what={copy.spend.what} onRetry={spend.retry} />
      </div>
    ) : (
      <Skeleton className="h-10 w-full motion-reduce:animate-none" />
    )
  }
  const low = m.show ? m.low : null
  const high = m.show ? m.high : null
  const over = high !== null && m.mtd > high
  const scale = Math.max(m.lastMonthTotal, high ?? 0, m.mtd) || 1
  const pct = (v: number) => `${Math.min(100, (v / scale) * 100)}%`
  return (
    <div className="flex flex-col gap-2" data-testid="overview-month">
      <p className="flex flex-wrap items-baseline justify-between gap-x-4 text-sm text-muted-foreground">
        <span>{copy.month.label}</span>
        <span className="tabular-nums" data-testid="overview-mtd">
          {m.mtd === 0
            ? copy.spend.noSpendYet
            : m.lastMonthTotal > 0
              ? copy.month.ofLast(fmtMoney(m.mtd), fmtMoneyShort(m.lastMonthTotal))
              : copy.month.noLast(fmtMoney(m.mtd))}
        </span>
      </p>
      <div aria-hidden className="relative h-2 w-full rounded-full bg-muted">
        {low !== null && high !== null ? (
          <span
            className="absolute inset-y-0 rounded-full bg-primary/20"
            style={{ left: pct(low), width: `calc(${pct(high)} - ${pct(low)})` }}
          />
        ) : null}
        <span
          className={cn(
            'absolute inset-y-0 left-0 rounded-full',
            over ? 'bg-warning' : 'bg-primary',
          )}
          style={{ width: pct(m.mtd) }}
        />
        {m.lastMonthTotal > 0 ? (
          <span
            className="absolute -inset-y-1 w-px bg-foreground/50"
            style={{ left: `calc(${pct(m.lastMonthTotal)} - 1px)` }}
          />
        ) : null}
      </div>
      <p className="text-xs text-muted-foreground tabular-nums" data-testid="overview-forecast">
        {low !== null && high !== null ? (
          <>
            {copy.spend.forecast(fmtMoneyShort(low), fmtMoneyShort(high))}
            {over ? <span className="font-medium text-warning"> · {copy.month.over}</span> : null}
          </>
        ) : (
          copy.spend.forecastFrom(FORECAST_MIN_DAYS)
        )}
      </p>
    </div>
  )
}
