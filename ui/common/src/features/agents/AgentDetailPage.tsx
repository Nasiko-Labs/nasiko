/**
 * Agent detail (plan §7.3). URLs carry the UUID; a name resolves through the directory (one
 * match redirects, several show a chooser, none is the not-found state), since the server's
 * name lookups aren't owner-scoped. Status comes from the reconciled list row
 * (`useAgentStatus`), not the detail. Manage-only tabs are hidden; a bad `tab` falls back.
 */
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { MoreHorizontal } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { ErrorState, StateCard } from '@/features/observability/StateCard'
import { meQuery } from '@/lib/api/auth'
import { ApiError } from '@/lib/api/client'
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
import { CopyButton, CopyMenuItem } from '@/components/shared/copy-button'
import { DeleteAgentDialog } from './components/dialogs'
import { LifecycleButtons } from './components/lifecycle'
import { copy } from './copy'
import { relTime } from './format'
import { ACTION_LABEL, useLifecycleFlow } from './lifecycleFlow'
import { ActivityTab } from './detail/ActivityTab'
import { AccessTab } from './detail/AccessTab'
import { OverviewTab } from './detail/OverviewTab'
import { SettingsTab } from './detail/SettingsTab'
import { VersionsTab } from './detail/VersionsTab'
import { AgentBuildsTab } from '@/features/deploy/AgentBuildsTab'
import { AgentMcpTab } from '@/features/mcp/components/AgentMcpTab'
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
      replace: true,
    })
  }, [id, tab, navigate])
  return <PageLoader label={copy.loadingAgent} />
}

function NotFound() {
  return (
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
  if (agent.isHarness) return ['overview']
  return agent.canManage ? [...DETAIL_TABS] : ['overview', 'activity', 'versions']
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
    if (badTab) void navigate({ to: '.', search: {}, replace: true })
  }, [badTab, navigate])

  if (detail.isPending) return <PageLoader label={copy.loadingAgent} />
  if (detail.isError) {
    return detail.error instanceof ApiError &&
      (detail.error.status === 404 || detail.error.status === 403) ? (
      <NotFound />
    ) : (
      <ErrorState error={detail.error} onRetry={() => void detail.refetch()} />
    )
  }
  if (!agent || !status.display) return null

  const display = w.watch?.kind === 'rollback' && !w.watch.outcome ? 'deploying' : status.display
  const mine = agent.ownerId === me?.sub
  const ownerLabel = mine ? copy.you : (users.data?.get(agent.ownerId) ?? agent.ownerId.slice(0, 8))
  const setTab = (t: string) =>
    void navigate({ to: '.', search: t === 'overview' ? {} : { tab: t }, replace: false })
  const hasErrors = !!errors.data?.length
  const labels: Record<DetailTab, string> = {
    overview: copy.tabOverview,
    activity: copy.tabActivity,
    versions: copy.tabVersions,
    builds: copy.tabBuilds,
    mcp: copy.tabMcp,
    access: copy.tabAccess,
    settings: copy.tabSettings,
  }

  return (
    <div className="space-y-4">
      <Header
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
        <TabsList className="max-w-full justify-start overflow-x-auto">
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
            <AgentMcpTab agentId={agent.id} />
          </TabsContent>
        ) : null}
        {allowed.includes('access') ? (
          <TabsContent value="access" className="pt-3">
            <AccessTab agent={agent} />
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
}: {
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
    <div className="flex items-start gap-3">
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
        actions={
          <div className="flex items-start gap-2">
            <TryItLink
              id={agent.id}
              name={agent.displayName}
              display={display}
              harness={agent.isHarness}
            />
            {agent.canManage ? (
              <LifecycleButtons flow={flow} display={display} name={agent.displayName} collapse />
            ) : null}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="outline"
                  size="sm"
                  className="size-8 p-0 pointer-coarse:size-11"
                  aria-label={copy.moreActionsFor(agent.displayName)}
                >
                  <MoreHorizontal className="size-4" aria-hidden />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-72">
                {secondary.map((a) => (
                  <DropdownMenuItem
                    key={a}
                    className="sm:hidden"
                    disabled={busy}
                    onSelect={() => flow.run(a)}
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
                    <DropdownMenuItem
                      className="text-destructive"
                      onSelect={() => setDelOpen(true)}
                    >
                      {copy.deleteAgent}
                    </DropdownMenuItem>
                  </>
                ) : null}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        }
      />
      <DeleteAgentDialog agent={agent} open={delOpen} onOpenChange={setDelOpen} />
    </div>
  )
}
