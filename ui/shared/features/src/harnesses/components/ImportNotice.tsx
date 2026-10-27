/**
 * Monitor while coding agents are being brought in. The desktop app's own run (the card's Yes, find my agents) and its
 * queue, or in a browser a run that `nasiko agents setup` reports to the server (re-read on focus, as the viewer comes
 * back from the terminal).
 *
 * - `ImportNotice`: a note above the page's data while an import run is active (not for a background delivery of
 *   hook turns: the data is there, and the turns are small).
 * - `ImportingOr`: in place of the page's empty state while anything syncs (import or delivery), the loading state:
 *   their spend is on its way, so "nothing recorded yet" would be wrong.
 */
import { Link } from '@tanstack/react-router'
import { Loader2 } from 'lucide-react'
import { useEffect, useRef, type ReactNode } from 'react'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { Alert, AlertDescription, AlertTitle } from '@nasiko/ui/primitives/alert'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@nasiko/ui/primitives/empty'
import { asButton } from '@nasiko/ui/primitives/button'
import { Progress } from '@nasiko/ui/primitives/progress'
import { cn } from '@nasiko/ui/lib/utils'
import { copy as pageCopy } from '../copy'
import { IMPORT_RETRY_MS, SYNC_POLL_MS } from '../constants'
import { useDesktopState } from '../desktop'
import { useSyncRuns } from '../onboarding'
import { isActivePhase, shownPhase, type LocalRun } from '../runs'
import { desktopSync, syncDetail, syncLine, type DesktopSync } from '../syncStatus'

const copy = pageCopy.importNotice

const importing = (run: LocalRun | null | undefined) => !!run && isActivePhase(shownPhase(run))

/** Where the viewer's coding-agent sync is: `pending` until the desktop probe (and in a browser the runs) answer. */
function useCodingAgentSync(): { pending: boolean; importing: boolean; sync: DesktopSync } {
  const desktop = useDesktopState()
  const browser = desktop.isSuccess && !desktop.data
  const runs = useSyncRuns({
    enabled: browser,
    interval: (r) => (r?.some(importing) ? SYNC_POLL_MS : false),
  })
  // ponytail: one device's run in a browser (the first importing); per-device lines live on Harnesses' card.
  const source = desktop.data ?? { run: runs.data?.find(importing) ?? null, backlog: null }
  return {
    pending: desktop.isPending || (browser && runs.isPending),
    importing: importing(source.run),
    sync: desktopSync(source),
  }
}

export function ImportNotice() {
  const { importing: active, sync } = useCodingAgentSync()
  if (!active) return null
  return (
    // A note, not an announcement: it stays on screen while the import runs.
    <Alert role="note" data-testid="import-notice" className="p-3">
      <Loader2 aria-hidden className="animate-spin motion-reduce:animate-none" />
      <AlertTitle>{copy.title}</AlertTitle>
      <AlertDescription className="flex flex-col gap-2">
        <p className="max-w-prose">{copy.body}</p>
        <Progress
          aria-label={copy.label}
          value={sync.value}
          aria-valuenow={sync.value ?? undefined}
          aria-valuetext={sync.value === null ? undefined : `${sync.value}%`}
          className={cn(
            'h-1 max-w-md motion-reduce:*:transition-none',
            sync.value === null && 'animate-pulse motion-reduce:animate-none',
          )}
        />
        <div className="flex flex-wrap items-center gap-x-3 text-xs">
          <span>{syncLine(sync)}</span>
          <Link
            to="/harnesses"
            {...asButton({ variant: 'link', size: 'sm', className: 'h-auto p-0 text-xs' })}
          >
            {copy.details}
          </Link>
        </div>
      </AlertDescription>
    </Alert>
  )
}

/**
 * A page's empty state, or while the viewer's coding agents sync, the loading state that replaces it: the empty
 * state's own frame (the same card, icon tile and type), with a spinner in the tile, what is happening, how far the
 * sync has got, and the way to the details. Until the sync state is known the page loader alone shows, so the empty
 * state never flashes before it.
 *
 * `body` replaces the loader's line for a page waiting on one item rather than on spend. `retry` (a stable function,
 * such as a query's `refetch`) runs every `IMPORT_RETRY_MS` while the sync goes and once when it ends, so that page
 * opens on its own the moment its item lands.
 */
export function ImportingOr({
  children,
  body = copy.loaderBody,
  retry,
}: {
  children: ReactNode
  body?: string
  retry?: () => unknown
}) {
  const { pending, sync } = useCodingAgentSync()
  const wasSyncing = useRef(false)
  useEffect(() => {
    if (!retry) return
    if (wasSyncing.current && !sync.syncing) void retry()
    wasSyncing.current = sync.syncing
    if (!sync.syncing) return
    const timer = setInterval(() => void retry(), IMPORT_RETRY_MS)
    return () => clearInterval(timer)
  }, [sync.syncing, retry])
  if (pending) return <PageLoader label={copy.title} />
  if (!sync.syncing) return children
  const detail = syncDetail(sync)
  return (
    <Empty
      aria-busy="true"
      data-testid="importing-loader"
      className="gap-4 border border-border px-4 py-12 md:p-8 md:py-14"
    >
      <EmptyHeader className="gap-1">
        <EmptyMedia variant="icon" className="mb-2 text-muted-foreground">
          <Loader2 aria-hidden className="size-5 motion-safe:animate-spin" />
        </EmptyMedia>
        <EmptyTitle className="text-base">
          <h2>{copy.title}</h2>
        </EmptyTitle>
        <EmptyDescription className="text-sm text-pretty">{body}</EmptyDescription>
      </EmptyHeader>
      <EmptyContent className="w-full max-w-xs gap-3">
        <div className="flex w-full flex-col gap-1.5">
          <Progress
            aria-label={copy.label}
            value={sync.value}
            aria-valuenow={sync.value ?? undefined}
            aria-valuetext={sync.value === null ? undefined : `${sync.value}%`}
            className={cn(
              'h-1.5 motion-reduce:*:transition-none',
              sync.value === null && 'animate-pulse motion-reduce:animate-none',
            )}
          />
          {detail ? <p className="text-xs text-muted-foreground tabular-nums">{detail}</p> : null}
        </div>
        <Link to="/harnesses" {...asButton({ variant: 'outline', size: 'sm' })}>
          {copy.details}
        </Link>
      </EmptyContent>
    </Empty>
  )
}
