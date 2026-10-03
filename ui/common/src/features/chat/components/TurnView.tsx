/**
 * One turn, top to bottom (plan §7.4, DS1): user message → pending request → reply → file chips
 * → activity → footer (Copy · usage · View trace) → notices. Each notice has one primary action.
 * The transcript is a plain region: nothing here is a live region; StatusAnnouncer speaks (§8.1).
 */
import { ChevronDown, ChevronUp, FileDown, Loader2, Wrench } from 'lucide-react'
import { memo, useId, useState } from 'react'
import { Link } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import { fmtLatency } from '@/lib/format'
import { cn } from '@/lib/utils'
import { replyText, type Step } from '../a2aReducer'
import { copy } from '../copy'
import type { ChatError } from '../errors'
import { agentLabel, usageFromMessage, usageFromMeta } from '../format'
import { splitStopped } from '../normalize'
import type { DisplayTurn, ReplyStatus } from '../turnModel'
import { isLivePhase, type LiveTurn } from '../turnRegistry'
import { tuning } from '../tuning'
import type { ChatMessage, FilePart, HitlDto } from '../types'
import { Markdown } from './Markdown'
import { KnownEmpty, RoutedLive, RoutedSavedReply, type RoutedContext } from './RoutedTurn'
import { CopyReply, ErrorNotice, UsageChip, type NoticeAction } from './turnParts'
import { LINK, LINK_BUTTON, STEP_CHIP, TOUCH } from './turnStyles'
import { RecordedCallsFor } from './RecordedCalls'
import { StepIcon } from './StepIcon'
import { RequestCard, type RequestActions } from './RequestCard'
import { useNow } from '../hooks'

export interface TurnHandlers {
  /** Open the trace sheet; `fresh` while spans may still be arriving (§7.5). */
  viewTrace(traceId: string, fresh: boolean): void
  refresh(): void
  runAgain(): void
  saveAgain(): void
  discard(): void
  tryAgain(): void
  /** 400 "Edit and send": put the message back in the composer. */
  editMessage(text: string): void
  /** Steps of a reply this tab saved (by message id, or a routed reply's trace id), so a finished turn keeps its steps. */
  stepsFor?(messageId: string, traceId?: string | null): Step[] | undefined
  requests: RequestActions
}

function StepChip({ step }: { step: Step }) {
  const [open, setOpen] = useState(false)
  const detailId = useId()
  return (
    <li className="max-w-60">
      <Button
        type="button"
        variant="outline"
        size="xs"
        className={STEP_CHIP}
        aria-expanded={step.detail ? open : undefined}
        aria-controls={step.detail ? detailId : undefined}
        onClick={() => setOpen((v) => !v)}
      >
        <StepIcon status={step.status} /> <span className="truncate">{step.name}</span>
        {step.durationMs ? (
          <span className="tabular-nums">{fmtLatency(step.durationMs)}</span>
        ) : null}
      </Button>
      {open && step.detail ? (
        <p id={detailId} className="mt-1 text-xs break-words text-muted-foreground">
          {step.detail}
        </p>
      ) : null}
    </li>
  )
}

function StepChips({ steps, live }: { steps: Step[]; live: boolean }) {
  const failed = steps.some((s) => s.status === 'error')
  // Open while live or failed, collapsed once done, unless the user chose (§7.4).
  const [chosen, setOpen] = useState<boolean | null>(null)
  const open = chosen ?? (live || failed)
  const listId = useId()
  if (!steps.length) return null
  const total = steps.reduce((n, s) => n + (s.durationMs ?? 0), 0)
  // One toggle that stays mounted, so focus stays on it when the list opens or closes.
  return (
    <div className="space-y-1.5">
      <Button
        type="button"
        variant="outline"
        size="xs"
        className={cn(STEP_CHIP, 'max-w-none')}
        aria-expanded={open}
        aria-controls={listId}
        onClick={() => setOpen(!open)}
      >
        <Wrench className="size-3.5" aria-hidden /> {copy.tools(steps.length)}
        {total ? ` · ${fmtLatency(total)}` : ''}{' '}
        {open ? (
          <ChevronUp className="size-3" aria-hidden />
        ) : (
          <ChevronDown className="size-3" aria-hidden />
        )}
      </Button>
      {open ? (
        <ul id={listId} className="flex flex-wrap gap-1.5" aria-label={copy.steps}>
          {steps.map((s) => (
            <StepChip key={s.key} step={s} />
          ))}
        </ul>
      ) : null}
    </div>
  )
}

function FileChips({ files }: { files: FilePart[] }) {
  if (!files.length) return null
  return (
    <div className="flex flex-wrap gap-2">
      {files.map((f) => (
        <a
          key={f.id}
          href={`/api/chat/files/${encodeURIComponent(f.id)}/download`}
          download
          className={cn(
            'inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-muted',
            TOUCH,
          )}
        >
          <FileDown className="size-3.5" aria-hidden /> {f.name}
          {f.size ? ` · ${Math.max(1, Math.round(f.size / 1024))} KB` : ''}
        </a>
      ))}
    </div>
  )
}

/** The only direct part that ticks each second: its elapsed time. */
function Waiting({
  live,
  agentName,
  now: fixed,
}: {
  live: LiveTurn
  agentName: string
  now?: number
}) {
  const now = useNow(1000, fixed)
  const secs = Math.max(0, Math.floor((now - live.startedAt) / 1000))
  const label = live.phase === 'creating' ? copy.phaseStarting : copy.phaseWaiting(agentName)
  return (
    <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> {label}
      {secs * 1000 >= tuning.SLOW_MS ? (
        <span className="tabular-nums">{copy.elapsed(secs)}</span>
      ) : null}
      {secs * 1000 >= tuning.LONG_MS ? <span>{copy.phaseStillWorking}</span> : null}
    </div>
  )
}

/**
 * The §6.6 actions for an error. Rejected-before-run errors each get their own way out:
 * 400 edit and send, 403 the agent's page, 404/503 Agents, 429 retry.
 */
function errorActions(
  error: ChatError,
  text: string,
  agentId: string | null,
  handlers: TurnHandlers,
): { actions: NoticeAction[]; links?: React.ReactNode } {
  if (error.certainty === 'not-dispatched')
    return { actions: [{ label: copy.tryAgain, onClick: handlers.tryAgain, primary: true }] }
  if (error.certainty === 'rejected-before-run') {
    switch (error.key) {
      case 'rateLimited':
        return { actions: [{ label: copy.retry, onClick: handlers.runAgain, primary: true }] }
      case 'badRequest':
        return {
          actions: [
            { label: copy.editAndSend, onClick: () => handlers.editMessage(text), primary: true },
          ],
        }
      case 'forbidden':
        return {
          actions: [],
          links: agentId ? (
            <Button asChild size="sm" className={TOUCH}>
              <Link to="/agents/$agentId" params={{ agentId }} search={{}}>
                {copy.openAgent}
              </Link>
            </Button>
          ) : (
            <Button asChild size="sm" className={TOUCH}>
              <Link to="/agents" search={{}}>
                {copy.agentsPage}
              </Link>
            </Button>
          ),
        }
      default:
        return {
          actions: [],
          links: (
            <Button asChild size="sm" className={TOUCH}>
              <Link to="/agents" search={{}}>
                {copy.agentsPage}
              </Link>
            </Button>
          ),
        }
    }
  }
  return {
    actions: [
      { label: copy.refreshStatus, onClick: handlers.refresh, primary: true },
      { label: copy.runAgain, onClick: handlers.runAgain },
    ],
  }
}

type TimelineItem =
  { kind: 'reply'; at: number; m: ChatMessage } | { kind: 'receipt'; at: number; r: HitlDto }

/** Saved replies and answered requests, oldest first; on a tie the question comes before the reply. */
function timeline(replies: ChatMessage[], receipts: HitlDto[]): TimelineItem[] {
  const items: TimelineItem[] = [
    ...replies.map((m) => ({ kind: 'reply' as const, at: Date.parse(m.timestamp) || 0, m })),
    ...receipts.map((r) => ({ kind: 'receipt' as const, at: Date.parse(r.created_at) || 0, r })),
  ]
  return items.sort(
    (a, b) => a.at - b.at || (a.kind === b.kind ? 0 : a.kind === 'receipt' ? -1 : 1),
  )
}

/** Several pending requests in one turn: one card at a time, "Request i of N" (§6.8). */
function PendingRequests({
  requests,
  agentName,
  actions,
  focusRequestId,
}: {
  requests: HitlDto[]
  agentName: (r: HitlDto) => string
  actions: RequestActions
  focusRequestId?: string
}) {
  const [index, setIndex] = useState(0)
  // A Waiting row asked for this request (v1c E5): the pager shows it.
  const [focusedFor, setFocusedFor] = useState<string | undefined>()
  if (focusRequestId && focusRequestId !== focusedFor) {
    setFocusedFor(focusRequestId)
    const at = requests.findIndex((r) => r.id === focusRequestId)
    if (at >= 0 && at !== index) setIndex(at)
  }
  if (!requests.length) return null
  const i = Math.min(index, requests.length - 1)
  const r = requests[i]
  return (
    <div className="space-y-2">
      {requests.length > 1 ? (
        <nav
          className="flex items-center gap-2 text-xs text-muted-foreground"
          aria-label={copy.request.pagerLabel}
        >
          {/* aria-disabled, not disabled: a disabled button drops the keyboard focus it holds. */}
          <Button
            size="xs"
            variant="outline"
            className={cn(TOUCH, 'aria-disabled:opacity-50')}
            aria-disabled={i === 0}
            onClick={() => i > 0 && setIndex(i - 1)}
          >
            {copy.request.previous}
          </Button>
          <span className="tabular-nums">{copy.request.pager(i + 1, requests.length)}</span>
          <Button
            size="xs"
            variant="outline"
            className={cn(TOUCH, 'aria-disabled:opacity-50')}
            aria-disabled={i === requests.length - 1}
            onClick={() => i < requests.length - 1 && setIndex(i + 1)}
          >
            {copy.request.next}
          </Button>
        </nav>
      ) : null}
      <RequestCard
        key={r.id}
        id={`request-${r.id}`}
        request={r}
        agentName={agentName(r)}
        actions={actions}
        label={requests.length > 1 ? copy.request.pager(i + 1, requests.length) : undefined}
      />
    </div>
  )
}

function TurnViewBody({
  turn,
  latest,
  agentName,
  agentId,
  sessionId,
  now,
  status,
  handlers,
  routed,
  recorded = false,
  focusRequestId,
}: {
  turn: DisplayTurn<LiveTurn>
  latest: boolean
  agentName: string
  agentId: string | null
  sessionId: string
  /** A fixed clock (tests). Without it only the parts that show time tick, each on its own. */
  now?: number
  status: ReplyStatus
  handlers: TurnHandlers
  /** A routed chat: the v1b anatomy (answer first, Activity, attribution, §5.5 end states). */
  routed?: RoutedContext
  /** A recorded harness chat: each reply's tool calls show as chips above its text (v1c §5.7). */
  recorded?: boolean
  /** A request a Waiting row opened this chat for (v1c E5). */
  focusRequestId?: string
}) {
  const live = turn.live
  const liveNow = live ? isLivePhase(live.phase) : false
  const userText = turn.user?.content ?? live?.userText ?? ''
  const saved: ChatMessage[] = turn.replies
  const pending = turn.requests.filter((r) => r.status === 'pending')
  const liveRequest = live?.state.request
  const liveFrame =
    liveRequest && !turn.requests.some((r) => r.id === liveRequest.id) ? liveRequest : undefined
  const liveText = live ? replyText(live.state) : ''
  const liveUsage = live ? usageFromMeta(live.state.usage) : null
  // no_reply has nothing to show but its notice (E5), which lives in this block too.
  const showLive =
    !routed &&
    !!live &&
    (liveNow ||
      live.phase === 'unsaved' ||
      live.phase === 'error' ||
      live.phase === 'no_reply' ||
      (!saved.length && !!liveText))
  const traceId = saved[saved.length - 1]?.trace_id ?? live?.state.traceId ?? null
  // EN-5: this tab saw the attempt end empty, so history's missing reply isn't a lost one.
  // Only the newest turn: Run again re-sends the chat's latest message, never an older one.
  const knownEmpty =
    !!routed &&
    latest &&
    !live &&
    !saved.length &&
    !!turn.user &&
    routed.endFor(turn.user.id)?.kind === 'empty'
  // A routed request is asked by the agent OpenRuntime called (its execution's agent), not OpenRuntime.
  const requestAgent = (r: HitlDto) => {
    const a =
      routed && r.execution.agent_id
        ? routed.agents?.find((x) => x.id === r.execution.agent_id)
        : undefined
    return a ? agentLabel(a) : agentName
  }
  // A resumed sub-agent that asks again sends a hitl frame with no agent (hitl/mod.rs:612-620):
  // it's the agent of the request being resumed.
  const frameAgent = routed
    ? (liveFrame?.agent ?? live?.state.awaiting?.agent ?? routed.resumedAsker ?? agentName)
    : agentName
  // An answered routed request with no reply yet (D3): its own notice replaces checking, E5 and
  // KnownEmpty, with Refresh status only. A live block shows its own notice instead.
  const resume = latest && !live ? (routed?.resume ?? null) : null
  // The routed attempt's own reply is in history, by its trace id. Without one (an older reply
  // may belong to an earlier attempt) the registry settles it on the baseline and forgets it.
  const ownSaved =
    !!live && !!live.state.traceId && saved.some((m) => m.trace_id === live.state.traceId)

  return (
    <div className="space-y-3" data-testid="turn">
      {turn.boundary ? (
        <div className="text-center text-xs text-muted-foreground">{copy.earlierMessages}</div>
      ) : null}
      {userText ? (
        <div className="flex justify-end">
          <div className="max-w-[85%] rounded-lg bg-muted px-3 py-2 text-sm whitespace-pre-wrap">
            {userText}
            {(live?.phase === 'error' || live?.phase === 'not_started') &&
            live.error?.certainty === 'not-dispatched' ? (
              <div className="mt-1 text-xs text-destructive">{copy.notSent}</div>
            ) : null}
          </div>
        </div>
      ) : null}

      <PendingRequests
        requests={pending}
        agentName={requestAgent}
        actions={handlers.requests}
        focusRequestId={focusRequestId}
      />
      {/* A request history doesn't have yet; Go to request targets the first card only. */}
      {liveFrame ? (
        <RequestCard
          id={`request-${liveFrame.id}`}
          frame={liveFrame}
          agentName={frameAgent}
          actions={handlers.requests}
        />
      ) : null}

      {/* Replies and answered requests in the order they happened: a receipt sits where its
          question was asked, before the reply it unlocked (§6.8). */}
      {timeline(
        saved,
        turn.requests.filter((r) => r.status !== 'pending'),
      ).map((item) => {
        if (item.kind === 'receipt')
          return (
            <RequestCard
              key={item.r.id}
              request={item.r}
              agentName={requestAgent(item.r)}
              actions={handlers.requests}
            />
          )
        const m = item.m
        if (routed)
          return (
            <RoutedSavedReply
              key={m.id}
              m={m}
              latest={latest}
              now={now}
              ctx={routed}
              handlers={handlers}
            />
          )
        const { text, stopped } = splitStopped(m)
        const usage = usageFromMessage(m)
        const traceId = m.trace_id
        return (
          <div key={m.id} className="space-y-2">
            {recorded && m.role === 'assistant' ? <RecordedCallsFor message={m} now={now} /> : null}
            <Markdown text={text} />
            {stopped ? (
              <p className="text-xs text-muted-foreground">{copy.receivingStopped}</p>
            ) : null}
            <FileChips files={Array.isArray(m.file_parts) ? m.file_parts : []} />
            {(() => {
              const steps = handlers.stepsFor?.(m.id)
              return steps?.length ? <StepChips steps={steps} live={false} /> : null
            })()}
            {/* Older turns show their footer on hover or focus; touch has no hover, so it stays visible there. */}
            <div
              className={cn(
                'flex flex-wrap items-center gap-2',
                !latest &&
                  'opacity-0 focus-within:opacity-100 hover:opacity-100 pointer-coarse:opacity-100',
              )}
            >
              <CopyReply text={text} />
              {usage ? <UsageChip usage={usage} /> : null}
              {traceId ? (
                <Button
                  size="xs"
                  variant="ghost"
                  className={TOUCH}
                  onClick={() =>
                    handlers.viewTrace(
                      traceId,
                      (now ?? Date.now()) - Date.parse(m.timestamp) < tuning.TRACE_FRESH_MS,
                    )
                  }
                >
                  {copy.viewTrace}
                </Button>
              ) : null}
            </div>
          </div>
        )
      })}

      {showLive && live ? (
        <div className="space-y-2">
          {live.state.steps.length ? <StepChips steps={live.state.steps} live={liveNow} /> : null}
          {liveText ? <Markdown text={liveText} live={liveNow} /> : null}
          {liveNow && !liveText ? <Waiting live={live} agentName={agentName} now={now} /> : null}
          {live.idle && liveNow ? (
            <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
              {copy.idleNotice}{' '}
              <Button size="xs" variant="outline" className={TOUCH} onClick={handlers.refresh}>
                {copy.refreshStatus}
              </Button>
            </div>
          ) : null}
          {live.phase === 'done' && live.stopped ? (
            <p className="text-xs text-muted-foreground">{copy.receivingStopped}</p>
          ) : null}
          {!liveNow && liveUsage ? <UsageChip usage={liveUsage} /> : null}
          {live.phase === 'unsaved' && live.error ? (
            <ErrorNotice
              error={live.error}
              actions={
                live.error.certainty === 'unknown'
                  ? [
                      { label: copy.refreshStatus, onClick: handlers.refresh, primary: true },
                      { label: copy.saveAgain, onClick: handlers.saveAgain },
                      { label: copy.discardReply, onClick: handlers.discard },
                    ]
                  : [
                      { label: copy.saveAgain, onClick: handlers.saveAgain, primary: true },
                      { label: copy.discardReply, onClick: handlers.discard },
                    ]
              }
            />
          ) : null}
          {live.phase === 'error' && live.error
            ? (() => {
                const { actions, links } = errorActions(
                  live.error,
                  live.userText,
                  agentId,
                  handlers,
                )
                return (
                  <ErrorNotice
                    error={live.error}
                    actions={actions}
                    links={
                      <>
                        {links}
                        {traceId ? (
                          <Button
                            type="button"
                            variant="link"
                            className={LINK_BUTTON}
                            onClick={() => handlers.viewTrace(traceId, true)}
                          >
                            {copy.viewTrace}
                          </Button>
                        ) : null}
                        {agentId && live.error.certainty !== 'not-dispatched' ? (
                          <Link
                            to="/agents/$agentId"
                            params={{ agentId }}
                            search={{ tab: 'activity' }}
                            className={LINK}
                          >
                            {copy.agentLogs}
                          </Link>
                        ) : null}
                      </>
                    }
                  />
                )
              })()
            : null}
          {live.phase === 'no_reply' ? (
            <ErrorNotice
              error={{ key: 'noReply', certainty: 'unknown' } as ChatError}
              actions={[
                { label: copy.refreshStatus, onClick: handlers.refresh, primary: true },
                { label: copy.runAgain, onClick: handlers.runAgain },
              ]}
            />
          ) : null}
        </div>
      ) : null}

      {routed && live ? (
        <RoutedLive
          live={live}
          sessionId={sessionId}
          hasSaved={ownSaved}
          now={now}
          ctx={routed}
          handlers={handlers}
          tryAgain={handlers.tryAgain}
        />
      ) : null}
      {resume?.mayArrive ? (
        // Usually still on its way: a quiet pending row, like checking, not an error.
        <PendingRow
          text={copy.resumeWaiting}
          onRefresh={handlers.refresh}
          testId="resume-waiting"
        />
      ) : resume ? (
        <ErrorNotice
          error={{ key: 'routedAnsweredNoReply', certainty: 'unknown' } as ChatError}
          actions={[{ label: copy.refreshStatus, onClick: handlers.refresh, primary: true }]}
        />
      ) : null}
      {knownEmpty && !resume ? <KnownEmpty onRunAgain={handlers.runAgain} /> : null}
      {latest && status === 'checking' && !knownEmpty && !resume ? (
        <PendingRow
          text={copy.checkingForReply}
          onRefresh={handlers.refresh}
          testId="checking-reply"
        />
      ) : null}
      {latest && status === 'unconfirmed' && !knownEmpty && !resume ? (
        <ErrorNotice
          error={{ key: 'noReply', certainty: 'unknown' } as ChatError}
          actions={[
            { label: copy.refreshStatus, onClick: handlers.refresh, primary: true },
            { label: copy.runAgain, onClick: handlers.runAgain },
          ]}
        />
      ) : null}
    </div>
  )
}

/**
 * Memoised: the page re-renders on its status clock (and on every live frame); a turn re-renders only when its
 * own props change, and a live turn's elapsed time ticks inside `Waiting` / `RoutedStatusLine` alone.
 */
export const TurnView = memo(TurnViewBody)

/** A reply that is usually still on its way (checking, or after an answer): quiet, not an error. */
function PendingRow({
  text,
  onRefresh,
  testId,
}: {
  text: string
  onRefresh(): void
  testId: string
}) {
  return (
    <div
      className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
      data-testid={testId}
    >
      <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> {text}{' '}
      <Button size="xs" variant="outline" className={TOUCH} onClick={onRefresh}>
        {copy.refreshStatus}
      </Button>
    </div>
  )
}
