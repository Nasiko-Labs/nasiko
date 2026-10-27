/**
 * Whether the desktop app is syncing coding-agent data, and the numbers Monitor's import state and the Harnesses card
 * both show (one source: the desktop's state, desktop.ts). It syncs while its run reads or sends history (discovering
 * … sending), while a delivery pass is going, or while turns wait in its queue (a hook's turns included).
 */
import { copy } from './copy'
import type { DesktopState } from './desktop'
import { isActivePhase, pendingTurns, runProgress, sentTurns, shownPhase } from './runs'

type SyncingPhase = 'discovering' | 'connecting' | 'importing' | 'sending'

export interface DesktopSync {
  syncing: boolean
  /** What it is doing: the run's phase while the run is active, else `sending` (background delivery). */
  phase: SyncingPhase | null
  /** Percent done; null while that is unknown (an indeterminate bar). */
  value: number | null
  /** The history window being read, while importing. */
  window: string | null
  /** Turns the active run has sent; null with no active run (a background delivery has no total). */
  sent: number | null
  /** Turns waiting to be sent. */
  waiting: number
}

/** Turns waiting to be sent: the whole queue when the desktop reports it, else the run's own count. */
/** What the numbers need: a browser has a server run and no queue of its own. */
type SyncSource = Pick<DesktopState, 'run' | 'backlog'>

export function waitingTurns(state: SyncSource): number {
  return state.backlog?.pending ?? (state.run ? pendingTurns(state.run) : 0)
}

const IDLE = { phase: null, value: null, window: null, sent: null } as const

export function desktopSync(state: SyncSource): DesktopSync {
  const { run } = state
  const waiting = waitingTurns(state)
  const runPhase = run ? shownPhase(run) : null
  const active = run && runPhase && isActivePhase(runPhase) ? run : null
  const syncing = !!active || !!state.backlog?.sending || waiting > 0
  if (!syncing) return { syncing, waiting, ...IDLE }
  if (!active) return { syncing, waiting, ...IDLE, phase: 'sending' }
  const phase = runPhase as SyncingPhase
  return {
    syncing,
    waiting,
    phase,
    value: runProgress(active, waiting),
    window:
      phase === 'importing'
        ? (active.windows.find((w) => w.status === 'running')?.label ?? null)
        : null,
    sent: phase === 'importing' || phase === 'sending' ? sentTurns(active) : null,
  }
}

/** One line: "Syncing coding agents · Oct 17 · 1,730 turns sent · 499 waiting". */
export const syncLine = (sync: DesktopSync): string =>
  [copy.sync.title, syncDetail(sync)].filter(Boolean).join(' · ')

/** The line without its title, where the title is already on screen: "Oct 17 · 1,730 turns sent · 499 waiting". */
export function syncDetail(sync: DesktopSync): string {
  const c = copy.sync
  const step =
    sync.phase === 'discovering'
      ? c.discovering
      : sync.phase === 'connecting'
        ? c.connecting
        : sync.window
  return [step, sync.sent ? c.sent(sync.sent) : null, sync.waiting ? c.waiting(sync.waiting) : null]
    .filter(Boolean)
    .join(' · ')
}
