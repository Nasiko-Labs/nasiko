/**
 * Your agents (plan §7.2): the caller's own agents (`owner=<Me.sub>`), attention first.
 * Order: header → pinned Needs attention → status tabs → table. Polls every 5 s only while a
 * row is Deploying or a restart is being checked. Harnesses have their own tab and are not
 * counted in All.
 */
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { Bot, MoreHorizontal } from 'lucide-react'
import { useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
// Owner picker hidden for now (header actions below).
// import { Field, FieldLabel } from '@/components/ui/field'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
// import {
//   Select,
//   SelectContent,
//   SelectItem,
//   SelectTrigger,
//   SelectValue,
// } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { EmptyState } from '@/components/shared/state-card'
import { DeployAgentButton } from '@/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@/features/deploy/copy'
import { ErrorState } from '@/features/observability/StateCard'
import { LogDrawer } from '@/features/sessions/LogDrawer'
import { fmtInt, fmtMoney } from '@/lib/format'
import { meQuery } from '@/lib/api/auth'
import { cn } from '@/lib/utils'
import {
  isUnavailable,
  useAnyWatching,
  useDeployment,
  useOwnedAgents,
  useUsage24h,
  // useUsers,
  useWatchesStep,
} from './api'
import { AgentLinkTo, AgentsNav, FirstRunSteps, StatusBadge } from './components/bits'
import { CopyMenuItem } from '@/components/shared/copy-button'
import { LifecycleButtons, StopDialog } from './components/lifecycle'
import { relTime } from './format'
import { ACTION_LABEL, outcomeText, useLifecycleFlow } from './lifecycleFlow'
import { copy } from './copy'
import { MINE_TABS, type DeletedNote, type MineSearch } from './search'
import {
  actionsFor,
  displayStatus,
  isHarness,
  STATUS,
  TAB_STATUSES,
  type DisplayStatus,
} from './status'
import type { Agent } from './types'

type Tab = (typeof MINE_TABS)[number]

/** Columns shown from 640 px; below it the row's summary line carries them. */
const WIDE = 'hidden sm:table-cell'

const tabLabel = (t: Tab) =>
  t === 'all'
    ? copy.tabAll
    : t === 'harnesses'
      ? copy.tabHarnesses
      : STATUS[t as DisplayStatus].label

export function MyAgentsPage({
  search,
  setSearch,
}: {
  search: MineSearch
  setSearch: (patch: Partial<MineSearch>) => void
}) {
  const me = useQuery(meQuery).data
  const superuser = !!me?.is_superuser
  const owner = (superuser && search.owner) || me?.sub
  // Polling follows the watch store, and the page (not each row) steps them:
  // a row that leaves the visible tab or unmounts is still checked until its watch ends.
  const [ids, setIds] = useState<readonly string[]>([])
  const watching = useAnyWatching(ids)
  const q = useOwnedAgents(owner, watching)
  const usage = useUsage24h(!!owner)
  // const users = useUsers(superuser)
  const [logsFor, setLogsFor] = useState<Agent | null>(null)

  const rows = useMemo(
    () =>
      (q.data ?? []).map((a) => ({ a, id: a.id, display: displayStatus(a.status, isHarness(a)) })),
    [q.data],
  )
  const rowIds = rows.map((r) => r.id).join(',')
  if (rowIds !== ids.join(',')) setIds(rowIds ? rowIds.split(',') : [])
  useWatchesStep(rows, q.dataUpdatedAt, q.errorUpdatedAt)
  const agents = rows.filter((r) => r.display !== 'harness')
  const harnesses = rows.filter((r) => r.display === 'harness')
  const attention = agents.filter((r) => r.display === 'attention')
  const tab: Tab = search.tab ?? 'all'
  const shown =
    tab === 'harnesses'
      ? harnesses
      : tab === 'all'
        ? agents
        : agents.filter((r) => r.display === tab)
  const count = (t: Tab) =>
    t === 'all'
      ? agents.length
      : t === 'harnesses'
        ? harnesses.length
        : agents.filter((r) => r.display === t).length

  // const ownerId = useId()
  const header = (
    <>
      <PageHeader
        title={
          <>
            {copy.mineTitle}
            {q.data ? (
              <span className="ml-2 text-base font-normal text-muted-foreground">
                {agents.length}
              </span>
            ) : null}
          </>
        }
        description={
          <>
            {copy.deployHint}{' '}
            <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em]">
              nasiko deploy
            </code>
          </>
        }
        actions={
          <>
            <DeployAgentButton />
            {/* Owner picker and Agents link hidden for now.
            {superuser && users.data ? (
              <Field orientation="horizontal" className="w-auto gap-2">
                <FieldLabel htmlFor={ownerId} className="font-normal text-muted-foreground">
                  {copy.owner}
                </FieldLabel>
                <Select
                  value={owner ?? ''}
                  onValueChange={(v) => setSearch({ owner: v === me?.sub ? undefined : v })}
                >
                  <SelectTrigger id={ownerId} size="sm">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {[...users.data.entries()].map(([id, name]) => (
                      <SelectItem key={id} value={id}>
                        {id === me?.sub ? `${name} (${copy.you})` : name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Field>
            ) : null}
            <Link
              to="/agents"
              search={{}}
              className="ml-1 text-sm text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
            >
              {copy.catalogTitle} →
            </Link>
            */}
          </>
        }
      />
      <AgentsNav current="/agents/mine" />
    </>
  )

  const deleted = useRouterState({ select: (s) => s.location.state.agentDeleted })
  const navigate = useNavigate()
  const deletedNote = deleted ? (
    <DeletedNotice
      note={deleted}
      onClose={() =>
        void navigate({
          to: '.',
          search: (p) => p,
          state: (p) => ({ ...p, agentDeleted: undefined }),
          replace: true,
        })
      }
    />
  ) : null

  if (q.isPending)
    return (
      <div className="space-y-4">
        {header}
        {deletedNote}
        <PageLoader label={copy.loadingAgents} />
      </div>
    )
  if (q.isError && !q.data)
    return (
      <div className="space-y-4">
        {header}
        {deletedNote}
        <ErrorState error={q.error} onRetry={() => void q.refetch()} />
      </div>
    )

  return (
    <div className="space-y-4">
      {header}
      {q.isError ? <p className="text-xs text-muted-foreground">{copy.couldntRefresh}</p> : null}
      {deletedNote}
      {rows.length === 0 ? (
        <>
          <EmptyState icon={Bot} title={copy.noAgentsMine} action={<DeployAgentButton />}>
            {copy.noAgentsMineHint}
          </EmptyState>
          <div>
            <p className="mb-2 text-xs text-muted-foreground">{deployCopy.entry.orCli}</p>
            <FirstRunSteps />
          </div>
        </>
      ) : (
        <>
          {attention.length ? (
            <section
              aria-label={copy.needsAttention}
              className="rounded-lg border border-warning/40 bg-warning/5"
            >
              <h2 className="px-4 pt-3 text-sm font-semibold">
                {copy.needsAttention}{' '}
                <span className="text-muted-foreground">{attention.length}</span>
              </h2>
              <ul className="divide-y divide-border">
                {attention.map(({ a }) => (
                  <AttentionRow key={a.id} agent={a} onLogs={() => setLogsFor(a)} />
                ))}
              </ul>
            </section>
          ) : null}
          {/* Radix tabs: arrow keys, Home/End and roving focus. The table below is the one panel. */}
          <Tabs
            value={tab}
            onValueChange={(t) => setSearch({ tab: t === 'all' ? undefined : (t as Tab) })}
          >
            <TabsList
              variant="line"
              aria-label={copy.colStatus}
              className="w-full max-w-full justify-start overflow-x-auto border-b border-border"
            >
              {(['all', ...TAB_STATUSES, 'harnesses'] as Tab[]).map((t) => (
                <TabsTrigger key={t} value={t} className="min-h-10 flex-none px-3">
                  {tabLabel(t)}
                  <span className="rounded-full bg-muted px-1.5 text-xs">{count(t)}</span>
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          <div className="flex flex-wrap gap-x-4 text-xs text-muted-foreground">
            {tab === 'all' && harnesses.length ? (
              <p>{copy.excludesHarnesses(harnesses.length)}</p>
            ) : null}
            {usage.isError ? <p>{copy.usageUnavailable}</p> : null}
          </div>
          {shown.length ? (
            // shadcn Table, not DataTable: each row owns its lifecycle flow (hooks + Stop dialog), which
            // DataTable's per-cell rendering can't share across a row's cells.
            <div className="rounded-lg border border-border">
              <Table aria-label={copy.mineTitle}>
                <TableHeader>
                  <TableRow className="text-xs hover:bg-transparent">
                    <TableHead scope="col" className="px-4 text-muted-foreground">
                      {copy.colName}
                    </TableHead>
                    <TableHead scope="col" className="text-muted-foreground">
                      {copy.colStatus}
                    </TableHead>
                    <TableHead scope="col" className={cn(WIDE, 'text-muted-foreground')}>
                      {copy.colVersion}
                    </TableHead>
                    <TableHead scope="col" className={cn(WIDE, 'text-muted-foreground')}>
                      {copy.colUpdated}
                    </TableHead>
                    <TableHead
                      scope="col"
                      className={cn(WIDE, 'text-right text-muted-foreground')}
                      title={copy.turnsTip}
                    >
                      {copy.colTurns}
                    </TableHead>
                    <TableHead scope="col" className={cn(WIDE, 'text-right text-muted-foreground')}>
                      {copy.colCost}
                    </TableHead>
                    <TableHead scope="col" className="w-12 pr-4">
                      <span className="sr-only">{copy.actions}</span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {shown.map(({ a, display }) => (
                    <AgentRow
                      key={a.id}
                      agent={a}
                      display={display}
                      usage={usage.data?.get(a.id)}
                      usageFailed={usage.isError}
                      onLogs={() => setLogsFor(a)}
                    />
                  ))}
                </TableBody>
              </Table>
            </div>
          ) : (
            <EmptyState
              icon={Bot}
              title={copy.noAgentsInTab(tabLabel(tab))}
              action={
                <Button size="sm" variant="outline" onClick={() => setSearch({ tab: undefined })}>
                  {copy.showAllAgents}
                </Button>
              }
            >
              {copy.noAgentsInTabHint}
            </EmptyState>
          )}
        </>
      )}
      {logsFor ? (
        <LogDrawer
          agent={logsFor.id}
          label={logsFor.display_name || logsFor.name}
          open
          onOpenChange={(o) => {
            if (!o) setLogsFor(null)
          }}
        />
      ) : null}
    </div>
  )
}

function DeletedNotice({ note, onClose }: { note: DeletedNote; onClose: () => void }) {
  const { name, stopped } = note
  const list = note.errors.filter(Boolean)
  return (
    <div
      role="status"
      className={cn(
        'flex items-start justify-between gap-3 rounded-lg border p-3 text-sm',
        list.length ? 'border-warning/50 bg-warning/5' : 'border-success/40 bg-success/5',
      )}
    >
      <div>
        <p>{list.length ? copy.deletedWithErrors(name, stopped) : copy.deletedNotice(name)}</p>
        {list.length ? (
          <ul className="mt-1 list-disc pl-5 text-muted-foreground">
            {list.map((e, i) => (
              // eslint-disable-next-line @eslint-react/no-array-index-key -- messages can repeat; the list is rebuilt whole, never reordered
              <li key={`${i}:${e}`}>{e}</li>
            ))}
          </ul>
        ) : null}
      </div>
      <Button size="sm" variant="ghost" onClick={onClose}>
        {copy.dismiss}
      </Button>
    </div>
  )
}

function AttentionRow({ agent, onLogs }: { agent: Agent; onLogs: () => void }) {
  const flow = useLifecycleFlow(agent.id)
  return (
    <li className="flex flex-wrap items-center justify-between gap-3 px-4 py-2.5">
      <div className="min-w-0">
        <AgentLinkTo id={agent.id} className="font-medium">
          {agent.display_name || agent.name}
        </AgentLinkTo>
        <div className="font-mono text-xs text-muted-foreground">{agent.name}</div>
      </div>
      <div className="flex items-center gap-2">
        <CrashReason agent={agent} />
        <Button size="sm" variant="outline" onClick={onLogs}>
          {copy.viewLogs}
        </Button>
        <LifecycleButtons flow={flow} display="attention" name={agent.display_name || agent.name} />
      </div>
    </li>
  )
}

/** Crash reason on click/tap (not hover-only); `/deployment` is fetched only when opened. */
function CrashReason({ agent }: { agent: Agent }) {
  const [open, setOpen] = useState(false)
  const dep = useDeployment(agent.id, open)
  const d = dep.data
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="h-auto rounded-md p-0 hover:bg-transparent"
          aria-label={copy.whyAttention(agent.display_name || agent.name)}
        >
          <StatusBadge display="attention" raw={agent.status} className="cursor-pointer" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-80 text-sm">
        {dep.isPending ? (
          <Skeleton className="h-10" />
        ) : dep.isError ? (
          <p>{copy.somethingWrong}</p>
        ) : isUnavailable(d) ? (
          <p>{copy.deploymentUnavailable}</p>
        ) : d?.crash_reason ? (
          <div className="space-y-1">
            <p className="font-medium">{d.crash_reason}</p>
            {d.crashed_at ? (
              <p className="text-xs text-muted-foreground">{relTime(d.crashed_at)}</p>
            ) : null}
          </div>
        ) : (
          <p className="text-muted-foreground">
            {STATUS.attention.hint} {copy.crashNoReason}
          </p>
        )}
      </PopoverContent>
    </Popover>
  )
}

function AgentRow({
  agent,
  display,
  usage,
  usageFailed,
  onLogs,
}: {
  agent: Agent
  display: DisplayStatus
  usage?: { turns: number; cost: number }
  usageFailed: boolean
  onLogs: () => void
}) {
  const flow = useLifecycleFlow(agent.id)
  const name = agent.display_name || agent.name
  const note = outcomeText(flow)
  const actions = actionsFor(display)
  const turns = usageFailed ? '—' : usage ? fmtInt(usage.turns) : '—'
  const cost = usageFailed ? '—' : usage ? fmtMoney(usage.cost) : '—'
  return (
    <TableRow>
      <TableCell className="max-w-0 min-w-40 px-4 py-2.5 whitespace-normal sm:w-2/5">
        <AgentLinkTo id={agent.id} className="font-medium">
          {name}
        </AgentLinkTo>
        <div className="truncate font-mono text-xs text-muted-foreground">{agent.name}</div>
        {note ? (
          <div
            role="status"
            className={cn(
              'text-xs',
              note.tone === 'warning' ? 'text-warning' : 'text-muted-foreground',
            )}
          >
            {note.text}
          </div>
        ) : null}
        {flow.m.isError && !flow.stopOpen ? (
          <div className="text-xs text-destructive">{copy.somethingWrong}</div>
        ) : null}
        {/* Below 640 px the four middle columns fold into one summary line. */}
        <div className="flex flex-wrap gap-x-3 text-xs text-muted-foreground sm:hidden">
          <span className="font-mono">{agent.version}</span>
          <span>{relTime(agent.updated_at)}</span>
          <span className="tabular-nums">
            {turns} {copy.turnsSuffix(usageFailed ? undefined : usage?.turns)}
          </span>
          <span className="tabular-nums">{cost}</span>
        </div>
      </TableCell>
      <TableCell>
        {display === 'attention' ? (
          <CrashReason agent={agent} />
        ) : (
          <StatusBadge display={display} raw={agent.status} />
        )}
      </TableCell>
      <TableCell className={WIDE}>{agent.version}</TableCell>
      <TableCell className={WIDE}>{relTime(agent.updated_at)}</TableCell>
      <TableCell className={cn(WIDE, 'text-right tabular-nums')}>{turns}</TableCell>
      <TableCell className={cn(WIDE, 'text-right tabular-nums')}>{cost}</TableCell>
      <TableCell className="pr-4 text-right">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              className="size-9 p-0 pointer-coarse:size-11"
              aria-label={copy.actionsFor(name)}
            >
              <MoreHorizontal className="size-4" aria-hidden />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-64">
            {actions.map((a) => (
              <DropdownMenuItem
                key={a}
                disabled={flow.m.isPending || flow.w.watching}
                onSelect={() => flow.run(a)}
              >
                {ACTION_LABEL[a]}
              </DropdownMenuItem>
            ))}
            {actions.length ? <DropdownMenuSeparator /> : null}
            {display !== 'harness' ? (
              <DropdownMenuItem onSelect={onLogs}>{copy.viewLogs}</DropdownMenuItem>
            ) : null}
            <DropdownMenuItem asChild>
              <Link to="/agents/$agentId" params={{ agentId: agent.id }} search={{}}>
                {copy.openAgent}
              </Link>
            </DropdownMenuItem>
            {display !== 'harness' ? (
              <>
                <DropdownMenuSeparator />
                <CopyMenuItem text={`nasiko logs ${agent.id}`} />
              </>
            ) : null}
          </DropdownMenuContent>
        </DropdownMenu>
        <StopDialog flow={flow} name={name} />
      </TableCell>
    </TableRow>
  )
}
