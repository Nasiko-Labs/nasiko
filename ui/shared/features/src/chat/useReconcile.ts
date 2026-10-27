/**
 * Applies `reconcile.ts`'s decisions for the open chat (EN14, §5.4): when its live turn ends, when history
 * moves, and the routed re-check backoff. The decisions are pure; this only schedules and dispatches them.
 */
import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useEffectEvent, useRef } from 'react'
import {
  NO_RECHECKS,
  reconcileDirect,
  reconcileRouted,
  routedMatches,
  turnEnded,
  type ReconcileAction,
  type ReconcileInput,
} from './reconcile'
import { invalidateChat } from './registry'
import type { TurnRegistry } from './turnRegistry'

export function useReconcile(
  registry: TurnRegistry,
  sessionId: string,
  view: Omit<ReconcileInput, 'endedAt'>,
  refetchHistory: () => void,
) {
  const client = useQueryClient()
  // When this view saw the turn end, and the routed re-check budget: per view, never rendered.
  const endedAt = useRef<number | null>(null)
  const rechecks = useRef(NO_RECHECKS)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)
  useEffect(() => () => clearTimeout(timer.current), [])

  const dispatch = (action: ReconcileAction) => {
    if (action.type === 'forget') registry.forget(sessionId)
    else if (action.type === 'reconcile') registry.reconcile(sessionId, action.result)
    else if (action.type === 'recheck') {
      registry.reconcile(sessionId, 'no_match')
      clearTimeout(timer.current)
      timer.current = setTimeout(refetchHistory, action.delayMs)
    }
  }

  // A turn that just ended: refetch, then forget it once history has caught up.
  const onPhase = useEffectEvent(() => {
    const next = turnEnded(view.live, endedAt.current, Date.now())
    endedAt.current = next.endedAt
    if (next.refresh === 'invalidate') void invalidateChat(client, sessionId)
    else if (next.refresh === 'refetch') refetchHistory()
  })
  // Each reads the view as it stands when its triggers below change, as the page always has.
  const onRouted = useEffectEvent(() => {
    const out = reconcileRouted({ ...view, endedAt: endedAt.current }, rechecks.current)
    rechecks.current = out.rechecks
    dispatch(out.action)
  })
  const onDirect = useEffectEvent(() =>
    dispatch(reconcileDirect({ ...view, endedAt: endedAt.current })),
  )

  const phase = view.live?.phase
  const { dataUpdatedAt, errorUpdatedAt } = view.history
  const routedMatch = routedMatches(view)
  useEffect(() => onPhase(), [phase])
  useEffect(
    () => onRouted(),
    [phase, dataUpdatedAt, errorUpdatedAt, routedMatch, view.pending, registry, sessionId],
  )
  useEffect(
    () => onDirect(),
    [phase, dataUpdatedAt, view.pending, view.newReplies, registry, sessionId],
  )
}
