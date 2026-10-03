/**
 * Per-agent log tail. The stream itself is `useLogStream` (sessions/api.ts).
 * - The server replays the last lines on every connect: lines are deduped by timestamp +
 *   message, so Reconnect never doubles them.
 * - Pause stops rendering new lines (they buffer); "Jump to latest" scrolls to the end.
 * - The server ends streams after an hour (`event: close`): shown as "Stream paused" with a
 *   Reconnect button and no automatic retry loop.
 * - 404 = deleted or not visible; 401 re-checks the session (the app redirects to login).
 */
import { Pause, Play, RotateCw } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { copy } from '@/features/observability/copy'
import { LOG_LINES_MAX } from '@/features/observability/tuning'
import type { LogLine } from '@/features/observability/types'
import { fmtUtcTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import { useLogStream } from './api'

const lineKey = (l: LogLine) => `${l.timestamp}|${l.message}`

export function LogDrawer({
  agent,
  label,
  open,
  onOpenChange,
}: {
  agent: string
  label: string
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const [lines, setLines] = useState<LogLine[]>([])
  const [paused, setPaused] = useState(false)
  const [bufferedCount, setBufferedCount] = useState(0)
  const buffered = useRef<LogLine[]>([])
  const seen = useRef(new Set<string>())
  const pausedRef = useRef(paused)
  const scroller = useRef<HTMLDivElement>(null)
  useEffect(() => {
    pausedRef.current = paused
  }, [paused])

  const push = useCallback((incoming: LogLine[]) => {
    const fresh = incoming.filter((l) => {
      const k = lineKey(l)
      if (seen.current.has(k)) return false
      seen.current.add(k)
      return true
    })
    // Keep the dedupe set bounded: replays only repeat recent lines, so drop the oldest keys.
    for (const k of seen.current) {
      if (seen.current.size <= LOG_LINES_MAX * 2) break
      seen.current.delete(k)
    }
    if (!fresh.length) return
    if (pausedRef.current) {
      buffered.current = [...buffered.current, ...fresh].slice(-LOG_LINES_MAX)
      setBufferedCount(buffered.current.length)
    } else setLines((prev) => [...prev, ...fresh].slice(-LOG_LINES_MAX))
  }, [])

  const { state, reconnect } = useLogStream(agent, open, push)

  const resume = () => {
    setPaused(false)
    setLines((prev) => [...prev, ...buffered.current].slice(-LOG_LINES_MAX))
    buffered.current = []
    setBufferedCount(0)
  }
  const jump = () => scroller.current?.scrollTo?.({ top: scroller.current.scrollHeight })

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="right" className="flex w-full flex-col gap-3 p-4 sm:max-w-xl">
        <SheetTitle>Logs · {label}</SheetTitle>
        <SheetDescription className="sr-only">Live log tail for {label}</SheetDescription>
        <div className="flex flex-wrap items-center gap-2">
          {paused ? (
            <Button size="sm" variant="default" onClick={resume}>
              <Play className="size-3.5" aria-hidden /> Resume
              {bufferedCount ? ` (${bufferedCount})` : ''}
            </Button>
          ) : (
            <Button size="sm" variant="outline" onClick={() => setPaused(true)}>
              <Pause className="size-3.5" aria-hidden /> Pause
            </Button>
          )}
          <Button size="sm" variant="ghost" onClick={jump}>
            Jump to latest
          </Button>
          <span role="status" className="text-xs text-muted-foreground">
            {state === 'connecting'
              ? 'Connecting…'
              : state === 'open'
                ? 'Streaming'
                : state === 'closed'
                  ? copy.streamPaused
                  : state === 'unavailable'
                    ? ''
                    : 'Stream error'}
          </span>
          {state === 'closed' || state === 'error' ? (
            <Button size="sm" variant="outline" onClick={reconnect}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.reconnect}
            </Button>
          ) : null}
        </div>
        {state === 'unavailable' ? (
          <p className="text-sm text-muted-foreground">{copy.logsUnavailable}</p>
        ) : lines.length === 0 && state !== 'connecting' ? (
          <p className="text-sm text-muted-foreground">{copy.logsEmpty}</p>
        ) : (
          <div
            ref={scroller}
            className="min-h-0 flex-1 overflow-auto rounded-md border border-border bg-muted/40 p-2 font-mono text-xs"
            aria-label="Log lines"
            role="log"
          >
            {lines.map((l) => (
              <div key={lineKey(l)} className="flex gap-2 py-0.5">
                <span className="shrink-0 text-muted-foreground tabular-nums">
                  {fmtUtcTime(l.timestamp, true)}
                </span>
                <span
                  className={cn(
                    'w-10 shrink-0',
                    l.level === 'ERROR'
                      ? 'text-destructive'
                      : l.level === 'WARN'
                        ? 'text-warning'
                        : 'text-muted-foreground',
                  )}
                >
                  {l.level ?? ''}
                </span>
                <span className="min-w-0 break-words">{l.message}</span>
              </div>
            ))}
          </div>
        )}
      </SheetContent>
    </Sheet>
  )
}
