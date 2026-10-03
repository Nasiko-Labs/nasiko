/**
 * F5 — "Is cost buying performance?" (plan A9, A2b, A11).
 *
 * Cost per operation (log x) against p95 latency (y); bubble area ∝ operations; dashed
 * medians split the plot into quadrants. Top-right = expensive and slow. Fewer than 3
 * agents with data always renders as a table; the Table view is also the keyboard and
 * screen-reader path, and each row can highlight its F3 row like a point click.
 */
import { BarChart3, Table2 } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { cn } from '@/lib/utils'
import type { AttributionRow } from '../attribution'
import { fmtCostPerOp, fmtHours, fmtInt, fmtLatency } from '@/lib/format'
import { Panel, PanelEmpty, PanelError, PanelSkeleton } from '@/components/shared/panel'
import { CELL, ChartTable, HEAD, NUM, ROW_HEAD, STICKY_HEAD } from './ChartTable'
import { CostScatter, type ScatterPoint } from './CostScatter'

type Point = ScatterPoint

export function CostPerformance({
  rows,
  loading,
  error,
  onRetry,
  onSelect,
}: {
  rows: AttributionRow[] | undefined
  loading: boolean
  error: unknown
  onRetry: () => void
  onSelect: (id: string) => void
}) {
  const [asTable, setAsTable] = useState(false)
  const points: Point[] = (rows ?? [])
    .filter((r) => r.kind === 'agent' && r.operations > 0 && r.costPerOp > 0 && r.p95 !== null)
    .map((r) => ({
      id: r.id,
      name: r.name,
      x: r.costPerOp,
      y: r.p95 as number,
      z: r.operations,
      hours: r.containerHours,
    }))
  const showTable = asTable || points.length < 3

  return (
    <Panel
      title="Cost vs performance"
      subtitle="Cost per operation × p95 latency · top-right is expensive and slow"
      labelledBy="cost-perf-title"
      actions={
        points.length >= 3 ? (
          <Button variant="ghost" size="sm" onClick={() => setAsTable((v) => !v)}>
            {asTable ? (
              <BarChart3 className="size-4" aria-hidden />
            ) : (
              <Table2 className="size-4" aria-hidden />
            )}
            {asTable ? 'Chart' : 'Table'}
          </Button>
        ) : undefined
      }
    >
      {loading && !rows ? (
        <PanelSkeleton height={240} />
      ) : error && !rows ? (
        <PanelError error={error} onRetry={onRetry} what="cost vs performance" />
      ) : points.length === 0 ? (
        <PanelEmpty title="No agents with priced operations in this window" />
      ) : showTable ? (
        <ChartTable>
          <TableHeader className={STICKY_HEAD}>
            <TableRow className="hover:bg-transparent">
              <TableHead scope="col" className={HEAD}>
                Agent
              </TableHead>
              <TableHead scope="col" className={cn(HEAD, 'text-right')}>
                Cost/op
              </TableHead>
              <TableHead scope="col" className={cn(HEAD, 'text-right')}>
                p95
              </TableHead>
              <TableHead scope="col" className={cn(HEAD, 'text-right')}>
                Ops
              </TableHead>
              <TableHead scope="col" className={cn(HEAD, 'text-right')}>
                Hours
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {[...points]
              .sort((a, b) => b.x - a.x)
              .map((p) => (
                <TableRow key={p.id}>
                  <TableHead scope="row" className={ROW_HEAD}>
                    <Button
                      variant="link"
                      size="sm"
                      className="h-auto p-0 text-xs"
                      onClick={() => onSelect(p.id)}
                      title="Highlight in the attribution table"
                    >
                      {p.name}
                    </Button>
                  </TableHead>
                  <TableCell className={cn(CELL, NUM)}>{fmtCostPerOp(p.x)}</TableCell>
                  <TableCell className={cn(CELL, NUM)}>{fmtLatency(p.y)}</TableCell>
                  <TableCell className={cn(CELL, NUM)}>{fmtInt(p.z)}</TableCell>
                  <TableCell className={cn(CELL, NUM)}>{fmtHours(p.hours)}</TableCell>
                </TableRow>
              ))}
          </TableBody>
        </ChartTable>
      ) : (
        <figure
          className="m-0 h-65"
          aria-label={`Cost per operation versus p95 latency for ${points.length} agents. Use the Table view to browse values by keyboard.`}
        >
          <CostScatter points={points} onSelect={onSelect} />
        </figure>
      )}
    </Panel>
  )
}
