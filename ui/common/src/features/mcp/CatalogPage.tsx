/**
 * The MCP catalog (plans/feat-mcp.md §2): custom servers ∪ platform toolkits in one grid, laid out like the Agents
 * catalog. Scope chips (`?view=`), connection tabs with counts (`?tab=`) and search (`?q=`) all replace history.
 * A custom server's name links to its page; its Connect control sits beside the link, never inside it.
 */
import { Link } from '@tanstack/react-router'
import { CheckCircle2, Plug, Plus, RotateCw, SearchX, Upload } from 'lucide-react'
import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Toggle } from '@/components/ui/toggle'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { SearchInput } from '@/components/shared/search-input'
import { EmptyState, StateCard } from '@/components/shared/state-card'
import { AgentMark } from '@/features/agents/components/bits'
import { useConnectors, useToolkits } from './api'
import { ServerStatusBadge } from './components/bits'
import { ConnectControl } from './components/ConnectControl'
import { RegisterDialog } from './components/RegisterDialog'
import { UploadDialog } from './components/UploadDialog'
import { copy, reason } from './copy'
import {
  canConnect,
  inTab,
  inView,
  matchesQuery,
  tabCounts,
  toServices,
  type Service,
} from './logic'
import { useOAuthPopup } from './oauth'
import { TABS, VIEWS, type CatalogSearch, type Tab } from './search'

export function CatalogPage({
  search,
  setSearch,
}: {
  search: CatalogSearch
  setSearch: (patch: Partial<CatalogSearch>) => void
}) {
  const servers = useConnectors()
  const toolkits = useToolkits()
  const openPopup = useOAuthPopup()
  const [dialog, setDialog] = useState<'register' | 'upload' | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // "/" focuses search unless the user is already typing somewhere (as in Agents).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== '/' || e.metaKey || e.ctrlKey || e.altKey) return
      const t = e.target as HTMLElement | null
      if (t && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName))) return
      e.preventDefault()
      inputRef.current?.focus()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const all = useMemo(
    () => toServices(servers.data, toolkits.data?.toolkits ?? []),
    [servers.data, toolkits.data],
  )
  const scoped = useMemo(() => all.filter((s) => inView(s, search.view)), [all, search.view])
  const counts = tabCounts(scoped)
  const tab: Tab = search.tab ?? 'all'
  const q = search.q ?? ''
  const shown = useMemo(
    () => scoped.filter((s) => inTab(s, tab) && matchesQuery(s, q)),
    [scoped, tab, q],
  )
  // The empty card repeats both header actions, so the header drops them (legacy: never each button twice).
  const scopeEmpty =
    !!servers.data && !scoped.length && search.view !== 'shared' && search.view !== 'toolkits'

  const actions = (
    <>
      <Button
        variant="outline"
        size="sm"
        className="pointer-coarse:min-h-11"
        onClick={() => setDialog('upload')}
      >
        <Upload aria-hidden /> {copy.upload}
      </Button>
      <Button size="sm" className="pointer-coarse:min-h-11" onClick={() => setDialog('register')}>
        <Plus aria-hidden /> {copy.register}
      </Button>
    </>
  )
  const dialogs =
    dialog === 'register' ? (
      <RegisterDialog onClose={() => setDialog(null)} />
    ) : dialog === 'upload' ? (
      <UploadDialog onClose={() => setDialog(null)} />
    ) : null
  const header = (
    <PageHeader
      title={copy.title}
      description={servers.data ? `${copy.sub} · ${copy.count(all.length)}` : copy.sub}
      actions={scopeEmpty ? null : actions}
    />
  )

  if (servers.isPending)
    return (
      <div className="space-y-4">
        {header}
        <PageLoader label={copy.loadingCatalog} />
        {dialogs}
      </div>
    )
  if (servers.isError && !servers.data)
    return (
      <div className="space-y-4">
        {header}
        <StateCard
          tone="error"
          title={copy.loadFailed}
          fix={`${copy.loadFailedFix} (${reason(servers.error)})`}
          action={
            <Button size="sm" variant="outline" onClick={() => void servers.refetch()}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
        {dialogs}
      </div>
    )

  return (
    <div className="space-y-4">
      {header}
      <div className="flex flex-wrap items-center gap-2">
        <SearchInput
          ref={inputRef}
          aria-label={copy.searchLabel}
          value={q}
          onChange={(e) => setSearch({ q: e.target.value || undefined })}
          placeholder={copy.searchPlaceholder}
          aria-keyshortcuts="/"
          className="w-full max-w-none sm:w-80"
        />
        {VIEWS.map((v) => (
          <Chip
            key={v}
            active={search.view === v}
            onClick={() => setSearch({ view: search.view === v ? undefined : v, tab: undefined })}
          >
            {copy.views[v]}
          </Chip>
        ))}
      </div>
      {toolkits.isError ? (
        <StateCard
          tone="warning"
          title={copy.toolkitsFailed}
          action={
            <Button size="sm" variant="outline" onClick={() => void toolkits.refetch()}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      ) : null}
      {!scoped.length ? (
        <ScopeEmpty view={search.view} actions={scopeEmpty ? actions : null} />
      ) : (
        <>
          <Tabs
            value={tab}
            onValueChange={(t) => setSearch({ tab: t === 'all' ? undefined : (t as Tab) })}
          >
            <TabsList aria-label={copy.tabsLabel}>
              {TABS.map((t) => (
                <TabsTrigger key={t} value={t}>
                  {copy.tabs[t]}
                  <span className="text-xs text-muted-foreground tabular-nums">{counts[t]}</span>
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          {shown.length ? (
            <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {shown.map((s) => (
                <li key={s.id}>
                  <ServiceCard service={s} openPopup={openPopup} />
                </li>
              ))}
            </ul>
          ) : q.trim() ? (
            <EmptyState
              icon={SearchX}
              title={copy.noResults(q.trim())}
              action={
                <Button size="sm" variant="outline" onClick={() => setSearch({ q: undefined })}>
                  {copy.clearSearch}
                </Button>
              }
            >
              {copy.noResultsHint}
            </EmptyState>
          ) : tab === 'connected' ? (
            <EmptyState
              icon={Plug}
              title={copy.nothingConnected}
              action={
                <Button size="sm" onClick={() => setSearch({ tab: 'available' })}>
                  {copy.browseAvailable}
                </Button>
              }
            >
              {copy.nothingConnectedDesc}
            </EmptyState>
          ) : (
            <EmptyState icon={CheckCircle2} title={copy.allConnected}>
              {copy.allConnectedDesc}
            </EmptyState>
          )}
        </>
      )}
      {dialogs}
    </div>
  )
}

/** Every scope gets the same card; only the copy and the actions differ (legacy `CATALOG_SCOPES`). */
function ScopeEmpty({ view, actions }: { view: CatalogSearch['view']; actions: React.ReactNode }) {
  const e = copy.empty[view ?? 'all']
  return (
    <EmptyState icon={Plug} title={e.title} action={actions ?? undefined}>
      {e.desc}
    </EmptyState>
  )
}

function Chip({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: string
}) {
  return (
    <Toggle
      variant="outline"
      size="sm"
      pressed={active}
      onPressedChange={onClick}
      className="min-h-8 rounded-full px-3 text-xs font-normal text-muted-foreground shadow-none hover:bg-transparent hover:text-foreground data-[state=on]:border-primary-text data-[state=on]:bg-primary/10 data-[state=on]:text-primary-text"
    >
      {children}
    </Toggle>
  )
}

/** Memoized: every search keystroke re-renders the page. */
const ServiceCard = memo(function ServiceCard({
  service: s,
  openPopup,
}: {
  service: Service
  openPopup: (url: string | undefined) => void
}) {
  const building = s.status === 'building'
  const failed = s.status === 'failed'
  const title =
    s.kind === 'server' ? (
      <Link
        to="/mcp/$connectorId"
        params={{ connectorId: s.id }}
        search={{}}
        className="truncate font-medium underline-offset-4 after:absolute after:inset-0 after:rounded-xl hover:underline focus-visible:outline-none"
      >
        {s.label}
      </Link>
    ) : (
      <span className="truncate font-medium">{s.label}</span>
    )
  return (
    // The title link stretches over the card; the Connect control sits above it (z-10).
    <Card className="relative h-full gap-2 p-4 transition-colors hover:border-primary/40 has-[a:focus-visible]:outline-2 has-[a:focus-visible]:outline-ring">
      <div className="flex items-start gap-3">
        <AgentMark name={s.label} iconUrl={s.logoUrl} size={36} />
        <div className="flex min-w-0 flex-1 flex-col">
          {title}
          <span className="truncate font-mono text-xs text-muted-foreground">{s.name}</span>
        </div>
        {s.status && s.status !== 'active' ? <ServerStatusBadge status={s.status} /> : null}
        {canConnect(s) ? (
          <ConnectControl target={s} openPopup={openPopup} className="relative z-10" />
        ) : null}
      </div>
      {building ? (
        <div className="text-sm">
          <p className="font-medium">{copy.building}</p>
          <p className="text-muted-foreground">{copy.buildingHint}</p>
        </div>
      ) : failed ? (
        <div className="text-sm">
          <p className="font-medium text-destructive">{copy.buildFailed}</p>
          <p className="text-muted-foreground">{copy.buildFailedHint}</p>
        </div>
      ) : (
        <p
          className="line-clamp-2 text-sm text-muted-foreground"
          title={s.description ?? undefined}
        >
          {s.description || copy.noDescription}
        </p>
      )}
      <div className="mt-auto flex flex-wrap items-center gap-1.5 pt-1 text-xs">
        <Badge variant="secondary" className="font-normal">
          {copy.tools(s.toolCount)}
        </Badge>
        {s.version ? (
          <Badge variant="outline" className="font-mono font-normal">
            {s.version}
          </Badge>
        ) : null}
        <Badge variant="outline" className="font-normal">
          {s.kind === 'toolkit' ? copy.toolkit : s.shared ? copy.sharedBy(s.owner) : copy.yours}
        </Badge>
      </div>
    </Card>
  )
})
