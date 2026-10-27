/**
 * The Duration panel (plans/feat-flows.md F11, D2, F26): p50 / p90 / p99 as display-only tracks built from shadcn
 * Slider parts, disabled and hidden from assistive tech (they're not controls), with the values as text beside them.
 * Its Table button swaps them for the same numbers in a table. p99 hides under P99_MIN finished flows (F16).
 */
import { useState } from 'react'
import { Slider } from '@nasiko/ui/primitives/slider'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { copy } from '../copy'
import { P99_MIN, type Percentiles } from '../list'
import { fmtDuration } from '../precision'

export function DurationPanel({ stats }: { stats: Percentiles | null }) {
  const [asTable, setAsTable] = useState(false)
  if (!stats) return <p className="text-sm text-muted-foreground">{copy.list.noDurations}</p>
  const rows: [string, number | null][] = [
    ['p50', stats.p50],
    ['p90', stats.p90],
    ['p99', stats.p99],
  ]
  const max = Math.max(1, stats.p99 ?? stats.p90)
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs text-muted-foreground">{copy.list.count(stats.count)}</span>
        {/* The same Chart / Table switch as the flow page's timeline panel: a state, not a label that flips. */}
        <ToggleGroup
          variant="outline"
          size="sm"
          aria-label={copy.list.durationView}
          value={[asTable ? 'table' : 'chart']}
          onValueChange={(vs) => {
            const v = vs[0] as string
            return v && setAsTable(v === 'table')
          }}
        >
          <ToggleGroupItem value="chart" className="pointer-coarse:min-h-11">
            {copy.list.chart}
          </ToggleGroupItem>
          <ToggleGroupItem value="table" className="pointer-coarse:min-h-11">
            {copy.list.table}
          </ToggleGroupItem>
        </ToggleGroup>
      </div>
      {asTable ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{copy.list.percentile}</TableHead>
              <TableHead className="text-right">{copy.list.value}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map(([label, v]) =>
              v === null ? null : (
                <TableRow key={label}>
                  <TableCell>{label}</TableCell>
                  <TableCell className="text-right tabular-nums">{fmtDuration(v)}</TableCell>
                </TableRow>
              ),
            )}
          </TableBody>
        </Table>
      ) : (
        <div className="grid grid-cols-[2.5rem_1fr_5rem] items-center gap-x-3 gap-y-3 text-sm">
          {rows.map(([label, v]) =>
            v === null ? null : (
              <div key={label} className="contents">
                <span className="text-muted-foreground">{label}</span>
                <div aria-hidden>
                  <Slider
                    disabled
                    value={[v]}
                    min={0}
                    max={max}
                    className="data-[disabled]:opacity-100"
                  />
                </div>
                <span className="text-right tabular-nums">{fmtDuration(v)}</span>
              </div>
            ),
          )}
        </div>
      )}
      {stats.p99 === null ? (
        <p className="text-xs text-muted-foreground">{copy.list.p99Hidden(P99_MIN)}</p>
      ) : null}
    </div>
  )
}
