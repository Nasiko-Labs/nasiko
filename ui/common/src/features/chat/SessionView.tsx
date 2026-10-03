/**
 * `/chat/$sessionId` (plan §7, v1b §5, v1c): history plus the live turn, rendered as turns; every action goes to
 * the registry or a mutation. Reconciling a finished turn with history is `reconcile.ts`, dispatched by
 * `useReconcile`.
 */
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { PageLoader } from '@/components/shared/page-loader'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { useAgentsDirectory } from '@/features/agents/api'
import { StateCard } from '@/features/observability/StateCard'
import { ApiError } from '@/lib/api/client'
import { downloadText } from '@/lib/download'
import {
  historyPages,
  sessionRows,
  useCancelRequest,
  useChatHistory,
  useChatSessions,
  useDeleteChat,
  useResolveRequest,
} from './api'
import { Composer, type ComposerHandle, type ComposerMode } from './components/Composer'
import { StatusAnnouncer } from './components/StatusAnnouncer'
import { Transcript } from './components/Transcript'
import { TraceSheet, type TraceTarget } from './components/TraceSheet'
import { TurnView, type TurnHandlers } from './components/TurnView'
import { copy } from './copy'
import { readDraft } from './drafts'
import { announce } from './announce'
import { focusIfIdle } from './focus'
import { needsRunAgainConfirm } from './errors'
import { agentLabel, exportMarkdown } from './format'
import {
  chatSignals,
  useLiveTurn,
  invalidateChat,
  useSavedSteps,
  useTitlePrefix,
  useTurnEnds,
} from './registry'
import { useSessionLookup } from './sessionLookup'
import { CHAT_COLUMN, TOUCH } from './components/turnStyles'
import { chatIdentity, targetIdentity } from './identity'
import { indexRequests } from './pending'
import { isMetadataOnly } from './recorded'
import { resolveChatTarget, type Target } from './target'
import { answeredResume, firstSeenAnswer, mergeTurns, replyStatus } from './turnModel'
import { endOf, isLivePhase, sawTerminal, stepsOf, type SendInput } from './turnRegistry'
import { tuning } from './tuning'
import type { HitlDto } from './types'
import { useDraft, useNow } from './hooks'
import { directoryOf } from './target'
import { sendErrorText } from './errors'
import {
  ConfirmDialog,
  MetadataOnly,
  NotRunningBanner,
  ReadOnlyNote,
  SendError,
  type ViewProps,
} from './components/PageParts'
import { DebugFrames, RoutedMetricLine } from './components/DebugPanels'
import { SessionHeader } from './components/SessionHeader'
import { ownTurn } from './reconcile'
import { useReconcile } from './useReconcile'

/** Rail pages searched for an existing chat's row; after that the chat reads as "agent removed" (§6.11). */
// quirk: §10.7 — no GET /chat/sessions/{id}: an old chat's agent is found by paging the list.
const ROW_SEARCH_PAGES = 5

/**
 * Replies each turn's message had when this tab first saw the turn, by `turnId:startedAt`. Written once per key
 * during render, so a compiled (memoised) read of it always returns the same value.
 */
const replyBaselines = new Map<string, number>()
const REPLY_BASELINES_MAX = 100
function replyBaseline(turnKey: string, replies: number): number {
  const known = replyBaselines.get(turnKey)
  if (known !== undefined) return known
  replyBaselines.set(turnKey, replies)
  if (replyBaselines.size > REPLY_BASELINES_MAX) {
    const oldest = replyBaselines.keys().next().value
    if (oldest !== undefined) replyBaselines.delete(oldest)
  }
  return replies
}

function SessionView({
  sessionId,
  userId,
  registry,
  search,
  railButton,
  newChatRef,
  username,
}: ViewProps & { sessionId: string }) {
  const client = useQueryClient()
  const navigate = useNavigate()
  const dir = useAgentsDirectory()
  const list = useChatSessions()
  const rows = sessionRows(list.data)
  const railRow = rows.find((r) => r.session_id === sessionId)
  const live = useLiveTurn(registry, sessionId)
  const ends = useTurnEnds(registry)
  const savedSteps = useSavedSteps(registry)
  const endFor = (userMessageId: string) => endOf(ends, sessionId, userMessageId)
  const stepsFor = (messageId: string, traceId?: string | null) =>
    stepsOf(savedSteps, messageId, traceId)
  const history = useChatHistory(sessionId)
  const pages = historyPages(history.data)
  const requests = useMemo(() => pages.flatMap((p) => p.hitl), [pages])
  // The Waiting queue places tool requests only through this chat's history (EN10, v1c §5.9).
  const requestIds = requests.map((r) => r.id).join('|')
  useEffect(() => {
    if (requestIds) indexRequests(userId, sessionId, requestIds.split('|'))
  }, [userId, sessionId, requestIds])
  // Opened from a Waiting row: focus that request's card (E5), or say it's gone.
  const waitingRequest = useRouterState({ select: (s) => s.location.state.waitingRequest })
  const navKey = useRouterState({ select: (s) => s.location.state.key ?? s.location.href })
  const [focusRequestId, setFocusRequestId] = useState<string | undefined>()
  const [requestGone, setRequestGone] = useState(false)
  const [handledNav, setHandledNav] = useState<string | null>(null)
  // Adjusted while rendering, once per navigation, as soon as history has loaded.
  if (waitingRequest && !history.isPending && handledNav !== navKey) {
    setHandledNav(navKey)
    const pendingNow = requests.some((r) => r.id === waitingRequest && r.status === 'pending')
    setRequestGone(!pendingNow)
    setFocusRequestId(pendingNow ? waitingRequest : undefined)
  } else if (!waitingRequest && handledNav !== null && handledNav !== navKey) {
    // Any later navigation in this chat that isn't from Waiting clears the note.
    setHandledNav(null)
    setRequestGone(false)
    setFocusRequestId(undefined)
  }
  // A request that's gone: the note says so and focus goes to the transcript (§5.9).
  useEffect(() => {
    if (!requestGone) return
    const id = requestAnimationFrame(() => document.getElementById('chat-transcript')?.focus())
    return () => cancelAnimationFrame(id)
  }, [requestGone, handledNav])
  useEffect(() => {
    if (!focusRequestId) return
    const id = requestAnimationFrame(() => {
      const el = document.getElementById(`request-${focusRequestId}`)
      el?.scrollIntoView({ block: 'center', behavior: 'smooth' })
      el?.focus()
    })
    return () => cancelAnimationFrame(id)
  }, [focusRequestId, navKey])
  const pagesSearched = list.data?.pages.length ?? 0
  const railDone = !list.isPending && (!list.hasNextPage || pagesSearched >= ROW_SEARCH_PAGES)
  // Past the rail's pages the chat's row is looked up on its own, up to LOOKUP_PAGES (EN-6).
  const needLookup = !railRow && railDone && !!list.hasNextPage
  // Start after the rail's own pages: they didn't have it.
  const lookup = useSessionLookup(
    sessionId,
    needLookup,
    list.data?.pages.at(-1)?.next_cursor ?? undefined,
  )
  const row = railRow ?? (lookup.data?.status === 'found' ? lookup.data.row : undefined)
  const capped = needLookup && (lookup.data?.status === 'capped' || lookup.isError)
  const listDone = railDone && (!needLookup || lookup.data?.status === 'absent')
  const target: Target = resolveChatTarget(row, requests, listDone, directoryOf(dir))
  const agent = target.kind === 'direct' ? target.agent : undefined
  const routed = target.kind === 'routed'
  // Inside sentences ("Waiting for …", "Reply from … complete"): the router takes the article (v1c §5.1).
  const agentName = agent
    ? agentLabel(agent)
    : routed
      ? copy.theOrchestrator
      : (row?.agent_name ?? copy.theAgent)
  const identity = row
    ? chatIdentity(row, dir.byId, username)
    : agent
      ? targetIdentity({ kind: 'direct', agent })
      : routed
        ? targetIdentity({ kind: 'routed' })
        : null
  const liveNow = !!live && isLivePhase(live.phase)
  // For the reply-status and resume windows only (a live turn's elapsed time ticks in its own line). Ticking each
  // second while live keeps `now` fresh when the turn ends; nothing below re-renders on it (stable props).
  const now = useNow(liveNow ? 1000 : 15_000)
  const turns = useMemo(() => mergeTurns(pages, live), [pages, live])
  const lastUserId = [...turns].reverse().find((t) => t.user)?.user?.id
  // EN-5: this tab saw the newest attempt end empty, so there's no "checking" or E5 for it.
  const knownEmpty = !live && !!lastUserId && endFor(lastUserId)?.kind === 'empty'
  const status = live || knownEmpty ? null : replyStatus(turns, now, tuning.LOST_REPLY_AFTER_MS)
  const pending = requests.filter((r) => r.status === 'pending')
  const paused = pending.length > 0 || live?.phase === 'paused'
  // The agent a resumed routed request came from: a chained pause's card and announcement name it.
  const resumedExec =
    routed && live?.resumedRequestId
      ? requests.find((r) => r.id === live.resumedRequestId)?.execution.agent_id
      : undefined
  const resumedAgent = resumedExec ? dir.data?.find((a) => a.id === resumedExec) : undefined
  const resumedAsker = resumedAgent ? agentLabel(resumedAgent) : undefined
  // An answered routed request with no reply saved after it (ship review D3): Refresh status only,
  // never Run again (it would repeat the approved step), and the composer waits while one can land.
  const lastTurn = turns[turns.length - 1]
  // This tab saw the attempt finish (its stream's terminal, with or without text): nothing more is
  // coming for it, so a reply history never shows won't hold the composer. Not an "empty" end on
  // its own: a resume cut right after the sub-agent's COMPLETED also reads as one.
  const lastEnd = lastTurn?.user ? endFor(lastTurn.user.id) : undefined
  const seenEnded =
    (!!live && !liveNow && sawTerminal(live.state, live.operation === 'resume')) ||
    !!lastEnd?.terminal
  // Memoised so handlers that read it stay stable across the page's clock ticks.
  const resume = useMemo(
    () =>
      routed && !liveNow
        ? answeredResume(lastTurn, now, { seenEnded, firstSeen: firstSeenAnswer })
        : null,
    [routed, liveNow, lastTurn, now, seenEnded],
  )
  const [draft, setDraft] = useDraft(userId, sessionId)
  // This chat is open (v1c E10): it's seen, and its finishes earn no dot or Reply ready. The mount token
  // keeps /chat/a → /chat/b (new mount before old cleanup) and StrictMode from clearing the wrong chat.
  useEffect(() => {
    const signals = chatSignals()
    if (!signals) return
    const token = signals.setOpenChat(sessionId)
    return () => signals.releaseOpenChat(token)
  }, [sessionId])
  const titlePrefix = useTitlePrefix()
  const [sendError, setSendError] = useState<unknown>(null)
  const [confirm, setConfirm] = useState<null | 'send' | 'runAgain' | 'delete'>(null)
  const composer = useRef<ComposerHandle>(null)
  const { mutateAsync: resolveRequest, isPending: resolving } = useResolveRequest(sessionId)
  const { mutateAsync: cancelRequest, isPending: canceling } = useCancelRequest(sessionId)
  const del = useDeleteChat(sessionId)
  const [traceTarget, setTraceTarget] = useState<TraceTarget | null>(null)
  // Focus stays in the composer after send (§8.1): the first send remounts this view (/chat →
  // /chat/$id), and delete or a fresh load leaves focus on <body>. Never steals it.
  const composerShown = target.kind !== 'readonly'
  useEffect(() => {
    if (composerShown) focusIfIdle(composer.current)
  }, [composerShown])

  // Search a few more rail pages for this chat's row (§6.11).
  const { hasNextPage: moreRows, isFetchingNextPage: fetchingRows, fetchNextPage: nextRows } = list
  useEffect(() => {
    if (!row && moreRows && !fetchingRows && pagesSearched < ROW_SEARCH_PAGES) void nextRows()
  }, [row, moreRows, fetchingRows, pagesSearched, nextRows])

  // Replies the turn already had when it started (Run again on a turn with a reply, another tab):
  // only a reply beyond these means this turn's own reply committed.
  const ownReplies = ownTurn(turns, live)?.replies.length ?? 0
  const turnKey = live ? `${live.id}:${live.startedAt}` : ''
  // Kept outside the view: leaving the chat and coming back must not re-measure from "now".
  // A routed resume brings the baseline measured when its request was answered (see resolve below).
  const newReplies = live
    ? ownReplies - (live.baselineReplies ?? replyBaseline(turnKey, ownReplies))
    : 0
  // A finished turn against history (EN14, §5.4): the decisions are reconcile.ts's, applied by the hook.
  const refetchHistory = history.refetch
  useReconcile(
    registry,
    sessionId,
    {
      live,
      history: {
        dataUpdatedAt: history.dataUpdatedAt,
        errorUpdatedAt: history.errorUpdatedAt,
        isError: history.isError,
      },
      turns,
      requests,
      pending: pending.length,
      answering: resolving || canceling,
      newReplies,
    },
    () => void refetchHistory(),
  )

  // A 403 locks the composer; coming back to the tab (after asking for access) lifts the lock.
  // The turn and its notice stay: forgetting it would make history read as a lost reply.
  const forbiddenTurn = live?.phase === 'error' && live.error?.key === 'forbidden' ? live.id : null
  const [unlockedTurn, setUnlockedTurn] = useState<string | null>(null)
  useEffect(() => {
    if (!forbiddenTurn) return
    const onFocus = () => setUnlockedTurn(forbiddenTurn)
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [forbiddenTurn])

  // The checking window ended: look once more before calling it unconfirmed (§6.7).
  useEffect(() => {
    if (status === 'unconfirmed') void refetchHistory()
  }, [status, refetchHistory])

  const refresh = () => void invalidateChat(client, sessionId)
  const lastUser = [...turns].reverse().find((t) => t.user)?.user ?? null

  const doSend = async () => {
    if (!agent && !routed) return
    const text = draft.trim()
    setDraft('')
    setSendError(null)
    registry.clearExecutionUnknown(sessionId)
    const input: SendInput = agent
      ? { sessionId, agentId: agent.id, text, transcript: row?.transcript }
      : { sessionId, chatMode: 'routed', text }
    try {
      await registry.send(input)
    } catch (err) {
      setDraft(text)
      setSendError(err)
    }
  }
  const onSend = () => {
    if (registry.isExecutionUnknown(sessionId) || status === 'unconfirmed') setConfirm('send')
    else void doSend()
  }
  const doRunAgain = () => {
    // Never re-send a routed message whose request was answered: that repeats the approved step (D3).
    if (resume) return
    setSendError(null)
    const from =
      !live && lastUser && (agent || routed)
        ? {
            agentId: agent?.id ?? null,
            chatMode: routed ? ('routed' as const) : undefined,
            userText: lastUser.content,
            userMessageId: lastUser.id,
          }
        : undefined
    void registry.runAgain(sessionId, from).catch((err: unknown) => setSendError(err))
  }

  const turnOfRequest = (id: string) => turns.find((t) => t.requests.some((r) => r.id === id))
  const resumeBaselines = useRef(new Map<string, number>())
  const handlers: TurnHandlers = {
    editMessage: (text) => {
      setDraft(text)
      composer.current?.focus()
    },
    stepsFor,
    viewTrace: (traceId, fresh) => setTraceTarget({ traceId, fresh }),
    refresh,
    runAgain: () => (needsRunAgainConfirm(live?.error) ? setConfirm('runAgain') : doRunAgain()),
    saveAgain: () => void registry.saveAgain(sessionId),
    discard: () => registry.discard(sessionId),
    tryAgain: () => {
      if (!live || (!agent && !routed)) return
      setSendError(null)
      const text = live.userText
      void registry
        .send(
          agent
            ? { sessionId, agentId: agent.id, text, transcript: row?.transcript }
            : { sessionId, chatMode: 'routed', text },
        )
        // Clear the box only if it still holds the retried text; anything newer is the user's.
        .then(() => {
          if (readDraft(userId, sessionId).trim() === text) setDraft('')
        })
        .catch((err: unknown) => setSendError(err))
    },
    requests: {
      resolve: (id, body) => {
        // Before the resolve: its refetch may already show the continuation's saved reply.
        resumeBaselines.current.set(id, turnOfRequest(id)?.replies.length ?? 0)
        return resolveRequest({ id, body })
      },
      cancel: (id) => cancelRequest(id),
      onDone: (req: HitlDto) => {
        // Handled in another tab: that tab resumes; here history is enough.
        if (
          req.already_resolved ||
          req.already_canceled ||
          (req.status !== 'resolved' && req.status !== 'rejected')
        ) {
          refresh()
          requestAnimationFrame(() => focusIfIdle(composer.current))
          return
        }
        const userMessageId = turnOfRequest(req.id)?.user?.id ?? live?.userMessageId
        // A routed request's agent_id is the sub-agent's: the reconnect goes to the orchestrator (EN-6).
        if (routed || req.execution?.origin === 'orchestrator')
          void registry.resume(
            sessionId,
            req.id,
            null,
            userMessageId,
            'routed',
            resumeBaselines.current.get(req.id),
          )
        else {
          const agentId = req.execution?.agent_id ?? agent?.id
          if (agentId) void registry.resume(sessionId, req.id, agentId, userMessageId)
        }
        // The answered card turns into a receipt, taking focus with it (§7.7 resuming → composer).
        requestAnimationFrame(() => focusIfIdle(composer.current))
      },
    },
  }

  // The oldest pending request, as Waiting orders them; with none loaded yet, the live frame's card (the newest).
  const goToRequest = () => {
    const first = [...pending].sort((a, b) => a.created_at.localeCompare(b.created_at))[0]
    const el =
      (first && document.getElementById(`request-${first.id}`)) ||
      [...document.querySelectorAll<HTMLElement>('[id^="request-"]')].at(-1)
    el?.scrollIntoView({ block: 'center', behavior: 'smooth' })
    el?.focus()
  }

  // Delete first, then drop the turn: a failed delete keeps everything, the running reply
  // included, and the dialog stays open with the reason (§6.10).
  const onDelete = async () => {
    try {
      await del.mutateAsync()
    } catch {
      announce(copy.deleteFailed)
      return
    }
    registry.abort(sessionId)
    setConfirm(null)
    await navigate({ to: '/chat', search: { mock: search.mock, debug: search.debug } })
    newChatRef.current?.focus()
  }

  const exportChat = () => {
    const messages = [...pages].reverse().flatMap((p) => p.data)
    downloadText(
      `${(row?.title ?? 'chat').replace(/[^\w.-]+/g, '-').slice(0, 60) || 'chat'}.md`,
      exportMarkdown(row?.title ?? 'Chat', messages),
      'text/markdown;charset=utf-8',
    )
  }

  // History states first: a 404 means deleted or not yours (same copy, §7.7).
  // A 404 while this tab is still creating the chat (its placeholder row was clicked) isn't
  // "deleted or not yours": the chat just doesn't exist on the server yet.
  const stillCreating = !!live && isLivePhase(live.phase)
  if (history.isError && !history.data && !stillCreating) {
    const missing = history.error instanceof ApiError && history.error.status === 404
    return (
      <>
        <header className="flex h-12 shrink-0 items-center border-b border-border">
          <div className={cn(CHAT_COLUMN, 'flex items-center gap-2')}>{railButton}</div>
        </header>
        {/* The rail still announces (a list failure) while this screen shows. */}
        <StatusAnnouncer live={undefined} agentName={agentName} />
        <div className="mx-auto mt-12 w-full max-w-md px-4">
          <StateCard
            tone={missing ? 'info' : 'error'}
            title={missing ? copy.historyMissing : copy.historyFailed}
            action={
              missing ? (
                <Button asChild size="sm" className="pointer-coarse:min-h-11">
                  <Link to="/chat" search={{}} ref={focusIfIdle}>
                    {copy.newChat}
                  </Link>
                </Button>
              ) : (
                <Button
                  size="sm"
                  variant="outline"
                  className="pointer-coarse:min-h-11"
                  onClick={() => void history.refetch()}
                >
                  {copy.retry}
                </Button>
              )
            }
          />
        </div>
      </>
    )
  }

  const readOnly = target.kind === 'readonly'
  let mode: ComposerMode = { kind: 'ready' }
  if (capped && target.kind === 'loading')
    mode = { kind: 'locked', reason: copy.chatDetailsMissing }
  else if (target.kind === 'loading') mode = { kind: 'locked', reason: copy.lockedLoading }
  else if (live && isLivePhase(live.phase))
    mode =
      (live.phase === 'waiting' || live.phase === 'streaming') &&
      live.operation !== 'resume' &&
      live.chatMode === 'direct'
        ? { kind: 'streaming' }
        : { kind: 'locked' }
  else if (live?.phase === 'loading_saved')
    mode = { kind: 'locked', reason: copy.loadingSavedReply }
  else if (paused) mode = { kind: 'paused' }
  else if (live?.phase === 'unsaved') mode = { kind: 'locked', reason: copy.lockedUnsaved }
  else if (resume?.mayArrive)
    mode = { kind: 'locked', reason: copy.lockedResumeArriving, newChat: true }
  else if (status === 'checking') mode = { kind: 'locked', reason: copy.lockedChecking }
  else if (forbiddenTurn && unlockedTurn !== forbiddenTurn)
    mode = { kind: 'locked', reason: copy.lockedForbidden }
  else if (agent && agent.status !== 'running') {
    // A stopped agent: Send stays focusable, and Enter or Send says why (E16).
    const reason = copy.targetNotRunning(agentName)
    mode = { kind: 'locked', reason, blocked: () => announce(reason) }
  }

  const liveLen = live
    ? live.state.artifacts.reduce((n, a) => n + a.text.length, 0) + live.state.steps.length
    : 0
  const contentKey = `${turns.length}:${liveLen}:${live?.phase ?? ''}:${status ?? ''}`
  // quirk: §10.6 — delete fails on any request row (FK without cascade), so it is disabled up front.
  const hasHitl = requests.length > 0
  const replies = pages.flatMap((p) => p.data).filter((m) => m.role === 'assistant')

  return (
    <>
      <title>{`${titlePrefix}${row?.title ?? copy.chatsNav} · OpenRuntime`}</title>
      <SessionHeader
        sessionId={sessionId}
        row={row}
        loading={target.kind === 'loading'}
        readOnly={readOnly}
        identity={identity}
        railButton={railButton}
        replies={replies}
        partial={!!history.hasNextPage}
        canExport={!!pages.length}
        onExport={exportChat}
        deleteBlocked={hasHitl}
        onDelete={() => setConfirm('delete')}
      />

      {agent && agent.status !== 'running' && !liveNow ? (
        <NotRunningBanner agentId={agent.id} name={agentName} refetch={() => void dir.refetch()} />
      ) : null}
      {capped && target.kind === 'loading' ? (
        <div
          role="note"
          className={cn(
            CHAT_COLUMN,
            'mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground',
          )}
        >
          {copy.chatDetailsMissing}{' '}
          <Button
            size="xs"
            variant="outline"
            className={TOUCH}
            onClick={() => void lookup.refetch()}
          >
            {copy.retry}
          </Button>
        </div>
      ) : null}

      {history.isPending ? (
        <PageLoader label={copy.loadingChat} inline className="min-h-0" />
      ) : (
        <Transcript
          contentKey={contentKey}
          hasOlder={!!history.hasNextPage}
          loadingOlder={history.isFetchingNextPage}
          olderFailed={history.isFetchNextPageError}
          onLoadOlder={() => void history.fetchNextPage()}
        >
          {requestGone ? (
            <p role="note" className="text-sm text-muted-foreground" data-testid="request-gone">
              {copy.requestGone}
            </p>
          ) : null}
          {!turns.length && isMetadataOnly(row) ? <MetadataOnly /> : null}
          {turns.map((t, i) => (
            <TurnView
              key={t.key}
              turn={t}
              latest={i === turns.length - 1}
              agentName={agentName}
              agentId={agent?.id ?? null}
              sessionId={sessionId}
              status={i === turns.length - 1 ? status : null}
              handlers={handlers}
              focusRequestId={focusRequestId}
              recorded={target.kind === 'readonly' && target.why === 'recorded'}
              routed={
                routed
                  ? {
                      agents: dir.data,
                      endFor,
                      resume,
                      resumedAsker,
                    }
                  : undefined
              }
            />
          ))}
        </Transcript>
      )}

      {search.debug ? <DebugFrames live={live} sessionId={sessionId} userId={userId} /> : null}
      {search.debug && routed ? (
        <RoutedMetricLine
          replies={replies}
          stepsFor={stepsFor}
          ends={[...ends.values()].filter((e) => e.sessionId === sessionId)}
        />
      ) : null}
      <SendError error={sendError} />
      {/* A recorded or removed-agent chat: the read-only notice takes the composer's place (v1c §5.5). */}
      {target.kind === 'readonly' ? (
        <ReadOnlyNote why={target.why} />
      ) : (
        <Composer
          ref={composer}
          agentName={agentName}
          label={routed ? copy.askOrchestrator : undefined}
          placeholder={routed ? copy.askOrchestrator : undefined}
          note={routed && liveNow ? copy.routedHint : undefined}
          value={draft}
          onChange={setDraft}
          onSend={onSend}
          onStop={() => registry.stop(sessionId)}
          onGoToRequest={goToRequest}
          onNewChat={() =>
            void navigate({
              to: '/chat',
              search: {
                ...(routed ? { auto: 1 } : { agent: agent?.id }),
                mock: search.mock,
                debug: search.debug,
              },
            })
          }
          mode={mode}
        />
      )}
      <StatusAnnouncer
        live={live}
        agentName={agentName}
        askedBy={resumedAsker}
        error={sendErrorText(sendError)}
      />
      <TraceSheet target={traceTarget} sessionId={sessionId} onClose={() => setTraceTarget(null)} />

      <ConfirmDialog
        open={confirm === 'send'}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={copy.sendWhileUnknownTitle}
        body={copy.sendWhileUnknownBody}
        primary={{
          label: copy.sendAnyway,
          onClick: () => {
            setConfirm(null)
            void doSend()
          },
        }}
      />
      <ConfirmDialog
        open={confirm === 'runAgain'}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={copy.runAgainConfirmTitle}
        body={copy.runAgainConfirmBody}
        primary={{
          label: copy.refreshStatus,
          onClick: () => {
            setConfirm(null)
            refresh()
          },
        }}
        secondary={{ label: copy.runAgain, onClick: doRunAgain }}
      />
      <ConfirmDialog
        open={confirm === 'delete'}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={copy.deleteTitle}
        body={
          del.isError
            ? `${copy.deleteFailed} ${del.error instanceof ApiError && del.error.serverMessage ? copy.serverSaid(del.error.serverMessage) : ''}`
            : copy.deleteBody
        }
        primary={{
          label: copy.delete,
          destructive: true,
          keepOpen: true,
          busy: del.isPending,
          onClick: () => void onDelete(),
        }}
      />
    </>
  )
}

export const SessionViewMemo = memo(SessionView)
