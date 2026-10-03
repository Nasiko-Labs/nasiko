/**
 * The rail's Waiting view (v1c §5.9, DP3, ND-6, DS9, DS10): one row per chat that has a pending request,
 * the oldest first, with no date groups and no Load more. Line 1 is the chat's title (or "Untitled chat" when
 * its row isn't loaded); line 2 is the request's text as one plain line, its age and "N requests". Opening a
 * row goes to the chat with the request to focus. When a row goes (answered) while focus is in the list,
 * focus moves to the next row, or to the view control.
 */
import { Link } from '@tanstack/react-router'
import { Hourglass, RotateCw, SearchX } from 'lucide-react'
import { useEffect, useRef } from 'react'
import { EmptyState } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { relTime } from '@/features/agents/format'
import { ApiError } from '@/lib/api/client'
import { RAIL_EMPTY, RAIL_ROW } from './turnStyles'
import { copy, errorCopy } from '../copy'
import { requestPreview } from '../pending'
import type { ChatCarry } from '../search'
import type { Waiting } from '../waiting'
import { CopyText } from './turnParts'
import { WaitingPill } from './WaitingPill'

export function WaitingList({
  waiting,
  titleOf,
  filter,
  now,
  search,
  onNavigate,
  onEmptied,
}: {
  waiting: Waiting
  titleOf(sessionId: string): string | undefined
  filter: string
  now: number
  search: ChatCarry
  onNavigate?(): void
  /** Focus left the list because its last focused row went: the rail puts it on the view control. */
  onEmptied(): void
}) {
  const { match } = waiting
  const f = filter.trim().toLowerCase()
  const rows = match.chats.map((c) => ({
    ...c,
    title: titleOf(c.sessionId) || copy.untitledChat,
    preview: requestPreview(c.requests[0]),
  }))
  const shown = f
    ? rows.filter((r) => r.title.toLowerCase().includes(f) || r.preview.toLowerCase().includes(f))
    : rows
  const ids = shown.map((r) => r.sessionId).join('|')
  const refs = useRef(new Map<string, HTMLAnchorElement>())
  const focused = useRef<{ id: string; index: number } | null>(null)
  // A focused row that goes (its request answered) hands focus to the next row, else to the control (§5.9).
  useEffect(() => {
    const was = focused.current
    if (!was || shown.some((r) => r.sessionId === was.id)) return
    focused.current = null
    const next = shown[Math.min(was.index, shown.length - 1)]
    if (next) refs.current.get(next.sessionId)?.focus()
    else onEmptied()
  }, [ids]) // eslint-disable-line react-hooks/exhaustive-deps

  if (waiting.isPending)
    return (
      <div className="space-y-1" aria-busy>
        {[0, 1, 2].map((i) => (
          <Skeleton key={i} className="h-11 motion-reduce:animate-none" />
        ))}
      </div>
    )
  if (waiting.failedCold) {
    const status = waiting.error instanceof ApiError ? waiting.error.status : 'none'
    const body = waiting.error instanceof ApiError ? waiting.error.body : null
    const correlation =
      body && typeof body === 'object' && 'correlation_id' in body
        ? String((body as { correlation_id: unknown }).correlation_id)
        : 'none'
    const details = [
      `http: ${status}`,
      `correlation_id: ${correlation}`,
      'docs: docs/chat.md#errors-pending',
    ].join('\n')
    return (
      <div role="note" className="space-y-2 px-2 py-3 text-sm" data-testid="waiting-error">
        <p className="font-medium">{errorCopy.pendingFailed.problem}</p>
        <p className="text-xs text-muted-foreground">{errorCopy.pendingFailed.cause}</p>
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="outline"
            className="pointer-coarse:min-h-11"
            onClick={waiting.retry}
          >
            <RotateCw aria-hidden /> {copy.retry}
          </Button>
          <CopyText text={details} label={copy.copyDetails} variant="ghost" />
        </div>
      </div>
    )
  }
  return (
    <div className="space-y-2">
      {waiting.stale ? (
        <p className="px-2 text-xs text-muted-foreground" data-testid="waiting-stale">
          {copy.lastChecked(relTime(new Date(waiting.lastChecked).toISOString(), now))}
        </p>
      ) : null}
      {!rows.length ? (
        <EmptyState icon={Hourglass} title={copy.noWaiting} className={RAIL_EMPTY}>
          {copy.noWaitingHint}
        </EmptyState>
      ) : !shown.length ? (
        <EmptyState
          icon={SearchX}
          title={copy.noWaitingMatch(filter.trim())}
          className={RAIL_EMPTY}
        >
          {copy.noMatchHint}
        </EmptyState>
      ) : (
        <ul className="space-y-0.5" aria-label={copy.viewWaiting}>
          {shown.map((r, i) => {
            const first = r.requests[0]
            return (
              <li key={r.sessionId}>
                <Link
                  ref={(el) => {
                    if (el) refs.current.set(r.sessionId, el)
                    else refs.current.delete(r.sessionId)
                  }}
                  to="/chat/$sessionId"
                  params={{ sessionId: r.sessionId }}
                  search={search}
                  state={{ waitingRequest: first.id }}
                  onClick={onNavigate}
                  onFocus={() => {
                    focused.current = { id: r.sessionId, index: i }
                  }}
                  onBlur={() => {
                    if (focused.current?.id === r.sessionId) focused.current = null
                  }}
                  data-testid="waiting-row"
                  className={RAIL_ROW}
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm">{r.title}</span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {r.preview} ·{' '}
                      <span className="tabular-nums">
                        {relTime(
                          first.created_at,
                          Math.max(now, Date.parse(first.created_at) || now),
                        )}
                      </span>
                      {r.requests.length > 1 ? ` · ${copy.requestCount(r.requests.length)}` : ''}
                    </span>
                  </span>
                  <span
                    aria-hidden
                    className="flex size-4 shrink-0 items-center justify-center"
                    data-testid="row-indicator"
                  >
                    <WaitingPill n={r.requests.length} />
                  </span>
                  <span className="sr-only">, {copy.rowWaiting}</span>
                </Link>
              </li>
            )
          })}
        </ul>
      )}
      {match.outside ? (
        <p className="px-2 text-xs text-muted-foreground" data-testid="outside-chat">
          {copy.outsideChat(match.outside)}
        </p>
      ) : null}
      {match.dropped ? (
        <p className="px-2 text-xs text-muted-foreground" data-testid="superuser-dropped">
          {copy.superuserDropped}
        </p>
      ) : null}
    </div>
  )
}
