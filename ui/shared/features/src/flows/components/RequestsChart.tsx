/**
 * Requests per day (per hour on 24h) stacked by status (plans/feat-flows.md §3, F9, F15): the kit's stacked bars in the
 * status tokens, legend in stack order. The kit's marks are aria-hidden, so the figure carries a text summary.
 */
import { Bar } from '@nasiko/ui/charts/bar'
import { BarChart } from '@nasiko/ui/charts/bar-chart'
import { BarXAxis } from '@nasiko/ui/charts/bar-x-axis'
import { Grid } from '@nasiko/ui/charts/grid'
import { ChartTooltip } from '@nasiko/ui/charts/tooltip/chart-tooltip'
import { YAxis } from '@nasiko/ui/charts/y-axis'
import { Swatch } from '@nasiko/ui/composites/chart-marks'
import type { Series } from '@nasiko/ui/lib/chart'
import { copy } from '../copy'
import { STATUSES, type Bucket, type ListStatus } from '../list'

/** F15's status map: completed success, running info, paused warning, failed destructive. */
const SERIES: Record<ListStatus, Series> = {
  completed: { fill: 'var(--success)', edge: 'var(--success)' },
  failed: { fill: 'var(--destructive)', edge: 'var(--destructive)' },
  paused: { fill: 'var(--warning)', edge: 'var(--warning)' },
  running: { fill: 'var(--info)', edge: 'var(--info)' },
}
const MARGIN = { top: 8, right: 8, bottom: 28, left: 36 }

export function RequestsChart({ rows }: { rows: Bucket[] }) {
  const total = rows.reduce((a, r) => a + r.total, 0)
  const failed = rows.reduce((a, r) => a + r.failed, 0)
  return (
    <figure className="m-0 flex flex-col gap-2">
      <figcaption className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
        {STATUSES.map((s) => (
          <span key={s} className="inline-flex items-center gap-1.5">
            <Swatch series={SERIES[s]} /> {copy.list.statuses[s]}
          </span>
        ))}
        <span className="sr-only">{copy.list.requestsSummary(total, failed)}</span>
      </figcaption>
      <div role="presentation" className="h-48">
        <BarChart
          data={rows as unknown as Record<string, unknown>[]}
          xDataKey="label"
          stacked
          aspectRatio="auto"
          className="h-full"
          margin={MARGIN}
          barGap={0.3}
        >
          <Grid />
          <YAxis numTicks={3} formatValue={(v) => String(Math.round(v))} />
          {STATUSES.map((s) => (
            <Bar
              key={s}
              dataKey={s}
              fill={SERIES[s].fill}
              edge={SERIES[s].edge}
              stroke={SERIES[s].edge}
              lineCap="butt"
            />
          ))}
          <BarXAxis maxLabels={7} />
          <ChartTooltip
            showDatePill={false}
            content={({ point }) => <Tip row={point as unknown as Bucket} />}
          />
        </BarChart>
      </div>
    </figure>
  )
}

function Tip({ row }: { row: Bucket }) {
  return (
    <div className="px-3 py-2 text-xs text-popover-foreground">
      <div className="mb-1 font-medium">{row.label}</div>
      <div className="grid grid-cols-[auto_auto] gap-x-3 gap-y-0.5 tabular-nums">
        {STATUSES.map((s) => (
          <span key={s} className="contents">
            <span className="text-muted-foreground">{copy.list.statuses[s]}</span>
            <span>{row[s]}</span>
          </span>
        ))}
      </div>
    </div>
  )
}
