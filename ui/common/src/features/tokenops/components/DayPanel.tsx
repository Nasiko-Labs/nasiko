/**
 * F2 day panel: 24 UTC hours stacked by the day's top 4 agents + Others (plan A5, A9, A11).
 *
 * On the visx chart kit (src/components/charts): segments grow in, at rest under reduced motion.
 * Clicking an agent (bar segment, legend or table header) applies it as the page filter;
 * the separate "View traces" button opens F4. Names here are DISPLAY names (the server
 * resolves day slices through the agents table); the page maps them to a UUID before
 * filtering (A12). The Table view is the keyboard and screen-reader path to the per-hour
 * values. Only names that map to exactly one agent are clickable (`canFilterAgent`).
 * A user-initiated open (`announce`) moves focus to the heading and scrolls it into view
 * (it renders below the fold); a panel restored by Back doesn't steal focus. Picking
 * another day leaves focus where the user is and is announced politely. Closing returns
 * focus to whatever opened the day last.
 */
import { BarChart3, ListTree, Table2, X } from 'lucide-react'
import { m } from 'motion/react'
import { useCallback, useEffect, useRef, useState, type MouseEvent, type ReactNode } from 'react'
import { Bar } from '@/components/charts/bar'
import { BarChart } from '@/components/charts/bar-chart'
import { BarXAxis } from '@/components/charts/bar-x-axis'
import type { TooltipData } from '@/components/charts/chart-context'
import { ChartHover } from '@/components/charts/chart-hover'
import { Grid } from '@/components/charts/grid'
import { ReferenceLine } from '@/components/charts/reference-line'
import { ChartTooltip } from '@/components/charts/tooltip/chart-tooltip'
import { YAxis } from '@/components/charts/y-axis'
import { Button } from '@/components/ui/button'
import {
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { morphId } from '@/features/observability/tuning'
import { prefersReducedMotion } from '@/lib/useMediaQuery'
import { cn } from '@/lib/utils'
import {
  buildConcentration,
  segmentAt,
  type Concentration,
  type ConcentrationRow,
} from '../concentration'
import { fmtLongDay, fmtMoney, fmtMoneyAxis } from '@/lib/format'
import type { FinopsDayDrilldown } from '../types'
import { Panel, PanelEmpty, PanelError, PanelSkeleton } from '@/components/shared/panel'
import { CELL, ChartTable, HEAD, NUM, ROW_HEAD, STICKY_HEAD } from './ChartTable'
import { Swatch } from '@/components/shared/chart-marks'
import { OTHER_SERIES, seriesAt } from '@/lib/chart'

const TITLE_ID = 'day-panel-title'

export function DayPanel({
  date,
  data,
  loading,
  error,
  onRetry,
  onClose,
  announce,
  onAnnounced,
  canFilterAgent,
  onFilterAgent,
  onViewTraces,
  sessionsLink,
  morph = true,
}: {
  date: string
  data: FinopsDayDrilldown | undefined
  loading: boolean
  error: unknown
  onRetry: () => void
  onClose: () => void
  /** True right after the user opened the panel: take focus and scroll into view, then call onAnnounced. */
  announce: boolean
  onAnnounced: () => void
  /** False for names the page can't map to one agent (ambiguous, not yet loaded, or no access). */
  canFilterAgent: (displayName: string) => boolean
  onFilterAgent: (displayName: string) => void
  onViewTraces: () => void
  /** "See sessions →" for this day; also the header's shared-layout (morph) source. */
  sessionsLink?: ReactNode
  /** False when another element on the page already owns this day's morph (the summary's spike clause). */
  morph?: boolean
}) {
  const [asTable, setAsTable] = useState(false)
  const opener = useRef<HTMLElement | null>(null)
  const panel = useRef<HTMLElement>(null)
  const heading = useRef<HTMLHeadingElement>(null)
  // Remember what opened this day (calendar cell, timeline row); focus goes back there on close.
  useEffect(() => {
    const active = document.activeElement
    if (
      active instanceof HTMLElement &&
      active !== document.body &&
      !panel.current?.contains(active)
    )
      opener.current = active
  }, [date])
  // A user open: bring the panel into view and move focus to it. Later day changes (e.g.
  // arrowing through the calendar with the panel open) leave focus where the user is.
  useEffect(() => {
    if (!announce) return
    heading.current?.scrollIntoView?.({
      block: 'start',
      behavior: prefersReducedMotion() ? 'auto' : 'smooth',
    })
    heading.current?.focus({ preventScroll: true })
    onAnnounced()
  }, [announce, onAnnounced])
  useEffect(
    () => () => {
      if (opener.current?.isConnected) opener.current.focus()
    },
    [],
  )
  // The body re-renders (skeleton) when the filter changes the query; keep focus on the heading.
  const filter = (name: string) => {
    onFilterAgent(name)
    heading.current?.focus({ preventScroll: true })
  }
  const label = fmtLongDay(date)
  const c = data ? buildConcentration(data) : null
  return (
    <Panel
      title={`${label} · hour by hour (UTC)`}
      subtitle={
        c && !c.isEmpty
          ? `${fmtMoney(c.total)} total · averaging ${fmtMoney(c.avgHourly)}/hour`
          : undefined
      }
      labelledBy={TITLE_ID}
      focusableTitle
      ref={panel}
      titleRef={heading}
      actions={
        <>
          {/* The label names the action; no aria-pressed ("Chart, pressed" would claim the chart is showing). */}
          <Button variant="ghost" size="sm" onClick={() => setAsTable((v) => !v)}>
            {asTable ? (
              <BarChart3 className="size-4" aria-hidden />
            ) : (
              <Table2 className="size-4" aria-hidden />
            )}
            {asTable ? 'Chart' : 'Table'}
          </Button>
          {sessionsLink ? (
            <m.span
              layoutId={morph ? morphId(`day-header-${date}`) : undefined}
              className="inline-flex items-center"
            >
              {sessionsLink}
            </m.span>
          ) : null}
          <Button variant="outline" size="sm" onClick={onViewTraces}>
            <ListTree className="size-4" aria-hidden /> View traces
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="size-10 md:size-8"
            onClick={onClose}
            aria-label="Close day panel"
          >
            <X className="size-4" aria-hidden />
          </Button>
        </>
      }
    >
      <span className="sr-only" aria-live="polite">{`Showing ${label}`}</span>
      {loading && !data ? (
        <PanelSkeleton height={220} />
      ) : error && !data ? (
        <PanelError error={error} onRetry={onRetry} what="the hourly breakdown" />
      ) : !c || c.isEmpty ? (
        <PanelEmpty title="No spend on this day" />
      ) : asTable ? (
        <HoursTable c={c} canFilterAgent={canFilterAgent} onFilterAgent={filter} />
      ) : (
        <div className="grid gap-4 md:grid-cols-[1fr_230px]">
          <HourChart c={c} label={label} canFilterAgent={canFilterAgent} onFilterAgent={filter} />
          <ul className="flex flex-col gap-1 text-xs" aria-label="Top agents this day">
            {c.series.map((s, i) => {
              const spend = s.isOthers
                ? (data?.others_spend_usd ?? 0)
                : (data?.top_agents[i]?.spend_usd ?? 0)
              return (
                <li key={s.key}>
                  {s.isOthers || !canFilterAgent(s.label) ? (
                    <div
                      className="flex items-center justify-between gap-2 px-2 py-1"
                      title={
                        s.isOthers
                          ? undefined
                          : "Can't filter by this name: it matches no single agent you can access, or the agent list is still loading."
                      }
                    >
                      <span className="inline-flex min-w-0 items-center gap-1.5">
                        <Swatch series={s.isOthers ? OTHER_SERIES : seriesAt(i)} />
                        <span
                          className={s.isOthers ? 'truncate' : 'truncate text-muted-foreground'}
                        >
                          {s.label}
                        </span>
                      </span>
                      <span className="tabular-nums">{fmtMoney(spend)}</span>
                    </div>
                  ) : (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => filter(s.label)}
                      className="group h-auto w-full justify-between gap-2 rounded px-2 py-1 text-left text-xs font-normal hover:bg-muted"
                      title={`Filter the page to ${s.label}`}
                    >
                      <span className="inline-flex min-w-0 items-center gap-1.5">
                        <Swatch series={seriesAt(i)} />
                        <span className="truncate underline-offset-2 group-hover:underline">
                          {s.label}
                        </span>
                      </span>
                      <span className="tabular-nums">{fmtMoney(spend)}</span>
                    </Button>
                  )}
                </li>
              )
            })}
          </ul>
        </div>
      )}
    </Panel>
  )
}

const HOUR_MARGIN = { top: 8, right: 8, bottom: 28, left: 52 }

/**
 * The stacked hourly bars. A click filters by the agent whose segment is under the pointer
 * (`segmentAt`, from the tops the kit reports for the hovered column).
 */
function HourChart({
  c,
  label,
  canFilterAgent,
  onFilterAgent,
}: {
  c: Concentration
  label: string
  canFilterAgent: (displayName: string) => boolean
  onFilterAgent: (displayName: string) => void
}) {
  const hovered = useRef<TooltipData | null>(null)
  const pressed = useRef<TooltipData | null>(null)
  const onHover = useCallback((h: TooltipData | null) => {
    hovered.current = h
  }, [])
  const colour = (i: number) => (c.series[i]?.isOthers ? OTHER_SERIES : seriesAt(i))
  const onClick = (e: MouseEvent<HTMLElement>) => {
    const h = pressed.current
    if (!h) return
    const y = e.clientY - e.currentTarget.getBoundingClientRect().top - HOUR_MARGIN.top
    const s = segmentAt(c.series, h.yPositions, h.point, y)
    if (s && !s.isOthers && canFilterAgent(s.label)) onFilterAgent(s.label)
  }
  return (
    <figure
      className="m-0 h-55"
      aria-label={`Hourly spend on ${label}; top agent ${c.series[0]?.label ?? 'none'}.`}
    >
      {/* Pointer-only, so presentation: the legend buttons and the Table view carry the same filter by keyboard. */}
      <div
        role="presentation"
        className="h-full cursor-pointer"
        onPointerDown={() => {
          pressed.current = hovered.current
        }}
        onClick={onClick}
      >
        <BarChart
          data={c.rows as unknown as Record<string, unknown>[]}
          xDataKey="label"
          stacked
          stackGap={2}
          aspectRatio="auto"
          className="h-full"
          margin={HOUR_MARGIN}
          barGap={0.25}
        >
          <Grid />
          <YAxis formatValue={fmtMoneyAxis} numTicks={4} />
          {c.series.map((s, i) => (
            <Bar
              key={s.key}
              dataKey={s.key}
              fill={colour(i).fill}
              edge={colour(i).edge}
              stroke={colour(i).edge}
              lineCap="butt"
            />
          ))}
          <ReferenceLine value={c.avgHourly} label={`avg ${fmtMoney(c.avgHourly)}`} />
          <BarXAxis maxLabels={8} />
          <ChartTooltip
            showDatePill={false}
            content={({ point }) => (
              <HourTooltip c={c} row={point as unknown as ConcentrationRow} />
            )}
          />
          <ChartHover onChange={onHover} />
        </BarChart>
      </div>
    </figure>
  )
}

function HourTooltip({ c, row }: { c: Concentration; row: ConcentrationRow }) {
  return (
    <div className="px-3 py-2 text-xs text-popover-foreground">
      <div className="mb-1 font-medium">
        {row.label} UTC · {fmtMoney(row.total)}
      </div>
      <div className="grid grid-cols-[auto_auto] gap-x-3 gap-y-0.5 tabular-nums">
        {c.series.map((s) => (
          <span key={s.key} className="contents">
            <span className="truncate text-muted-foreground">{s.label}</span>
            <span>{fmtMoney(Number(row[s.key]))}</span>
          </span>
        ))}
      </div>
    </div>
  )
}

function HoursTable({
  c,
  canFilterAgent,
  onFilterAgent,
}: {
  c: Concentration
  canFilterAgent: (displayName: string) => boolean
  onFilterAgent: (displayName: string) => void
}) {
  return (
    <ChartTable className="max-h-80">
      <TableCaption className="sr-only">
        Spend per UTC hour by agent; agent headers filter the page.
      </TableCaption>
      <TableHeader className={STICKY_HEAD}>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col" className={HEAD}>
            Hour (UTC)
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Total
          </TableHead>
          {c.series.map((s) => (
            <TableHead key={s.key} scope="col" className={cn(HEAD, 'text-right')}>
              {s.isOthers || !canFilterAgent(s.label) ? (
                s.label
              ) : (
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto p-0 text-xs"
                  onClick={() => onFilterAgent(s.label)}
                  title={`Filter the page to ${s.label}`}
                >
                  {s.label}
                </Button>
              )}
            </TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody>
        {c.rows.map((r) => (
          <TableRow key={r.hour}>
            <TableHead scope="row" className={ROW_HEAD}>
              {r.label}
            </TableHead>
            <TableCell className={cn(CELL, NUM)}>{fmtMoney(r.total)}</TableCell>
            {c.series.map((s) => (
              <TableCell key={s.key} className={cn(CELL, NUM)}>
                {fmtMoney(Number(r[s.key]))}
              </TableCell>
            ))}
          </TableRow>
        ))}
      </TableBody>
    </ChartTable>
  )
}
