/**
 * Builds (plans/feat-deploy.md §6): every build the viewer can see (their agents' builds; every build for a superuser),
 * in-progress ones pinned on top and refreshing while they run (design review 2), failed rows with a one-line reason.
 * Below 768 px rows become two lines (design review 16).
 */
import { Link } from '@tanstack/react-router'
import { Hammer, SearchX } from 'lucide-react'
import { useDeferredValue } from 'react'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { AgentsNav } from '@/features/agents/components/bits'
import { PanelError } from '@/components/shared/panel'
import { SearchInput } from '@/components/shared/search-input'
import { EmptyState, StateCard } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { relTime } from '@/features/agents/format'
import { useAgentsDirectory } from '@/features/agents/api'
import { isEndpointAbsent } from '@/lib/api/detect'
import { useFailureDetails, useBuildsPage } from './api'
import { copy } from './copy'
import { failureReason } from './errors'
import { BUILDS_FILTERS, type BuildsFilter, type BuildsSearch } from './search'
import { buildSource, fmtElapsed, isActive } from './steps'
import type { BuildRecord } from './types'
import { BuildBadge } from './components/BuildBadge'
import { DeployAgentButton } from './components/DeployAgentButton'

export function BuildsPage({
  search,
  setSearch,
}: {
  search: BuildsSearch
  setSearch: (patch: Partial<BuildsSearch>, opts?: { replace?: boolean }) => void
}) {
  const filter: BuildsFilter = search.status ?? 'all'
  const page = search.page ?? 0
  // Deferred like the trace search: renders stay responsive while typing (it lets React skip intermediate values; it
  // isn't a debounce).
  const q = useDeferredValue(search.q)
  const data = useBuildsPage(filter, q, page)
  const dir = useAgentsDirectory()
  const all = [...data.pinned, ...data.rows]
  const failedIds = all.filter((b) => b.status === 'failed').map((b) => b.id)
  const reasons = useFailureDetails(failedIds)
  const now = data.updatedAt || Date.parse(all[0]?.updated_at ?? '') || 0
  const nameOf = (b: BuildRecord) => {
    const a = dir.byId.get(b.agent_id)
    return a ? a.display_name || a.name : copy.builds.unknownAgent
  }

  const header = (
    <>
      <PageHeader title={copy.builds.title} description={copy.builds.description} />
      <AgentsNav current="/builds" />
    </>
  )
  const filters = (
    <div className="flex flex-wrap items-center gap-2">
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        value={filter}
        onValueChange={(v) => {
          if (v)
            setSearch(
              { status: v === 'all' ? undefined : (v as BuildsFilter), page: undefined },
              { replace: true },
            )
        }}
        aria-label={copy.builds.filterLabel}
        className="max-w-full overflow-x-auto"
      >
        {BUILDS_FILTERS.map((f) => (
          <ToggleGroupItem key={f} value={f} className="pointer-coarse:min-h-11">
            {copy.filter[f]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      <SearchInput
        aria-label={copy.builds.searchLabel}
        value={search.q ?? ''}
        onChange={(e) =>
          setSearch({ q: e.target.value || undefined, page: undefined }, { replace: true })
        }
        placeholder={copy.builds.searchPlaceholder}
        className="w-full sm:w-64"
      />
    </div>
  )

  let body
  if (data.error && isEndpointAbsent(data.error))
    body = <StateCard title={copy.builds.newerServer} />
  else if (data.noRights) body = <StateCard title={copy.builds.noRights} />
  else if (data.error && !all.length)
    body = <PanelError error={data.error} onRetry={data.retry} what={copy.builds.what} />
  else if (data.isPending && !all.length) body = <PageLoader label={copy.builds.loading} />
  else if (!all.length) {
    const filtered = filter !== 'all' || !!search.q || page > 0
    body = filtered ? (
      <EmptyState
        icon={SearchX}
        title={copy.builds.noMatch}
        action={
          <Button
            size="sm"
            variant="outline"
            className="pointer-coarse:min-h-11"
            onClick={() => setSearch({ status: undefined, q: undefined, page: undefined })}
          >
            {copy.builds.clearFilters}
          </Button>
        }
      >
        {copy.builds.noMatchHint}
      </EmptyState>
    ) : (
      <EmptyState icon={Hammer} title={copy.builds.empty} action={<DeployAgentButton />}>
        {copy.builds.emptyHint}
      </EmptyState>
    )
  } else {
    body = (
      <>
        <Table className="text-sm" data-testid="builds-table">
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead className="max-md:sr-only">{copy.builds.colBuild}</TableHead>
              <TableHead className="max-md:sr-only">{copy.builds.colStatus}</TableHead>
              <TableHead className="max-md:sr-only">{copy.builds.colSource}</TableHead>
              <TableHead className="max-md:sr-only">{copy.builds.colStarted}</TableHead>
              <TableHead className="text-right max-md:sr-only">{copy.builds.colDuration}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {all.map((b) => {
              const reason = b.status === 'failed' ? reasons.get(b.id) : undefined
              const end = isActive(b.status) ? now : Date.parse(b.updated_at)
              return (
                <TableRow
                  key={b.id}
                  data-testid="build-row"
                  data-status={b.status}
                  className="max-md:grid max-md:grid-cols-[1fr_auto] max-md:gap-x-2 max-md:py-2"
                >
                  <TableCell className="min-w-0 max-md:p-0">
                    <Link
                      to="/builds/$buildId"
                      params={{ buildId: b.id }}
                      className="font-medium underline-offset-4 hover:underline pointer-coarse:inline-flex pointer-coarse:min-h-11 pointer-coarse:items-center"
                    >
                      {nameOf(b)}{' '}
                      <span className="font-mono text-muted-foreground">{b.version_tag}</span>
                    </Link>
                    {b.status === 'failed' ? (
                      <p
                        className="truncate text-xs text-muted-foreground"
                        data-testid="build-reason"
                      >
                        {reason ? failureReason(reason) : copy.status.failed}
                      </p>
                    ) : null}
                  </TableCell>
                  <TableCell className="max-md:p-0 max-md:text-right">
                    <BuildBadge badge={b.status} />
                  </TableCell>
                  <TableCell className="text-muted-foreground max-md:col-span-2 max-md:inline max-md:p-0 max-md:text-xs">
                    {buildSource(b)}
                  </TableCell>
                  <TableCell className="text-muted-foreground tabular-nums max-md:hidden">
                    {relTime(b.created_at, Math.max(now, Date.parse(b.created_at)))}
                  </TableCell>
                  <TableCell className="text-right text-muted-foreground tabular-nums max-md:hidden">
                    {fmtElapsed(end - Date.parse(b.created_at))}
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
        {page > 0 || data.hasNext ? (
          <div className="flex justify-end gap-2">
            <Button
              size="sm"
              variant="outline"
              className="pointer-coarse:min-h-11"
              disabled={page === 0}
              onClick={() => setSearch({ page: page - 1 || undefined })}
            >
              {copy.builds.previous}
            </Button>
            <Button
              size="sm"
              variant="outline"
              className="pointer-coarse:min-h-11"
              disabled={!data.hasNext}
              onClick={() => setSearch({ page: page + 1 })}
            >
              {copy.builds.next}
            </Button>
          </div>
        ) : null}
      </>
    )
  }
  return (
    <div className="mx-auto flex w-full max-w-page flex-col gap-4">
      {header}
      {filters}
      {body}
    </div>
  )
}
