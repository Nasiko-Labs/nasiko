/**
 * A harness session's ledger (plans/feat-agent-centric.md PR1b; design "Ledger"; eng "Ledger"): the session route's
 * `?view=ledger`. One row per turn: tools, tokens, cost and the running total, then the controls that acted. Mounted by
 * the route, outside the trace page's success branch, so recorded messages render even when the observability session
 * read or Tempo fails (the "live floor"). Rules in `ledger.ts`; reads in `api.ts`.
 *
 * - Messages are owner-only (the server's rule, no superuser bypass): anyone else gets metadata from the trace.
 * - Context, memory, applied controls and budget use are previews (UC2): drawn from mocks only, labelled, and absent in
 *   live mode. Live, Context and Memory read "not reported".
 * - The ledger never edits a control: "Controls set now" links to where each is set.
 */
import type { Slots } from '@nasiko/app-core/edition/edition'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { Link } from '@tanstack/react-router'
import { Download, ListTree } from 'lucide-react'
import { useEffect, useRef, type ReactNode } from 'react'
import { DataTable, type DataTableColumn } from '@nasiko/ui/composites/data-table'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { Panel } from '@nasiko/ui/composites/panel'
import { EmptyState } from '@nasiko/ui/composites/state-card'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { pickShared } from '@nasiko/app-core/shell/context'
import { useAgentsDirectory } from '@nasiko/features/agents/api'
import { isHarness } from '@nasiko/features/agents/status'
import { sessionRows, useChatSessions } from '@nasiko/features/chat/api'
import { useSessionLookup } from '@nasiko/features/chat/sessionLookup'
import { isMetadataOnly, type RecordedStep } from '@nasiko/features/chat/recorded'
import { copy as obsCopy } from '@nasiko/features/observability/copy'
import { detailSubagents, type SubagentRun } from '@nasiko/features/observability/sessions'
import { ErrorState } from '@nasiko/features/observability/StateCard'
import { useSessionDetail } from '@nasiko/features/sessions/api'
import type { SessionsSearch, TraceSearch } from '@nasiko/features/sessions/search'
import { downloadCsv } from '@nasiko/features/tokenops/csv'
import { Crumbs } from '@nasiko/features/trace/Crumbs'
import { isPreview } from '@nasiko/app-core/env'
import {
  fmtMoney,
  fmtMoneyFloor,
  fmtTokens,
  fmtUtcDayTime,
  fmtUtcTime,
} from '@nasiko/ui/lib/format'
import { useLedgerHistory } from '../api'
import { copy } from '../copy'
import { ledgerCsv, type Ledger, type LedgerTurn, type Total } from '../ledger'
import { RetryLine } from './RetryLine'

const c = copy.ledger
const LINK =
  'inline-flex items-center text-sm text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11'
const pct = (v: number) =>
  v > 0 && v < 0.1 ? '<0.1%' : `${v < 1 && v > 0 ? v.toFixed(1) : Math.round(v)}%`

/** A spawning tool call's id → the subagent session it started (R15). */
type Spawned = ReadonlyMap<string, SubagentRun>

export function LedgerPage({
  sessionId,
  search,
  lineage,
}: {
  sessionId: string
  search: TraceSearch
  /** The parent link and the Subagents panel (R15), from the route; drawn under the header. */
  lineage?: ReactNode
}) {
  // Budgets are Enterprise only: an OSS build has no session budget here, and no per-turn budget column.
  const { LedgerSection: LedgerBudget, useLedgerBudgets } = useSlots().budgets
  const { query, ledger, complete, notOwner, refreshFailed } = useLedgerHistory(sessionId)
  const session = useSessionDetail(sessionId)
  const chats = useChatSessions()
  const dir = useAgentsDirectory()
  const loaded = sessionRows(chats.data).find((r) => r.session_id === sessionId)
  // Past the rail's loaded pages, chat's own lookup pages on from where the rail stopped (review: Codex).
  const lastPage = chats.data?.pages.at(-1)
  const lookup = useSessionLookup(
    sessionId,
    // Only once ownership is known: a non-owner's ledger never pages through the rail.
    !!ledger && !loaded && !!lastPage?.has_more,
    lastPage?.next_cursor ?? undefined,
  )
  const row = loaded ?? (lookup.data?.status === 'found' ? lookup.data.row : undefined)
  const agentName = session.data?.agent_name || row?.agent_name || ''
  // By id when the chat names it; by name only when one visible agent has it (never last-wins).
  const named = dir.byNameAll.get(agentName)
  const agent =
    (row?.agent_id ? dir.byId.get(row.agent_id) : undefined) ??
    (named?.length === 1 ? named[0] : undefined)
  const harness =
    !!row?.is_coding_agent ||
    (agent ? isHarness(agent) : false) ||
    !!ledger?.turns.some((t) => t.tools.length)
  const costHeader = harness ? c.estCost : c.cost
  const budgets = useLedgerBudgets(sessionId, !!ledger && !notOwner)

  const back = {
    ...pickShared(search),
    day: search.day,
    sort: search.sort,
    lane: search.lane,
    status: search.status,
    live: search.live,
  }
  // Best effort: a subagent whose spawning call is one of this ledger's recorded calls marks that call.
  const spawned: Spawned = new Map(
    (session.data ? detailSubagents(session.data).subagents : []).flatMap((r) =>
      r.spawnToolCallId ? [[r.spawnToolCallId, r] as const] : [],
    ),
  )
  const title = row?.title || session.data?.title || sessionId
  const firstAt = ledger?.turns[0]?.at ?? session.data?.traces[0]?.root_span.start_time

  const exportCsv = () => {
    if (ledger)
      downloadCsv(
        c.csvName(sessionId),
        ledgerCsv(ledger, { ...c.csv, cost: costHeader, notReported: copy.notReported }),
      )
  }

  return (
    // The turns table is wider than the page at most widths: containment keeps it scrolling inside its own box instead
    // of widening the shell's content column (which has no min-width of 0).
    <div className="flex flex-col gap-4 contain-inline-size">
      <PageHeader
        breadcrumb={<Crumbs back={back} day={search.day} title={title} />}
        title={
          <span className="inline-block">
            {c.title}
            {agentName ? ` · ${agent?.display_name || agentName}` : ''}
            {firstAt ? ` · ${fmtUtcDayTime(firstAt)}` : ''}
          </span>
        }
        actions={
          <>
            <Link
              to="/sessions/$sessionId"
              params={{ sessionId }}
              search={{ ...search, view: undefined }}
              {...asButton({
                variant: 'outline',
                size: 'sm',
                className: 'pointer-coarse:min-h-11',
              })}
            >
              <ListTree className="size-3.5" aria-hidden /> {c.trace}
            </Link>
            {ledger?.turns.length ? (
              <Button
                variant="outline"
                size="sm"
                className="pointer-coarse:min-h-11"
                onClick={exportCsv}
              >
                <Download className="size-3.5" aria-hidden /> {c.exportCsv}
              </Button>
            ) : null}
          </>
        }
      />

      {lineage}

      {notOwner ? (
        <NotOwner session={session} />
      ) : query.isPending ? (
        <PageLoader label={c.loading} />
      ) : query.isError && !ledger ? (
        <ErrorState error={query.error} onRetry={() => void query.refetch()} />
      ) : ledger && !ledger.turns.length && !ledger.unplaced.length ? (
        <EmptyState icon={ListTree} title={c.empty}>
          {isMetadataOnly(row) ? c.metadataOnly : null}
        </EmptyState>
      ) : ledger ? (
        <LedgerBody
          ledger={ledger}
          costHeader={costHeader}
          complete={complete}
          budgets={budgets}
          olderPending={query.isFetchingNextPage}
          olderError={query.isFetchNextPageError}
          onOlder={() => void query.fetchNextPage()}
          refreshFailed={refreshFailed}
          onRefresh={() => void query.refetch()}
          agentId={agent?.id ?? row?.agent_id ?? null}
          spawned={spawned}
          back={back}
        />
      ) : null}
      {/* The session's own budget (plans/feat-budget-burn.md F9, B5): its burn on its turns and its limit. */}
      {LedgerBudget ? (
        <LedgerBudget sessionId={sessionId} title={title} ownSession={!!row} />
      ) : null}
    </div>
  )
}

function LedgerBody({
  ledger,
  costHeader,
  complete,
  budgets,
  olderPending,
  olderError,
  onOlder,
  refreshFailed,
  onRefresh,
  agentId,
  spawned,
  back,
}: {
  ledger: Ledger
  costHeader: string
  complete: boolean
  budgets: ReturnType<Slots['budgets']['useLedgerBudgets']>
  olderPending: boolean
  olderError: boolean
  onOlder: () => void
  refreshFailed: boolean
  onRefresh: () => void
  agentId: string | null
  spawned: Spawned
  back: Partial<SessionsSearch>
}) {
  const preview = isPreview('chat')
  // When the last older page lands, Load older turns unmounts: focus moves to the turns instead of the page body.
  const region = useRef<HTMLDivElement>(null)
  const loadingOlder = useRef(false)
  useEffect(() => {
    if (complete && loadingOlder.current) region.current?.focus()
    if (complete) loadingOlder.current = false
  }, [complete])
  // Times carry the date when the turns span more than one UTC day.
  const days = new Set(ledger.turns.map((x) => x.at.slice(0, 10)))
  // Budgets are Enterprise only: an OSS build reads nothing here and the column, the headline and the panel are
  // all absent. A budget's running share needs its use before the session, which the layer applies.
  const { shares, headline: budgetHeadline, panel: budgetPanel } = budgets
  const t = ledger.totals
  return (
    <>
      {/* Sticky running total: what the loaded turns add up to, and whether that's the whole session. */}
      <section
        aria-label={c.totals.label}
        className="sticky top-0 z-20 -mx-4 flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-border bg-background/95 px-4 py-2 text-sm backdrop-blur"
      >
        <span className="font-medium tabular-nums">{c.totals.turns(t.turns)}</span>
        <span className="tabular-nums">{c.totals.calls(t.toolCalls, t.failedCalls)}</span>
        <span className="tabular-nums">
          {c.totals.tokens(
            totalText(t.tokensIn, t.turns, fmtTokens),
            totalText(t.tokensOut, t.turns, fmtTokens),
          )}
        </span>
        <span className="tabular-nums">
          {costHeader}: {totalText(t.cost, t.turns, fmtMoney)}
        </span>
        {budgetHeadline}
        <span className="text-xs text-muted-foreground">
          {complete ? c.totals.complete : c.totals.subtotal}
        </span>
      </section>

      {refreshFailed ? <RetryLine label={c.refreshError} onRetry={onRefresh} /> : null}

      {preview && ledger.hasPreview ? (
        <p className="m-0 flex items-center gap-2 text-xs text-muted-foreground">
          <Badge variant="outline">{copy.rail.preview}</Badge>
          {c.previewColumns}
        </p>
      ) : null}

      {/* The turns are wider than most screens: this box scrolls them, and takes focus so the keyboard can too. */}
      <div
        ref={region}
        // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scrollable region must be focusable (axe scrollable-region-focusable)
        tabIndex={0}
        role="region"
        aria-label={c.label}
        className="overflow-x-auto rounded-md focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        <DataTable
          label={c.label}
          columns={columns(costHeader, preview, shares, days.size > 1, { spawned, back })}
          data={ledger.turns}
          getRowId={(r) => r.key}
          className="w-max min-w-full"
        />
      </div>

      {!complete ? (
        <div className="flex flex-wrap items-center gap-3">
          <Button
            variant="outline"
            size="sm"
            className="pointer-coarse:min-h-11"
            onClick={() => {
              loadingOlder.current = true
              onOlder()
            }}
            disabled={olderPending}
          >
            {olderPending ? c.loadingOlder : c.loadOlder}
          </Button>
          {olderError ? (
            <span role="status" className="text-sm text-destructive">
              {c.olderError}
            </span>
          ) : null}
          <span className="text-xs text-muted-foreground">{c.csvSubtotal}</span>
        </div>
      ) : null}

      {ledger.unplaced.length ? (
        <Unplaced steps={ledger.unplaced} links={{ spawned, back }} />
      ) : null}

      {budgetPanel}

      <ControlsNow agentId={agentId} />
    </>
  )
}

function columns(
  costHeader: string,
  preview: boolean,
  shares: Map<string, number> | null,
  dated: boolean,
  links: SpawnLinks,
): DataTableColumn<LedgerTurn>[] {
  const cols: DataTableColumn<LedgerTurn>[] = [
    {
      id: 'turn',
      header: c.turn,
      meta: { rowHeader: true },
      cell: ({ row }) => (
        <span className="tabular-nums">
          {row.original.n}
          {row.original.partial ? (
            <span className="block text-xs text-muted-foreground">{c.turnCut}</span>
          ) : null}
        </span>
      ),
    },
    {
      id: 'time',
      header: c.time,
      cell: ({ row }) => (
        <span className="tabular-nums">
          {dated ? fmtUtcDayTime(row.original.at) : fmtUtcTime(row.original.at)}
        </span>
      ),
    },
    {
      id: 'prompt',
      header: c.prompt,
      meta: { cellClassName: 'min-w-48 max-w-72' },
      cell: ({ row }) =>
        row.original.prompt ? (
          <span className="line-clamp-2 whitespace-normal" title={row.original.prompt}>
            {row.original.prompt}
          </span>
        ) : (
          <span className="text-muted-foreground">—</span>
        ),
    },
    {
      id: 'tools',
      header: c.tools,
      // Badges wrap inside a capped column, so the numbers stay in view on most screens.
      meta: { cellClassName: 'min-w-56 max-w-96 whitespace-normal' },
      cell: ({ row }) => <Tools steps={row.original.tools} links={links} />,
    },
    {
      id: 'tokens',
      header: c.tokens,
      meta: { numeric: true },
      cell: ({ row }) => {
        const r = row.original
        if (r.tokensIn === null && r.tokensOut === null) return copy.notReported
        const cached = (r.cacheRead ?? 0) + (r.cacheWrite ?? 0)
        return (
          <span>
            {fmtTokens(r.tokensIn)} / {fmtTokens(r.tokensOut)}
            {cached ? (
              <span className="block text-xs text-muted-foreground">
                {c.cachedLine(fmtTokens(cached))}
              </span>
            ) : null}
          </span>
        )
      },
    },
    {
      id: 'cost',
      header: costHeader,
      meta: { numeric: true },
      cell: ({ row }) =>
        row.original.cost === null ? copy.notReported : fmtMoney(row.original.cost),
    },
    {
      id: 'running',
      header: c.running,
      meta: { numeric: true },
      cell: ({ row }) =>
        row.original.runningCost === null
          ? '—'
          : fmtMoneyFloor(row.original.runningCost, row.original.runningFloor),
    },
    {
      id: 'context',
      header: c.context,
      cell: ({ row }) =>
        row.original.context
          ? c.contextLine(
              fmtTokens(row.original.context.kept.tokens),
              fmtTokens(row.original.context.pool.tokens),
            )
          : notReported(),
    },
    {
      id: 'memory',
      header: c.memory,
      cell: ({ row }) =>
        row.original.memory
          ? c.memoryLine(fmtTokens(row.original.memory.tokens), row.original.memory.items)
          : notReported(),
    },
  ]
  if (preview)
    cols.push({
      id: 'controls',
      header: c.controls,
      cell: ({ row }) => <Controls turn={row.original} />,
    })
  if (shares)
    cols.push({
      id: 'budget',
      header: c.budgetUse,
      meta: { numeric: true },
      cell: ({ row }) => {
        const v = shares.get(row.original.key)
        return v === undefined ? '—' : pct(v)
      },
    })
  return cols
}

/** A total over some turns reads as a floor with its coverage; over none, "not reported" (review: adversarial). */
function totalText(t: Total, turns: number, fmt: (v: number) => string): string {
  if (t.value === null) return copy.notReported
  return t.reported < turns ? c.totals.partly(fmt(t.value), t.reported, turns) : fmt(t.value)
}

const notReported = () => <span className="text-muted-foreground">{copy.notReported}</span>

interface SpawnLinks {
  spawned: Spawned
  back: Partial<SessionsSearch>
}

/** A ledger step's key is `<message id>:<call id>` (ledger.ts `buildLedger`); message ids are UUIDs, without colons. */
const callIdOf = (key: string) => key.slice(key.indexOf(':') + 1)

function Tools({ steps, links }: { steps: RecordedStep[]; links: SpawnLinks }) {
  if (!steps.length) return <span className="text-muted-foreground">{c.noTools}</span>
  return (
    <ul className="m-0 flex list-none flex-wrap gap-1 p-0">
      {steps.map((s) => {
        const child = links.spawned.get(callIdOf(s.key))
        return (
          <li key={s.key} className="inline-flex flex-wrap items-center gap-1">
            <Badge variant={s.status === 'error' ? 'destructive' : 'outline'}>
              {s.name}
              {s.statusWord ? ` · ${s.statusWord}` : ''}
            </Badge>
            {child ? (
              <Link
                to="/sessions/$sessionId"
                params={{ sessionId: child.sessionId }}
                search={links.back}
                aria-label={obsCopy.subagents.spawnedStepLabel(
                  child.type ?? obsCopy.subagents.untyped,
                )}
                className="text-xs text-primary-text underline-offset-4 hover:underline"
              >
                {obsCopy.subagents.spawnedStep}
              </Link>
            ) : null}
          </li>
        )
      })}
    </ul>
  )
}

function Controls({ turn }: { turn: LedgerTurn }) {
  // Only recorded decisions show (eng "Ledger"): no record, no control, never the current setting.
  if (!turn.controls.length) return <span className="text-muted-foreground">—</span>
  return (
    <ul className="m-0 flex list-none flex-col gap-0.5 p-0 text-xs">
      {turn.controls.map((x) => (
        <li key={x.key}>
          {c.controlLine(x.rule.name, c.outcome[x.outcome] ?? x.outcome, x.tool)}
          {x.tokensSaved ? ` (${c.trimmedSaved(fmtTokens(x.tokensSaved))})` : ''}
        </li>
      ))}
    </ul>
  )
}

function Unplaced({ steps, links }: { steps: RecordedStep[]; links: SpawnLinks }) {
  return (
    <Panel title={c.turnNotRecorded} subtitle={c.unplacedNote(steps.length)}>
      <Tools steps={steps} links={links} />
    </Panel>
  )
}

function ControlsNow({ agentId }: { agentId: string | null }) {
  return (
    <Panel title={c.controlsNow.title} subtitle={c.controlsNow.hint}>
      <ul className="m-0 flex list-none flex-wrap gap-x-6 gap-y-1 p-0">
        {agentId ? (
          <>
            <li>
              <Link
                to="/agents/$agentId"
                params={{ agentId }}
                search={{ tab: 'mcp' }}
                className={LINK}
              >
                {c.controlsNow.tools}
              </Link>
            </li>
            <li>
              <Link to="/agents/$agentId" params={{ agentId }} search={{}} className={LINK}>
                {c.controlsNow.optimization}
              </Link>
            </li>
          </>
        ) : null}
        <li>
          <Link to="/router" hash="router-budgets" className={LINK}>
            {c.controlsNow.budgets}
          </Link>
        </li>
      </ul>
    </Panel>
  )
}

function NotOwner({ session }: { session: ReturnType<typeof useSessionDetail> }) {
  return (
    <Panel title={c.title}>
      <div className="flex flex-col gap-1 text-sm">
        <p className="m-0">{c.notOwner}</p>
        {session.data ? (
          <p className="m-0 text-muted-foreground tabular-nums">
            {c.notOwnerTraces(
              session.data.num_traces ?? session.data.traces.length,
              fmtMoney(session.data.cost_summary.total.cost),
            )}
          </p>
        ) : session.isError ? (
          <p className="m-0 text-muted-foreground">{c.notOwnerNoTrace}</p>
        ) : null}
      </div>
    </Panel>
  )
}
