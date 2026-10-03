/**
 * The chat rail (v1c §5.3): a header row with Search and New chat icon buttons, loaded rows grouped by
 * date, each with its identity icon, title, identity · time and a reserved indicator slot, then Load
 * more. Titles are owner text and render as text only.
 */
import { Link } from '@tanstack/react-router'
import {
  AlertCircle,
  Loader2,
  MessageSquare,
  RotateCw,
  Search,
  SearchX,
  SquarePen,
  SquareTerminal,
  X,
} from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type Ref } from 'react'
import { EmptyState } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useAgentsDirectory } from '@/features/agents/api'
import { relTime } from '@/features/agents/format'
import { StateCard } from '@/features/observability/StateCard'
import { cn } from '@/lib/utils'
import { sessionRows, useChatSessions } from '../api'
import { announce } from '../announce'
import { chatSignals, useSignals } from '../registry'
import type { ChatCarry } from '../search'
import { rowOf, type SignalsSnapshot } from '../signals'
import type { Waiting } from '../waiting'
import { WaitingList } from './WaitingList'
import { WaitingPill } from './WaitingPill'
import { copy } from '../copy'
import { tuning } from '../tuning'
import { chatIdentity } from '../identity'
import { groupByDate, rowStates, rowTooltip, type DateGroup } from '../railGroups'
import { readRailView, rememberRailView, type RailView } from '../rememberTarget'
import { IdentityIcon } from './ChatIdentity'
import { RAIL_EMPTY, RAIL_ROW, RAIL_ROW_ACTIVE } from './turnStyles'

const GROUP_LABEL: Record<DateGroup, string> = {
  today: copy.groupToday,
  yesterday: copy.groupYesterday,
  week: copy.groupWeek,
  older: copy.groupOlder,
}

/** The filter takes focus when the search button opens it (a stable ref callback runs once, on mount). */
const focusOnMount = (el: HTMLInputElement | null) => {
  el?.focus()
}

export function ChatRail({
  activeId,
  onNavigate,
  search,
  username,
  userId,
  waiting,
  ref: newChatRef,
}: {
  activeId?: string
  onNavigate?(): void
  search: ChatCarry
  username?: string
  userId?: string
  waiting?: Waiting
  ref?: Ref<HTMLAnchorElement>
}) {
  const list = useChatSessions()
  const dir = useAgentsDirectory()
  const signals = useSignals()
  // Speak a list failure once per failed fetch (the page's announcer is the only live region).
  useEffect(() => {
    if (list.isError) announce(copy.chatsFailed)
  }, [list.isError, list.errorUpdatedAt])
  // Date groups move at midnight: re-read the clock every minute. A row newer than this clock (just saved)
  // reads as now, never "in 5 seconds".
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), tuning.RAIL_CLOCK_MS)
    return () => clearInterval(id)
  }, [])
  const [searchOpen, setSearchOpen] = useState(false)
  const [filter, setFilter] = useState('')
  const searchButton = useRef<HTMLButtonElement>(null)
  const viewsRef = useRef<HTMLDivElement>(null)
  const rows = useMemo(
    () =>
      sessionRows(list.data).map((r) => ({
        row: r,
        identity: chatIdentity(r, dir.byId, username),
      })),
    [list.data, dir.byId, username],
  )
  // Views (v1c D1): recorded chats sort by ingest time, so they get their own view and "Chats" leaves
  // them out. The chosen view is remembered per viewer (DX-T1).
  const [view, setViewState] = useState<RailView>(
    () => (userId ? readRailView(userId) : null) ?? 'chats',
  )
  const setView = (v: RailView) => {
    setViewState(v)
    if (userId) rememberRailView(userId, v)
  }
  const recorded = rows.filter((x) => x.identity.kind === 'recorded')
  const live = rows.filter((x) => x.identity.kind !== 'recorded')
  // Opening a chat of the other kind shows its view, so the active row is always in the list.
  // (Adjusted while rendering, once per opened chat, so the list never flashes the wrong view.)
  const [synced, setSynced] = useState<string | undefined>(undefined)
  const activeKind = rows.find((x) => x.row.session_id === activeId)?.identity.kind
  if (activeId && activeKind && synced !== activeId) {
    setSynced(activeId)
    // Only when the opened chat isn't in the current view: a chat opened from Waiting keeps Waiting.
    const inCurrent =
      view === 'recorded'
        ? activeKind === 'recorded'
        : view === 'waiting'
          ? (waiting?.countFor(activeId) ?? 0) > 0
          : activeKind !== 'recorded'
    if (!inCurrent) setViewState(activeKind === 'recorded' ? 'recorded' : 'chats')
  }
  const inView = view === 'recorded' ? recorded : live
  // quirk: §10.7 — no `q` on the list, so the filter covers loaded rows only.
  const shown = useMemo(() => {
    const f = filter.trim().toLowerCase()
    return f
      ? inView.filter(
          ({ row, identity }) =>
            row.title.toLowerCase().includes(f) ||
            identity.name.toLowerCase().includes(f) ||
            (row.agent_name ?? '').toLowerCase().includes(f),
        )
      : inView
  }, [inView, filter])
  const groups = groupByDate(
    shown.map((x) => ({ ...x, updated_at: x.row.updated_at, created_at: x.row.created_at })),
    now,
  )
  // Dev aids survive navigation between chats (DX-A2).
  const carry = { mock: search.mock, debug: search.debug }

  const closeSearch = () => {
    setSearchOpen(false)
    setFilter('')
    searchButton.current?.focus()
  }

  return (
    <nav aria-label={copy.chats} className="flex h-full min-h-0 flex-col">
      <div className="flex h-12 shrink-0 items-center gap-1 pr-2 pl-4">
        <h2 className="min-w-0 flex-1 truncate text-sm font-medium">{copy.chats}</h2>
        <Button
          ref={searchButton}
          size="icon-sm"
          variant="ghost"
          className="pointer-coarse:size-11"
          aria-label={copy.searchChats}
          title={copy.searchChats}
          aria-expanded={searchOpen}
          aria-controls="chat-rail-filter"
          onClick={() => (searchOpen ? closeSearch() : setSearchOpen(true))}
        >
          <Search aria-hidden />
        </Button>
        <Button asChild size="icon-sm" variant="ghost" className="pointer-coarse:size-11">
          <Link
            ref={newChatRef}
            to="/chat"
            search={carry}
            onClick={onNavigate}
            aria-label={copy.newChat}
            title={copy.newChat}
          >
            <SquarePen aria-hidden />
          </Link>
        </Button>
      </div>
      {searchOpen ? (
        <div className="shrink-0 px-3 pb-2">
          <Input
            id="chat-rail-filter"
            ref={focusOnMount}
            aria-label={copy.filterChats}
            placeholder={copy.filterChats}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && !filter) {
                e.preventDefault()
                e.stopPropagation()
                closeSearch()
              }
            }}
            className="h-8 pointer-coarse:h-11"
          />
        </div>
      ) : null}
      <RailViews
        ref={viewsRef}
        view={view}
        onChange={setView}
        recorded={recorded.length}
        more={!!list.hasNextPage}
        waiting={
          waiting ? { n: waiting.match.chats.length, failed: waiting.failedCold } : undefined
        }
      />
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
        {view === 'waiting' && waiting ? (
          <WaitingList
            waiting={waiting}
            titleOf={(id) => rows.find((x) => x.row.session_id === id)?.row.title}
            filter={filter}
            now={now}
            search={carry}
            onNavigate={onNavigate}
            onEmptied={() =>
              viewsRef.current?.querySelector<HTMLButtonElement>('[aria-checked="true"]')?.focus()
            }
          />
        ) : list.isPending ? (
          <div className="space-y-1" aria-busy>
            {[0, 1, 2, 3, 4].map((i) => (
              <Skeleton key={i} className="h-11 motion-reduce:animate-none" />
            ))}
          </div>
        ) : list.isError && !rows.length ? (
          <StateCard
            tone="error"
            title={copy.chatsFailed}
            action={
              <Button
                size="sm"
                variant="outline"
                className="pointer-coarse:min-h-11"
                onClick={() => void list.refetch()}
              >
                <RotateCw aria-hidden /> {copy.retry}
              </Button>
            }
          />
        ) : !rows.length || (view === 'chats' && !live.length && !list.hasNextPage) ? (
          <EmptyState icon={MessageSquare} title={copy.noChats} className={RAIL_EMPTY}>
            {copy.noChatsHint}
          </EmptyState>
        ) : view === 'recorded' && !recorded.length ? (
          <EmptyState icon={SquareTerminal} title={copy.noRecorded} className={RAIL_EMPTY}>
            {copy.noRecordedHint}
          </EmptyState>
        ) : view === 'chats' && !live.length ? (
          <EmptyState icon={MessageSquare} title={copy.noLiveLoaded} className={RAIL_EMPTY}>
            {copy.loadMoreToSearch}
          </EmptyState>
        ) : (
          <>
            {groups.map(({ group, rows: inGroup }) => (
              <section key={group} aria-labelledby={`rail-group-${group}`}>
                <h3
                  id={`rail-group-${group}`}
                  className="px-2 pt-3 pb-1 text-xs text-muted-foreground"
                >
                  {GROUP_LABEL[group]}
                </h3>
                <ul className="space-y-0.5">
                  {inGroup.map(({ row: r, identity }) => {
                    const active = r.session_id === activeId
                    const muted = identity.kind === 'removed'
                    const sig = rowOf(signals, r.session_id)
                    const waitingN = waiting?.countFor(r.session_id) ?? 0
                    const states = rowStates({ waiting: waitingN, ...sig })
                    return (
                      <li key={r.session_id}>
                        <Link
                          to="/chat/$sessionId"
                          params={{ sessionId: r.session_id }}
                          search={carry}
                          onClick={onNavigate}
                          title={rowTooltip(r)}
                          aria-current={active ? 'page' : undefined}
                          data-testid="rail-row"
                          className={cn(
                            RAIL_ROW,
                            active && RAIL_ROW_ACTIVE,
                            muted && !active && 'text-muted-foreground',
                          )}
                        >
                          <IdentityIcon kind={identity.kind} name={identity.name} size={20} />
                          <span className="min-w-0 flex-1">
                            <span className={cn('block truncate text-sm', active && 'font-medium')}>
                              {r.title || '—'}
                            </span>
                            {/* The name shrinks first; the time always stays whole. */}
                            <span
                              className={cn(
                                'flex min-w-0 text-xs',
                                active ? 'text-accent-foreground' : 'text-muted-foreground',
                              )}
                            >
                              <span className="min-w-0 truncate">{identity.railLabel}</span>
                              <span className="shrink-0 whitespace-pre">
                                {' '}
                                ·{' '}
                                <span className="tabular-nums">
                                  {relTime(
                                    r.updated_at,
                                    Math.max(now, Date.parse(r.updated_at ?? '') || now),
                                  )}
                                </span>
                              </span>
                            </span>
                          </span>
                          {/* The indicator slot (§5.3, §5.8): Waiting > in progress > failed > new reply, one mark each (DS2). */}
                          <span
                            aria-hidden
                            className="flex size-4 shrink-0 items-center justify-center"
                            data-testid="row-indicator"
                          >
                            {waitingN ? (
                              <WaitingPill n={waitingN} />
                            ) : sig.live ? (
                              <Loader2
                                className="size-3.5 animate-spin text-muted-foreground motion-reduce:animate-none"
                                data-mark="live"
                              />
                            ) : sig.failed ? (
                              <AlertCircle
                                className="size-3.5 text-destructive"
                                data-mark="failed"
                              />
                            ) : sig.unseen ? (
                              <span className="size-2 rounded-full bg-primary" data-mark="unseen" />
                            ) : null}
                          </span>
                          {states.length ? (
                            <span className="sr-only">, {states.join(', ')}</span>
                          ) : null}
                        </Link>
                      </li>
                    )
                  })}
                </ul>
              </section>
            ))}
            {filter && !shown.length ? (
              <EmptyState
                icon={SearchX}
                title={copy.noLoadedMatch(filter.trim())}
                className={RAIL_EMPTY}
              >
                {list.hasNextPage ? copy.loadMoreToSearch : copy.noMatchHint}
              </EmptyState>
            ) : null}
          </>
        )}
        {list.hasNextPage && view !== 'waiting' ? (
          <Button
            size="sm"
            variant="ghost"
            className="mt-2 w-full pointer-coarse:min-h-11"
            disabled={list.isFetchingNextPage}
            onClick={() => void list.fetchNextPage()}
          >
            {list.isFetchingNextPage ? copy.loading : copy.loadMore}
          </Button>
        ) : null}
        {list.isError && rows.length ? (
          <p className="mt-2 px-1 text-xs text-destructive">
            {copy.chatsFailed}{' '}
            <Button
              variant="link"
              className="h-auto p-0 text-xs font-normal text-destructive underline pointer-coarse:min-h-11 pointer-coarse:px-2"
              onClick={() => void list.refetch()}
            >
              {copy.retry}
            </Button>
          </p>
        ) : null}
      </div>
      <ReplyReady
        signals={signals}
        titleOf={(id) => rows.find((x) => x.row.session_id === id)?.row.title}
        search={carry}
        onNavigate={onNavigate}
      />
    </nav>
  )
}

/**
 * The rail's views (v1c §5.7, §5.9 control): Chats, Waiting and Recorded, always all three, so users learn that
 * Waiting exists before anything waits (user decision, 2026-09-28, superseding DS1's "only when a count is above
 * 0"). An empty view says so. Arrow keys switch (the radiogroup is shadcn `ToggleGroup type="single"`). Counts cover loaded rows and read "n+" while more pages exist
 * (DS12). Waiting is absent only where the rail has no pending query (it always has one in ChatLayout).
 */
function RailViews({
  view,
  onChange,
  recorded,
  more,
  waiting,
  ref,
}: {
  view: RailView
  onChange(v: RailView): void
  recorded: number
  more: boolean
  waiting?: { n: number; failed: boolean }
  ref?: Ref<HTMLDivElement>
}) {
  const refs = useRef(new Map<RailView, HTMLButtonElement>())
  const focusedCounted = useRef<RailView | null>(null)
  const options: { id: RailView; label: string; pill?: number; count?: string }[] = [
    { id: 'chats', label: copy.viewChats },
  ]
  // Waiting carries its count as a pill; with none (or before the first poll) it reads plain "Waiting".
  if (waiting)
    options.push({
      id: 'waiting',
      label:
        waiting.n > 0 ? copy.viewWaitingCount(`${waiting.n}${more ? '+' : ''}`) : copy.viewWaiting,
      pill: waiting.n || undefined,
    })
  options.push({
    id: 'recorded',
    label:
      recorded > 0 ? copy.viewRecorded(`${recorded}${more ? '+' : ''}`) : copy.viewRecordedShort,
    count: recorded > 0 ? `${recorded}${more ? '+' : ''}` : undefined,
  })
  const shownIds = options.map((o) => o.id).join('|')
  useEffect(() => {
    const was = focusedCounted.current
    if (was && !options.some((o) => o.id === was)) {
      focusedCounted.current = null
      refs.current.get(view)?.focus()
    }
  }, [shownIds, view]) // eslint-disable-line react-hooks/exhaustive-deps
  if (options.length < 2) return null
  const move = (delta: number) => {
    const i = options.findIndex((o) => o.id === view)
    const next = options[(i + delta + options.length) % options.length]
    onChange(next.id)
    refs.current.get(next.id)?.focus()
  }
  // ToggleGroup `single` is a radiogroup of radios; its own roving focus is off so arrows keep
  // selecting as they move (radio behaviour), with the checked view as the one tab stop.
  return (
    <ToggleGroup
      ref={ref}
      type="single"
      rovingFocus={false}
      spacing={0.5}
      value={view}
      onValueChange={(v) => {
        if (v) onChange(v as RailView)
      }}
      aria-label={copy.railViews}
      className="mx-3 mb-2 flex w-auto shrink-0 rounded-md border border-border p-0.5"
      onKeyDown={(e) => {
        if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
          e.preventDefault()
          move(1)
        }
        if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
          e.preventDefault()
          move(-1)
        }
      }}
    >
      {options.map((o) => {
        const checked = o.id === view
        return (
          <ToggleGroupItem
            key={o.id}
            value={o.id}
            ref={(el) => {
              if (el) refs.current.set(o.id, el)
              else refs.current.delete(o.id)
            }}
            tabIndex={checked ? 0 : -1}
            aria-label={o.label}
            onFocus={() => {
              focusedCounted.current = o.id === 'chats' ? null : o.id
            }}
            onBlur={() => {
              if (focusedCounted.current === o.id) focusedCounted.current = null
            }}
            className="h-auto min-h-7 min-w-0 flex-auto gap-1 rounded px-1.5 text-xs font-normal text-muted-foreground hover:bg-transparent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background data-[state=on]:bg-muted data-[state=on]:font-medium data-[state=on]:text-foreground pointer-coarse:min-h-11"
          >
            {/* Three views fit 240 px: counts are small numbers, the full "(n)" form is the accessible name. */}
            {o.pill ? (
              <>
                {copy.viewWaiting}
                <WaitingPill n={o.pill} />
              </>
            ) : o.count ? (
              <>
                {copy.viewRecordedShort}
                <span className="text-xs text-muted-foreground tabular-nums">{o.count}</span>
              </>
            ) : (
              o.label
            )}
          </ToggleGroupItem>
        )
      })}
    </ToggleGroup>
  )
}

/**
 * "Reply ready in <title>" (v1c §5.8, S4, ND-13): the newest unseen reply, pinned to the rail's footer so the
 * list never moves (DS1). It goes when the chat opens, on dismiss, or after REPLY_READY_MS of visible time
 * (DS4). Announced once.
 */
function ReplyReady({
  signals,
  titleOf,
  search,
  onNavigate,
}: {
  signals: SignalsSnapshot | null
  titleOf(id: string): string | undefined
  search: ChatCarry
  onNavigate?(): void
}) {
  const ready = signals?.replyReady ?? null
  const title = ready ? titleOf(ready.sessionId) || copy.untitledChat : ''
  const attempt = ready?.attempt
  useEffect(() => {
    const api = chatSignals()
    if (!api || !attempt) return
    api.replyReadyShown(attempt)
    if (api.firstNotice(attempt)) announce(copy.announceReplyReady(title))
  }, [attempt]) // eslint-disable-line react-hooks/exhaustive-deps -- once per attempt; the title is read as it stands then
  if (!ready) return null
  return (
    <div
      className="flex shrink-0 items-center gap-1 border-t border-border py-1.5 pr-1 pl-4 text-xs"
      data-testid="reply-ready"
    >
      <Link
        to="/chat/$sessionId"
        params={{ sessionId: ready.sessionId }}
        search={search}
        onClick={onNavigate}
        title={`${copy.replyReadyIn} ${title}`}
        className="min-w-0 flex-1 truncate text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11 pointer-coarse:leading-11"
      >
        {copy.replyReadyIn} {title}
      </Link>
      <Button
        size="icon-sm"
        variant="ghost"
        className="shrink-0 pointer-coarse:size-11"
        aria-label={copy.dismissReplyReady}
        onClick={() => chatSignals()?.dismissReplyReady()}
      >
        <X aria-hidden />
      </Button>
    </div>
  )
}
