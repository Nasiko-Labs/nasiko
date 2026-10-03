/** Lifecycle action state shared by the Your agents rows and the detail header (plan §6.2). */
import { useState } from 'react'
import { useLifecycle, useWatch } from './api'
import { copy } from './copy'
import type { LifecycleAction } from './status'

export function useLifecycleFlow(id: string) {
  const m = useLifecycle(id)
  const w = useWatch(id)
  const [stopOpen, setStopOpen] = useState(false)
  const [stopped, setStopped] = useState(false)
  const run = (action: LifecycleAction) => {
    if (action === 'stop') return setStopOpen(true)
    setStopped(false)
    w.clear()
    m.mutate(action, { onSuccess: () => w.begin(action) })
  }
  const confirmStop = () =>
    m.mutate('stop', {
      onSuccess: () => {
        setStopOpen(false)
        setStopped(true)
        w.clear()
      },
    })
  return { m, w, run, stopOpen, setStopOpen, confirmStop, stopped }
}

export type LifecycleFlow = ReturnType<typeof useLifecycleFlow>

export const ACTION_LABEL: Record<LifecycleAction, string> = {
  restart: copy.restart,
  stop: copy.stop,
  start: copy.start,
}

export interface Outcome {
  text: string
  tone: 'muted' | 'warning'
}

/** The outcome line under the controls; null while nothing happened. */
export function outcomeText(flow: LifecycleFlow): Outcome | null {
  const { watch } = flow.w
  if (flow.stopped) return { text: copy.stoppedDone, tone: 'muted' }
  // The restart call itself redeploys and can take ~10 s before it answers: say so meanwhile.
  if (flow.m.isPending && (flow.m.variables === 'restart' || flow.m.variables === 'start')) {
    return { text: flow.m.variables === 'restart' ? copy.restarting : copy.starting, tone: 'muted' }
  }
  if (!watch || watch.kind === 'rollback') return null
  if (!watch.outcome)
    return { text: watch.kind === 'restart' ? copy.restarting : copy.starting, tone: 'muted' }
  if (watch.outcome === 'crashed') return { text: copy.crashedAgain, tone: 'warning' }
  if (watch.outcome === 'timeout') return { text: copy.stillStarting, tone: 'warning' }
  return { text: watch.kind === 'restart' ? copy.restarted : copy.started, tone: 'muted' }
}
