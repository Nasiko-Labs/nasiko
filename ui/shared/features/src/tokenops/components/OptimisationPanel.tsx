/**
 * Token optimisation: what each layer saved, over the selected window.
 *
 * Three readings, in the order someone needs them: the fleet total, the per-category split
 * ("smaller prompts saved this much, less code written saved this much"), and the per-agent table that answers
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
import { m } from 'motion/react'
import { useState, type ReactNode } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { BetaBadge } from '@nasiko/ui/composites/beta-badge'
import { asButton } from '@nasiko/ui/primitives/button'
import { TableBody, TableCell, TableHead, TableHeader, TableRow } from '@nasiko/ui/primitives/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { Swatch } from '@nasiko/ui/composites/chart-marks'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { transitions } from '@nasiko/ui/lib/motion'
import { Panel } from '@nasiko/ui/composites/panel'
import { AgentLink } from '@nasiko/features/agents/components/AgentLink'
import { fmtInt, fmtMoney, fmtPct, fmtTokens } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
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

export function OptimisationPanel({
  data,
  history,
}: {
  data: OptimisationView
  /** The chat-history tile (CX-V3), beside the coverage figures; null when there is none. */
  history?: ReactNode
}) {
  const [tab, setTab] = useState('category')
  const tabs = [
    { value: 'category', label: 'By category', show: true },
    { value: 'agent', label: 'By agent', show: data.rows.length > 0 },
    { value: 'session', label: 'By session', show: data.sessions.length > 0 },
  ].filter((t) => t.show)
  const current = tabs.some((t) => t.value === tab) ? tab : 'category'
  return (
    <Panel
      title="Token optimization savings"
      subtitle="Text we strip out before a call reaches the model — bulky tool output, old chat history, long answers. Everything here is what you did not have to pay for."
      labelledBy="optimization-title"
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
          <div className="grid gap-3 lg:grid-cols-12">
            <Saved data={data} />
            <Coverage data={data} />
          </div>
          {history}

          <Tabs value={current} onValueChange={setTab} className="gap-3">
            <TabsList variant="line" aria-label="Savings breakdown">
              {tabs.map((t) => (
                <TabsTrigger key={t.value} value={t.value} className="relative px-3 after:hidden">
                  {t.label}
                  {t.value === current ? (
                    <m.span
                      layoutId="optimization-tab"
                      transition={transitions.morph}
                      aria-hidden
                      className="absolute inset-x-1 -bottom-1.5 h-0.5 rounded-full bg-foreground"
                    />
                  ) : null}
                </TabsTrigger>
              ))}
            </TabsList>
            <TabsContent value="category">
              <CategoryTable data={data} />
            </TabsContent>
            <TabsContent value="agent">
              <AgentTable data={data} />
            </TabsContent>
            <TabsContent value="session">
              <SessionTable data={data} />
            </TabsContent>
          </Tabs>
        </>
      )}

      {data.unoptimisedCount > 0 ? (
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-md bg-muted px-3 py-2.5 text-sm">
          <p>
            {fmtInt(data.unoptimisedCount)}{' '}
            {data.unoptimisedCount === 1 ? "agent doesn't" : "agents don't"} have optimization on.
            They made <strong>{fmtMoney(data.unoptimisedSpend)}</strong> (
            {Math.round(data.unoptimisedSharePct)}%) of this spend.
            {data.topUnoptimised ? (
              <>
                {' '}
                The biggest is <strong>{data.topUnoptimised.agent_name}</strong>.
              </>
            ) : null}
          </p>
          <Link to="/agents" {...asButton({ variant: 'outline', size: 'sm' })}>
            Turn on for an agent <span aria-hidden>→</span>
          </Link>
        </div>
      ) : null}
    </Panel>
  )
}

/**
 * What was sent against what would have been, for tokens and for cost: two bars on one scale each, the saved part
 * in the second colour. Both reductions are shown together because they differ (savings are mostly input, the cheap
 * side), and the sentence under them says which way, never assuming.
 */
function Saved({ data }: { data: OptimisationView }) {
  const tokenPct = data.savedPct
  const costPct = data.costSavedPct
  const gap =
    tokenPct == null || costPct == null || Math.abs(tokenPct - costPct) < 0.5
      ? null
      : costPct < tokenPct
        ? 'The cost cut is smaller than the token cut: what we trim is mostly input, the cheaper half of a bill.'
        : 'The cost cut is bigger than the token cut: what we trim is mostly output or expensive-model traffic.'
  return (
    <div className="flex flex-col gap-4 rounded-lg border bg-muted/30 p-4 lg:col-span-7">
      <dl className="grid grid-cols-2 gap-4">
        <div>
          <dt className="text-xs text-muted-foreground">Tokens saved</dt>
          <dd className="text-3xl font-semibold tracking-tight tabular-nums">
            {fmtTokens(data.tokensSaved)}
          </dd>
          <dd className="text-xs text-muted-foreground">
            {showPct(tokenPct)} of {fmtTokens(data.tokensBefore)} you would have sent
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Cost saved</dt>
          <dd className="text-3xl font-semibold tracking-tight tabular-nums">
            {fmtMoney(data.costSaved)}
          </dd>
          <dd className="text-xs text-muted-foreground">
            {showPct(costPct)} of {fmtMoney(data.costBefore)}, in this window
          </dd>
        </div>
      </dl>
      <div className="flex flex-col gap-2.5">
        <CompareBar
          label="Tokens"
          sent={data.tokensActual}
          saved={data.tokensSaved}
          fmt={fmtTokens}
        />
        <CompareBar label="Cost" sent={data.costActual} saved={data.costSaved} fmt={fmtMoney} />
        <p className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
          <span className="inline-flex items-center gap-1.5">
            <Swatch series={SENT} /> Sent
          </span>
          <span className="inline-flex items-center gap-1.5">
            <Swatch series={SAVED} /> Saved (not sent)
          </span>
        </p>
      </div>
      {gap ? <p className="text-xs text-muted-foreground">{gap}</p> : null}
    </div>
  )
}

const SENT = seriesAt(0)
const SAVED = seriesAt(3)

function CompareBar({
  label,
  sent,
  saved,
  fmt,
}: {
  label: string
  sent: number
  saved: number
  fmt: (v: number) => string
}) {
  const total = sent + Math.max(0, saved)
  const savedPct = total > 0 ? (Math.max(0, saved) / total) * 100 : 0
  return (
    <div className="grid grid-cols-[3.5rem_1fr] items-center gap-3 text-xs">
      <span className="text-muted-foreground">{label}</span>
      <div className="flex flex-col gap-1">
        <div
          className="flex h-2.5 w-full gap-0.5 overflow-hidden rounded-full"
          role="img"
          aria-label={`${label}: ${fmt(sent)} sent, ${fmt(saved)} saved`}
        >
          <div className="h-full flex-1 rounded-l-full" style={{ backgroundColor: SENT.fill }} />
          <m.div
            className="h-full rounded-r-full"
            style={{ backgroundColor: SAVED.fill }}
            initial={{ width: 0 }}
            animate={{ width: `max(${savedPct}%, 2px)` }}
            transition={transitions.disclosure}
          />
        </div>
        <span className="font-mono text-muted-foreground tabular-nums">
          {fmt(sent)} sent · {fmt(saved)} saved
        </span>
      </div>
    </div>
  )
}

/**
 * Which switch is under-used. A category reading zero because nobody enabled it and one reading zero because it did
 * nothing look identical without this.
 */
function Coverage({ data }: { data: OptimisationView }) {
  const meters = [
    {
      label: 'Agents optimized',
      on: data.optimisedCount,
      of: data.totalAgents,
    },
    { label: 'Calls through it', on: data.callsOptimised, of: data.callsInWindow },
    ...data.adoption.map((a) => ({ label: a.label, on: a.on, of: data.totalAgents })),
  ]
  return (
    <ul
      className="flex flex-col justify-center gap-2.5 rounded-lg border p-4 lg:col-span-5"
      aria-label="Coverage"
    >
      {meters.map((x) => (
        <li key={x.label} className="flex flex-col gap-1 text-xs">
          <span className="flex justify-between gap-2">
            <span className="truncate">{x.label}</span>
            <span className="shrink-0 font-mono text-muted-foreground tabular-nums">
              {fmtInt(x.on)} of {fmtInt(x.of)}
            </span>
          </span>
          <span className="h-1.5 w-full rounded-full bg-muted" aria-hidden>
            <m.span
              className="block h-full rounded-full bg-chart-1-edge"
              initial={{ width: 0 }}
              animate={{ width: `${x.of > 0 ? Math.min(100, (x.on / x.of) * 100) : 0}%` }}
              transition={transitions.disclosure}
            />
          </span>
        </li>
      ))}
    </ul>
  )
}

/** The per-category split: the question "what did each program save" answered directly. */
function CategoryTable({ data }: { data: OptimisationView }) {
  return (
    <ChartTable className="max-h-none">
      <caption className="sr-only">Savings per optimization category</caption>
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
      <caption className="sr-only">Savings per optimized agent</caption>
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
      <TooltipTrigger
        render={
          <span
            role="img"

            // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only to open the tooltip; it has no action
            tabIndex={0}
            className="inline-flex text-muted-foreground"
            aria-label={basis === 'fixture' ? 'Measured by holdout' : 'Estimated'}
          />
        }
      >
        {/* An icon with a name (axe: aria-label needs a role), focusable so keyboard users can open its tooltip. */}

        <Info className="size-3.5" aria-hidden />
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
      <p className="text-sm font-medium">No optimization is switched on yet</p>
      <p className="mx-auto mt-1 max-w-md text-sm text-muted-foreground">
        None of your {fmtInt(total)} {total === 1 ? 'agent has' : 'agents have'} token optimization
        enabled, so there is nothing to measure. Turn it on for one agent and savings appear here as
        soon as it handles traffic.
      </p>
      <Link to="/agents" {...asButton({ variant: 'outline', size: 'sm', className: 'mt-3' })}>
        Choose an agent <span aria-hidden>→</span>
      </Link>
    </div>
  )
}
