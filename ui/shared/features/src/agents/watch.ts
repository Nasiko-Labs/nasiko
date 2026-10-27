/**
 * Truth checks after Restart/Start and roll back (plan §6.2). The server answers these
 * actions before the outcome is known: restart writes `running` synchronously
 * (admin/routes.rs:568-575), and a roll back only queues a build: `deploying` is written when
 * a worker claims it (agents/update.rs:855-915), which a busy queue can delay indefinitely.
 * So one poll can't be trusted; `stepWatch` folds each observed status
 * into a watch until it has a real outcome.
 *
 *   restart/start:  ignore polls < GRACE ─▶ Running ×STABLE_POLLS ─▶ done
 *                                        └▶ Needs attention       ─▶ crashed
 *                   failed polls only expire the watch at the cap (expireWatch)
 *   roll back:      wait until Deploying seen ─▶ leaves Deploying ─▶ done | crashed
 *                   (never seen by the cap ─▶ timeout: a queued build is not a rollback)
 *   any:            now − startedAt ≥ CAP ─▶ timeout ("Keep watching" starts a new watch)
 */
import type { DisplayStatus } from './status'
import { GRACE_MS, STABLE_POLLS, WATCH_CAP_MS } from './tuning'

export type WatchKind = 'restart' | 'start' | 'rollback'
type WatchOutcome = 'done' | 'crashed' | 'timeout'

export interface Watch {
  kind: WatchKind
  startedAt: number
  buildId?: string
  seenDeploying: boolean
  stable: number
  outcome: WatchOutcome | null
}

export function startWatch(kind: WatchKind, now: number, buildId?: string): Watch {
  return { kind, startedAt: now, buildId, seenDeploying: false, stable: 0, outcome: null }
}

export function stepWatch(w: Watch, observed: DisplayStatus, now: number): Watch {
  if (w.outcome) return w
  const elapsed = now - w.startedAt
  if (w.kind === 'rollback') {
    const seenDeploying = w.seenDeploying || observed === 'deploying'
    if (seenDeploying && observed !== 'deploying') {
      return { ...w, seenDeploying, outcome: observed === 'attention' ? 'crashed' : 'done' }
    }
    return elapsed >= WATCH_CAP_MS
      ? { ...w, seenDeploying, outcome: 'timeout' }
      : { ...w, seenDeploying }
  }
  if (elapsed < GRACE_MS) return { ...w, stable: 0 }
  if (observed === 'attention') return { ...w, outcome: 'crashed' }
  // Readings win over the cap: a first poll after a hidden tab can still report the outcome.
  // Past the cap one Running reading is enough (the synchronous write is long gone).
  if (observed === 'running') {
    const stable = w.stable + 1
    if (stable >= STABLE_POLLS || elapsed >= WATCH_CAP_MS) return { ...w, stable, outcome: 'done' }
    return { ...w, stable }
  }
  return elapsed >= WATCH_CAP_MS ? { ...w, stable: 0, outcome: 'timeout' } : { ...w, stable: 0 }
}

/** A failed poll is no reading: it can only end the watch at the cap (else "Restarting…" would stick). */
export function expireWatch(w: Watch, now: number): Watch {
  return !w.outcome && now - w.startedAt >= WATCH_CAP_MS ? { ...w, outcome: 'timeout' } : w
}

export const isWatching = (w: Watch | null | undefined): w is Watch => !!w && !w.outcome
