/**
 * Catalog (plan §7.1): every agent the caller can see, searched and filtered client-side
 * (the list has no `q`). Page 1 renders at once; later pages stream in. Coding harnesses are
 * hidden unless asked for; they never run and the Harnesses page owns their usage.
 */
import { useQuery } from '@tanstack/react-query'
import { Bot, RotateCw, SearchX, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { Label } from '@nasiko/ui/primitives/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { Switch } from '@nasiko/ui/primitives/switch'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { SearchInput } from '@nasiko/ui/composites/search-input'
import { EmptyState } from '@nasiko/ui/composites/state-card'
import { DeployAgentButton } from '@nasiko/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@nasiko/features/deploy/copy'
import { ErrorState, StateCard } from '@nasiko/features/observability/StateCard'
import { useFleetHealth } from '@nasiko/features/overview/api'
import { copy as overviewCopy } from '@nasiko/features/overview/copy'
import { meQuery } from '@nasiko/data-access/auth'
import { useSiblingState } from '@nasiko/app-core/siblings'
import { useCatalogAgents, useCatalogTabs, useUsers } from './api'
import { AgentCard } from './components/AgentCard'
import { AgentsHeader, FirstRunSteps } from './components/bits'
import { copy } from './copy'
import type { CatalogSearch } from './search'
import { isHarness } from './status'
import { TOP_TAGS } from './tuning'

/** The Tag filter's "no tag" value (a Select item can't be empty). */
const ALL_TAGS = '__all'

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
  // Ratings are fetched only while the ?health= filter (the home's rating badges link here) is on (eng review R1).
  const [now] = useState(() => new Date())
  // Members never read the fleet-wide spend series (the home and the hub rate them the same way).
  const fleet = useFleetHealth(now, !!search.health, !!me?.is_superuser)

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
  // Previous/Next on the agent page walks the cards as shown when one is clicked.
  const linkState = useSiblingState(shown.map((a) => a.id))

  const header = (
    <AgentsHeader
      current="/agents"
      description={
        <>
          {copy.catalogSub}
          {(q.hasNextPage || q.isFetchingNextPage) && !q.isFetchNextPageError
            ? ` · ${copy.loadingMore}`
            : ''}
        </>
      }
      // An empty catalog's card offers Deploy itself.
      action={q.data && all.length === 0 ? null : <DeployAgentButton />}
    />
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
      <div data-tour="agents-toolbar" className="flex flex-wrap items-center gap-2">
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
        {/* The filters keep to the right: search reads first, then what narrows it. */}
        <div className="flex flex-wrap items-center gap-2 sm:ml-auto">
          <Select
            value={search.yours ? 'yours' : 'anyone'}
            onValueChange={(v) => setSearch({ yours: v === 'yours' ? true : undefined })}
          >
            <SelectTrigger size="sm" aria-label={copy.ownerFilter} className="text-xs">
              <span className="text-muted-foreground">{copy.ownerLabel}:</span> <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="anyone">{copy.anyone}</SelectItem>
              <SelectItem value="yours">{copy.yours}</SelectItem>
            </SelectContent>
          </Select>
          {tags.length || search.tag ? (
            <Select
              value={search.tag ?? ALL_TAGS}
              onValueChange={(v) => setSearch({ tag: v === ALL_TAGS ? undefined : v })}
            >
              <SelectTrigger size="sm" aria-label={copy.tagFilter} className="max-w-56 text-xs">
                <span className="text-muted-foreground">{copy.tagLabel}:</span> <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL_TAGS}>{copy.allTags}</SelectItem>
                {/* A tag from a link that isn't among the top ones still shows what is applied. */}
                {search.tag && !tags.includes(search.tag) ? (
                  <SelectItem value={search.tag}>{search.tag}</SelectItem>
                ) : null}
                {tags.map((t) => (
                  <SelectItem key={t} value={t}>
                    {t}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : null}
          {harnessCount ? (
            <Label className="gap-2 text-xs font-normal text-muted-foreground">
              <Switch
                checked={!!search.harnesses}
                onCheckedChange={(on) => setSearch({ harnesses: on ? true : undefined })}
              />
              {copy.harnessesToggle(harnessCount)}
            </Label>
          ) : null}
        </div>
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
        <ul data-tour="agents-grid" className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {shown.map((a) => (
            <li key={a.id}>
              <AgentCard
                agent={a}
                meId={me?.sub}
                ownerName={names?.get(a.owner_id)}
                reason={search.health ? rated?.get(a.id)?.reasons[0]?.text : undefined}
                linkState={linkState}
              />
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
