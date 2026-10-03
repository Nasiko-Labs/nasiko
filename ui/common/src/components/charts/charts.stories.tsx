// The chart kit (Bklit-derived, MIT; see NOTICE): a line chart the way pages compose it. Charts keep a Table view
// on the page itself; this story checks the kit renders and animates to rest.
import type { Meta, StoryObj } from '@storybook/react-vite'
import { fmtShortDay } from '@/lib/format'
import { ChartDateFormat } from './chart-formatters'
import { Grid } from './grid'
import { Line } from './line'
import { LineChart } from './line-chart'
import { ChartTooltip } from './tooltip/chart-tooltip'
import { XAxis } from './x-axis'
import { YAxis } from './y-axis'

const meta: Meta = { title: 'Charts/Line' }
export default meta

const DAY = 86_400_000
const START = Date.UTC(2026, 8, 1)
const data = Array.from({ length: 14 }, (_, i) => ({
  date: new Date(START + i * DAY),
  claude: 8 + Math.round(6 * Math.sin(i / 2)) + i,
  codex: 4 + ((i * 7) % 5),
}))
const SERIES = [
  { key: 'claude', label: 'Claude Code', color: 'var(--chart-1-edge)' },
  { key: 'codex', label: 'Codex', color: 'var(--chart-2-edge)' },
] as const

export const TwoSeries: StoryObj = {
  render: () => (
    <figure aria-label="Turns per day by harness" className="h-64 max-w-2xl">
      <ChartDateFormat value={(d) => fmtShortDay(d.toISOString())}>
        <LineChart
          data={data}
          xDataKey="date"
          aspectRatio="auto"
          className="h-full"
          margin={{ top: 8, right: 12, bottom: 28, left: 40 }}
        >
          <Grid />
          <YAxis numTicks={4} />
          {SERIES.map((s) => (
            <Line key={s.key} dataKey={s.key} stroke={s.color} strokeWidth={2} fadeEdges={false} />
          ))}
          <XAxis numTicks={6} />
          <ChartTooltip
            showDatePill={false}
            rows={(p) =>
              SERIES.map((s) => ({ color: s.color, label: s.label, value: String(p[s.key]) }))
            }
          />
        </LineChart>
      </ChartDateFormat>
    </figure>
  ),
}
