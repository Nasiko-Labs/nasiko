/**
 * The KPI tiles: Spend, Tokens and Agent runs over the page's range (the fourth, Harnesses used, is `Harnesses.tsx`).
 * Each label links to the page that owns the number. Spend and runs come from the fleet-wide timeseries (the same
 * series as the Spend chart), cost per run from the same totals, tokens from the dashboard's KPIs and the savings read.
 * The month line is the summary card's `MonthBar`; the agents' run states are Fleet health's first line.
 * Sparklines are decorative: the figures above them carry the numbers.
 */
import { Link } from '@tanstack/react-router'
import { Coins, DollarSign, Workflow, type LucideIcon } from 'lucide-react'
import type { CSSProperties, ReactNode } from 'react'
import { Delta } from '@nasiko/ui/composites/delta'
import { KpiTile } from '@nasiko/ui/composites/kpi-tile'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { seriesAt, type Series } from '@nasiko/ui/lib/chart'
import { POLARITY } from '@nasiko/ui/lib/delta'
import { cn } from '@nasiko/ui/lib/utils'
import {
  fmtCostPerOp,
  fmtInt,
  fmtMoneyFloor,
  fmtMoneyShort,
  fmtPct,
  fmtTokens,
} from '@nasiko/ui/lib/format'
import type { Spend } from '../api'
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

/** A tiny decorative chart under a figure: a line with a light area, or bars. The village's meters use it too. */
export function Spark({
  points,
  kind,
  series,
  className,
}: {
  /** One point per day, keyed by its ISO start. */
  points: readonly { key: string; value: number }[]
  kind: 'line' | 'bars'
  series: Series
  /** Overrides the size (the tiles' is `h-10 w-full`). */
  className?: string
}) {
  const values = points.map((p) => p.value)
  const max = Math.max(0, ...values)
  if (!values.length || max <= 0)
    return <div className={cn('mt-auto h-10', className)} aria-hidden />
  const y = (v: number) => 30 - (v / max) * 28
  if (kind === 'bars') {
    const w = 100 / values.length
    return (
      <svg
        aria-hidden
        viewBox="0 0 100 32"
        preserveAspectRatio="none"
        className={cn('mt-auto h-10 w-full', className)}
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
      className={cn('mt-auto h-10 w-full', className)}
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
        <p className="flex flex-wrap items-baseline gap-x-1.5 text-xs text-muted-foreground tabular-nums">
          {copy.kpi.perDay(fmtInt(Math.round(totals.runs / spend.rangeDays)))}
          {totals.runs > 0 ? (
            <>
              <span aria-hidden>·</span>
              <span data-testid="kpi-cost-per-run">
                {copy.kpi.perRun(fmtCostPerOp(totals.spend / totals.runs))}
              </span>
              {totals.previous && totals.previous.runs > 0 ? (
                <Delta
                  changePct={pctChange(
                    totals.spend / totals.runs,
                    totals.previous.spend / totals.previous.runs,
                  )}
                  polarity={POLARITY.costPerOp}
                  current={totals.spend / totals.runs}
                />
              ) : null}
            </>
          ) : null}
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

/**
 * Tokens over the range with their change and the input / output split (a bar over its two percentages). What
 * optimisation saved stays in the summary card's Token optimisation box, never a sent / saved bar here: its share is of
 * the optimised requests' pool, not of every token.
 */
export function TokensTile({ spend }: { spend: Spend }) {
  const { tokens } = spend
  const io = tokens ? tokens.input + tokens.output : 0
  const inputShare = tokens && io > 0 ? (tokens.input / io) * 100 : null
  return (
    <KpiTile
      data-testid="kpi-tokens"
      className={TILE}
      label={
        <TileLabel Icon={Coins} to="/tokenops">
          {copy.kpi.tokens}
        </TileLabel>
      }
      aside={
        tokens ? (
          <Delta changePct={tokens.changePct} polarity={POLARITY.tokens} current={tokens.total} />
        ) : null
      }
      value={tokens ? fmtTokens(tokens.total) : spend.driversError ? '—' : PENDING}
    >
      {spend.driversError && !tokens ? (
        <SourceFailed what={copy.spend.rangeWhat} onRetry={spend.retry} />
      ) : null}
      {inputShare !== null ? (
        // The split is in the words under the bar; the bar only draws it.
        <div className="mt-auto flex flex-col gap-1">
          <div aria-hidden className="flex h-2 gap-0.5 overflow-hidden rounded-full bg-muted">
            <span
              className="h-full bg-(--input)"
              style={{ width: `${inputShare}%`, '--input': seriesAt(0).fill } as CSSProperties}
            />
            <span
              className="h-full flex-1 bg-(--output)"
              style={{ '--output': seriesAt(1).fill } as CSSProperties}
            />
          </div>
          <p className="flex justify-between text-2xs text-muted-foreground tabular-nums">
            <span>{copy.kpi.tokensInput(fmtPct(inputShare))}</span>
            <span>{copy.kpi.tokensOutput(fmtPct(100 - inputShare))}</span>
          </p>
        </div>
      ) : (
        <div className="mt-auto h-10" aria-hidden />
      )}
    </KpiTile>
  )
}
