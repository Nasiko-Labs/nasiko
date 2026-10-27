/**
 * The hub's Table view (design "Table view", the Optimization/Flows pattern): one row per bucket with every lane's
 * value and the bucket's recorded changes, and the same action as a bar click (select the bucket).
 */
import { Button } from '@nasiko/ui/primitives/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { fmtInt, fmtLatency, fmtMoneyFloor, fmtTokens } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import { markerText } from '../events'
import type { Flag, HubRow } from '../timeline'

const HEAD = 'h-8 font-medium text-muted-foreground'
const NUM = 'py-1 text-right tabular-nums'

export function HubTable({
  rows,
  flags,
  money,
  unpriced,
  saved,
  selected,
  onPick,
}: {
  rows: readonly HubRow[]
  flags: readonly Flag[]
  /** The viewer may see the numbers; otherwise only buckets and changes are listed. */
  money: boolean
  /** Some calls had no price: spend is a floor. */
  unpriced: boolean
  saved: boolean
  selected: number
  onPick: (i: number) => void
}) {
  const changes = new Map(flags.map((f) => [f.index, f.markers]))
  return (
    <div className="max-h-96 overflow-auto [&>[data-slot=table-container]]:overflow-visible">
      <Table className="text-xs" aria-label={copy.table.label}>
        <TableHeader className="sticky top-0 z-10 bg-background">
          <TableRow className="hover:bg-transparent">
            <TableHead scope="col" className={HEAD}>
              {copy.table.bucket}
            </TableHead>
            {money ? (
              <>
                <TableHead scope="col" className={cn(HEAD, 'text-right')}>
                  {copy.lanes.spend}
                </TableHead>
                <TableHead scope="col" className={cn(HEAD, 'text-right')}>
                  {copy.lanes.requests}
                </TableHead>
                <TableHead scope="col" className={cn(HEAD, 'text-right')}>
                  {copy.lanes.p95}
                </TableHead>
                {saved ? (
                  <TableHead scope="col" className={cn(HEAD, 'text-right')}>
                    {copy.lanes.saved}
                  </TableHead>
                ) : null}
              </>
            ) : null}
            <TableHead scope="col" className={HEAD}>
              {copy.table.markers}
            </TableHead>
            <TableHead scope="col" className={HEAD}>
              <span className="sr-only">{copy.table.action}</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r, i) => {
            const ms = changes.get(i) ?? []
            const name = r.label
            return (
              <TableRow key={r.iso} data-state={selected === i ? 'selected' : undefined}>
                <TableHead scope="row" className="h-auto py-1 font-normal">
                  {name}
                </TableHead>
                {money ? (
                  r.before ? (
                    <TableCell
                      colSpan={saved ? 4 : 3}
                      className="py-1 text-right text-muted-foreground"
                    >
                      {copy.lanes.notDeployedYet}
                    </TableCell>
                  ) : (
                    <>
                      <TableCell className={NUM}>{fmtMoneyFloor(r.spend, unpriced)}</TableCell>
                      <TableCell className={NUM}>{fmtInt(r.requests)}</TableCell>
                      <TableCell className={NUM}>{fmtLatency(r.p95)}</TableCell>
                      {saved ? <TableCell className={NUM}>{fmtTokens(r.saved)}</TableCell> : null}
                    </>
                  )
                ) : null}
                <TableCell className="py-1">
                  {ms.length ? ms.map(markerText).join('; ') : '—'}
                </TableCell>
                <TableCell className="py-1 text-right">
                  <Button
                    variant="link"
                    size="sm"
                    className="h-auto p-0 text-xs pointer-coarse:min-h-11"
                    aria-pressed={selected === i}
                    aria-label={`${copy.table.select} ${name}`}
                    onClick={() => onPick(i)}
                  >
                    {copy.table.select}
                  </Button>
                </TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </div>
  )
}
