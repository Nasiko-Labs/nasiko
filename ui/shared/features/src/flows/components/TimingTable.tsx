/**
 * The swimlane panel's Table view (plans/feat-flows.md F26): every call the chart draws, as a timing table: agent,
 * start and end offsets, duration, status, parallel group, critical path and source. The agent cell selects the call,
 * like a bar does (`?step=`).
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
import { cn } from '@nasiko/ui/lib/utils'
import type { Call } from '../calls'
import { copy, stepStatusLabel } from '../copy'
import { fmtDuration } from '../precision'
import type { FanOut } from '../timeline'

export function TimingTable({
  calls,
  fanOuts,
  critical,
  flowStartMs,
  now,
  selected,
  onSelect,
}: {
  calls: readonly Call[]
  fanOuts: readonly FanOut[]
  critical: ReadonlySet<string> | null
  flowStartMs: number
  now: number
  selected: string | null
  onSelect: (key: string | null) => void
}) {
  const groupOf = new Map<string, number>()
  fanOuts.forEach((f, i) => {
    for (const k of f.keys) groupOf.set(k, i + 1)
  })
  const off = (t: number) => `+${fmtDuration(t - flowStartMs)}`
  const t = copy.table
  return (
    <Table aria-label={t.label}>
      <TableHeader>
        <TableRow>
          <TableHead>{t.agent}</TableHead>
          <TableHead className="text-right">{t.start}</TableHead>
          <TableHead className="text-right">{t.end}</TableHead>
          <TableHead className="text-right">{t.duration}</TableHead>
          <TableHead>{t.status}</TableHead>
          <TableHead>{t.parallel}</TableHead>
          <TableHead>{t.critical}</TableHead>
          <TableHead>{t.source}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {calls.map((c) => {
          const group = groupOf.get(c.key)
          return (
            <TableRow key={c.key} data-state={selected === c.key ? 'selected' : undefined}>
              <TableCell>
                <Button
                  variant="link"
                  size="sm"
                  className={cn('h-auto p-0 text-left', selected === c.key && 'font-semibold')}
                  aria-pressed={selected === c.key}
                  onClick={() => onSelect(selected === c.key ? null : c.key)}
                >
                  {c.agentName ?? copy.panel.unknownAgent}
                </Button>
              </TableCell>
              <TableCell className="text-right tabular-nums">{off(c.startMs)}</TableCell>
              <TableCell className="text-right tabular-nums">
                {c.endMs === null ? '—' : off(c.endMs)}
              </TableCell>
              <TableCell className="text-right tabular-nums">
                {c.endMs === null
                  ? `${fmtDuration(now - c.startMs, false)}…`
                  : fmtDuration(c.endMs - c.startMs, c.exactEnd)}
              </TableCell>
              <TableCell>{stepStatusLabel(c.status)}</TableCell>
              <TableCell>{group ? t.group(group) : '—'}</TableCell>
              <TableCell>{critical?.has(c.key) ? t.yes : '—'}</TableCell>
              <TableCell>{c.source === 'trace' ? copy.steps.fromTrace : t.recorded}</TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}
