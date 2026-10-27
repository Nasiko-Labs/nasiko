/**
 * The trace's Optimization block (plans/feat-context-optimization.md §5, design review 1B, 2A, 5A, 6A; sketch v2 2a):
 * what one request carried against the baseline, on the span that holds the report. Contained in the span Summary
 * (option A). Savings stay in the neutral foreground (5A); the column header is two lines at every width (6A).
 */
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { fmtBytes } from '@nasiko/ui/lib/format'
import { copy } from '../copy'
import { approx, tokenDelta, type ContextReport } from '../logic'

// Tokens are estimates (shown with ~); message counts are exact.
const n = (v: number) => approx(v).toLocaleString('en-US')
const exact = (v: number) => v.toLocaleString('en-US')
// Row labels are row headers, so each value is read with its row; styled as the cells.
const ROW = 'h-auto p-2 font-normal text-muted-foreground'

export function ContextBlock({ report }: { report: ContextReport }) {
  const delta = tokenDelta(report)
  const detail = copy.trace.withDetail(
    report.level ? copy.level(report.level) : null,
    report.strategy ? copy.strategies[report.strategy].name : null,
  )
  return (
    <div className="flex flex-col gap-1.5">
      <div className="rounded-lg border border-border">
        <p className="px-3 pt-2.5 pb-1 text-sm font-medium">{copy.trace.title}</p>
        {/* Fits the span panel: fixed columns that wrap, so nothing hides behind the table's own scroll (6A). */}
        <Table
          aria-label={copy.trace.label}
          className="table-fixed [&_td]:whitespace-normal [&_th]:whitespace-normal"
        >
          <TableHeader>
            <TableRow>
              <TableHead className="w-[28%]">
                <span className="sr-only">{copy.trace.measure}</span>
              </TableHead>
              <TableHead className="font-normal text-muted-foreground">
                <Tooltip>
                  <TooltipTrigger
                    render={
                      <span
                        // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only so keyboard users can open the baseline tooltip; it has no action, so no button role
                        tabIndex={0}
                        className="underline decoration-dotted underline-offset-4"
                      />
                    }
                  >
                    {copy.preview.without}
                  </TooltipTrigger>
                  <TooltipContent className="max-w-xs">{copy.preview.baselineTip}</TooltipContent>
                </Tooltip>
              </TableHead>
              <TableHead className="font-normal text-muted-foreground">
                {copy.trace.with}
                {detail ? <span className="block text-xs">{detail}</span> : null}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow>
              <TableHead scope="row" className={ROW}>
                {copy.trace.messages}
              </TableHead>
              <TableCell className="tabular-nums">{exact(report.pool.messages)}</TableCell>
              <TableCell className="tabular-nums">{exact(report.kept.messages)}</TableCell>
            </TableRow>
            <TableRow>
              <TableHead scope="row" className={ROW}>
                {copy.trace.tokens}
              </TableHead>
              <TableCell className="tabular-nums">~{n(report.pool.tokens)}</TableCell>
              <TableCell className="tabular-nums">
                ~{n(report.kept.tokens)}
                {delta !== null && delta !== 0 ? (
                  <span className="text-muted-foreground">
                    {' '}
                    {delta > 0 ? `+${delta}` : `−${-delta}`}%
                  </span>
                ) : null}
              </TableCell>
            </TableRow>
            {report.compressedBytesSaved ? (
              <TableRow>
                <TableHead scope="row" className={ROW}>
                  {copy.trace.compressed}
                </TableHead>
                <TableCell />
                <TableCell className="tabular-nums">
                  −{fmtBytes(report.compressedBytesSaved)}
                </TableCell>
              </TableRow>
            ) : null}
            {report.orgPolicyApplied ? (
              <TableRow>
                <TableHead scope="row" className={ROW}>
                  {copy.trace.policy}
                </TableHead>
                <TableCell />
                <TableCell>{copy.trace.policyApplied}</TableCell>
              </TableRow>
            ) : null}
          </TableBody>
        </Table>
      </div>
      <p className="text-xs text-muted-foreground">{copy.preview.estimates}</p>
    </div>
  )
}
