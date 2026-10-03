/**
 * KPI strip (plan A1, A7, A14): four primary KPIs plus a "More metrics" expander.
 * Deltas come straight from the server's `KpiValue.change_pct` (null → "new").
 */
import { AlertTriangle, ChevronDown } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { POLARITY, type Polarity } from '@/lib/delta'
import { fmtCostPerOp, fmtInt, fmtLatency, fmtMoneyFloor, fmtTokens } from '@/lib/format'
import type { FinopsDashboardData, KpiValue } from '../types'
import { PanelError, PanelSkeleton } from '@/components/shared/panel'
import { Delta } from '@/components/shared/delta'
import { KpiTile } from '@/components/shared/kpi-tile'

interface KpiDef {
  label: string
  value: (d: FinopsDashboardData, floor: boolean) => string
  kpi: (d: FinopsDashboardData) => KpiValue
  polarity: Polarity
}

const PRIMARY: KpiDef[] = [
  {
    label: 'Spend',
    value: (d, floor) => fmtMoneyFloor(d.kpis.total_spend.current, floor),
    kpi: (d) => d.kpis.total_spend,
    polarity: POLARITY.spend,
  },
  {
    label: 'Operations',
    value: (d) => fmtInt(d.kpis.total_operations.current),
    kpi: (d) => d.kpis.total_operations,
    polarity: POLARITY.operations,
  },
  {
    label: 'Cost / operation',
    value: (d) => fmtCostPerOp(d.kpis.cost_per_operation.current),
    kpi: (d) => d.kpis.cost_per_operation,
    polarity: POLARITY.costPerOp,
  },
  {
    label: 'p95 latency',
    value: (d) => fmtLatency(d.kpis.latency_p95_ms.current),
    kpi: (d) => d.kpis.latency_p95_ms,
    polarity: POLARITY.p95,
  },
]

const MORE: KpiDef[] = [
  {
    label: 'Tokens',
    value: (d) => fmtTokens(d.kpis.total_tokens.current),
    kpi: (d) => d.kpis.total_tokens,
    polarity: POLARITY.tokens,
  },
  {
    label: 'Tool calls',
    value: (d) => fmtInt(d.kpis.total_tool_calls.current),
    kpi: (d) => d.kpis.total_tool_calls,
    polarity: POLARITY.toolCalls,
  },
  {
    label: 'p50 latency',
    value: (d) => fmtLatency(d.kpis.avg_latency_ms.current),
    kpi: (d) => d.kpis.avg_latency_ms,
    polarity: POLARITY.latency,
  },
  {
    label: 'p99 latency',
    value: (d) => fmtLatency(d.kpis.latency_p99_ms.current),
    kpi: (d) => d.kpis.latency_p99_ms,
    polarity: POLARITY.p99,
  },
  {
    label: 'Active agents',
    value: (d) => `${fmtInt(d.kpis.active_agents.current)} of ${fmtInt(d.summary.total_agents)}`,
    kpi: (d) => d.kpis.active_agents,
    polarity: POLARITY.agents,
  },
]

export function KpiStrip({
  data,
  loading,
  error,
  onRetry,
  more,
  onToggleMore,
  windowLabel,
  compareLabel,
  hideDeltas,
}: {
  data: FinopsDashboardData | undefined
  /** Compare period off: every Δ reads "comparison unavailable". */
  hideDeltas?: boolean
  loading: boolean
  error: unknown
  onRetry: () => void
  more: boolean
  onToggleMore: () => void
  windowLabel: string
  /** What Δ compares against, when "previous period" alone would mislead (This month). */
  compareLabel?: string
}) {
  if (loading && !data) return <PanelSkeleton height={92} />
  if (error && !data) return <PanelError error={error} onRetry={onRetry} what="KPIs" />
  if (!data) return null
  const floor = data.summary.unpriced_calls > 0
  return (
    <section aria-label={`Key metrics, ${windowLabel}`}>
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {PRIMARY.map((k) => (
          <Tile key={k.label} def={k} data={data} floor={floor} hideDeltas={hideDeltas} />
        ))}
      </div>
      <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <span>Source: agent dashboard (agents you can access)</span>
          {compareLabel ? <span>Δ vs {compareLabel}</span> : null}
          {floor ? (
            <Badge
              variant="warning"
              title="These calls used tokens but their model has no price, so spend totals are a floor."
            >
              <AlertTriangle className="size-3" aria-hidden />{' '}
              {data.summary.unpriced_calls.toLocaleString()} calls unpriced in this window
            </Badge>
          ) : null}
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={onToggleMore}
          aria-expanded={more}
          aria-controls="more-kpis"
          className="text-xs"
        >
          {more ? 'Fewer metrics' : 'More metrics'}{' '}
          <ChevronDown
            className={cn('size-3.5 transition-transform', more && 'rotate-180')}
            aria-hidden
          />
        </Button>
      </div>
      {more ? (
        <div id="more-kpis" className="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-5">
          {MORE.map((k) => (
            <Tile key={k.label} def={k} data={data} floor={floor} compact hideDeltas={hideDeltas} />
          ))}
        </div>
      ) : null}
    </section>
  )
}

function Tile({
  def,
  data,
  floor,
  compact,
  hideDeltas,
}: {
  def: KpiDef
  data: FinopsDashboardData
  floor: boolean
  compact?: boolean
  hideDeltas?: boolean
}) {
  const k = def.kpi(data)
  return (
    <KpiTile
      label={def.label}
      aside={
        <Delta
          changePct={k.change_pct}
          polarity={def.polarity}
          current={k.current}
          unavailable={hideDeltas}
        />
      }
      value={def.value(data, floor)}
      compact={compact}
    />
  )
}
