/**
 * A custom MCP server's page (plans/feat-mcp.md §5), laid out like the agent detail: header with its status and
 * Connect, tabs in `?tab=`. Access, Logs and Settings are the owner's (or a superuser's): the server gates those reads.
 * A hidden or unknown tab falls back to Overview with replace. A 404 is "not found or not visible" (the server never
 * says which).
 */
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { MoreHorizontal } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { CopyButton, CopyMenuItem } from '@/components/shared/copy-button'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { StateCard } from '@/components/shared/state-card'
import { AgentMark } from '@/features/agents/components/bits'
import { isUuid } from '@/features/agents/normalize'
import { meQuery } from '@/lib/api/auth'
import { ApiError } from '@/lib/api/client'
import { useConnector } from './api'
import { ServerStatusBadge } from './components/bits'
import { ConnectControl } from './components/ConnectControl'
import { DeleteDialog } from './components/DeleteDialog'
import { copy, reason } from './copy'
import { AccessTab } from './detail/AccessTab'
import { AgentsTab } from './detail/AgentsTab'
import { LogsTab } from './detail/LogsTab'
import { OverviewTab } from './detail/OverviewTab'
import { SettingsTab } from './detail/SettingsTab'
import { authFlowOf, labelOf, serverStatus } from './logic'
import { useOAuthPopup } from './oauth'
import type { DetailSearch, DetailTab } from './search'
import type { ConnectorDetail } from './types'

export function DetailPage({ id, search }: { id: string; search: DetailSearch }) {
  // The server's path parameter is a UUID (anything else is a 400); ids are lowercase on the wire.
  if (!isUuid(id)) return <NotFound />
  return <Detail id={id.toLowerCase()} search={search} />
}

function NotFound() {
  return (
    <StateCard
      title={copy.notFound}
      fix={copy.notFoundFix}
      action={
        <Link
          to="/mcp"
          search={{}}
          className="text-sm text-primary-text underline-offset-4 hover:underline"
        >
          {copy.backToCatalog}
        </Link>
      }
    />
  )
}

function tabsFor(c: ConnectorDetail, manage: boolean): DetailTab[] {
  const tabs: DetailTab[] = ['overview', 'agents']
  if (manage) tabs.push('access')
  if (manage && c.source_kind === 'uploaded_build') tabs.push('logs')
  if (manage) tabs.push('settings')
  return tabs
}

function Detail({ id, search }: { id: string; search: DetailSearch }) {
  const q = useConnector(id)
  const me = useQuery(meQuery).data
  const navigate = useNavigate()
  const openPopup = useOAuthPopup()
  const c = q.data
  const manage = !!c && (c.is_owner || !!me?.is_superuser)
  const allowed = c ? tabsFor(c, manage) : []
  const current: DetailTab = allowed.includes(search.tab as DetailTab)
    ? (search.tab as DetailTab)
    : 'overview'
  const badTab = !!c && !!search.tab && current !== search.tab
  useEffect(() => {
    if (badTab) void navigate({ to: '.', search: (s) => ({ ...s, tab: undefined }), replace: true })
  }, [badTab, navigate])

  if (q.isPending) return <PageLoader label={copy.loadingServer} />
  if (q.isError)
    return q.error instanceof ApiError && (q.error.status === 404 || q.error.status === 403) ? (
      <NotFound />
    ) : (
      <StateCard
        tone="error"
        title={copy.loadFailed}
        fix={reason(q.error)}
        action={
          <Button size="sm" variant="outline" onClick={() => void q.refetch()}>
            {copy.retry}
          </Button>
        }
      />
    )

  if (!c) return null
  const setTab = (t: string) =>
    void navigate({ to: '.', search: t === 'overview' ? {} : { tab: t } })

  return (
    <div className="space-y-4">
      <Header connector={c} manage={manage} meName={me?.username} openPopup={openPopup} />
      <Tabs value={current} onValueChange={setTab}>
        <TabsList className="max-w-full justify-start overflow-x-auto">
          {allowed.map((t) => (
            <TabsTrigger key={t} value={t}>
              {copy.tabsNames[t]}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent value="overview" className="pt-3">
          <OverviewTab connector={c} manage={manage} openPopup={openPopup} />
        </TabsContent>
        <TabsContent value="agents" className="pt-3">
          <AgentsTab connector={c} manage={manage} agentId={search.agent} openPopup={openPopup} />
        </TabsContent>
        {allowed.includes('access') ? (
          <TabsContent value="access" className="pt-3">
            <AccessTab connector={c} />
          </TabsContent>
        ) : null}
        {allowed.includes('logs') ? (
          <TabsContent value="logs" className="pt-3">
            <LogsTab connector={c} />
          </TabsContent>
        ) : null}
        {allowed.includes('settings') ? (
          <TabsContent value="settings" className="pt-3">
            <SettingsTab connector={c} />
          </TabsContent>
        ) : null}
      </Tabs>
    </div>
  )
}

function Header({
  connector: c,
  manage,
  meName,
  openPopup,
}: {
  connector: ConnectorDetail
  manage: boolean
  meName?: string
  openPopup: (url: string | undefined) => void
}) {
  const [delOpen, setDelOpen] = useState(false)
  const label = labelOf(c)
  const status = serverStatus(c)
  const link = `${globalThis.location?.origin ?? ''}/mcp/${c.connector_id}`
  const version = c.version ?? c.upload_info?.version
  const owner = c.is_owner ? copy.you : (c.owner_username ?? meName ?? '—')
  return (
    <div className="flex items-start gap-3">
      <AgentMark name={label} iconUrl={c.logo_url} size={40} />
      <PageHeader
        className="min-w-0 flex-1"
        title={
          <span className="flex flex-wrap items-center gap-2">
            <span className="truncate">{label}</span>
            <ServerStatusBadge status={status} />
          </span>
        }
        description={
          <span className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
            <span className="font-mono">{c.name}</span>
            <span className="inline-flex items-center font-mono" title={c.connector_id}>
              {c.connector_id.slice(0, 8)}…
              <CopyButton text={c.connector_id} label={copy.copyId} />
            </span>
            {version ? <span className="font-mono">{version}</span> : null}
            <span>{copy.ownerLine(owner)}</span>
            <Badge variant="outline">{c.is_owner ? copy.yours : copy.sharedWithYou}</Badge>
            {c.is_public ? <Badge variant="outline">{copy.publicChip}</Badge> : null}
          </span>
        }
        actions={
          <div className="flex items-start gap-2">
            {status !== 'building' && status !== 'failed' ? (
              <ConnectControl
                target={{
                  id: c.connector_id,
                  label,
                  authFlow: authFlowOf(c.auth_type),
                  authType: c.auth_type ?? null,
                  connected: c.is_connected,
                }}
                openPopup={openPopup}
              />
            ) : null}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="outline"
                  size="sm"
                  className="size-8 p-0 pointer-coarse:size-11"
                  aria-label={copy.moreActions(label)}
                >
                  <MoreHorizontal className="size-4" aria-hidden />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-56">
                <CopyMenuItem text={c.connector_id} label={copy.copyId} />
                <CopyMenuItem text={link} label={copy.copyLink} />
                {manage ? (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      className="text-destructive"
                      onSelect={() => setDelOpen(true)}
                    >
                      {copy.delete}
                    </DropdownMenuItem>
                  </>
                ) : null}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        }
      />
      <DeleteDialog id={c.connector_id} label={label} open={delOpen} onOpenChange={setDelOpen} />
    </div>
  )
}
