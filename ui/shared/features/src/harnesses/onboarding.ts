/**
 * Coding-agent onboarding on Harnesses (contract v3). Two modes, decided by the desktop probe (desktop.ts):
 *
 * - **Desktop app**: consent is local. The card asks, and Yes / Not now / Retry are the desktop's own POSTs; progress
 *   is the desktop's local run.
 * - **Browser**: no consent. Running `nasiko agents setup` is the consent, so the card shows the command, then the
 *   progress the CLI (or a desktop elsewhere) reports to the server:
 *
 *     GET  /api/me/coding-agents/sync                       {runs: [Run]}  (updated in the last 24 h, newest first)
 *     POST /api/me/coding-agents/sync/{device_id}/retry     the Run, `retry_requested_at` set (404: no such device)
 *
 * A bare 404 from the sync read is an older server: no runs, the command card still shows.
 */
import { useMutation, useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { ApiError, apiFetch } from '@nasiko/data-access/client'
import { copy } from './copy'
import { SYNC_IDLE_POLL_MS, SYNC_POLL_MS, SYNC_SENDING_POLL_MS } from './constants'
import { useDesktopAction, useDesktopState, type DesktopAction, type DesktopState } from './desktop'
import {
  awaitingRetry,
  isActivePhase,
  isFinishedPhase,
  rejectedTurns,
  shownPhase,
  syncRunSchema,
  syncRunsSchema,
  type LocalRun,
  type SyncRun,
  type SyncRuns,
} from './runs'
import { waitingTurns } from './syncStatus'

const SYNC = '/api/me/coding-agents/sync'

export const onboardingKeys = {
  all: ['harnesses', 'onboarding'] as const,
  runs: ['harnesses', 'onboarding', 'sync'] as const,
}

// ─── Pure rules ─────────────────────────────────────────────────────────────────────────────────────────────────────

/**
 * What the card shows. Browser: `command` (no run yet), then `running` / `finished`. Desktop: `ask` (consent
 * unasked), `declined`, `idle` (granted, no run), then `running` / `finished`.
 */
export type CardView =
  | { kind: 'command' }
  | { kind: 'ask' }
  | { kind: 'declined' }
  | { kind: 'idle' }
  | { kind: 'running'; runs: readonly SyncRun[] }
  | { kind: 'finished'; runs: readonly SyncRun[] }

function progressView(runs: readonly SyncRun[]): CardView | null {
  const shown = runs.filter((r) => r.phase !== 'awaiting_consent')
  if (shown.some((r) => isActivePhase(shownPhase(r)))) return { kind: 'running', runs: shown }
  if (shown.some((r) => isFinishedPhase(shownPhase(r)))) return { kind: 'finished', runs: shown }
  return null
}

/** Browser: the server's runs, or the command while there is none. */
export const browserView = (runs: readonly SyncRun[]): CardView =>
  progressView(runs) ?? { kind: 'command' }

/** The desktop's local run in the card's shape (it has no device of its own: it is this computer). */
export const localAsSyncRun = (run: LocalRun): SyncRun => ({
  ...run,
  device_id: 'local',
  device_name: copy.agentsOnboarding.thisComputer,
  client: 'desktop',
  retry_requested_at: null,
})

/** Desktop: ask first; a declined viewer gets one quiet line unless a run is still going; then the run. */
export function desktopView(state: DesktopState): CardView {
  if (state.consent === 'unasked') return { kind: 'ask' }
  const run = state.run ? localAsSyncRun(state.run) : null
  const progress = run ? progressView([run]) : null
  if (state.consent === 'declined' && progress?.kind !== 'running') return { kind: 'declined' }
  return progress ?? { kind: 'idle' }
}

const needsAttention = (runs: readonly SyncRun[]) =>
  runs.some((r) => r.phase === 'failed' || rejectedTurns(r) > 0)

/**
 * Whether the card is on the page. Always while it asks or a run is active, and for a run that failed or had turns
 * refused. Browser: a finished run shows too (a run exists); the command only for a viewer with no harnesses yet.
 * Desktop: the rest only for a viewer with no harnesses yet, or one who took part during this visit.
 */
export function cardVisible(
  view: CardView,
  mode: 'browser' | 'desktop',
  noHarnesses: boolean,
  engaged: boolean,
): boolean {
  if (view.kind === 'ask' || view.kind === 'running') return true
  if (view.kind === 'finished' && (mode === 'browser' || needsAttention(view.runs))) return true
  return noHarnesses || engaged
}

/**
 * Browser: how often to re-read the server's runs, or false to stop. Every `SYNC_POLL_MS` while a run reads
 * history; every `SYNC_SENDING_POLL_MS` while runs only deliver queued turns or wait for a requested Retry; every
 * `SYNC_IDLE_POLL_MS` while the command card waits for a first run; never once the runs are finished.
 */
export function pollInterval(view: CardView): number | false {
  if (view.kind === 'command') return SYNC_IDLE_POLL_MS
  if (view.kind !== 'running' && view.kind !== 'finished') return false
  const phases = view.runs.map(shownPhase)
  if (phases.some((p) => isActivePhase(p) && p !== 'sending')) return SYNC_POLL_MS
  if (phases.includes('sending') || view.runs.some(awaitingRetry)) return SYNC_SENDING_POLL_MS
  return false
}

// ─── Browser: the server's runs ─────────────────────────────────────────────────────────────────────────────────────

/** The caller's runs. `interval` decides, from the latest answer, when to read again (false: don't). Re-read when the
 *  window regains focus (the viewer may have just run the command); intervals pause while the page is hidden. */
export function useSyncRuns({
  enabled,
  interval,
}: {
  enabled: boolean
  interval: (runs: SyncRun[] | undefined) => number | false
}) {
  return useQuery({
    queryKey: onboardingKeys.runs,
    // apiFetch only checks the body; parsing again fills the v2 defaults an older server leaves out.
    queryFn: async ({ signal }) =>
      syncRunsSchema.parse(await apiFetch<SyncRuns>(SYNC, { signal, schema: syncRunsSchema })).runs,
    enabled,
    refetchInterval: (q) => interval(q.state.data),
    refetchOnWindowFocus: true,
    // A poll: the next tick is its retry, and an older server's bare 404 must settle at once.
    retry: false,
  })
}

/** Ask a device's client to resend the turns the server refused. A mutation: never retried. The answer is the run,
 *  which replaces the cached one so the confirmation shows before the next poll. */
export function useRetryRejected() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async (deviceId: string) =>
      syncRunSchema.parse(
        await apiFetch<SyncRun>(`${SYNC}/${encodeURIComponent(deviceId)}/retry`, {
          method: 'POST',
          schema: syncRunSchema,
        }),
      ),
    onSuccess: (run) =>
      qc.setQueryData<SyncRun[]>(onboardingKeys.runs, (runs) =>
        runs?.map((r) => (r.device_id === run.device_id ? run : r)),
      ),
  })
}

/** An import finished: harness usage, the agent lists (harness rows), Sessions and Chat's rail can show new rows. */
function refreshImported(qc: QueryClient) {
  void qc.invalidateQueries({
    predicate: (q) => q.queryKey[0] === 'harnesses' && q.queryKey[1] !== 'onboarding',
  })
  // The restart watches (`['agents','watch',id]`) are cache-only state and stay (as deploy/follower.ts does).
  void qc.invalidateQueries({ queryKey: ['agents'], predicate: (q) => q.queryKey[1] !== 'watch' })
  void qc.invalidateQueries({ queryKey: ['sessions'] })
  void qc.invalidateQueries({ queryKey: ['chat', 'list'] })
}

/** A run that reaches `done` while this page watches it brings new rows: re-read what shows them. */
function useRefreshWhenDone(runs: readonly SyncRun[] | undefined) {
  const qc = useQueryClient()
  const seen = useRef<Map<string, string> | null>(null)
  useEffect(() => {
    if (!runs) return
    const prev = seen.current
    const next = new Map(runs.map((r) => [r.device_id, `${r.phase}@${r.updated_at}`]))
    seen.current = next
    if (
      prev &&
      runs.some((r) => r.phase === 'done' && prev.get(r.device_id) !== next.get(r.device_id))
    )
      refreshImported(qc)
  }, [runs, qc])
}

// ─── The card's state ───────────────────────────────────────────────────────────────────────────────────────────────

/** Retry for one run's refused turns, whichever mode asks. */
export interface RetryControl {
  send: (run: SyncRun) => void
  pending: (run: SyncRun) => boolean
  failed: (run: SyncRun) => boolean
  /** Asked, and the client hasn't picked it up yet. */
  requested: (run: SyncRun) => boolean
}

export interface DesktopControl {
  act: (action: DesktopAction) => void
  /** The action in flight, if any. */
  busy: DesktopAction | null
  /** The last action, when it failed (Try again repeats it), and the desktop's own words for why. */
  failedAction: DesktopAction | null
  error: string | null
}

export interface AgentsOnboarding {
  /** The card is on the page (so the CLI connect panel steps aside). */
  visible: boolean
  mode: 'browser' | 'desktop'
  view: CardView | null
  retry: RetryControl
  /** Desktop only: its POSTs. */
  desktop: DesktopControl | null
  /** Desktop only: turns waiting in its whole queue (Monitor's number too); null in a browser (each run's own). */
  waiting: number | null
}

/**
 * The card's state for the viewer's own page. `enabled` is false for anyone else's (onboarding is the viewer's).
 * `noHarnesses`: the viewer has no registered harness yet.
 */
export function useAgentsOnboarding({
  enabled,
  noHarnesses,
}: {
  enabled: boolean
  noHarnesses: boolean
}): AgentsOnboarding {
  const [acted, setActed] = useState(false)
  const [sawActive, setSawActive] = useState(false)
  const engaged = acted || sawActive

  const desktopQ = useDesktopState(enabled)
  const desktopState = desktopQ.data ?? null
  const mode = desktopState ? 'desktop' : 'browser'
  const action = useDesktopAction()
  const retryServer = useRetryRejected()

  const runsQ = useSyncRuns({
    // The server's runs are the browser's progress; the desktop reads its own.
    enabled: enabled && desktopQ.isSuccess && !desktopState,
    interval: (runs) => {
      const v = browserView(runs ?? [])
      return cardVisible(v, 'browser', noHarnesses, engaged) && pollInterval(v)
    },
  })

  // Nothing until the probe (and in a browser, the runs) answer, so the card never flashes the wrong mode or state. A
  // failed runs read (an older server's bare 404 included) reads as no runs.
  const view: CardView | null =
    !enabled || desktopQ.isPending
      ? null
      : desktopState
        ? desktopView(desktopState)
        : runsQ.isPending
          ? null
          : browserView(runsQ.data ?? [])
  // Seen running during this visit: the card stays once the run finishes (React's "adjust state on change" pattern).
  if (view?.kind === 'running' && !sawActive) setSawActive(true)

  const shownRuns = view?.kind === 'running' || view?.kind === 'finished' ? view.runs : undefined
  useRefreshWhenDone(shownRuns)

  const act = (a: DesktopAction) => {
    setActed(true)
    action.mutate(a)
  }
  const retry: RetryControl = desktopState
    ? {
        send: () => act('retry'),
        pending: () => action.isPending && action.variables === 'retry',
        failed: () => action.isError && action.variables === 'retry',
        requested: (r) =>
          action.isSuccess && action.variables === 'retry' && shownPhase(r) !== 'done',
      }
    : {
        send: (r) => {
          setActed(true)
          retryServer.mutate(r.device_id)
        },
        pending: (r) => retryServer.isPending && retryServer.variables === r.device_id,
        failed: (r) => retryServer.isError && retryServer.variables === r.device_id,
        requested: awaitingRetry,
      }

  return {
    visible: !!view && cardVisible(view, mode, noHarnesses, engaged),
    mode,
    view,
    retry,
    desktop: desktopState
      ? {
          act,
          busy: action.isPending ? (action.variables ?? null) : null,
          failedAction: action.isError ? (action.variables ?? null) : null,
          error: action.error instanceof ApiError ? action.error.serverMessage : null,
        }
      : null,
    waiting: desktopState ? waitingTurns(desktopState) : null,
  }
}
