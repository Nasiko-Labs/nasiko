/**
 * The KPI row: Spend, Agent runs and Agents running over the page's range (the fourth tile, Harnesses used, is
 * `Harnesses.tsx`). Each label links to the page that owns the number. Spend and runs come from the fleet-wide
 * timeseries (the same series as the Spend chart), agents from the directory. The month line is the summary card's
 * `MonthBar`.
 * Sparklines are decorative: the figures above them carry the numbers.
 */
import { Link } from '@tanstack/react-router'
import { Bot, DollarSign, Workflow, type LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'
import { Delta } from '@/components/shared/delta'
import { KpiTile } from '@/components/shared/kpi-tile'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { displayStatus, isHarness, type DisplayStatus } from '@/features/agents/status'
import { seriesAt, type Series } from '@/lib/chart'
import { POLARITY } from '@/lib/delta'
import { fmtInt, fmtMoneyFloor, fmtMoneyShort } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { FleetHealth, Spend } from '../api'
import { copy } from '../copy'
import { SourceFailed } from './Card'

const pctChange = (now: number, before: number) =>
  before > 0 ? ((now - before) / before) * 100 : null

/** A tile's label: an icon and the owning page's link. */
export function TileLabel({
  Icon,
  to,
  children,
}: {
  Icon: LucideIcon
  to: '/tokenops' | '/sessions' | '/agents' | '/harnesses'
  children: ReactNode
}) {
  return (
    <Link
      to={to}
      search={{} as never}
      className="inline-flex items-center gap-1.5 rounded-sm text-sm text-muted-foreground underline-offset-4 hover:text-foreground hover:underline pointer-coarse:min-h-11"
    >
      <Icon className="size-4" aria-hidden />
      {children}
    </Link>
  )
}

export const TILE = 'min-w-0 gap-2 p-4'
const PENDING = <Skeleton className="h-8 w-28 motion-reduce:animate-none" />

/** A tiny decorative chart under a figure: a line with a light area, or bars. */
function Spark({
  points,
  kind,
  series,
}: {
  /** One point per day, keyed by its ISO start. */
  points: readonly { key: string; value: number }[]
  kind: 'line' | 'bars'
  series: Series
}) {
  const values = points.map((p) => p.value)
  const max = Math.max(0, ...values)
  if (!values.length || max <= 0) return <div className="mt-auto h-10" aria-hidden />
  const y = (v: number) => 30 - (v / max) * 28
  if (kind === 'bars') {
    const w = 100 / values.length
    return (
      <svg
        aria-hidden
        viewBox="0 0 100 32"
        preserveAspectRatio="none"
        className="mt-auto h-10 w-full"
      >
        {points.map(({ key, value: v }, i) => (
          <rect
            key={key}
            x={i * w + w * 0.15}
            width={w * 0.7}
            y={y(v)}
            height={Math.max(0, 30 - y(v))}
            fill={series.fill}
            stroke={series.edge}
            strokeWidth={0.5}
            vectorEffect="non-scaling-stroke"
          />
        ))}
      </svg>
    )
  }
  const step = values.length > 1 ? 100 / (values.length - 1) : 0
  const pts = values.map((v, i) => `${i * step},${y(v)}`).join(' ')
  return (
    <svg
      aria-hidden
      viewBox="0 0 100 32"
      preserveAspectRatio="none"
      className="mt-auto h-10 w-full"
    >
      <polygon points={`0,32 ${pts} 100,32`} fill={series.fill} opacity={0.35} />
      <polyline
        points={pts}
        fill="none"
        stroke={series.edge}
        strokeWidth={1.5}
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  )
}

export function SpendTile({ spend }: { spend: Spend }) {
  const { totals } = spend
  return (
    <KpiTile
      data-testid="kpi-spend"
      className={TILE}
      label={
        <TileLabel Icon={DollarSign} to="/tokenops">
          {copy.kpi.spend}
        </TileLabel>
      }
      aside={
        totals?.previous ? (
          <Delta
            changePct={pctChange(totals.spend, totals.previous.spend)}
            polarity={POLARITY.spend}
            current={totals.spend}
          />
        ) : null
      }
      value={
        totals ? fmtMoneyFloor(totals.spend, spend.unpriced) : spend.totalsError ? '—' : PENDING
      }
    >
      {spend.totalsError && !totals ? (
        <SourceFailed what={copy.spend.rangeWhat} onRetry={spend.retry} />
      ) : null}
      {totals ? (
        <p className="text-xs text-muted-foreground tabular-nums">
          {copy.kpi.perDay(fmtMoneyShort(totals.spend / spend.rangeDays))}
        </p>
      ) : null}
      <Spark
        points={spend.days.map((d) => ({ key: d.iso, value: d.spend }))}
        kind="line"
        series={seriesAt(0)}
      />
    </KpiTile>
  )
}

export function RunsTile({ spend }: { spend: Spend }) {
  const { totals } = spend
  return (
    <KpiTile
      data-testid="kpi-runs"
      className={TILE}
      label={
        <TileLabel Icon={Workflow} to="/sessions">
          {copy.kpi.runs}
        </TileLabel>
      }
      aside={
        totals?.previous ? (
          <Delta
            changePct={pctChange(totals.runs, totals.previous.runs)}
            polarity={POLARITY.operations}
            current={totals.runs}
          />
        ) : null
      }
      value={totals ? fmtInt(totals.runs) : spend.totalsError ? '—' : PENDING}
    >
      {totals ? (
        <p className="text-xs text-muted-foreground tabular-nums">
          {copy.kpi.perDay(fmtInt(Math.round(totals.runs / spend.rangeDays)))}
        </p>
      ) : null}
      <Spark
        points={spend.days.map((d) => ({ key: d.iso, value: d.operations }))}
        kind="bars"
        series={seriesAt(1)}
      />
    </KpiTile>
  )
}

/** Status tokens, never the accent; each square's status is also in the counts line. */
const SQUARE: Record<DisplayStatus, string> = {
  running: 'bg-success',
  deploying: 'bg-info',
  attention: 'bg-destructive',
  stopped: 'bg-muted-foreground/30',
  'not-deployed': 'bg-muted-foreground/30',
  unknown: 'bg-muted-foreground/30',
  harness: 'bg-muted-foreground/30',
}
const ORDER: readonly DisplayStatus[] = [
  'running',
  'deploying',
  'attention',
  'stopped',
  'not-deployed',
  'unknown',
]

export function AgentsTile({ fleet }: { fleet: FleetHealth }) {
  const agents = [...fleet.byId.values()]
    .filter((a) => !isHarness(a))
    .map((a) => ({
      id: a.id,
      name: a.display_name || a.name,
      status: displayStatus(a.status, false),
    }))
    .sort(
      (a, b) => ORDER.indexOf(a.status) - ORDER.indexOf(b.status) || a.name.localeCompare(b.name),
    )
  const count = (s: DisplayStatus) => agents.filter((a) => a.status === s).length
  const parts = (['deploying', 'attention', 'stopped', 'not-deployed'] as const)
    .filter((s) => count(s) > 0)
    .map((s) => copy.kpi.agentParts[s](count(s)))
  const loaded = !fleet.isPending || fleet.byId.size > 0
  return (
    <KpiTile
      data-testid="kpi-agents"
      className={TILE}
      label={
        <TileLabel Icon={Bot} to="/agents">
          {copy.kpi.agents}
        </TileLabel>
      }
      value={
        fleet.error && !fleet.byId.size ? (
          '—'
        ) : !loaded ? (
          PENDING
        ) : (
          <>
            {count('running')}{' '}
            <span className="text-base font-normal text-muted-foreground">
              {copy.kpi.of(agents.length)}
            </span>
          </>
        )
      }
    >
      {fleet.error && !fleet.byId.size ? (
        <SourceFailed what={copy.health.what} onRetry={fleet.retry} />
      ) : loaded ? (
        <p className="text-xs text-muted-foreground tabular-nums">
          {!agents.length
            ? copy.kpi.noAgents
            : parts.length
              ? parts.join(' · ')
              : copy.kpi.allRunning}
        </p>
      ) : null}
      <div
        aria-hidden
        className="mt-auto grid grid-cols-[repeat(auto-fill,minmax(0.875rem,1fr))] gap-1"
      >
        {agents.map((a) => (
          <Tooltip key={a.id}>
            <TooltipTrigger asChild>
              <span className={cn('h-6 rounded-xs', SQUARE[a.status])} />
            </TooltipTrigger>
            <TooltipContent>{`${a.name} · ${a.status}`}</TooltipContent>
          </Tooltip>
        ))}
      </div>
    </KpiTile>
  )
}
