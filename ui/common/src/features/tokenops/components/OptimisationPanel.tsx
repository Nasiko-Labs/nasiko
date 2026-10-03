/**
 * Token optimisation: what each layer saved, over the selected window.
 *
 * Three readings, in the order someone needs them: the fleet total, the per-category split
 * ("Caveman saved this much, Ponytail saved this much"), and the per-agent table that answers
 * which agent to turn it on for next.
 *
 * Both reduction percentages are always shown together. The gap between them is real and expected
 * — savings are overwhelmingly input tokens and input is the cheap side — and showing only the
 * token figure invites the reader to assume the cost figure matches it.
 *
 * Figures whose basis is not `measured` carry a marker. Those two layers (the brevity directive and
 * the minimal-code ladder) have no observable counterfactual, so their number is a percentage
 * applied to counted traffic rather than a subtraction, and that has to be visible rather than
 * blended in silently.
 */
import { Link } from '@tanstack/react-router'
import { Info } from 'lucide-react'
import type { ReactNode } from 'react'
import { Badge } from '@/components/ui/badge'
import { BetaBadge } from '@/components/shared/beta-badge'
import { Button } from '@/components/ui/button'
import { TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { Panel } from '@/components/shared/panel'
import { AgentLink } from '@/features/agents/components/AgentLink'
import { fmtInt, fmtMoney, fmtPct, fmtTokens } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { OptimisationView } from '../optimisation'
import { isUnconfigured } from '../optimisation'
import type { SavingsBasis } from '../types'
import { CELL, ChartTable, HEAD, NUM, ROW_HEAD, STICKY_HEAD } from './ChartTable'

/**
 * What each layer trims, in plain words.
 *
 * The ledger's names are injection points (`compress_payload`, `context_selection`); nobody reading
 * a spend dashboard knows or needs to know where in the stack a saving was made. They need to know
 * what got shorter.
 */
const LAYER_LABEL: Record<string, string> = {
  compress_payload: 'tool output',
  compress_tool_result: 'repeated tool output',
  compress_history: 'chat history',
  context_selection: 'history kept per message',
  brevity: 'answer length',
  minimal_code: 'code written',
  prompt_comments: 'agent instructions',
}

/** `null` percentages mean "no baseline to compare against", not zero. */
const showPct = (v: number | null | undefined) => (v == null ? '—' : fmtPct(v))

export function OptimisationPanel({ data }: { data: OptimisationView }) {
  return (
    <Panel
      title="Token optimisation savings"
      subtitle="Text we strip out before a call reaches the model — bulky tool output, old chat history, long answers. Everything here is what you did not have to pay for."
      labelledBy="optimisation-title"
      actions={
        <span className="flex items-center gap-1.5">
          <BetaBadge />
          <BasisBadge basis={data.basis} calibratedPct={data.calibratedPct} />
        </span>
      }
    >
      {isUnconfigured(data) ? (
        <EmptyState total={data.totalAgents} />
      ) : (
        <>
          <dl className="grid gap-px overflow-hidden rounded-md border bg-border sm:grid-cols-4">
            <Figure label="Tokens saved" value={fmtTokens(data.tokensSaved)}>
              you would have sent {fmtTokens(data.tokensBefore)} without this
            </Figure>
            <Figure label="Token reduction" value={showPct(data.savedPct)}>
              of everything these agents would have sent
            </Figure>
            <Figure label="Cost saved" value={fmtMoney(data.costSaved)}>
              on this month&apos;s bill
            </Figure>
            <Figure label="Cost reduction" value={showPct(data.costSavedPct)}>
              smaller than the token figure — what we trim is the cheaper half of a bill
            </Figure>
          </dl>

          {/* Which switch is under-used. A category reading zero because nobody enabled it and one
              reading zero because it did nothing look identical without this. */}
          <p className="text-xs text-muted-foreground">
            Turned on for: {data.adoption.map((a) => `${a.label} ${fmtInt(a.on)}`).join(' · ')} —
            out of {fmtInt(data.totalAgents)} agents. {fmtInt(data.callsOptimised)} of{' '}
            {fmtInt(data.callsInWindow)} calls went through it.
          </p>

          <CategoryTable data={data} />
          <AgentTable data={data} />
          <SessionTable data={data} />
        </>
      )}

      {data.unoptimisedCount > 0 ? (
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-md bg-muted px-3 py-2.5 text-sm">
          <p>
            {fmtInt(data.unoptimisedCount)}{' '}
            {data.unoptimisedCount === 1 ? "agent doesn't" : "agents don't"} have optimisation on.
            They made <strong>{fmtMoney(data.unoptimisedSpend)}</strong> (
            {Math.round(data.unoptimisedSharePct)}%) of fleet spend.
            {data.topUnoptimised ? (
              <>
                {' '}
                The biggest is <strong>{data.topUnoptimised.agent_name}</strong>.
              </>
            ) : null}
          </p>
          <Button asChild variant="outline" size="sm">
            <Link to="/agents">
              Turn on for an agent <span aria-hidden>→</span>
            </Link>
          </Button>
        </div>
      ) : null}
    </Panel>
  )
}

/** The per-category split: the question "what did Caveman save vs Ponytail" answered directly. */
function CategoryTable({ data }: { data: OptimisationView }) {
  return (
    <ChartTable className="max-h-none">
      <caption className="sr-only">Savings per optimisation category</caption>
      <TableHeader className={STICKY_HEAD}>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col" className={HEAD}>
            Category
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'w-1/4')}>
            Tokens saved
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Token reduction
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Cost saved
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Cost reduction
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {data.categories.map((c) => (
          <TableRow key={c.program}>
            <TableHead scope="row" className={ROW_HEAD}>
              <span className="flex items-center gap-1.5 font-medium">
                {c.label}
                <BasisMark basis={c.basis} notes={c.factorNotes} />
              </span>
              {c.note ? (
                <span className="block text-xs font-normal text-muted-foreground">{c.note}</span>
              ) : null}
              {c.byTier.length > 0 ? (
                <span className="block text-xs font-normal text-muted-foreground">
                  {c.byTier.map((t) => `${t.tier}: ${fmtTokens(t.saved_tokens)}`).join(' · ')}
                </span>
              ) : null}
              {/* A program is several injection points; the layer names are what an engineer
                  needs to know which one is carrying the category. */}
              {c.layers.length > 1 ? (
                <span className="block text-xs font-normal text-muted-foreground">
                  {c.layers
                    .filter((l) => l.savedTokens !== 0)
                    .map((l) => `${LAYER_LABEL[l.layer] ?? l.layer}: ${fmtTokens(l.savedTokens)}`)
                    .join(' · ')}
                </span>
              ) : null}
            </TableHead>
            <TableCell className={CELL}>
              <div className="flex items-center gap-2.5">
                <div className="h-2 flex-1 overflow-hidden rounded-xs bg-muted" aria-hidden>
                  <div className="h-full bg-chart-1" style={{ width: `${c.barPct}%` }} />
                </div>
                <span className="w-16 text-right font-mono text-xs tabular-nums">
                  {fmtTokens(c.savedTokens)}
                </span>
              </div>
            </TableCell>
            <TableCell className={cn(CELL, NUM, 'font-semibold')}>{showPct(c.tokenPct)}</TableCell>
            <TableCell className={cn(CELL, NUM)}>{fmtMoney(c.savedCost)}</TableCell>
            <TableCell className={cn(CELL, NUM, 'font-semibold')}>{showPct(c.costPct)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </ChartTable>
  )
}

function AgentTable({ data }: { data: OptimisationView }) {
  if (data.rows.length === 0) return null
  return (
    <ChartTable className="max-h-none">
      <caption className="sr-only">Savings per optimised agent</caption>
      <TableHeader className={STICKY_HEAD}>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col" className={HEAD}>
            Agent
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Calls
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Sent before → after
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'w-1/3')}>
            Tokens saved
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Cost saved
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {data.rows.map((r) => (
          <TableRow key={r.id}>
            <TableHead scope="row" className={ROW_HEAD}>
              <AgentLink id={r.id} name={r.name} className="font-medium">
                {r.name}
              </AgentLink>
            </TableHead>
            <TableCell className={cn(CELL, NUM)}>{fmtInt(r.calls)}</TableCell>
            <TableCell className={cn(CELL, NUM, 'font-mono')}>
              {fmtTokens(r.before)} <span className="text-muted-foreground">→</span>{' '}
              {fmtTokens(r.after)}
            </TableCell>
            <TableCell className={CELL}>
              <div className="flex items-center gap-2.5">
                <div className="h-2 flex-1 overflow-hidden rounded-xs bg-muted" aria-hidden>
                  <div className="h-full bg-chart-1" style={{ width: `${r.barPct}%` }} />
                </div>
                <span className="w-12 text-right font-semibold tabular-nums">
                  {fmtPct(r.savedPct)}
                </span>
              </div>
            </TableCell>
            <TableCell className={cn(CELL, NUM, 'font-medium')}>{fmtMoney(r.costSaved)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </ChartTable>
  )
}

/**
 * Which conversations this actually helped.
 *
 * Savings recur per turn, so a long session compounds them in a way the per-call view understates —
 * this is where that shows up, and where you look when someone claims optimisation broke an answer.
 */
function SessionTable({ data }: { data: OptimisationView }) {
  if (data.sessions.length === 0) return null
  return (
    <ChartTable className="max-h-none">
      <caption className="sr-only">Savings per session</caption>
      <TableHeader className={STICKY_HEAD}>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col" className={HEAD}>
            Session
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Turns
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Tokens saved
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Reduction
          </TableHead>
          <TableHead scope="col" className={cn(HEAD, 'text-right')}>
            Cost saved
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {data.sessions.map((sess) => (
          <TableRow key={sess.id}>
            <TableHead scope="row" className={ROW_HEAD}>
              <Link
                to="/sessions/$sessionId"
                params={{ sessionId: sess.id }}
                className="font-medium hover:underline"
              >
                {sess.id.slice(0, 12)}
              </Link>
              {sess.agents.length > 0 ? (
                <span className="block text-xs font-normal text-muted-foreground">
                  {sess.agents.join(', ')}
                </span>
              ) : null}
            </TableHead>
            <TableCell className={cn(CELL, NUM)}>{fmtInt(sess.turns)}</TableCell>
            <TableCell className={cn(CELL, NUM, 'font-mono')}>
              {fmtTokens(sess.savedTokens)}
            </TableCell>
            <TableCell className={cn(CELL, NUM, 'font-semibold')}>
              {showPct(sess.tokenPct)}
            </TableCell>
            <TableCell className={cn(CELL, NUM, 'font-medium')}>
              {fmtMoney(sess.savedCost)}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </ChartTable>
  )
}

/**
 * Marks a figure that is not a measurement.
 *
 * Not colour-only: the icon and its tooltip carry the meaning, so it survives a colour-blind
 * reader and a greyscale print.
 */
function BasisMark({ basis, notes }: { basis: SavingsBasis; notes?: string }) {
  if (basis === 'measured') return null
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          className="inline-flex text-muted-foreground"
          aria-label={basis === 'fixture' ? 'Measured by holdout' : 'Estimated'}
        >
          <Info className="size-3.5" aria-hidden />
        </span>
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">
        {notes ??
          (basis === 'fixture'
            ? 'Measured against a holdout arm rather than subtracted directly.'
            : 'This layer has no observable counterfactual, so its figure is an estimate applied to counted eligible traffic.')}
      </TooltipContent>
    </Tooltip>
  )
}

function BasisBadge({
  basis,
  calibratedPct,
}: {
  basis: SavingsBasis
  calibratedPct: number | null
}) {
  if (basis === 'measured') {
    return (
      <Badge variant="outline" className="text-muted-foreground">
        Measured
        {calibratedPct != null ? ` · ${Math.round(calibratedPct)}% calibrated` : null}
      </Badge>
    )
  }
  return (
    <Badge variant="outline" className="border-dashed text-muted-foreground">
      {basis === 'mixed' ? 'Measured + estimated' : 'Estimated'}
    </Badge>
  )
}

/** Before anything is switched on, a row of zeroes is noise. Say what to do instead. */
function EmptyState({ total }: { total: number }) {
  return (
    <div className="rounded-md border border-dashed px-4 py-6 text-center">
      <p className="text-sm font-medium">No optimisation is switched on yet</p>
      <p className="mx-auto mt-1 max-w-md text-sm text-muted-foreground">
        None of your {fmtInt(total)} {total === 1 ? 'agent has' : 'agents have'} token optimisation
        enabled, so there is nothing to measure. Turn it on for one agent and savings appear here as
        soon as it handles traffic.
      </p>
      <Button asChild variant="outline" size="sm" className="mt-3">
        <Link to="/agents">
          Choose an agent <span aria-hidden>→</span>
        </Link>
      </Button>
    </div>
  )
}

function Figure({
  label,
  value,
  children,
}: {
  label: string
  value: ReactNode
  children: ReactNode
}) {
  return (
    <div className="flex min-w-0 flex-col gap-1 bg-card p-4">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="flex flex-col gap-1.5">
        <span className="text-2xl font-semibold tabular-nums sm:text-3xl">{value}</span>
        <span className="text-xs text-muted-foreground">{children}</span>
      </dd>
    </div>
  )
}
