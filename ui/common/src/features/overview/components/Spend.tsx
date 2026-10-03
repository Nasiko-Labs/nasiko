/**
 * Spend by agent (plans/feat-overview.md §7): fleet spend over the page's range against the window before it, the chart
 * stacked by the drivers (or as lines), the spike, and the top 5 drivers plus Other in a table (ACL-scoped, with a
 * scope note), each keyed to its colour in the chart; a row's eye hides its series in the chart. The month-to-date and
 * forecast line is the Spend tile's (eng review R2: the calendar is fleet-wide, so it is said as fleet spend).
 */
import { Link } from '@tanstack/react-router'
import { BarChart3, Eye, EyeOff, LineChart } from 'lucide-react'
import { useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { spikeSentence } from '@/features/narrative/tokenops'
import { Swatch } from '@/components/shared/chart-marks'
import { EmptyState } from '@/components/shared/state-card'
import { Delta } from '@/components/shared/delta'
import { Button } from '@/components/ui/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { OTHER_SERIES, seriesAt, type Series } from '@/lib/chart'
import { POLARITY } from '@/lib/delta'
import { TEMPO_MAX_SEARCH_MS } from '@/features/observability/tuning'
import { fmtMoney, fmtMoneyFloor, fmtPct } from '@/lib/format'
import type { Spend as SpendData } from '../api'
import { copy } from '../copy'
import { Card, CardError, CardSkeleton, SourceFailed, TOUCH } from './Card'
import { SpendChart, type ChartMode } from './SpendChart'

const OTHER = 'other'

export function Spend({
  spend,
  now,
  className,
}: {
  spend: SpendData
  now: number
  className?: string
}) {
  const titleRef = useRef<HTMLHeadingElement>(null)
  const [mode, setMode] = useState<ChartMode>('bars')
  // Hidden series by driver id (or "other"), so a hidden driver stays hidden when the range reorders the stack.
  const [hiddenIds, setHiddenIds] = useState<ReadonlySet<string>>(new Set())
  const { totals, stack, rangeDays } = spend
  // A driver's colour and layer in the chart, when the chart is stacked by drivers.
  const keyed = new Map(stack?.series.map((s, i) => [s.id, { key: s.key, series: seriesAt(i) }]))
  if (stack) keyed.set(OTHER, { key: OTHER, series: OTHER_SERIES })
  const hidden = new Set(
    [...hiddenIds].flatMap((id) => {
      const k = keyed.get(id)
      return k ? [k.key] : []
    }),
  )
  // The last visible series can't be hidden: an empty chart says nothing.
  const visible = keyed.size - hidden.size
  const toggle = (id: string) =>
    setHiddenIds((prev) => {
      const next = new Set(prev)
      if (!next.delete(id)) next.add(id)
      return next
    })
  const change =
    totals?.previous && totals.previous.spend > 0
      ? ((totals.spend - totals.previous.spend) / totals.previous.spend) * 100
      : null
  const anySpend = spend.days.some((d) => d.spend > 0)
  return (
    <Card
      id="overview-spend"
      title={copy.spend.title}
      meta={copy.spend.byAgent(rangeDays)}
      to="/tokenops"
      linkLabel={copy.spend.link}
      titleRef={titleRef}
      className={className}
    >
      {!totals && !spend.totalsError ? (
        <CardSkeleton rows={6} />
      ) : !totals ? (
        <CardError what={copy.spend.rangeWhat} onRetry={spend.retry} titleRef={titleRef} />
      ) : (
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <span className="text-3xl font-semibold tracking-tight tabular-nums">
                {fmtMoneyFloor(totals.spend, spend.unpriced)}
              </span>
              {totals.previous ? (
                <span className="inline-flex items-center gap-2 text-xs text-muted-foreground">
                  <Delta changePct={change} polarity={POLARITY.spend} current={totals.spend} />
                  {copy.spend.vsPrevious(rangeDays)}
                </span>
              ) : null}
            </p>
            {anySpend ? (
              <ToggleGroup
                type="single"
                variant="outline"
                size="sm"
                value={mode}
                onValueChange={(v) => v && setMode(v as ChartMode)}
                aria-label={copy.spend.chartMode}
              >
                <ToggleGroupItem value="bars" aria-label={copy.spend.bars} className={TOUCH}>
                  <BarChart3 aria-hidden />
                </ToggleGroupItem>
                <ToggleGroupItem value="lines" aria-label={copy.spend.lines} className={TOUCH}>
                  <LineChart aria-hidden />
                </ToggleGroupItem>
              </ToggleGroup>
            ) : null}
          </div>
          {anySpend ? (
            <SpendChart
              days={spend.days}
              stack={stack}
              pending={spend.stackPending}
              rangeDays={rangeDays}
              mode={mode}
              hidden={hidden}
            />
          ) : null}
          {spend.spike ? (
            <p className="text-xs">
              {spikeSentence(spend.spike)}.{' '}
              {/* Sessions searches the trace store from the day to now, which fails past 7 days: older spikes open
                  TokenOps' day panel instead (/ship review). */}
              {now - Date.parse(`${spend.spike.date}T00:00:00Z`) < TEMPO_MAX_SEARCH_MS ? (
                <Button asChild variant="link" size="sm" className={`h-auto px-0 text-xs ${TOUCH}`}>
                  <Link to="/sessions" search={{ day: spend.spike.date, sort: 'cost' } as never}>
                    {copy.spend.seeSessions} <span aria-hidden>→</span>
                  </Link>
                </Button>
              ) : (
                <Button asChild variant="link" size="sm" className={`h-auto px-0 text-xs ${TOUCH}`}>
                  <Link to="/tokenops" search={{ day: spend.spike.date } as never}>
                    {copy.spend.seeDay} <span aria-hidden>→</span>
                  </Link>
                </Button>
              )}
            </p>
          ) : null}
          {spend.driversError && !spend.drivers.length ? (
            <SourceFailed what={copy.spend.driversWhat} onRetry={spend.retry} />
          ) : spend.drivers.length ? (
            <Table className="text-sm" aria-label={copy.spend.drivers(rangeDays)}>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead className="h-9 px-2 text-xs font-normal text-muted-foreground">
                    {copy.spend.colAgent}
                  </TableHead>
                  {/* Columns drop out as the card narrows (its own width, not the page's). */}
                  <TableHead className="hidden h-9 w-[30%] px-2 text-xs font-normal text-muted-foreground @[640px]/card:table-cell">
                    {copy.spend.colShare}
                  </TableHead>
                  <TableHead className="h-9 px-2 text-right text-xs font-normal text-muted-foreground">
                    {copy.spend.colSpend}
                  </TableHead>
                  <TableHead className="hidden h-9 px-2 text-right text-xs font-normal text-muted-foreground @[480px]/card:table-cell">
                    {copy.spend.colAvg}
                  </TableHead>
                  <TableHead className="h-9 px-2 text-right text-xs font-normal text-muted-foreground">
                    {copy.spend.colChange}
                  </TableHead>
                  {stack ? (
                    <TableHead className="h-9 w-10 px-0">
                      <span className="sr-only">{copy.spend.colShow}</span>
                    </TableHead>
                  ) : null}
                </TableRow>
              </TableHeader>
              <TableBody>
                {spend.drivers.map((r) => (
                  <DriverRow
                    key={r.id}
                    id={r.id}
                    name={r.name}
                    series={keyed.get(r.id)?.series}
                    sharePct={r.sharePct}
                    cost={r.cost}
                    perDay={r.cost / rangeDays}
                    delta={
                      <Delta
                        changePct={r.deltaPct}
                        polarity={POLARITY.spend}
                        unavailable={r.deltaKind === 'unavailable'}
                        current={r.cost}
                      />
                    }
                    eye={stack ? { hidden: hiddenIds.has(r.id), last: visible <= 1 } : null}
                    onToggle={toggle}
                  />
                ))}
                {spend.other ? (
                  <DriverRow
                    id={OTHER}
                    name={copy.spend.otherAgents(spend.other.count)}
                    series={stack ? OTHER_SERIES : undefined}
                    sharePct={spend.other.sharePct}
                    cost={spend.other.cost}
                    perDay={spend.other.cost / rangeDays}
                    delta={
                      <Delta
                        changePct={spend.other.deltaPct}
                        polarity={POLARITY.spend}
                        unavailable={spend.other.unavailable}
                        current={spend.other.cost}
                      />
                    }
                    eye={stack ? { hidden: hiddenIds.has(OTHER), last: visible <= 1 } : null}
                    onToggle={toggle}
                  />
                ) : null}
              </TableBody>
            </Table>
          ) : (
            <EmptyState title={copy.spend.noDrivers(rangeDays)} className="py-6 md:py-6" />
          )}
          <p className="text-xs text-muted-foreground">{copy.spend.scopeNote}</p>
        </div>
      )}
    </Card>
  )
}

function DriverRow({
  id,
  name,
  series,
  sharePct,
  cost,
  perDay,
  delta,
  eye,
  onToggle,
}: {
  id: string
  name: string
  series: Series | undefined
  sharePct: number
  cost: number
  perDay: number
  delta: ReactNode
  eye: { hidden: boolean; last: boolean } | null
  onToggle: (id: string) => void
}) {
  return (
    <TableRow
      data-testid="spend-driver"
      className={eye?.hidden ? 'text-muted-foreground' : undefined}
    >
      <TableCell className="px-2 py-2.5">
        <span className="flex min-w-0 items-center gap-2">
          {series ? <Swatch series={series} /> : null}
          <span className="max-w-40 truncate font-medium @[640px]/card:max-w-56">{name}</span>
        </span>
      </TableCell>
      <TableCell className="hidden px-2 @[640px]/card:table-cell">
        <span className="flex items-center gap-3">
          <span aria-hidden className="h-1.5 flex-1 overflow-hidden rounded-full bg-muted">
            <span
              className="block h-full rounded-full bg-(--bar)"
              style={
                {
                  width: `${Math.min(100, Math.max(1, sharePct))}%`,
                  '--bar': series?.edge ?? 'var(--chart-other-edge)',
                } as CSSProperties
              }
            />
          </span>
          <span className="w-12 text-right font-mono text-xs text-muted-foreground tabular-nums">
            {fmtPct(sharePct)}
          </span>
        </span>
      </TableCell>
      <TableCell className="px-2 text-right font-mono text-xs tabular-nums">
        {fmtMoney(cost)}
      </TableCell>
      <TableCell className="hidden px-2 text-right font-mono text-xs text-muted-foreground tabular-nums @[480px]/card:table-cell">
        {fmtMoney(perDay)}
      </TableCell>
      <TableCell className="px-2 text-right">{delta}</TableCell>
      {eye ? (
        <TableCell className="px-0 text-right">
          <Button
            variant="ghost"
            size="icon-sm"
            className="text-muted-foreground pointer-coarse:size-11"
            // A toggle keeps one name; pressed means shown.
            aria-pressed={!eye.hidden}
            aria-label={copy.spend.show(name)}
            disabled={!eye.hidden && eye.last}
            onClick={() => onToggle(id)}
          >
            {eye.hidden ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
          </Button>
        </TableCell>
      ) : null}
    </TableRow>
  )
}
