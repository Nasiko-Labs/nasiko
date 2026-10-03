/** `?debug=turn` and dev-only lines (DX9, NC-2, DX-7): frames, signals, Waiting matches, the routed metric. */
import { skipToken, useQueries } from '@tanstack/react-query'
import { ChevronRight } from 'lucide-react'
import { useEffect } from 'react'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { cn } from '@/lib/utils'
import { chatKeys } from '../api'
import type { TurnHandlers } from './TurnView'
import { copy } from '../copy'
import { useSignals } from '../registry'
import { signalsDebug } from '../signals'
import { CHAT_COLUMN, DISCLOSE } from './turnStyles'
import { useWaitingDebug } from '../waiting'
import { unknownMockEntries } from '../search'
import { attemptKeyOf, type LiveTurn, type TurnEnd } from '../turnRegistry'
import { agentsAsked, routedMetric } from '../activity'
import { CHAT_PAGE_VARIANT_KEYS, CHAT_SCENARIO_KEYS } from '../scenarioKeys'
import type { ChatMessage } from '../types'

function DebugWaiting({ userId, sessionId }: { userId: string; sessionId: string }) {
  const w = useWaitingDebug(userId, sessionId)
  return (
    <p data-testid="debug-waiting">
      {copy.debugWaiting(w?.source ?? null, w?.requests.join(', ') ?? '')}
    </p>
  )
}

export function DebugFrames({
  live,
  sessionId,
  userId,
}: {
  live: LiveTurn | undefined
  sessionId?: string
  userId?: string
}) {
  // DX9: this chat's last look, its completions and how its waiting requests were matched, next to the frames.
  const snap = useSignals()
  const signal = sessionId && snap ? signalsDebug(snap, sessionId) : undefined
  const signalLine = signal ? (
    <div className={cn(CHAT_COLUMN, 'text-xs text-muted-foreground')}>
      <p data-testid="debug-signals">
        {copy.debugSignals(
          signal.lastSeenAt === null ? null : String(signal.lastSeenAt),
          signal.completions.map((c) => `${c.kind}@${c.attempt}`).join(', '),
        )}
      </p>
      {userId && sessionId ? <DebugWaiting userId={userId} sessionId={sessionId} /> : null}
    </div>
  ) : null
  if (!live) return signalLine
  // v1b E-A7, NX-9: what decides a routed turn's end.
  const routedInfo =
    live.chatMode === 'routed'
      ? ` · routed/${live.operation} · attempt ${attemptKeyOf(live)} · settle ${live.state.traceId ?? 'baseline'} · asked ${agentsAsked(live.state).join(', ') || 'none'}`
      : ''
  return (
    <>
      {signalLine}
      <Collapsible className={cn(CHAT_COLUMN, 'text-xs')}>
        <CollapsibleTrigger asChild>
          <Button variant="ghost" size="xs" className={cn(DISCLOSE, 'text-left whitespace-normal')}>
            <ChevronRight aria-hidden />
            {copy.debugFrames} · {live.phase} · {live.frames.length}
            {routedInfo}
          </Button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <pre className="max-h-60 overflow-auto rounded bg-muted p-2 whitespace-pre-wrap">
            {live.frames.join('\n')}
          </pre>
        </CollapsibleContent>
      </Collapsible>
    </>
  )
}

/**
 * `?debug=turn` on a routed chat (NC-2): how often OpenRuntime called no agent, one or several,
 * over the loaded replies whose agents are known (this tab's steps, or flows already fetched),
 * and how many attempts this tab saw end empty.
 */
export function RoutedMetricLine({
  replies,
  stepsFor,
  ends,
}: {
  replies: ChatMessage[]
  stepsFor: NonNullable<TurnHandlers['stepsFor']>
  ends: TurnEnd[]
}) {
  // Reads the flows the replies already fetched (never fetches): a subscribed read, not the cache mid-render.
  const flows = useQueries({
    queries: replies.map((m) => ({
      queryKey: chatKeys.flows(m.trace_id),
      queryFn: skipToken,
      staleTime: Infinity,
    })),
  })
  const known = replies.flatMap((m, i) => {
    const steps = stepsFor(m.id, m.trace_id)
    const names = steps?.length
      ? agentsAsked({ steps })
      : m.trace_id
        ? (flows[i]?.data as string[] | undefined)
        : undefined
    return names ? [{ agents: names.length, empty: false }] : []
  })
  const m = routedMetric([
    ...known,
    ...ends.filter((e) => e.kind === 'empty').map(() => ({ agents: 0, empty: true })),
  ])
  return (
    <p
      className={cn(CHAT_COLUMN, 'text-xs text-muted-foreground tabular-nums')}
      data-testid="routed-metric"
    >
      {copy.routedMetric(m)}
    </p>
  )
}

/** Dev only (DX-7, NX-7; v1c DX1): `?mock=` entries Chat doesn't know fall back to the default; name each one. */
export function UnknownScenarioBanner({ mock }: { mock: string | undefined }) {
  const unknown = import.meta.env.DEV ? unknownMockEntries(mock) : []
  const names = unknown.join(', ')
  useEffect(() => {
    if (names) console.warn(`[chat] unknown mock entry '${names}'; using the default`)
  }, [names])
  if (!names) return null
  return (
    <p
      role="note"
      className="border-b border-warning/40 bg-warning/5 px-3 py-1.5 text-xs"
      data-testid="unknown-scenario"
    >
      {copy.unknownScenario(names, [...CHAT_SCENARIO_KEYS, ...CHAT_PAGE_VARIANT_KEYS].join(', '))}
    </p>
  )
}
