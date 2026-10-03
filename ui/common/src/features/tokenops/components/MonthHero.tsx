/**
 * F1 — "Where am I this month?" (plan A1, A3, A6, A8, A11).
 *
 * Always month-to-date, independent of the page's time window. Forecast is a band,
 * hidden before 3 days. The calendar is a keyboard grid (arrow keys move a roving
 * focus, Enter opens a day) whose cells carry an aria-label with the value, so colour
 * is never the only signal.
 */
import { AlertTriangle, CalendarDays } from 'lucide-react'
import { useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { cn } from '@/lib/utils'
import { POLARITY } from '@/lib/delta'
import { FORECAST_MIN_DAYS, type MonthSummary } from '../forecast'
import {
  fmtMoney,
  fmtMoneyFloor,
  fmtMoneyShort,
  fmtMonthYear,
  fmtPct,
  fmtShortDay,
} from '@/lib/format'
import type { SpendCalendarDay } from '../types'
import { DAY_MS } from '../window'
import { PanelError, PanelSkeleton } from '@/components/shared/panel'
import { Delta } from '@/components/shared/delta'

export function MonthHero({
  summary,
  unpricedCalls,
  days,
  monthStart,
  today,
  selectedDay,
  onSelectDay,
  loading,
  error,
  onRetry,
}: {
  summary: MonthSummary | null
  /** This month's unpriced calls, or null when not known (only the MTD window reports it). */
  unpricedCalls: number | null
  days: SpendCalendarDay[] | undefined
  monthStart: Date
  today: string
  selectedDay?: string
  onSelectDay: (date: string) => void
  loading: boolean
  error: unknown
  onRetry: () => void
}) {
  const [showCalendar, setShowCalendar] = useState(false)
  const floor = (unpricedCalls ?? 0) > 0
  return (
    <Card asChild className="grid gap-4 p-4 lg:grid-cols-12">
      <section aria-labelledby="month-hero-title">
        <div className="flex flex-col gap-3 lg:col-span-5">
          <div className="flex items-center justify-between gap-2">
            <h2 id="month-hero-title" className="text-sm font-semibold">
              This month{' '}
              <span className="font-normal text-muted-foreground">
                · {fmtMonthYear(monthStart)} (UTC)
              </span>
            </h2>
            <Button
              variant="ghost"
              size="sm"
              className="lg:hidden"
              onClick={() => setShowCalendar((v) => !v)}
              aria-expanded={showCalendar}
              aria-controls="month-calendar"
            >
              <CalendarDays className="size-4" aria-hidden /> Calendar
            </Button>
          </div>
          {loading && !summary ? (
            <PanelSkeleton height={96} />
          ) : error && !summary ? (
            <PanelError error={error} onRetry={onRetry} what="this month's spend" />
          ) : summary ? (
            <>
              <div>
                <div className="text-xs text-muted-foreground">Spend so far</div>
                <div className="text-3xl font-semibold tabular-nums" data-testid="mtd-spend">
                  {fmtMoneyFloor(summary.mtd, floor)}
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
                  {summary.vsLastMonthPct !== null ? (
                    <span className="inline-flex items-center gap-1">
                      <Delta changePct={summary.vsLastMonthPct} polarity={POLARITY.spend} /> vs same
                      days last month ({fmtMoney(summary.lastMonthSameDays)})
                    </span>
                  ) : (
                    <span>No spend in the same days last month</span>
                  )}
                  <span>Last month total {fmtMoney(summary.lastMonthTotal)}</span>
                </div>
              </div>
              <div className="rounded-md bg-muted/60 p-3">
                <div className="text-xs text-muted-foreground">Month-end at current pace</div>
                {summary.show && summary.low !== null && summary.high !== null ? (
                  <div className="text-lg font-semibold tabular-nums" data-testid="forecast">
                    {fmtMoney(summary.low)} – {fmtMoney(summary.high)}
                  </div>
                ) : (
                  <div className="text-sm text-muted-foreground" data-testid="forecast">
                    Not enough data yet (available from day {FORECAST_MIN_DAYS} with spend)
                  </div>
                )}
                <p className="mt-1 text-sm text-muted-foreground">
                  The range uses the lower and higher of the median and mean daily spend, over{' '}
                  {Math.floor(summary.elapsedDays)} completed days (
                  {fmtPct((summary.elapsedDays / summary.daysInMonth) * 100)} of the month).
                </p>
              </div>
              {floor ? (
                <Badge
                  variant="warning"
                  title="These calls used tokens but their model has no price, so totals are a floor."
                >
                  <AlertTriangle className="size-3" aria-hidden /> {unpricedCalls?.toLocaleString()}{' '}
                  calls unpriced this month — totals are a floor
                </Badge>
              ) : null}
              <p className="text-xs text-muted-foreground">
                Source: spend calendar (all trace spend, UTC)
              </p>
            </>
          ) : null}
        </div>
        <div
          id="month-calendar"
          className={cn('lg:col-span-7', showCalendar ? 'block' : 'hidden lg:block')}
        >
          {loading && !days ? (
            <PanelSkeleton height={220} />
          ) : error && !days ? (
            <PanelError error={error} onRetry={onRetry} what="the spend calendar" />
          ) : (
            <MonthCalendar
              days={days ?? []}
              monthStart={monthStart}
              today={today}
              selectedDay={selectedDay}
              onSelectDay={onSelectDay}
            />
          )}
        </div>
      </section>
    </Card>
  )
}

const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

function MonthCalendar({
  days,
  monthStart,
  today,
  selectedDay,
  onSelectDay,
}: {
  days: SpendCalendarDay[]
  monthStart: Date
  today: string
  selectedDay?: string
  onSelectDay: (date: string) => void
}) {
  const byDate = useMemo(() => new Map(days.map((d) => [d.date, d])), [days])
  const maxDate = useMemo(
    () =>
      days.reduce<SpendCalendarDay | null>(
        (m, d) => (!m || d.spend_usd > m.spend_usd ? d : m),
        null,
      ),
    [days],
  )
  const year = monthStart.getUTCFullYear()
  const month = monthStart.getUTCMonth()
  const count = new Date(Date.UTC(year, month + 1, 0)).getUTCDate()
  const lead = (new Date(Date.UTC(year, month, 1)).getUTCDay() + 6) % 7 // Monday-first
  const cells: (string | null)[] = [
    ...Array(lead).fill(null),
    ...Array.from(
      { length: count },
      (_, i) => `${year}-${String(month + 1).padStart(2, '0')}-${String(i + 1).padStart(2, '0')}`,
    ),
  ]
  while (cells.length % 7) cells.push(null)
  const weeks = Array.from({ length: cells.length / 7 }, (_, w) => cells.slice(w * 7, w * 7 + 7))
  const refs = useRef<Map<string, HTMLButtonElement>>(new Map())
  const firstDay = cells.find(Boolean) as string
  const shown = (d: string | undefined): d is string =>
    !!d && d.slice(0, 7) === firstDay.slice(0, 7) && d <= today
  const initial = shown(selectedDay) ? selectedDay : shown(today) ? today : firstDay
  // Roving tabindex: follows keyboard focus, so Tab out and back returns to the last cell.
  // Falls back to `initial` when the remembered cell is no longer shown or enabled
  // (month rolled over, or a future ?day=), so exactly one enabled cell stays tabbable.
  const [focusedState, setFocused] = useState(initial)
  const focused = shown(focusedState) ? focusedState : initial

  const onKey = (e: KeyboardEvent, date: string) => {
    const step = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 }[e.key]
    if (!step) return
    e.preventDefault()
    const next = new Date(Date.parse(`${date}T00:00:00Z`) + step * DAY_MS)
      .toISOString()
      .slice(0, 10)
    const el = refs.current.get(next)
    if (el && !el.disabled) {
      setFocused(next)
      el.focus()
    }
  }

  return (
    // The one allowed hand-rolled grid (plan §8 Phase 1 exceptions): shadcn `Table` parts, the ARIA grid pattern kept.
    <Table
      role="grid"
      aria-label="Daily spend this month (UTC). Use arrow keys to move, Enter to open a day."
      className="table-fixed border-separate border-spacing-1 text-xs"
    >
      <TableHeader className="[&_tr]:border-0">
        <TableRow className="border-0 hover:bg-transparent">
          {WEEKDAYS.map((d) => (
            <TableHead
              key={d}
              scope="col"
              className="h-auto px-0 text-center font-normal text-muted-foreground"
            >
              {d}
            </TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody>
        {weeks.map((week, wi) => (
          // eslint-disable-next-line @eslint-react/no-array-index-key -- a calendar row: its week position is its identity
          <TableRow key={wi} className="border-0 hover:bg-transparent">
            {week.map((date, di) => {
              // eslint-disable-next-line @eslint-react/no-array-index-key -- a padding cell: its weekday column is its identity
              if (!date) return <TableCell key={di} className="p-0" />
              const d = byDate.get(date)
              const future = date > today
              const intensity = d?.intensity ?? 0
              const label = `${fmtShortDay(date)}, ${d ? fmtMoney(d.spend_usd) : 'no spend'}${maxDate && d && maxDate.date === date ? ', highest this month' : ''}`
              return (
                <TableCell
                  key={date}
                  role="gridcell"
                  aria-selected={selectedDay === date}
                  className="p-0"
                >
                  <Button
                    ref={(el) => {
                      if (el) refs.current.set(date, el)
                      else refs.current.delete(date)
                    }}
                    variant="ghost"
                    disabled={future}
                    tabIndex={date === focused ? 0 : -1}
                    aria-label={label}
                    title={label}
                    onFocus={() => setFocused(date)}
                    onKeyDown={(e) => onKey(e, date)}
                    onClick={() => onSelectDay(date)}
                    className={cn(
                      // Disabled (future) cells keep pointer events so their title still explains them.
                      'relative h-10 w-full flex-col items-start justify-between gap-0 border border-border/60 p-1 text-left text-xs font-normal disabled:pointer-events-auto disabled:cursor-not-allowed disabled:opacity-40',
                      selectedDay === date && 'ring-2 ring-ring',
                      date === today && 'border-primary-text',
                    )}
                    // Capped at ~55% tint so cell text keeps readable contrast in both themes.
                    style={{
                      backgroundColor:
                        intensity > 0
                          ? `color-mix(in oklch, var(--chart-1-edge) ${Math.round(10 + intensity * 45)}%, transparent)`
                          : undefined,
                    }}
                  >
                    <span className="leading-none tabular-nums">{Number(date.slice(8))}</span>
                    {d ? (
                      <span className="text-2xs leading-none tabular-nums">
                        {fmtMoneyShort(d.spend_usd)}
                      </span>
                    ) : null}
                  </Button>
                </TableCell>
              )
            })}
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
