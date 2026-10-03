/**
 * A routed turn (v1b §5.5-§5.8). Answer first: reply → "Answered by OpenRuntime[, using …]" →
 * footer → "Activity · N agents", collapsed. While live, one status line (Working / Asking /
 * Writing) with Activity expandable. Agent names are plain text; links to the current agent of a
 * name sit only inside an expanded agent row (DP9). Every model- or agent-supplied string here
 * renders as text, never Markdown (NE-11), except the reply itself.
 */
import { Link } from '@tanstack/react-router'
import { Check, ChevronDown, ChevronRight, CircleAlert, Loader2, Minus, X } from 'lucide-react'
import { useId, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import type { Agent } from '@/features/agents/types'
import { fmtLatency } from '@/lib/format'
import { cn } from '@/lib/utils'
import { notesFor, routedReplyText, type Step, type TurnState } from '../a2aReducer'
import {
  agentActivity,
  agentsAsked,
  listAgents,
  statusLine,
  type AgentActivity,
  type AgentSummary,
} from '../activity'
import { useFlowAgents } from '../api'
import { useNow } from '../hooks'
import { copy, policyCopy } from '../copy'
import { copyDetails } from '../copyDetails'
import { ChatError } from '../errors'
import { usageFromMessage, usageFromMeta } from '../format'
import { splitStopped } from '../normalize'
import { useSeen } from '../onScreen'
import { policyLimit, POLICY_ENV } from '../serverContract'
import { resolveAskedAgent } from '../target'
import { isLivePhase, type LiveTurn, type TurnEnd } from '../turnRegistry'
import { tuning } from '../tuning'
import type { ChatMessage } from '../types'
import { Markdown } from './Markdown'
import { CopyReply, ErrorNotice, UsageChip, type NoticeAction } from './turnParts'
import { LINK, LINK_BUTTON, STEP_CHIP, TOUCH } from './turnStyles'

/** What a routed turn needs from its page beyond the v1a handlers. */
export interface RoutedContext {
  /** The agents directory, for the "current agent of this name" links. */
  agents: readonly Agent[] | undefined
  /** The recorded end of a user message's newest attempt (EN-5). */
  endFor(userMessageId: string): TurnEnd | undefined
  /** The newest turn's answered request with no reply yet (ship review D3); the page derives it once. */
  resume?: { mayArrive: boolean } | null
  /** The agent of the request a live resume answered (a chained pause's frame names no agent). */
  resumedAsker?: string
}

export interface RoutedHandlers {
  viewTrace(traceId: string, fresh: boolean): void
  refresh(): void
  runAgain(): void
  editMessage(text: string): void
  stepsFor?(messageId: string, traceId?: string | null): Step[] | undefined
}

const truncate = (text: string, max: number) =>
  text.length > max ? `${text.slice(0, max)}…` : text

const SUMMARY: Record<AgentSummary, string> = {
  running: copy.summaryRunning,
  completed: copy.summaryCompleted,
  failed: copy.summaryFailed,
  mixed: copy.summaryMixed,
  didnt_finish: copy.summaryDidntFinish,
}

function Spinner({ small }: { small?: boolean }) {
  // Reduced motion: a still icon; the text beside it carries the meaning (DP8).
  return (
    <Loader2
      className={cn(small ? 'size-3' : 'size-4', 'animate-spin motion-reduce:animate-none')}
      aria-hidden
    />
  )
}

function SummaryIcon({ summary }: { summary: AgentSummary }) {
  if (summary === 'running')
    return <Loader2 className="size-3.5 animate-spin motion-reduce:animate-none" aria-hidden />
  if (summary === 'completed') return <Check className="size-3.5 text-success" aria-hidden />
  if (summary === 'failed') return <X className="size-3.5 text-destructive" aria-hidden />
  return <Minus className="size-3.5 text-warning" aria-hidden />
}

/** The one live status line (§5.5), with v1a's slow/long timers: the only routed part that ticks each second. */
function RoutedStatusLine({ live, now: fixed }: { live: LiveTurn; now?: number }) {
  const now = useNow(1000, fixed)
  const line = live.phase === 'creating' ? null : statusLine(live.state)
  const text = !line
    ? copy.phaseStarting
    : line.kind === 'asking'
      ? copy.statusAsking(line.agent)
      : line.kind === 'writing'
        ? copy.statusWriting
        : copy.statusWorking
  const secs = Math.max(0, Math.floor((now - live.startedAt) / 1000))
  return (
    <div
      className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
      data-testid="routed-status"
    >
      <Spinner />{' '}
      <span className="min-w-0 truncate" title={text}>
        {text}
      </span>
      {secs * 1000 >= tuning.SLOW_MS ? (
        <span className="tabular-nums">{copy.elapsed(secs)}</span>
      ) : null}
      {secs * 1000 >= tuning.LONG_MS ? <span>{copy.phaseStillWorking}</span> : null}
    </div>
  )
}

/** One agent: summary row, expandable to its calls, its latest excerpt and the links (§5.7). */
function AgentRow({
  row,
  live,
  note,
  excerpt,
  agents,
  traceId,
  onViewTrace,
}: {
  row: AgentActivity | { name: string; calls: Step[]; summary?: undefined; totalMs: 0; failed: 0 }
  live: boolean
  note?: string
  excerpt?: string
  agents: readonly Agent[] | undefined
  traceId: string | null
  onViewTrace?: (traceId: string) => void
}) {
  const [open, setOpen] = useState(false)
  const panel = useId()
  const resolved = open ? resolveAskedAgent(row.name, agents) : null
  return (
    <li className="space-y-1">
      <Button
        type="button"
        variant="ghost"
        className={cn(
          'h-auto w-full min-w-0 flex-wrap justify-start gap-x-2 gap-y-0.5 px-1 py-0 text-left font-normal whitespace-normal hover:bg-muted has-[>svg]:px-1',
          TOUCH,
        )}
        aria-expanded={open}
        aria-controls={panel}
        onClick={() => setOpen((v) => !v)}
      >
        {open ? (
          <ChevronDown className="size-3.5 shrink-0" aria-hidden />
        ) : (
          <ChevronRight className="size-3.5 shrink-0" aria-hidden />
        )}
        {row.summary ? <SummaryIcon summary={row.summary} /> : null}
        <span className="max-w-[40ch] truncate font-medium" title={row.name}>
          {row.name}
        </span>
        {row.summary ? <span className="text-muted-foreground">{SUMMARY[row.summary]}</span> : null}
        {row.totalMs ? (
          <span className="text-xs text-muted-foreground tabular-nums">
            {copy.totalCallTime} {fmtLatency(row.totalMs)}
          </span>
        ) : null}
      </Button>
      {/* Outside the button, so its accessible name doesn't change with every sub_status. */}
      {live && note ? (
        <p className="truncate pl-6 text-xs text-muted-foreground" title={note}>
          {truncate(note, tuning.ACTIVITY_NOTE_CHARS)}
        </p>
      ) : null}
      {open ? (
        <div id={panel} className="space-y-1.5 pl-6 text-xs text-muted-foreground">
          {row.calls.length ? (
            <ul className="space-y-0.5">
              {row.calls.map((c) => (
                <li key={c.key} className="flex items-center gap-1.5">
                  {c.status === 'running' ? (
                    <Spinner small />
                  ) : c.status === 'error' ? (
                    <X className="size-3 text-destructive" aria-hidden />
                  ) : (
                    <Check className="size-3 text-success" aria-hidden />
                  )}
                  <span className="truncate">
                    {c.status === 'running'
                      ? copy.stepRunning
                      : c.status === 'error'
                        ? copy.stepFailed
                        : copy.stepDone}
                  </span>
                  {c.durationMs ? (
                    <span className="tabular-nums">{fmtLatency(c.durationMs)}</span>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
          {excerpt ? (
            <p className="break-words whitespace-pre-wrap" data-testid="activity-excerpt">
              {truncate(excerpt, tuning.ACTIVITY_EXCERPT_CHARS)}
            </p>
          ) : null}
          <div className="flex flex-wrap items-center gap-x-3">
            {traceId && onViewTrace ? (
              <Button
                type="button"
                variant="link"
                className={LINK_BUTTON}
                onClick={() => onViewTrace(traceId)}
              >
                {copy.viewTrace}
              </Button>
            ) : null}
            {resolved ? (
              <>
                <Link
                  to="/agents/$agentId"
                  params={{ agentId: resolved.id }}
                  search={{}}
                  className={LINK}
                  title={copy.currentAgentTooltip(row.name)}
                >
                  {copy.openCurrentAgent}
                </Link>
                <Link to="/chat" search={{ agent: resolved.id }} className={LINK}>
                  {copy.chatWith(row.name)}
                </Link>
              </>
            ) : null}
          </div>
        </div>
      ) : null}
    </li>
  )
}

/** "Activity · N agents" (§5.7): collapsed by default, never opened for the user while live. */
export function Activity({
  state,
  live,
  paused = false,
  names,
  agents,
  traceId,
  onViewTrace,
  flows,
}: {
  state?: Pick<TurnState, 'steps' | 'agentNotes'>
  live: boolean
  paused?: boolean
  /** Names only (the flows fallback): no per-call status. */
  names?: string[]
  agents: readonly Agent[] | undefined
  traceId: string | null
  onViewTrace?: (traceId: string) => void
  flows?: { failed: boolean; retry(): void }
}) {
  const [open, setOpen] = useState(false)
  const listId = useId()
  // A paused turn hasn't ended: its open call waits on the request, so it reads Running.
  const rows = state ? agentActivity(state.steps, !live && !paused) : []
  const policies = (state?.steps ?? []).filter((s) => s.kind === 'policy')
  const nameRows =
    !rows.length && names
      ? names.map((name) => ({
          name,
          calls: [] as Step[],
          totalMs: 0 as const,
          failed: 0 as const,
        }))
      : []
  const count = rows.length || nameRows.length
  // "Didn't finish" is its own summary, not a failure.
  const failed =
    rows.filter((r) => r.summary === 'failed' || r.summary === 'mixed').length + policies.length
  if (!count && !policies.length && !flows?.failed) return null
  if (flows?.failed && !count) {
    return (
      <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
        {copy.agentDetailsUnavailable}{' '}
        <Button size="xs" variant="outline" className={TOUCH} onClick={flows.retry}>
          {copy.retry}
        </Button>
      </div>
    )
  }
  return (
    <div className="space-y-1" data-testid="activity">
      <Button
        type="button"
        variant="outline"
        size="xs"
        className={cn(STEP_CHIP, 'max-w-none gap-1.5')}
        aria-expanded={open}
        aria-controls={listId}
        onClick={() => setOpen((v) => !v)}
      >
        {open ? (
          <ChevronDown className="size-3" aria-hidden />
        ) : (
          <ChevronRight className="size-3" aria-hidden />
        )}
        {copy.activity(count)}
        {failed ? (
          <span className="inline-flex items-center gap-0.5 text-destructive">
            <CircleAlert className="size-3" aria-hidden />
            {copy.activityFailed(failed)}
          </span>
        ) : null}
      </Button>
      {open ? (
        <div id={listId} className="space-y-1">
          <ul className="space-y-1" aria-label={copy.agentsUsedLabel}>
            {(rows.length ? rows : nameRows).map((r) => {
              const note = state ? notesFor(state, r.name) : undefined
              const lastResult = [...r.calls]
                .reverse()
                .find((c) => c.detail && c.status !== 'running')?.detail
              return (
                <AgentRow
                  key={r.name}
                  row={r}
                  live={live}
                  note={note?.status}
                  excerpt={note?.content ?? lastResult}
                  agents={agents}
                  traceId={traceId}
                  onViewTrace={onViewTrace}
                />
              )
            })}
          </ul>
          {policies.map((p) => {
            const limit = policyLimit(p.detail)
            const env = limit ? POLICY_ENV[limit] : undefined
            const fix =
              limit === 'cycle'
                ? policyCopy.cycleFix
                : limit === 'guard'
                  ? policyCopy.guardFix
                  : env
                    ? policyCopy.raise(env)
                    : null
            return (
              <p
                key={p.key}
                className="flex items-start gap-1.5 pl-1 text-xs text-muted-foreground"
              >
                <CircleAlert className="mt-0.5 size-3 shrink-0 text-destructive" aria-hidden />
                <span>
                  {limit
                    ? policyCopy.stopped(policyCopy[limit])
                    : truncate(p.detail ?? p.name, tuning.ACTIVITY_NOTE_CHARS)}{' '}
                  {fix}
                </span>
              </p>
            )
          })}
        </div>
      ) : null}
    </div>
  )
}

/** "Answered by OpenRuntime", plus ", using …" only from observed data (C6, DP9). */
function Attribution({ names }: { names: readonly string[] }) {
  return (
    <p className="text-xs text-muted-foreground" data-testid="attribution">
      {names.length ? copy.answeredByUsing(listAgents(names)) : copy.answeredBy}
    </p>
  )
}

/** A saved routed reply: reply → attribution → footer → Activity (DP6). */
export function RoutedSavedReply({
  m,
  latest,
  now,
  ctx,
  handlers,
}: {
  m: ChatMessage
  latest: boolean
  /** A fixed clock (tests); by default it ticks on its own. */
  now?: number
  ctx: RoutedContext
  handlers: RoutedHandlers
}) {
  const ref = useRef<HTMLDivElement>(null)
  const steps = handlers.stepsFor?.(m.id, m.trace_id)
  const seen = useSeen(ref)
  const flows = useFlowAgents(m.trace_id, seen && !steps?.length)
  const { text, stopped } = splitStopped(m)
  const usage = usageFromMessage(m)
  const traceId = m.trace_id
  const names = steps?.length ? agentsAsked({ steps }) : (flows.data ?? [])
  const state = steps?.length ? { steps, agentNotes: {} } : undefined
  return (
    <div ref={ref} className="space-y-2">
      <Markdown text={text} />
      {stopped ? <p className="text-xs text-muted-foreground">{copy.receivingStopped}</p> : null}
      <Attribution names={names} />
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
      <Activity
        state={state}
        live={false}
        names={state ? undefined : flows.data}
        agents={ctx.agents}
        traceId={m.trace_id ?? null}
        onViewTrace={(id) => handlers.viewTrace(id, false)}
        flows={flows.isError ? { failed: true, retry: () => void flows.refetch() } : undefined}
      />
    </div>
  )
}

/**
 * The §5.6 actions for a routed notice. A resume never re-runs anything (its continuation already
 * ran server-side and saves its own reply), so its only way forward is Refresh status.
 */
function routedActions(
  error: ChatError,
  text: string,
  handlers: RoutedHandlers,
  resume: boolean,
): { actions: NoticeAction[]; links?: React.ReactNode } {
  if (resume)
    return error.key === 'routedReconnectForbidden'
      ? { actions: [] }
      : { actions: [{ label: copy.refreshStatus, onClick: handlers.refresh, primary: true }] }
  switch (error.key) {
    case 'routedBadRequest':
      return {
        actions: [
          { label: copy.editAndSend, onClick: () => handlers.editMessage(text), primary: true },
        ],
      }
    case 'routedRateLimited':
      return { actions: [{ label: copy.retry, onClick: handlers.runAgain, primary: true }] }
    case 'routedNoAgents':
    case 'routedForbidden':
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
    case 'routedMayStillArrive':
      return { actions: [{ label: copy.refreshStatus, onClick: handlers.refresh, primary: true }] }
    case 'routedFailed':
      return { actions: [{ label: copy.runAgain, onClick: handlers.runAgain, primary: true }] }
    default:
      return {
        actions: [
          { label: copy.refreshStatus, onClick: handlers.refresh, primary: true },
          { label: copy.runAgain, onClick: handlers.runAgain },
        ],
      }
  }
}

/**
 * The live block of a routed turn: streaming or kept text, the status line or the end state
 * (§5.5), then Activity. Returns null once there's nothing live left to show.
 */
export function RoutedLive({
  live,
  sessionId,
  hasSaved,
  now,
  ctx,
  handlers,
  tryAgain,
}: {
  live: LiveTurn
  sessionId: string
  hasSaved: boolean
  /** A fixed clock (tests); by default it ticks on its own. */
  now?: number
  ctx: RoutedContext
  handlers: RoutedHandlers
  tryAgain(): void
}) {
  const liveNow = isLivePhase(live.phase)
  const resume = live.operation === 'resume'
  const text = routedReplyText(live.state, resume)
  const traceId = live.state.traceId
  const liveUsage = usageFromMeta(live.state.usage)
  const details = (e: ChatError) => copyDetails(e, { sessionId, traceId })
  const trace = traceId ? (
    <Button
      type="button"
      variant="link"
      className={LINK_BUTTON}
      onClick={() => handlers.viewTrace(traceId, true)}
    >
      {copy.viewTrace}
    </Button>
  ) : null
  const note = (msg: string, action?: React.ReactNode) => (
    <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
      {live.phase === 'loading_saved' ? <Spinner /> : null}
      {msg}
      {action}
    </div>
  )
  // Once history holds the reply the saved rendering takes over, except while an end state needs saying.
  if (hasSaved && (live.phase === 'done' || live.phase === 'paused')) return null

  let end: React.ReactNode = null
  switch (live.phase) {
    case 'draining':
      end = (
        <>
          {note(copy.replyTooLarge)}
          {trace}
        </>
      )
      break
    case 'loading_saved':
      end = note(copy.loadingSavedReply)
      break
    case 'history_failed':
      end = note(
        copy.savedReplyFailed,
        <Button size="sm" className={TOUCH} onClick={handlers.refresh}>
          {copy.retry}
        </Button>,
      )
      break
    case 'known_empty':
      end = (
        <KnownEmpty
          onRunAgain={resume ? handlers.refresh : handlers.runAgain}
          label={resume ? copy.refreshStatus : copy.runAgain}
        >
          {trace}
        </KnownEmpty>
      )
      break
    case 'lost':
    case 'error':
    case 'not_started':
    case 'resume_uncertain':
    case 'resume_forbidden': {
      const e =
        live.error ?? new ChatError({ phase: 'stream', key: 'cutOff', certainty: 'unknown' })
      if (live.phase === 'lost' && text.trim()) {
        end = (
          <div className="space-y-1.5">
            <p className="text-sm text-muted-foreground">{copy.partialUnconfirmed}</p>
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" className={TOUCH} onClick={handlers.refresh}>
                {copy.refreshStatus}
              </Button>
              {resume ? null : (
                <Button size="sm" variant="outline" className={TOUCH} onClick={handlers.runAgain}>
                  {copy.runAgain}
                </Button>
              )}
              {trace}
            </div>
          </div>
        )
        break
      }
      const create = e.key === 'createFailed' || e.key === 'saveUserFailed'
      const { actions, links } = create
        ? {
            actions: [{ label: copy.tryAgain, onClick: tryAgain, primary: true }],
            links: undefined,
          }
        : routedActions(e, live.userText, handlers, resume)
      end = (
        <ErrorNotice
          error={e}
          actions={actions}
          links={
            <>
              {links}
              {live.phase === 'error' ? trace : null}
            </>
          }
          details={details(e)}
        />
      )
      break
    }
    default:
      break
  }
  const showText = !!text.trim() && live.phase !== 'known_empty'
  return (
    <div className="space-y-2" data-testid="routed-live">
      {showText ? <Markdown text={text} live={liveNow} /> : null}
      {liveNow && live.phase !== 'draining' ? <RoutedStatusLine live={live} now={now} /> : null}
      {live.idle && liveNow ? (
        <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
          {copy.idleNotice}{' '}
          <Button size="xs" variant="outline" className={TOUCH} onClick={handlers.refresh}>
            {copy.refreshStatus}
          </Button>
        </div>
      ) : null}
      {end}
      {!liveNow && live.phase === 'done' ? (
        <>
          <Attribution names={agentsAsked(live.state)} />
          {/* The same footer the saved reply will have, so nothing shifts when history takes over. */}
          <div className="flex flex-wrap items-center gap-2">
            <CopyReply text={text} />
            {liveUsage ? <UsageChip usage={liveUsage} /> : null}
            {traceId ? (
              <Button
                size="xs"
                variant="ghost"
                className={TOUCH}
                onClick={() => handlers.viewTrace(traceId, true)}
              >
                {copy.viewTrace}
              </Button>
            ) : null}
          </div>
        </>
      ) : null}
      <Activity
        state={live.state}
        live={liveNow}
        paused={live.phase === 'paused'}
        agents={ctx.agents}
        traceId={traceId}
        onViewTrace={(id) => handlers.viewTrace(id, true)}
      />
    </div>
  )
}

/** A routed attempt that ended empty (EN-5), live or from the turn-end store: says so instead of E5. */
export function KnownEmpty({
  onRunAgain,
  label = copy.runAgain,
  children,
}: {
  onRunAgain(): void
  label?: string
  children?: React.ReactNode
}) {
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm" data-testid="known-empty">
      <span>{copy.finishedWithoutReply}</span>
      <Button size="sm" className={TOUCH} onClick={onRunAgain}>
        {label}
      </Button>
      {children}
    </div>
  )
}
