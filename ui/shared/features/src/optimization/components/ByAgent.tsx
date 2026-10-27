/**
 * By agent (plans/feat-optimization-page.md R1C, R4A, R6A, R7A, R7B; eng E1): each agent's history tokens over the
 * window, history volume first, top 6 with "Show all N". Column headers re-sort (`?sort=`, replacing history). The
 * Token optimization state is a neutral badge (On muted, Off outline; never the success colour near a saving). Below
 * 768 px of content each agent is two lines, Requests and Sent hidden and the headers kept for screen readers (the LLM router's
 * pattern). A server without CX-V3b keeps the block and says what will show here (R2B).
 * Not the shared DataTable (review, user decision D2): its sort button wraps the header label, and the 'Without'
 * label carries the focusable 2A tooltip (R4C), which would nest one control in another. Loads and fails on its own
 * (R2C).
 */
import { ArrowDown } from 'lucide-react'
import type { ReactNode } from 'react'
import { Panel, PanelError, PanelSkeleton } from '@nasiko/ui/composites/panel'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { AgentLink } from '@nasiko/features/agents/components/AgentLink'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { cn } from '@nasiko/ui/lib/utils'
import type { Proposed } from '../api'
import { sortAgents, TOP_AGENTS } from '../breakdown'
import type { AgentSort } from '../search'
import { copy } from '../copy'
import { pctOf, savedShare, tokensShort } from '../lead'
import { savedOf } from '@nasiko/app-core/slice'
import type { SavingsAgent } from '../types'
import { Outline } from './Outline'
import { Without } from './Without'

/** The trend chart's Saved colour, so the share reads as the same thing. */
const SAVED = seriesAt(1)

const ROW =
  'text-sm hover:bg-transparent @max-3xl:grid @max-3xl:gap-0.5 @max-3xl:px-4 @max-3xl:py-2.5'
const CELL =
  'p-0 whitespace-normal @max-3xl:block @3xl:px-1.5 @3xl:py-2 @3xl:first:pl-4 @3xl:last:pr-4'
const HEAD = 'h-auto px-1.5 py-2 text-xs text-muted-foreground first:pl-4 last:pr-4'
const NUM = '@3xl:text-right tabular-nums'

export function ByAgent({
  result,
  pending,
  error,
  onRetry,
  period,
  sort,
  all,
  onSort,
  onShowAll,
  noAgents,
}: {
  result: Proposed<SavingsAgent[]> | undefined
  pending: boolean
  error: unknown
  onRetry: () => void
  period: string
  sort: AgentSort
  all: boolean
  onSort: (by: AgentSort) => void
  onShowAll: (all: boolean) => void
  /** R2E(2): you can access no agents, so the block shows what will show here. */
  noAgents: boolean
}) {
  const body = () => {
    if (!result && pending) return <PanelSkeleton height={240} />
    if (!result) return <PanelError error={error} onRetry={onRetry} what={copy.byAgent.what} />
    if (result.kind === 'absent' || noAgents) return <Outline small>{copy.byAgent.outline}</Outline>
    // An agent none of whose eligible requests reported (`requests` counts from recorded_since on) has nothing to show.
    const reported = result.data.filter((r) => r.reports > 0)
    if (!reported.length)
      return <p className="m-0 text-sm text-muted-foreground">{copy.byAgent.empty}</p>
    const sorted = sortAgents(reported, sort)
    const shown = all ? sorted : sorted.slice(0, TOP_AGENTS)
    const head = (by: AgentSort, label: ReactNode, name: string, num = true) => (
      <TableHead
        scope="col"
        className={cn(HEAD, num && 'text-right')}
        aria-sort={sort === by ? (by === 'name' ? 'ascending' : 'descending') : undefined}
      >
        <span className="inline-flex items-center gap-1">
          {label}
          <Button
            variant="ghost"
            size="icon"
            // Below 768 px the header is for screen readers only: no hidden control in the tab order (review: design).
            className={cn(
              'size-6 @max-3xl:hidden pointer-coarse:size-11',
              sort !== by && 'opacity-40',
            )}
            aria-label={copy.byAgent.sortBy(name)}
            aria-pressed={sort === by}
            onClick={() => onSort(by)}
          >
            <ArrowDown aria-hidden className={cn('size-3.5', by === 'name' && 'rotate-180')} />
          </Button>
        </span>
      </TableHead>
    )
    return (
      // R6A: the switch is the block's own width (768 px of content), not the viewport (review: design).
      <div className="@container -mx-4 flex flex-col">
        <Table>
          <TableHeader className="@max-3xl:sr-only">
            <TableRow className="hover:bg-transparent">
              {head('name', copy.byAgent.colAgent, copy.byAgent.colAgent, false)}
              {head('requests', copy.byAgent.colRequests, copy.byAgent.colRequests)}
              {head(
                'without',
                <>
                  <span className="@max-3xl:hidden">
                    <Without label={copy.byAgent.colWithout} />
                  </span>
                  <span className="@3xl:hidden">{copy.byAgent.colWithout}</span>
                </>,
                copy.byAgent.colWithout,
              )}
              {head('sent', copy.byAgent.colSent, copy.byAgent.colSent)}
              {head('saved', copy.byAgent.colSaved, copy.byAgent.colSaved)}
              <TableHead scope="col" className={HEAD}>
                {copy.byAgent.colSwitch}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {shown.map((r) => {
              const saved = savedOf(r)
              const pct = pctOf(savedShare(r.pool_tokens, r.sent_tokens))
              return (
                <TableRow key={r.agent_id} className={ROW}>
                  <TableCell className={cn(CELL, 'font-medium')}>
                    <span className="inline-flex flex-wrap items-center gap-2">
                      <AgentLink id={r.agent_id} className="truncate">
                        {r.name}
                      </AgentLink>
                      {/* Phone: the state rides on the name line (R6A). */}
                      <span className="@3xl:hidden">
                        <SwitchBadge on={r.compress_enabled} />
                      </span>
                    </span>
                  </TableCell>
                  <TableCell className={cn(CELL, NUM, '@max-3xl:hidden')}>
                    {r.requests.toLocaleString('en-US')}
                  </TableCell>
                  <TableCell className={cn(CELL, NUM, '@max-3xl:hidden')}>
                    ~{tokensShort(r.pool_tokens)}
                  </TableCell>
                  <TableCell className={cn(CELL, NUM, '@max-3xl:hidden')}>
                    ~{tokensShort(r.sent_tokens)}
                  </TableCell>
                  <TableCell
                    className={cn(CELL, NUM, '@max-3xl:text-xs @max-3xl:text-muted-foreground')}
                  >
                    <span className="inline-flex items-center justify-end gap-2 @max-3xl:hidden">
                      {/* The saved share as a bar too; the figure beside it carries the value. */}
                      <span aria-hidden className="h-1.5 w-14 rounded-full bg-muted">
                        <span
                          className="block h-full rounded-full"
                          style={{ width: `${saved > 0 ? pct : 0}%`, backgroundColor: SAVED.edge }}
                        />
                      </span>
                      {saved > 0 ? `~${tokensShort(saved)} · ${pct}%` : '0'}
                    </span>
                    <span className="@3xl:hidden">
                      {copy.byAgent.phoneLine(tokensShort(saved), pct, tokensShort(r.pool_tokens))}
                    </span>
                  </TableCell>
                  <TableCell className={cn(CELL, '@max-3xl:hidden')}>
                    <SwitchBadge on={r.compress_enabled} />
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
        {sorted.length > TOP_AGENTS ? (
          <p className="m-0 border-t border-border px-4 pt-3 text-xs text-muted-foreground">
            {all ? null : `${copy.byAgent.caption(shown.length, sorted.length, sort)} `}
            <Button
              variant="link"
              size="sm"
              className="h-auto p-0 text-xs pointer-coarse:min-h-11"
              onClick={() => onShowAll(!all)}
            >
              {all ? copy.byAgent.showTop : copy.byAgent.showAll(sorted.length)}
            </Button>
          </p>
        ) : null}
      </div>
    )
  }
  return (
    <Panel title={copy.byAgent.title} subtitle={period} labelledBy="optimization-by-agent-h">
      {body()}
    </Panel>
  )
}

/** R4A: neutral, never success-green. */
function SwitchBadge({ on }: { on: boolean }) {
  return on ? (
    <Badge variant="secondary">{copy.byAgent.on}</Badge>
  ) : (
    <Badge variant="outline">{copy.byAgent.off}</Badge>
  )
}
