/**
 * View trace (plan §7.5, E1, EN16): a read-only waterfall of one reply's trace in a side sheet.
 * A fresh reply's trace may still be arriving in Tempo, so a 404 retries at 2, 5, 10 and 20 s
 * before "Trace not in yet". 503 means Tempo isn't configured; 403 stops.
 */
import { Link } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { flattenSpans, traceDurationMs, type FlatSpan } from '@/features/observability/spans'
import { ErrorState, StateCard } from '@/features/observability/StateCard'
import { useTraceDetail } from '@/features/trace/api'
import { Legend, Waterfall } from '@/features/trace/Waterfall'
import { ApiError } from '@/lib/api/client'
import { copy } from '../copy'

/** Seconds after the first 404 at which to look again. */
const NOT_FOUND_RETRIES_S = [2, 5, 10, 20]

export interface TraceTarget {
  traceId: string
  /** The turn is live or the reply is younger than TRACE_FRESH_MS. */
  fresh: boolean
}

export function TraceSheet({
  target,
  sessionId,
  onClose,
}: {
  target: TraceTarget | null
  sessionId: string
  onClose(): void
}) {
  return (
    <Sheet open={!!target} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="w-full gap-0 overflow-y-auto sm:max-w-2xl">
        <SheetHeader>
          <SheetTitle>{copy.traceTitle}</SheetTitle>
          <SheetDescription className="font-mono text-xs">
            {target?.traceId.slice(0, 16)}
          </SheetDescription>
        </SheetHeader>
        {target ? <TraceBody key={target.traceId} target={target} sessionId={sessionId} /> : null}
      </SheetContent>
    </Sheet>
  )
}

function TraceBody({ target, sessionId }: { target: TraceTarget; sessionId: string }) {
  const trace = useTraceDetail(target.traceId, target.fresh)
  const [misses, setMisses] = useState(0)
  const notFound = trace.error instanceof ApiError && trace.error.status === 404
  const gaveUp = notFound && misses >= NOT_FOUND_RETRIES_S.length
  const refetch = trace.refetch

  // Schedule the next look after each 404, until the retries run out.
  useEffect(() => {
    if (!notFound || misses >= NOT_FOUND_RETRIES_S.length || !trace.errorUpdatedAt) return
    const prev = misses ? NOT_FOUND_RETRIES_S[misses - 1] : 0
    const id = setTimeout(
      () => {
        setMisses((m) => m + 1)
        void refetch()
      },
      (NOT_FOUND_RETRIES_S[misses] - prev) * 1000,
    )
    return () => clearTimeout(id)
  }, [notFound, misses, trace.errorUpdatedAt, refetch])

  const spans = useMemo(() => (trace.data ? flattenSpans(trace.data) : []), [trace.data])
  const [selected, setSelected] = useState<FlatSpan>()
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set())
  const toggle =
    (set: (f: (s: ReadonlySet<string>) => ReadonlySet<string>) => void) => (key: string) =>
      set((s) => {
        const next = new Set(s)
        if (!next.delete(key)) next.add(key)
        return next
      })

  const fullTrace = (
    <Button asChild size="sm" variant="outline" className="pointer-coarse:min-h-11">
      <Link to="/sessions/$sessionId" params={{ sessionId }} search={{ trace: target.traceId }}>
        {copy.openFullTrace}
      </Link>
    </Button>
  )

  let body
  if (trace.data) {
    body = (
      <div className="space-y-3">
        <Legend />
        <Waterfall
          spans={spans}
          total={traceDurationMs(spans)}
          selectedId={selected?.node.id}
          onSelect={setSelected}
          expanded={expanded}
          collapsed={collapsed}
          onToggleGroup={toggle(setExpanded)}
          onToggleNode={toggle(setCollapsed)}
          highlight={new Set()}
        />
        {trace.isFetching ? (
          <p className="text-xs text-muted-foreground">{copy.traceStillArriving}</p>
        ) : null}
      </div>
    )
  } else if (trace.isPending || (notFound && !gaveUp)) {
    body = (
      <div className="space-y-2" aria-busy>
        {notFound ? <p className="text-sm text-muted-foreground">{copy.traceWaiting}</p> : null}
        <Skeleton className="h-6" />
        <Skeleton className="h-6 w-5/6" />
        <Skeleton className="h-6 w-2/3" />
      </div>
    )
  } else if (gaveUp) {
    body = (
      <StateCard
        title={copy.traceNotInYet}
        fix={copy.traceNotInYetFix}
        action={
          <Button
            size="sm"
            variant="outline"
            className="pointer-coarse:min-h-11"
            onClick={() => {
              setMisses(0)
              void refetch()
            }}
          >
            {copy.retry}
          </Button>
        }
      />
    )
  } else if (trace.error instanceof ApiError && trace.error.status === 403) {
    body = <StateCard tone="warning" title={copy.traceForbidden} />
  } else {
    body = <ErrorState error={trace.error} onRetry={() => void refetch()} />
  }

  return (
    <div className="space-y-4 px-4 pb-6">
      {body}
      {fullTrace}
    </div>
  )
}
