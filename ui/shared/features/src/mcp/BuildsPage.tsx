/**
 * MCP Builds (a dedicated section, mirroring the Agents Builds page): every MCP server upload the
 * caller can see, in-progress ones pinned on top. Unlike the catalog grid (`useConnectors`), this
 * reads `useMyUploads()` (`mcp_upload_status`), so an upload whose connector was deleted after a
 * first-time build failure still shows up here, with its real error, instead of nowhere at all.
 */
import { Link } from '@tanstack/react-router'
import { Hammer, SearchX } from 'lucide-react'
import { useDeferredValue, useMemo } from 'react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PanelError } from '@nasiko/ui/composites/panel'
import { SearchInput } from '@nasiko/ui/composites/search-input'
import { EmptyState } from '@nasiko/ui/composites/state-card'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { Badge } from '@nasiko/ui/primitives/badge'
import { relTime } from '@nasiko/features/agents/format'
import { fmtDuration } from '@nasiko/ui/lib/format'
import { useMyUploads } from './api'
import { copy } from './copy'
import { McpNav } from './components/McpNav'
import { MCP_BUILDS_FILTERS, type McpBuildsFilter, type McpBuildsSearch } from './search'
import type { UploadedConnector } from './types'

/** `upload_info.upload_status` → the filter bucket it belongs to — also what the status column
 * shows (`BuildStatusBadge`). Deliberately past-tense/outcome wording ("Succeeded"), not the
 * catalog's `ServerStatusBadge` ("Active"): a build's result is a historical fact, and showing
 * "Active" for one whose connector was since deleted reads as a live-state claim that's now simply
 * false — exactly the trap the Agents Builds page avoids by saying "Succeeded", never "Running". */
function bucketOf(u: UploadedConnector): Exclude<McpBuildsFilter, 'all'> {
  switch (u.upload_info.upload_status) {
    case 'Active':
      return 'success'
    case 'Failed':
      return 'failed'
    default:
      return 'building'
  }
}

const BUILD_BADGE_TONE = {
  building: 'info',
  success: 'success',
  failed: 'destructive',
} as const satisfies Record<Exclude<McpBuildsFilter, 'all'>, string>

function BuildStatusBadge({ bucket }: { bucket: Exclude<McpBuildsFilter, 'all'> }) {
  return <Badge variant={BUILD_BADGE_TONE[bucket]}>{copy.buildStatus[bucket]}</Badge>
}

export function BuildsPage({
  search,
  setSearch,
}: {
  search: McpBuildsSearch
  setSearch: (patch: Partial<McpBuildsSearch>) => void
}) {
  const uploads = useMyUploads()
  const filter: McpBuildsFilter = search.status ?? 'all'
  // Deferred like the Agents Builds page: renders stay responsive while typing.
  const q = useDeferredValue(search.q ?? '')

  const all = useMemo(() => uploads.data ?? [], [uploads.data])
  const shown = useMemo(() => {
    const query = q.trim().toLowerCase()
    return (
      all
        .filter((u) => filter === 'all' || bucketOf(u) === filter)
        .filter((u) => !query || u.connector_name.toLowerCase().includes(query))
        .slice()
        // In-progress first, then newest.
        .sort((a, b) => {
          const ap = bucketOf(a) === 'building' ? 0 : 1
          const bp = bucketOf(b) === 'building' ? 0 : 1
          return ap !== bp ? ap - bp : b.created_at.localeCompare(a.created_at)
        })
    )
  }, [all, filter, q])

  const nothingYet = !uploads.isPending && !uploads.error && !all.length
  const header = (
    <>
      <TopBar />
      <PageHeader title={copy.title} description={copy.builds.description} />
      <McpNav current="/mcp/builds" />
    </>
  )

  const filters = (
    <div className="flex flex-wrap items-center gap-2 border-b border-border p-2">
      <ToggleGroup
        variant="outline"
        size="sm"
        value={[filter]}
        onValueChange={(vs) => {
          const v = vs[0] as string
          if (v) setSearch({ status: v === 'all' ? undefined : (v as McpBuildsFilter) })
        }}
        aria-label={copy.builds.filterLabel}
        className="max-w-full overflow-x-auto"
      >
        {MCP_BUILDS_FILTERS.map((f) => (
          <ToggleGroupItem key={f} value={f} className="pointer-coarse:min-h-11">
            {copy.buildFilter[f]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      <SearchInput
        aria-label={copy.builds.searchLabel}
        value={search.q ?? ''}
        onChange={(e) => setSearch({ q: e.target.value || undefined })}
        placeholder={copy.builds.searchPlaceholder}
        className="w-full sm:ml-auto sm:w-64"
      />
    </div>
  )

  let body
  if (uploads.isError && !all.length)
    body = (
      <div className="p-2">
        <PanelError
          error={uploads.error}
          onRetry={() => void uploads.refetch()}
          what={copy.builds.what}
        />
      </div>
    )
  else if (uploads.isPending) body = <PageLoader label={copy.builds.loading} />
  else if (!all.length)
    body = (
      <EmptyState icon={Hammer} title={copy.builds.empty} className="border-0">
        {copy.builds.emptyHint}
      </EmptyState>
    )
  else if (!shown.length)
    body = (
      <EmptyState
        icon={SearchX}
        title={copy.builds.noMatch}
        className="border-0"
        action={
          <Button
            size="sm"
            variant="outline"
            className="pointer-coarse:min-h-11"
            onClick={() => setSearch({ status: undefined, q: undefined })}
          >
            {copy.builds.clearFilters}
          </Button>
        }
      >
        {copy.builds.noMatchHint}
      </EmptyState>
    )
  else
    body = (
      <Table className="text-sm" data-testid="mcp-builds-table">
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
          {shown.map((u) => {
            const building = bucketOf(u) === 'building'
            // `dataUpdatedAt` (TanStack Query's own fetch-resolved timestamp), never Date.now()
            // directly: a render must stay pure (react-hooks/purity).
            const end = building ? uploads.dataUpdatedAt : Date.parse(u.updated_at)
            return (
              <TableRow
                key={u.upload_id}
                data-testid="mcp-build-row"
                data-status={bucketOf(u)}
                className="max-md:grid max-md:grid-cols-[1fr_auto] max-md:gap-x-2 max-md:py-2"
              >
                <TableCell className="min-w-0 max-md:p-0">
                  {u.connector_id ? (
                    <Link
                      to="/mcp/$connectorId"
                      params={{ connectorId: u.connector_id }}
                      search={{}}
                      className="font-medium underline-offset-4 hover:underline pointer-coarse:inline-flex pointer-coarse:min-h-11 pointer-coarse:items-center"
                    >
                      {u.connector_name}
                    </Link>
                  ) : u.upload_info.upload_status === 'Failed' ? (
                    // Never existed as a real connector to delete — the attempted name, same as today.
                    <span className="font-medium">{u.connector_name}</span>
                  ) : (
                    // Succeeded, then deleted on purpose: say so plainly instead of the original name,
                    // exactly as the Agents Builds page shows "Deleted agent" rather than risk reading
                    // as still-present (no secondary line either — the name alone says it, as there).
                    <span className="font-medium text-muted-foreground">
                      {copy.builds.deletedConnectorName}
                    </span>
                  )}
                  {u.upload_info.error_detail ? (
                    <p
                      className="truncate text-xs text-muted-foreground"
                      data-testid="build-reason"
                    >
                      {u.upload_info.error_detail}
                    </p>
                  ) : !u.connector_id && u.upload_info.upload_status === 'Failed' ? (
                    <p className="truncate text-xs text-muted-foreground">
                      {copy.builds.deletedConnector}
                    </p>
                  ) : null}
                </TableCell>
                <TableCell className="max-md:p-0 max-md:text-right">
                  <BuildStatusBadge bucket={bucketOf(u)} />
                </TableCell>
                <TableCell className="text-muted-foreground max-md:col-span-2 max-md:inline max-md:p-0 max-md:text-xs">
                  {copy.buildSource[u.source]}
                </TableCell>
                <TableCell className="text-muted-foreground tabular-nums max-md:hidden">
                  {relTime(u.created_at, Math.max(end, Date.parse(u.created_at)))}
                </TableCell>
                <TableCell className="text-right text-muted-foreground tabular-nums max-md:hidden">
                  {fmtDuration(end - Date.parse(u.created_at))}
                </TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    )

  return (
    <div className="mx-auto flex w-full max-w-page flex-col gap-4">
      {header}
      {nothingYet ? (
        body
      ) : (
        <div data-tour="mcp-builds-panel" className="rounded-lg border border-border">
          {filters}
          {body}
        </div>
      )}
    </div>
  )
}
