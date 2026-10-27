/**
 * Agent detail (plan §7.3). URLs carry the UUID; a name resolves through the directory (one
 * match redirects, several show a chooser, none is the not-found state), since the server's
 * name lookups aren't owner-scoped. Status comes from the reconciled list row
 * (`useAgentStatus`), not the detail. Manage-only tabs are hidden; a bad `tab` falls back.
 */
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { MoreHorizontal } from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@nasiko/ui/primitives/dropdown-menu'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PrevNext } from '@nasiko/ui/composites/prev-next'
import { ErrorState, StateCard } from '@nasiko/features/observability/StateCard'
import { meQuery } from '@nasiko/data-access/auth'
import { ApiError } from '@nasiko/data-access/client'
import { useSiblings } from '@nasiko/app-core/siblings'
import {
  useAgentDetail,
  useAgentsDirectory,
  useAgentStatus,
  useErrorLogs,
  useGrants,
  useUsers,
  useWatch,
  useWatchStep,
} from './api'
import { AgentMark, StatusBadge, TryItLink } from './components/bits'
import { CopyButton, CopyMenuItem } from '@nasiko/ui/composites/copy-button'
import { DeleteAgentDialog } from './components/dialogs'
import { LifecycleButtons } from './components/lifecycle'
import { copy } from './copy'
import { relTime } from './format'
import { ACTION_LABEL, useLifecycleFlow } from './lifecycleFlow'
import { ActivityTab } from './detail/ActivityTab'
import { AccessTab } from './detail/AccessTab'
import { DetailsTab } from './detail/DetailsTab'
import { OverviewTab } from './detail/OverviewTab'
import { SettingsTab } from './detail/SettingsTab'
import { VersionsTab } from './detail/VersionsTab'
import { AgentBuildsTab } from '@nasiko/features/deploy/AgentBuildsTab'
import { AgentMcpTab } from '@nasiko/features/mcp/components/AgentMcpTab'
import { isUuid, type AgentView } from './normalize'
import { DETAIL_TABS, type DetailTab } from './search'
import { actionsFor, displayStatus, isHarness } from './status'

export function AgentDetailPage({ agentRef, tab }: { agentRef: string; tab?: string }) {
  if (!isUuid(agentRef)) return <ResolveByName name={agentRef} tab={tab} />
  // Watches and caches are keyed by the server's (lowercase) id; a pasted mixed-case UUID redirects.
  const id = agentRef.toLowerCase()
  return id === agentRef ? <AgentDetail id={id} tab={tab} /> : <Redirect id={id} tab={tab} />
}

function Redirect({ id, tab }: { id: string; tab?: string }) {
  const navigate = useNavigate()
  useEffect(() => {
    void navigate({
      to: '/agents/$agentId',
      params: { agentId: id },
      search: tab ? { tab } : {},
      state: (s) => ({ siblings: s.siblings }),
      replace: true,
    })
  }, [id, tab, navigate])
  return <PageLoader label={copy.loadingAgent} />
}

/** `id`: the page's own agent (a name lookup has none), for the arrows. */
function NotFound({ id }: { id?: string }) {
  const card = (
    <StateCard
      title={copy.notFound}
      fix={copy.notFoundFix}
      action={
        <Link
          to="/agents"
          search={{}}
          className="text-sm text-primary-text underline-offset-4 hover:underline"
        >
          {copy.catalogTitle}
        </Link>
      }
    />
  )
  return id ? <WithSiblings id={id}>{card}</WithSiblings> : card
}

/** A state shown in place of an agent stepped to from a list keeps the arrows, so the user can step on. */
function WithSiblings({ id, children }: { id: string; children: ReactNode }) {
  return (
    <>
      <TopBar stepper={<AgentSiblings id={id} />} />
      {children}
    </>
  )
}

function ResolveByName({ name, tab }: { name: string; tab?: string }) {
  const dir = useAgentsDirectory()
  const navigate = useNavigate()
  const matches = dir.byNameAll.get(name) ?? []
  const only = matches.length === 1 ? (matches[0]?.id ?? null) : null
  // The directory is cached for minutes (shared with Sessions): a miss refetches once before "not found",
  // so a link to a just-deployed agent still resolves.
  const retried = useRef(false)
  const miss = dir.isSuccess && !matches.length
  const { refetch } = dir
  useEffect(() => {
    if (miss && !retried.current) {
      retried.current = true
      void refetch()
    }
  }, [miss, refetch])
  useEffect(() => {
    if (only)
      void navigate({
        to: '/agents/$agentId',
        params: { agentId: only },
        search: tab ? { tab } : {},
        state: (s) => ({ siblings: s.siblings }),
        replace: true,
      })
  }, [only, tab, navigate])
  if (dir.isPending || only || (miss && dir.isFetching))
    return <PageLoader label={copy.loadingAgent} />
  if (dir.isError) return <ErrorState error={dir.error} onRetry={() => void dir.refetch()} />
  if (!matches.length) return <NotFound />
  return (
    <StateCard title={copy.multipleNamed(name)} fix={copy.multipleNamedFix}>
      <ul className="mt-2 space-y-1">
        {matches.map((a) => (
          <li key={a.id}>
            <Link
              to="/agents/$agentId"
              params={{ agentId: a.id }}
              search={{}}
              className="underline-offset-4 hover:underline"
            >
              {a.display_name || a.name}
            </Link>
            <span className="ml-2 font-mono text-xs text-muted-foreground">{a.id.slice(0, 8)}</span>
            <StatusBadge
              display={displayStatus(a.status, isHarness(a))}
              raw={a.status}
              className="ml-2"
            />
          </li>
        ))}
      </ul>
    </StateCard>
  )
}

function tabsFor(agent: AgentView): DetailTab[] {
  // A harness is an ordinary agent id to the MCP gateway, so its manager gets the MCP tab too.
  if (agent.isHarness)
    return agent.canManage ? ['overview', 'mcp', 'details'] : ['overview', 'details']
  return agent.canManage ? [...DETAIL_TABS] : ['overview', 'activity', 'versions', 'details']
}

function AgentDetail({ id, tab }: { id: string; tab?: string }) {
  const w = useWatch(id)
  const [offPage, setOffPage] = useState(false)
  // Off the owner's first list page, the detail is the only status source: poll it while watching.
  const detail = useAgentDetail(id, true, w.watching && offPage)
  const navigate = useNavigate()
  const me = useQuery(meQuery).data
  const agent = detail.data
  const status = useAgentStatus(agent, w.watching)
  const statusOffPage = !!agent && !agent.isHarness && status.updatedAt > 0 && !status.row
  if (statusOffPage !== offPage) setOffPage(statusOffPage)
  useWatchStep(
    id,
    status.display,
    offPage ? detail.dataUpdatedAt : status.updatedAt,
    offPage ? detail.errorUpdatedAt : status.errorAt,
  )
  const users = useUsers(!!me?.is_superuser)
  const grants = useGrants(id, !!agent?.canManage && !agent.isHarness)
  const errors = useErrorLogs(id, !!agent && !agent.isHarness)
  const [tailOpen, setTailOpen] = useState(false)

  const allowed = agent ? tabsFor(agent) : []
  const current: DetailTab = allowed.includes(tab as DetailTab) ? (tab as DetailTab) : 'overview'
  const badTab = !!agent && !!tab && current !== tab
  // An unknown or hidden tab falls back to Overview without a history entry.
  useEffect(() => {
    if (badTab)
      void navigate({
        to: '.',
        search: (prev) => ({ ...prev, tab: undefined }),
        state: (s) => ({ siblings: s.siblings }),
        replace: true,
      })
  }, [badTab, navigate])

  if (detail.isPending) return <PageLoader label={copy.loadingAgent} />
  if (detail.isError) {
    return detail.error instanceof ApiError &&
      (detail.error.status === 404 || detail.error.status === 403) ? (
      <NotFound id={id} />
    ) : (
      <WithSiblings id={id}>
        <ErrorState error={detail.error} onRetry={() => void detail.refetch()} />
      </WithSiblings>
    )
  }
  if (!agent || !status.display) return null

  const display = w.watch?.kind === 'rollback' && !w.watch.outcome ? 'deploying' : status.display
  const mine = agent.ownerId === me?.sub
  const ownerLabel = mine ? copy.you : (users.data?.get(agent.ownerId) ?? agent.ownerId.slice(0, 8))
  // The hub's window keys follow the tab switch, so coming back to Overview keeps the window.
  const setTab = (t: string) =>
    void navigate({
      to: '.',
      search: (prev) => ({ ...prev, tab: t === 'overview' ? undefined : t }),
      // Keep the list Previous/Next walks.
      state: (s) => ({ siblings: s.siblings }),
      replace: false,
    })
  const hasErrors = !!errors.data?.length
  const labels: Record<DetailTab, string> = {
    overview: copy.tabOverview,
    activity: copy.tabActivity,
    versions: copy.tabVersions,
    builds: copy.tabBuilds,
    mcp: copy.tabMcp,
    access: copy.tabAccess,
    settings: copy.tabSettings,
    details: copy.tabDetails,
  }

  return (
    <div className="space-y-4">
      <Header
        stepper={<AgentSiblings id={id} />}
        agent={agent}
        display={display}
        raw={status.raw}
        ownerLabel={ownerLabel}
        mine={mine}
        isPublic={grants.data?.is_public}
      />
      {status.failed ? (
        <p className="text-xs text-muted-foreground">{copy.couldntRefresh}</p>
      ) : null}
      <Tabs value={current} onValueChange={setTab}>
        <TabsList data-tour="agent-tabs" className="max-w-full justify-start overflow-x-auto">
          {allowed.map((t) => (
            <TabsTrigger key={t} value={t}>
              {labels[t]}
              {t === 'activity' && hasErrors ? (
                <span
                  className="ml-1 size-1.5 rounded-full bg-destructive"
                  role="img"
                  aria-label={copy.errorsDot}
                  title={copy.errorsDot}
                />
              ) : null}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent value="overview" className="pt-3">
          <OverviewTab
            agent={agent}
            display={display}
            raw={status.raw}
            ownerLabel={ownerLabel}
            onViewLogs={() => {
              setTab('activity')
              setTailOpen(true)
            }}
          />
          {agent.isHarness ? (
            <p className="pt-3 text-sm">
              <Link
                to="/harnesses"
                search={{}}
                className="text-primary-text underline-offset-4 hover:underline"
              >
                {copy.harnessUsage} →
              </Link>
            </p>
          ) : null}
        </TabsContent>
        {allowed.includes('activity') ? (
          <TabsContent value="activity" className="pt-3">
            <ActivityTab agent={agent} tailOpen={tailOpen} onTailChange={setTailOpen} />
          </TabsContent>
        ) : null}
        {allowed.includes('versions') ? (
          <TabsContent value="versions" className="pt-3">
            <VersionsTab agent={agent} />
          </TabsContent>
        ) : null}
        {/* The owner's or a superuser's view only: the server answers anyone else {available:false} (plans/feat-deploy.md §6). */}
        {allowed.includes('builds') ? (
          <TabsContent value="builds" className="pt-3">
            <AgentBuildsTab agentId={agent.id} agentName={agent.name} />
          </TabsContent>
        ) : null}
        {allowed.includes('mcp') ? (
          <TabsContent value="mcp" className="pt-3">
            <AgentMcpTab agentId={agent.id} harness={agent.isHarness} />
          </TabsContent>
        ) : null}
        {allowed.includes('access') ? (
          <TabsContent value="access" className="pt-3">
            <AccessTab agent={agent} />
          </TabsContent>
        ) : null}
        {allowed.includes('details') ? (
          <TabsContent value="details" className="pt-3">
            <DetailsTab agent={agent} display={display} ownerLabel={ownerLabel} />
          </TabsContent>
        ) : null}
        {allowed.includes('settings') ? (
          <TabsContent value="settings" className="pt-3">
            <SettingsTab agent={agent} />
          </TabsContent>
        ) : null}
      </Tabs>
    </div>
  )
}

function Header({
  agent,
  display,
  raw,
  ownerLabel,
  mine,
  isPublic,
  stepper,
}: {
  /** Rightmost in the top bar: the arrows keep one place for every agent (and one that fails to load). */
  stepper: ReactNode
  agent: AgentView
  display: ReturnType<typeof displayStatus>
  raw?: string
  ownerLabel: string
  mine: boolean
  isPublic?: boolean
}) {
  const flow = useLifecycleFlow(agent.id)
  const [delOpen, setDelOpen] = useState(false)
  const link = `${globalThis.location?.origin ?? ''}/agents/${agent.id}`
  // Below 640 px only the primary action is a button; the others move into this menu (§7.3).
  const secondary = agent.canManage ? actionsFor(display).slice(1) : []
  const busy = flow.m.isPending || flow.w.watching
  return (
    <>
      <div data-tour="agent-header" className="flex items-start gap-3">
        <AgentMark name={agent.displayName} iconUrl={agent.iconUrl} size={40} />
        <PageHeader
          className="min-w-0 flex-1"
          title={
            <span className="flex flex-wrap items-center gap-2">
              <span className="truncate">{agent.displayName}</span>
              <StatusBadge display={display} raw={raw} />
            </span>
          }
          description={
            <span className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
              <span className="font-mono">{agent.name}</span>
              <span className="inline-flex items-center font-mono" title={agent.id}>
                {agent.id.slice(0, 8)}…<CopyButton text={agent.id} label={copy.copyUuid} />
              </span>
              {agent.version ? <span className="font-mono">v{agent.version}</span> : null}
              <span>{copy.ownerLine(ownerLabel)}</span>
              <span>{copy.updatedLine(relTime(agent.updatedAt))}</span>
              {agent.isHarness ? (
                <Badge variant="outline">{copy.harnessChip}</Badge>
              ) : (
                <Badge variant="outline">{mine ? copy.yours : copy.availableToYou}</Badge>
              )}
              {isPublic ? <Badge variant="outline">{copy.publicChip}</Badge> : null}
            </span>
          }
        />
      </div>
      <TopBar
        trail={[agent.displayName]}
        stepper={stepper}
        actions={
          <>
            <TryItLink
              id={agent.id}
              name={agent.displayName}
              display={display}
              harness={agent.isHarness}
              variant="default"
            />
            {agent.canManage ? (
              <LifecycleButtons flow={flow} display={display} name={agent.displayName} collapse />
            ) : null}
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    variant="outline"
                    size="sm"
                    className="size-8 p-0 pointer-coarse:size-11"
                    aria-label={copy.moreActionsFor(agent.displayName)}
                  />
                }
              >
                <MoreHorizontal className="size-4" aria-hidden />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-72">
                {secondary.map((a) => (
                  <DropdownMenuItem
                    key={a}
                    className="sm:hidden"
                    disabled={busy}
                    onClick={() => flow.run(a)}
                  >
                    {ACTION_LABEL[a]}
                  </DropdownMenuItem>
                ))}
                {secondary.length ? <DropdownMenuSeparator className="sm:hidden" /> : null}
                <DropdownMenuLabel>{copy.copyCli}</DropdownMenuLabel>
                {agent.isHarness ? null : <CopyMenuItem text={`nasiko logs ${agent.id}`} />}
                {agent.canManage && !agent.isHarness ? (
                  <CopyMenuItem text={`nasiko restart ${agent.id}`} />
                ) : null}
                <CopyMenuItem text={agent.id} label={copy.copyUuid} />
                <CopyMenuItem text={link} label={copy.copyLink} />
                <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">
                  {copy.cliNote}
                </DropdownMenuLabel>
                {agent.canManage ? (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem className="text-destructive" onClick={() => setDelOpen(true)}>
                      {copy.deleteAgent}
                    </DropdownMenuItem>
                  </>
                ) : null}
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />
      <DeleteAgentDialog agent={agent} open={delOpen} onOpenChange={setDelOpen} />
    </>
  )
}

/** Previous/Next through the list this agent was opened from (lib/siblings.ts); nothing on a deep link. */
function AgentSiblings({ id, className }: { id: string; className?: string }) {
  const navigate = useNavigate()
  const around = useSiblings(id)
  if (!around) return null
  // Replace: Back still returns to the list. The tab carries over; one the next agent lacks falls back to Overview.
  const go = (agentId: string | undefined) => {
    if (agentId)
      void navigate({
        to: '/agents/$agentId',
        params: { agentId },
        search: true,
        state: { siblings: around.siblings },
        replace: true,
      })
  }
  return (
    <PrevNext
      noun="agent"
      index={around.index}
      count={around.count}
      onPrev={() => go(around.prev)}
      onNext={() => go(around.next)}
      className={className}
    />
  )
}
