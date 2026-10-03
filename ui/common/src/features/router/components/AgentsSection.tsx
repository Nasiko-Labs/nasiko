/**
 * Your agents (plan §4.2): one row per owned agent with its configured routing, key, 30-day router spend and
 * "Change routing". Each row's routing arrives separately (fan-out), so a pending or failed row never blocks
 * the others. Agents that just follow your default fold into one summary row (the rest are the ones worth a look);
 * a name search appears from AGENT_SEARCH_MIN agents.
 */
import { Link } from '@tanstack/react-router'
import { m } from 'motion/react'
import { Bot, ChevronRight, RotateCw, Search, SearchX } from 'lucide-react'
import { useId, useState } from 'react'
import { EmptyState } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { AgentLinkTo, AgentMark } from '@/features/agents/components/bits'
import { isHarness } from '@/features/agents/status'
import type { Agent } from '@/features/agents/types'
import { fmtInt, fmtLocalTime, fmtMoney } from '@/lib/format'
import { transitions } from '@/lib/motion'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import { PREF_DEFAULTS, readOpen, writeOpen } from '../prefs'
import { groupAgentRows, keySource, matchesAgent, routingSentence, type RowRead } from '../routing'
import { AGENT_SEARCH_MIN, CONFIG_CACHE_SECONDS, DEFAULT_GROUP_MIN } from '../tuning'
import type { AgentUsage } from '../types'
import { KeySourceChip, LinkButton, SentenceText, SourceBadge } from './bits'

export interface AgentRow {
  agent: Agent
  read: RowRead
}

export interface SpendState {
  byAgent: Map<string, AgentUsage>
  failed: boolean
  pending: boolean
  /** Calls from rows the page dropped (deleted agents, other owners). */
  dropped: number
  partial: boolean
  anyUnpriced: boolean
}

// A table from md (plan §4.2). Below 768 px each row stacks, and the header stays for screen readers only.
const ROW = 'text-sm hover:bg-transparent max-md:grid max-md:gap-1.5 max-md:px-4 max-md:py-2.5'
const CELL = 'p-0 whitespace-normal max-md:block md:px-1.5 md:py-2.5 md:first:pl-4 md:last:pr-4'
const HEAD = 'h-auto px-1.5 py-2 text-xs text-muted-foreground first:pl-4 last:pr-4'

export function AgentsSection({
  rows,
  total,
  spend,
  updated,
  onChange,
  onRetry,
  onShowConfig,
  filterLabel,
  onClearFilter,
}: {
  rows: readonly AgentRow[]
  /** All owned agents, before a filter: decides whether the search shows (so a filter never hides a typed query). */
  total: number
  /** Scroll to and focus a config's row in Your configs (plan §4.1). */
  onShowConfig: (configId: string) => void
  spend: SpendState
  updated: ReadonlyMap<string, number>
  onChange: (a: Agent) => void
  onRetry: (id: string) => void
  filterLabel: string | null
  onClearFilter: () => void
}) {
  const ids = useId()
  const [query, setQuery] = useState('')
  const [open, setOpen] = useState(() => readOpen(PREF_DEFAULTS) ?? false)
  const searchable = total >= AGENT_SEARCH_MIN
  const visible = searchable ? rows.filter((r) => matchesAgent(r.agent, query)) : rows
  const { shown, folded } = groupAgentRows(visible, {
    narrowed: !!filterLabel || !!query.trim(),
    recent: new Set(updated.keys()),
    min: DEFAULT_GROUP_MIN,
  })
  const toggle = () => {
    setOpen(!open)
    writeOpen(PREF_DEFAULTS, !open)
  }
  const row = (r: AgentRow) => (
    <Row
      key={r.agent.id}
      row={r}
      spend={spend}
      updatedAt={updated.get(r.agent.id)}
      onChange={() => onChange(r.agent)}
      onRetry={() => onRetry(r.agent.id)}
      onShowConfig={onShowConfig}
    />
  )
  return (
    <section id="router-agents" aria-labelledby="router-agents-h" className="scroll-mt-4 space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="router-agents-h" tabIndex={-1} className="text-sm font-semibold">
          {copy.agentsTitle}
        </h2>
        {filterLabel ? (
          <p className="flex items-center gap-2 text-xs text-muted-foreground">
            {copy.filteredBy(filterLabel)}
            <Button
              size="sm"
              variant="ghost"
              className="h-7 pointer-coarse:min-h-11"
              onClick={onClearFilter}
            >
              {copy.clearFilter}
            </Button>
          </p>
        ) : null}
      </div>
      {searchable ? (
        <div className="relative max-w-xs">
          <Search
            className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
            aria-hidden
          />
          <Input
            type="search"
            aria-label={copy.searchAgents}
            placeholder={copy.searchAgents}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="pl-8"
          />
        </div>
      ) : null}
      {rows.length > 0 && visible.length === 0 ? (
        <EmptyState icon={SearchX} title={copy.searchNone(query.trim())}>
          {copy.searchNoneText}
        </EmptyState>
      ) : rows.length === 0 ? (
        filterLabel ? (
          // The filter line above already offers Show all.
          <EmptyState icon={SearchX} title={copy.noneInFilter}>
            {copy.noneInFilterText}
          </EmptyState>
        ) : (
          <EmptyState
            icon={Bot}
            title={copy.noAgents}
            action={
              <Button asChild size="sm" variant="outline" className="pointer-coarse:min-h-11">
                <Link to="/agents" search={{}}>
                  {copy.toAgents}
                </Link>
              </Button>
            }
          >
            {copy.noAgentsText}
          </EmptyState>
        )
      ) : (
        <div className="rounded-lg border border-border bg-card">
          <Table aria-label={copy.agentsTitle} className="max-md:block md:table-fixed">
            <TableHeader className="max-md:sr-only">
              <TableRow className="hover:bg-transparent">
                <TableHead className={HEAD}>{copy.colAgent}</TableHead>
                <TableHead className={cn(HEAD, 'w-28')}>{copy.colSource}</TableHead>
                <TableHead className={cn(HEAD, 'w-[32%]')}>{copy.colRouting}</TableHead>
                <TableHead className={HEAD}>{copy.colKey}</TableHead>
                <TableHead className={cn(HEAD, 'w-34 text-right')}>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <LinkButton className="font-medium text-inherit underline decoration-dotted">
                        {copy.colSpend}
                      </LinkButton>
                    </TooltipTrigger>
                    <TooltipContent className="max-w-xs">{copy.spendTip}</TooltipContent>
                  </Tooltip>
                </TableHead>
                <TableHead className={cn(HEAD, 'w-32')}>
                  <span className="sr-only">{copy.colActions}</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody className="max-md:block">
              {shown.map(row)}
              {folded.length ? (
                <FoldedRow
                  rows={folded}
                  spend={spend}
                  open={open}
                  controls={`${ids}-defaults`}
                  onToggle={toggle}
                  more={shown.some(
                    (r) => r.read.state === 'ok' && r.read.routing.source === 'owner-default',
                  )}
                />
              ) : null}
            </TableBody>
            {/* Fades in only: an exit animation would keep the old rows on screen (and clickable) while the same agents
                already render above when a filter, search or save unfolds the group. */}
            {folded.length && open ? (
              <m.tbody
                id={`${ids}-defaults`}
                className="border-t border-border max-md:block [&_tr:last-child]:border-0"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                transition={transitions.disclosure}
              >
                {folded.map(row)}
              </m.tbody>
            ) : null}
          </Table>
          <Footer spend={spend} />
        </div>
      )}
    </section>
  )
}

function Row({
  row,
  spend,
  updatedAt,
  onChange,
  onRetry,
  onShowConfig,
}: {
  row: AgentRow
  spend: SpendState
  updatedAt?: number
  onChange: () => void
  onRetry: () => void
  onShowConfig: (id: string) => void
}) {
  const { agent, read } = row
  const name = agent.display_name || agent.name
  const harness = isHarness(agent)
  const config = read.state === 'ok' ? read.routing.llm_config : null
  return (
    <TableRow data-agent={agent.id} className={ROW}>
      <TableCell className={CELL}>
        <div className="flex min-w-0 items-center gap-2">
          <AgentMark name={name} size={28} />
          <div className="min-w-0">
            <AgentLinkTo id={agent.id} className="block truncate font-medium">
              {name}
            </AgentLinkTo>
            {harness ? (
              <span className="text-xs text-muted-foreground">{copy.codingHarness}</span>
            ) : null}
          </div>
        </div>
      </TableCell>
      {read.state === 'ok' ? (
        <>
          <TableCell className={CELL}>
            <SourceBadge source={read.routing.source} />
          </TableCell>
          <TableCell className={cn(CELL, 'min-w-0 text-muted-foreground')}>
            {config ? (
              <LinkButton
                className="block max-w-full truncate text-xs text-foreground"
                onClick={() => onShowConfig(config.id)}
              >
                {config.name}
              </LinkButton>
            ) : null}
            <SentenceText
              sentence={routingSentence(read.routing.llm_config, read.routing.pinned_model)}
            />
            {updatedAt ? (
              <span className="block text-xs">
                {copy.updatedMarker(
                  fmtLocalTime(updatedAt),
                  fmtLocalTime(updatedAt + CONFIG_CACHE_SECONDS * 1000),
                )}
              </span>
            ) : null}
          </TableCell>
          <TableCell className={cn(CELL, 'min-w-0')}>
            <KeySourceChip source={keySource(read.routing.llm_config)} />
          </TableCell>
        </>
      ) : read.state === 'pending' ? (
        <TableCell colSpan={3} className={cn(CELL, 'text-muted-foreground')}>
          {copy.readingRouting}
        </TableCell>
      ) : (
        <TableCell colSpan={3} className={CELL}>
          <div className="flex items-center gap-2">
            <span className="text-destructive">{copy.couldntRead}</span>
            <Button
              size="sm"
              variant="outline"
              className="h-7 pointer-coarse:min-h-11"
              onClick={onRetry}
            >
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          </div>
        </TableCell>
      )}
      <TableCell className={cn(CELL, 'text-muted-foreground md:text-right')}>
        <SpendCell agentId={agent.id} spend={spend} />
      </TableCell>
      <TableCell className={cn(CELL, 'md:text-right')}>
        <Button
          size="sm"
          variant="ghost"
          className="h-8 px-2 text-primary-text pointer-coarse:min-h-11"
          onClick={onChange}
          disabled={read.state !== 'ok'}
          aria-label={`${copy.changeRouting}: ${name}`}
        >
          {copy.changeRouting}
        </Button>
      </TableCell>
    </TableRow>
  )
}

function SpendCell({ agentId, spend }: { agentId: string; spend: SpendState }) {
  if (spend.pending) return <span aria-busy="true">…</span>
  // The reason is shown once, in the footer; each cell only marks the gap.
  if (spend.failed)
    return (
      <span>
        <span aria-hidden>—</span>
        <span className="sr-only">{copy.spendFailed}</span>
      </span>
    )
  const u = spend.byAgent.get(agentId)
  if (!u || u.request_count === 0) return <span>{copy.spendNone}</span>
  // COALESCE(SUM(cost_usd), 0): calls with no price add $0, so $0 with calls means unpriced.
  const unpriced = u.total_cost_usd === 0
  return (
    <span>
      {copy.spendValue(
        `${fmtMoney(u.total_cost_usd)}${unpriced ? copy.unpricedMark : ''}`,
        fmtInt(u.request_count),
      )}
    </span>
  )
}

/** The agents on your default, as one row: what they share, their spend together, and Show all / Hide. */
function FoldedRow({
  rows,
  spend,
  open,
  controls,
  onToggle,
  more,
}: {
  rows: readonly AgentRow[]
  spend: SpendState
  open: boolean
  controls: string
  onToggle: () => void
  /** Some agents on the default already show above (overridden, or just changed): the title says "more". */
  more: boolean
}) {
  const first = rows[0]?.read
  const config = first?.state === 'ok' ? first.routing.llm_config : null
  const key = keySource(config)
  const total = rows.reduce(
    (t, r) => {
      const u = spend.byAgent.get(r.agent.id)
      // $0 with calls means unpriced (COALESCE), as SpendCell marks it.
      return u
        ? {
            cost: t.cost + u.total_cost_usd,
            calls: t.calls + u.request_count,
            unpriced: t.unpriced || (u.request_count > 0 && u.total_cost_usd === 0),
          }
        : t
    },
    { cost: 0, calls: 0, unpriced: false },
  )
  return (
    <TableRow data-folded className="bg-muted/30 hover:bg-muted/30 max-md:block">
      <TableCell colSpan={6} className="p-0 max-md:block">
        <Button
          variant="ghost"
          aria-expanded={open}
          aria-controls={controls}
          onClick={onToggle}
          className="h-auto w-full flex-wrap justify-start gap-x-3 gap-y-1 rounded-none px-4 py-2.5 text-left font-normal whitespace-normal hover:bg-muted/60 has-[>svg]:px-4 pointer-coarse:min-h-11"
        >
          <ChevronRight
            className={cn(
              'size-4 shrink-0 text-muted-foreground transition-transform motion-reduce:transition-none',
              open && 'rotate-90',
            )}
            aria-hidden
          />
          <span className="font-medium">
            {more
              ? copy.foldedTitleMore(fmtInt(rows.length))
              : copy.foldedTitle(fmtInt(rows.length))}
          </span>
          {config ? (
            <span className="text-muted-foreground">
              {copy.foldedDetail(
                config.name,
                key.kind === 'user' ? copy.yourKey(key.secret) : copy.platformKey,
              )}
            </span>
          ) : null}
          <span className="ml-auto flex items-center gap-3">
            {!spend.pending && !spend.failed && total.calls ? (
              <span className="text-muted-foreground">
                {copy.foldedSpend(
                  `${fmtMoney(total.cost)}${total.unpriced ? copy.unpricedMark : ''}`,
                  fmtInt(total.calls),
                )}
              </span>
            ) : null}
            <span className="font-medium text-primary-text">
              {open ? copy.foldedHide : copy.foldedShow(fmtInt(rows.length))}
            </span>
          </span>
        </Button>
      </TableCell>
    </TableRow>
  )
}

function Footer({ spend }: { spend: SpendState }) {
  const notes = [
    spend.failed ? copy.spendFailed : null,
    spend.anyUnpriced ? copy.unpricedNote : null,
    spend.dropped ? copy.droppedNote(fmtInt(spend.dropped)) : null,
    spend.partial ? copy.partialNote : null,
  ].filter(Boolean)
  if (!notes.length) return null
  return (
    <div className="space-y-0.5 border-t border-border px-4 py-2 text-xs text-muted-foreground">
      {notes.map((n) => (
        <p key={n}>{n}</p>
      ))}
    </div>
  )
}
