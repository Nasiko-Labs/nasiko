// Lab: the agent hub's stacked lanes (plans/feat-agent-centric.md eng, chart spike): bars, a line with a gap, hatched
// buckets before the subject existed, a previous-window ghost, one shared hover and the keyboard axis. Axe runs on it.
import type { Meta, StoryObj } from '@storybook/react-vite'
import { useState } from 'react'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { fmtShortDay } from '@nasiko/ui/lib/format'
import { LaneChart } from './lanes'

const meta: Meta = { title: 'Charts/Lanes' }
export default meta

const DAY = 86_400_000
const START = Date.UTC(2026, 8, 1)
const rows = Array.from({ length: 21 }, (_, i) => ({
  iso: new Date(START + i * DAY).toISOString(),
  before: i < 3,
  spend: i < 3 ? 0 : 4 + Math.round(3 * Math.sin(i / 2)) + i / 3,
  prev: i < 3 ? null : 5 + ((i * 7) % 4),
  requests: i < 3 ? 0 : 40 + ((i * 13) % 30),
  p95: i < 3 || i === 12 ? null : 800 + ((i * 97) % 600),
}))
type Row = (typeof rows)[number]

function Header({ name, total }: { name: string; total: string }) {
  return (
    <div className="flex items-baseline gap-2 text-xs">
      <span className="font-medium">{name}</span>
      <span className="text-sm font-semibold tabular-nums">{total}</span>
    </div>
  )
}

function Demo() {
  const [hover, setHover] = useState<number | null>(null)
  const [picked, setPicked] = useState<number | null>(null)
  return (
    <figure aria-label="Spend, requests and p95 latency by day" className="m-0 max-w-3xl">
      <LaneChart<Row>
        rows={rows}
        rowKey={(r) => r.iso}
        lanes={[
          {
            key: 'spend',
            kind: 'bar',
            series: seriesAt(0),
            value: (r) => r.spend,
            ghost: (r) => r.prev,
            header: <Header name="Spend" total="$121.40" />,
          },
          {
            key: 'requests',
            kind: 'bar',
            series: seriesAt(1),
            value: (r) => r.requests,
            header: <Header name="Requests" total="1,012" />,
          },
          {
            key: 'p95',
            kind: 'line',
            series: seriesAt(2),
            value: (r) => r.p95,
            header: <Header name="p95 latency" total="1.4 s" />,
          },
        ]}
        height={72}
        hatched={(r) => r.before}
        axisLabel={(r) => fmtShortDay(r.iso)}
        bucketName={(r) => `${fmtShortDay(r.iso)}: $${r.spend.toFixed(2)}, ${r.requests} requests`}
        axisLabelText="Days. Arrow keys move between them; Enter selects one."
        hoverIndex={hover}
        selectedIndex={picked}
        onHover={setHover}
        onPick={setPicked}
        tooltip={(r) => (
          <div>
            <div className="font-medium">{fmtShortDay(r.iso)}</div>
            <div className="tabular-nums">${r.spend.toFixed(2)}</div>
          </div>
        )}
      />
    </figure>
  )
}

export const ThreeLanes: StoryObj = { render: () => <Demo /> }
