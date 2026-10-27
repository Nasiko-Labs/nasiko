/**
 * A coding-agent import run, as a local client reports it (the desktop app's own state, or the server's
 * `GET /api/me/coding-agents/sync` rows the CLI and the desktop report), and the card's pure rules over it.
 *
 * v2 fields (`sending`, `windows`, per-agent `pending`/`rejected`/`rejected_reasons`, `retry_requested_at`) are
 * defaulted here, so an older server's runs still parse and render with the v1 session math. Bare JSON, not in the
 * OpenAPI spec: zod here (oss/server/src/coding_agent_onboarding.rs).
 */
import { z } from 'zod'
import { OLDER_WINDOW_LABEL } from './constants'

// `awaiting_consent` is v1's: older clients may still report it, and the card never shows it.
const PHASES = [
  'awaiting_consent',
  'discovering',
  'connecting',
  'importing',
  'sending',
  'done',
  'failed',
] as const
const IMPORT_STATUSES = ['waiting', 'running', 'done', 'failed', 'skipped'] as const
const WINDOW_STATUSES = ['waiting', 'running', 'done'] as const

export type SyncPhase = (typeof PHASES)[number]

const agentImportSchema = z.looseObject({
  status: z.enum(IMPORT_STATUSES),
  done: z.number(),
  total: z.number(),
  sessions: z.number(),
  turns: z.number(),
  failed: z.number(),
  error: z.string().nullish(),
  // v2: turns queued locally and not yet accepted, and turns the server refused (with why).
  pending: z.number().default(0),
  rejected: z.number().default(0),
  rejected_reasons: z.array(z.looseObject({ reason: z.string(), count: z.number() })).default([]),
})

const syncAgentSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  detected: z.boolean(),
  connected: z.boolean(),
  import: agentImportSchema,
})
export type SyncAgent = z.infer<typeof syncAgentSchema>

/** v2: one slice of history, newest first. `done` = read and every turn accepted. */
const syncWindowSchema = z.looseObject({
  label: z.string(),
  status: z.enum(WINDOW_STATUSES),
  sessions: z.number(),
  turns: z.number(),
})
export type SyncWindow = z.infer<typeof syncWindowSchema>

/** A run without its device: the desktop app's local `run`. */
export const localRunSchema = z.looseObject({
  phase: z.enum(PHASES),
  agents: z.array(syncAgentSchema),
  error: z.string().nullish(),
  started_at: z.string().nullable(),
  updated_at: z.string(),
  windows: z.array(syncWindowSchema).default([]),
  // The desktop app's own counts (server.rs `run_json`), absent from the server's runs: windows whose sessions are
  // all read (their turns may still be on their way), and turns delivered so far.
  windows_read: z.number().optional(),
  delivered: z.number().optional(),
})
export type LocalRun = z.infer<typeof localRunSchema>

/** A run as the server stores it: the device that reports it, and when a Retry was last asked of it. */
export const syncRunSchema = z.looseObject({
  ...localRunSchema.shape,
  device_id: z.string(),
  device_name: z.string(),
  client: z.string(),
  retry_requested_at: z.string().nullish().default(null),
})
export type SyncRun = z.infer<typeof syncRunSchema>

export const syncRunsSchema = z.looseObject({ runs: z.array(syncRunSchema) })
export type SyncRuns = z.infer<typeof syncRunsSchema>

/** Turns a run still has to deliver, over its agents. */
export const pendingTurns = (run: LocalRun) => run.agents.reduce((n, a) => n + a.import.pending, 0)
/** Turns the server refused, over a run's agents. */
export const rejectedTurns = (run: LocalRun) =>
  run.agents.reduce((n, a) => n + a.import.rejected, 0)
/** Turns a run has delivered: the desktop's own count, else those its import queued less those still waiting and
 *  those refused (never below 0: the queue also holds turns the agents' hooks captured). */
export const sentTurns = (run: LocalRun) =>
  run.delivered ??
  Math.max(
    0,
    run.agents.reduce((n, a) => n + a.import.turns, 0) - pendingTurns(run) - rejectedTurns(run),
  )

/** The phase the card shows: a run is never done while turns wait to be sent. */
export const shownPhase = (run: LocalRun): SyncPhase =>
  run.phase === 'done' && pendingTurns(run) > 0 ? 'sending' : run.phase

/** A Retry was asked for and the client hasn't reported since (both are the server's timestamps). */
export const awaitingRetry = (run: SyncRun) =>
  !!run.retry_requested_at && Date.parse(run.updated_at) <= Date.parse(run.retry_requested_at)

export const isActivePhase = (p: SyncPhase) =>
  p === 'discovering' || p === 'connecting' || p === 'importing' || p === 'sending'
export const isFinishedPhase = (p: SyncPhase) => p === 'done' || p === 'failed'

/** History is read newest first; once every window but the oldest ("Older") is in, the last month is in. */
export function lastMonthIn(windows: readonly SyncWindow[]): boolean {
  const older = windows.filter((w) => w.label === OLDER_WINDOW_LABEL)
  const rest = windows.filter((w) => w.label !== OLDER_WINDOW_LABEL)
  return (
    older.length > 0 &&
    rest.length > 0 &&
    older.some((w) => w.status !== 'done') &&
    rest.every((w) => w.status === 'done')
  )
}

/**
 * One run's progress, the one rule the card and Monitor's import state share. Importing: windows read over windows when
 * the client reports them, else sessions processed over sessions to process. Sending: turns sent over turns sent and
 * waiting, where `waiting` is what still waits to be sent (the run's own count unless the caller knows the whole
 * queue's), 99 at most. Null while that is unknown (discovering, connecting, nothing counted yet).
 */
export function runProgress(run: LocalRun, waiting = pendingTurns(run)): number | null {
  const phase = shownPhase(run)
  if (phase === 'done' && waiting === 0) return 100
  if (phase === 'discovering' || phase === 'connecting') return null
  if (phase === 'sending' || phase === 'done') return sendingProgress(sentTurns(run), waiting)
  const share = run.windows.length ? windowShare(run) : sessionShare(run)
  return share === null ? null : Math.round(share * 100)
}

/** Sent over sent and waiting, never full while turns are still on their way; null with nothing counted. */
export function sendingProgress(sent: number, waiting: number): number | null {
  if (sent + waiting === 0) return null
  return Math.min(Math.round((sent / (sent + waiting)) * 100), ALMOST_DONE_PCT)
}

const ALMOST_DONE_PCT = 99
/** Windows read over windows: the desktop's count when it gives one, else the windows done. */
const windowShare = (run: LocalRun) =>
  Math.min(
    run.windows_read ?? run.windows.filter((w) => w.status === 'done').length,
    run.windows.length,
  ) / run.windows.length
function sessionShare(run: LocalRun): number | null {
  const total = run.agents.reduce((n, a) => n + a.import.total, 0)
  if (!total) return null
  return run.agents.reduce((n, a) => n + Math.min(a.import.done, a.import.total), 0) / total
}

/** A finished run's totals over the agents that imported. */
export function runTotals(run: LocalRun) {
  const imported = run.agents.filter((a) => a.import.status === 'done')
  return {
    sessions: imported.reduce((n, a) => n + a.import.sessions, 0),
    turns: imported.reduce((n, a) => n + a.import.turns, 0),
    agents: imported,
  }
}
