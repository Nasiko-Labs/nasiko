/**
 * Catalog (plan §7.1): every agent the caller can see, searched and filtered client-side
 * (the list has no `q`). Page 1 renders at once; later pages stream in. Coding harnesses are
 * hidden unless asked for; they never run and the Harnesses page owns their usage.
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Bot, RotateCw, SearchX, X } from 'lucide-react'
import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Toggle } from '@/components/ui/toggle'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { SearchInput } from '@/components/shared/search-input'
import { EmptyState } from '@/components/shared/state-card'
import { DeployAgentButton } from '@/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@/features/deploy/copy'
import { ErrorState, StateCard } from '@/features/observability/StateCard'
import { useFleetHealth } from '@/features/overview/api'
import { copy as overviewCopy } from '@/features/overview/copy'
import { meQuery } from '@/lib/api/auth'
import { useCatalogAgents, useCatalogTabs, useUsers } from './api'
import { AgentMark, AgentsNav, FirstRunSteps, StatusBadge } from './components/bits'
import { copy } from './copy'
import type { CatalogSearch } from './search'
import { displayStatus, isHarness } from './status'
import { CARD_SKILLS, TOP_TAGS } from './tuning'
import type { Agent } from './types'

export function CatalogPage({
  search,
  setSearch,
}: {
  search: CatalogSearch
  setSearch: (patch: Partial<CatalogSearch>) => void
}) {
  const me = useQuery(meQuery).data
  const q = useCatalogAgents()
  const pinnedTabs = useCatalogTabs().data
  const names = useUsers(!!me?.is_superuser).data
  const inputRef = useRef<HTMLInputElement>(null)
  // Ratings are fetched only while the Overview's ?health= filter is on (eng review R1).
  const [now] = useState(() => new Date())
  const fleet = useFleetHealth(now, !!search.health)

  // "/" focuses search unless the user is already typing somewhere.
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

  const all = q.agents
  const harnessCount = useMemo(() => all.filter(isHarness).length, [all])
  const pool = useMemo(
    () => (search.harnesses ? all : all.filter((a) => !isHarness(a))),
    [all, search.harnesses],
  )
  const rated = fleet.summary?.byId
  const tags = useMemo(() => {
    if (pinnedTabs?.length) return pinnedTabs
    const counts = new Map<string, number>()
    for (const a of pool)
      for (const t of a.tags)
        if (t !== 'coding-agent' && t !== 'local') counts.set(t, (counts.get(t) ?? 0) + 1)
    return [...counts.entries()]
      .sort((x, y) => y[1] - x[1] || x[0].localeCompare(y[0]))
      .slice(0, TOP_TAGS)
      .map(([t]) => t)
  }, [pool, pinnedTabs])

  const term = (search.q ?? '').trim().toLowerCase()
  const shown = useMemo(
    () =>
      pool.filter((a) => {
        if (search.health && rated?.get(a.id)?.rating !== search.health) return false
        if (search.tag && !a.tags.includes(search.tag)) return false
        if (search.yours && a.owner_id !== me?.sub) return false
        if (!term) return true
        const hay = [
          a.display_name ?? '',
          a.name,
          a.description ?? '',
          ...a.tags,
          ...(a.skills ?? []).map((s) => s.name),
        ]
          .join(' ')
          .toLowerCase()
        return hay.includes(term)
      }),
    [pool, search.health, rated, search.tag, search.yours, term, me?.sub],
  )

  const header = (
    <>
      <PageHeader
        title={copy.catalogTitle}
        description={
          q.data ? (
            <>
              {copy.agentsCount(all.length - harnessCount)}
              {(q.hasNextPage || q.isFetchingNextPage) && !q.isFetchNextPageError
                ? ` · ${copy.loadingMore}`
                : ''}
            </>
          ) : null
        }
        actions={<DeployAgentButton />}
      />
      <AgentsNav current="/agents" />
    </>
  )

  // Wait for the ratings, not just the directory: until the dashboards land every agent rates Unknown (/ship review).
  if (
    q.isPending ||
    (search.health && !(fleet.error && !fleet.summary) && (!fleet.summary || fleet.ratingsPending))
  ) {
    return (
      <div className="space-y-4">
        {header}
        <PageLoader label={copy.loadingAgents} />
      </div>
    )
  }
  if (q.isError && !q.data)
    return (
      <div className="space-y-4">
        {header}
        <ErrorState error={q.error} onRetry={() => void q.refetch()} />
      </div>
    )
  // The health filter needs the ratings' agent list: without it nothing can match, so say why (/ship adversarial).
  if (search.health && fleet.error && !fleet.summary)
    return (
      <div className="space-y-4">
        {header}
        <ErrorState error={fleet.error} onRetry={fleet.retry} />
      </div>
    )

  return (
    <div className="space-y-4">
      {header}
      <div className="flex flex-wrap items-center gap-2">
        <SearchInput
          ref={inputRef}
          aria-label={copy.searchLabel}
          value={search.q ?? ''}
          onChange={(e) => setSearch({ q: e.target.value || undefined })}
          placeholder={copy.searchPlaceholder}
          aria-keyshortcuts="/"
          className="w-full max-w-none sm:w-80"
        />
        {search.health ? (
          <Button
            variant="outline"
            size="sm"
            onClick={() => setSearch({ health: undefined })}
            aria-label={copy.clearHealth(overviewCopy.rating[search.health])}
            className="rounded-full border-primary-text bg-primary/10 text-xs font-normal text-primary-text hover:bg-primary/15 hover:text-primary-text"
          >
            {copy.healthChip(overviewCopy.rating[search.health])}{' '}
            <X className="size-3" aria-hidden />
          </Button>
        ) : null}
        <Chip
          active={!!search.yours}
          onClick={() => setSearch({ yours: search.yours ? undefined : true })}
        >
          {copy.yours}
        </Chip>
        {tags.map((t) => (
          <Chip
            key={t}
            active={search.tag === t}
            onClick={() => setSearch({ tag: search.tag === t ? undefined : t })}
          >
            {t}
          </Chip>
        ))}
        {harnessCount ? (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setSearch({ harnesses: search.harnesses ? undefined : true })}
          >
            {search.harnesses ? copy.hideHarnesses : copy.showHarnesses(harnessCount)}
          </Button>
        ) : null}
      </div>
      {search.health && fleet.costFailed ? (
        <StateCard
          tone="warning"
          title={copy.healthCostFailed}
          action={
            <Button size="sm" variant="outline" onClick={fleet.retryCost}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      ) : null}
      {q.isFetchNextPageError ? (
        <StateCard
          tone="warning"
          title={copy.partialLoad}
          action={
            <Button size="sm" variant="outline" onClick={() => void q.fetchNextPage()}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      ) : null}
      {all.length === 0 ? (
        <>
          <EmptyState icon={Bot} title={copy.noAgentsCatalog} action={<DeployAgentButton />}>
            {copy.noAgentsCatalogHint}
          </EmptyState>
          <div>
            <p className="mb-2 text-xs text-muted-foreground">{deployCopy.entry.orCli}</p>
            <FirstRunSteps />
          </div>
        </>
      ) : shown.length === 0 ? (
        <EmptyState
          icon={SearchX}
          title={copy.noResults}
          action={
            <Button
              size="sm"
              variant="outline"
              onClick={() =>
                setSearch({ q: undefined, tag: undefined, yours: undefined, health: undefined })
              }
            >
              {copy.clearSearch}
            </Button>
          }
        >
          {copy.noResultsHint}
        </EmptyState>
      ) : (
        <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {shown.map((a) => (
            <li key={a.id}>
              <AgentCard
                agent={a}
                meId={me?.sub}
                ownerName={names?.get(a.owner_id)}
                reason={search.health ? rated?.get(a.id)?.reasons[0]?.text : undefined}
              />
            </li>
          ))}
        </ul>
      )}
    </div>
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

/** Memoized: every search keystroke re-renders the page, and the catalog can hold hundreds of cards. */
const AgentCard = memo(function AgentCard({
  agent,
  meId,
  ownerName,
  reason,
}: {
  agent: Agent
  meId?: string
  ownerName?: string
  reason?: string
}) {
  const harness = isHarness(agent)
  const name = agent.display_name || agent.name
  const mine = agent.owner_id === meId
  const skills = agent.skills ?? []
  const extra = skills.length - CARD_SKILLS
  const owner = mine ? copy.you : ownerName
  return (
    // One link, no nested interactive elements (plan §7.1).
    <Card
      asChild
      className="h-full gap-2 p-4 transition-[color,border-color,box-shadow] hover:border-primary/40 hover:shadow-lg focus-visible:outline-2 focus-visible:outline-ring"
    >
      <Link to="/agents/$agentId" params={{ agentId: agent.id }} search={{}}>
        <div className="flex items-start gap-3">
          <AgentMark name={name} iconUrl={agent.icon_url} size={36} />
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium">{name}</div>
            <div className="truncate font-mono text-xs text-muted-foreground">{agent.name}</div>
          </div>
          {harness ? null : (
            <StatusBadge display={displayStatus(agent.status, false)} raw={agent.status} />
          )}
        </div>
        <p
          className="line-clamp-1 text-sm text-muted-foreground"
          title={agent.description ?? undefined}
        >
          {agent.description || copy.noDescription}
        </p>
        {/* The Overview's health filter says why each agent is listed (overview design 15A; QA ISSUE-001). */}
        {reason ? (
          <p className="line-clamp-1 text-xs font-medium" data-testid="health-reason">
            {copy.healthReason(reason)}
          </p>
        ) : null}
        {skills.length ? (
          <div className="flex flex-wrap gap-1">
            {skills.slice(0, CARD_SKILLS).map((s) => (
              <Badge key={s.id} variant="secondary" className="font-normal">
                {s.name}
              </Badge>
            ))}
            {extra > 0 ? (
              <Badge variant="outline" className="font-normal">
                +{extra}
              </Badge>
            ) : null}
          </div>
        ) : null}
        <div className="mt-auto flex flex-wrap items-center gap-2 pt-1 text-xs text-muted-foreground">
          {harness ? (
            <Badge variant="outline">{copy.harnessChip}</Badge>
          ) : (
            <Badge variant="outline">{mine ? copy.yours : copy.availableToYou}</Badge>
          )}
          {owner && !mine ? <span>{owner}</span> : null}
        </div>
      </Link>
    </Card>
  )
})
