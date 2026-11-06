/**
 * The try-a-run card's rules (components/TryAgentRun.tsx): a call to run the agent the viewer picked during setup, then
 * the same task with Token optimization on. Simulated for now: a run is a timed walk through
 * starting → running → completed, then the button resets. Which runs finished is kept per viewer in this tab
 * (sessionStorage), so the card outlives the refresh it asks for (after the first run the optimized one is still
 * offered, and once both have run it shows only while the workspace has no agent activity), while a new tab or browser
 * starts the demo over from no runs, as the `try-run` mock workspace (mocks/tryRun.ts) then starts from no spend.
 */
import { DRAFT_PREFIX } from '@nasiko/app-core/draftKeys'
import { mandatoryAgent, SEEDED_AGENTS } from '@nasiko/features/onboarding/logic'
import { TRY_RUN_COMPLETED_MS, TRY_RUN_RUNNING_MS, TRY_RUN_STARTING_MS } from './tuning'

/** The agent the viewer picked during setup: the guide's always-included prebuilt one (a seed agent, so the mock records
 *  the runs against it). */
export const TRY_RUN_AGENT = mandatoryAgent(SEEDED_AGENTS).name

export type TryRunKind = 'baseline' | 'optimized'
export type TryRunPhase = 'idle' | 'starting' | 'running' | 'completed'

/** The phase after `phase` and how long `phase` lasts; idle has no next. */
export function nextPhase(phase: TryRunPhase): { next: TryRunPhase; afterMs: number } | null {
  switch (phase) {
    case 'starting':
      return { next: 'running', afterMs: TRY_RUN_STARTING_MS }
    case 'running':
      return { next: 'completed', afterMs: TRY_RUN_RUNNING_MS }
    case 'completed':
      return { next: 'idle', afterMs: TRY_RUN_COMPLETED_MS }
    case 'idle':
      return null
  }
}

/** Per tab, keyed by viewer: a refresh keeps the runs, a new tab starts over. */
const doneKey = (sub: string) => `${DRAFT_PREFIX}${sub}:meta:tryRun`

/** The runs this viewer finished in this tab; blocked storage reads as none. */
export function readDone(sub: string): TryRunKind[] {
  try {
    const raw = globalThis.sessionStorage?.getItem(doneKey(sub))
    return raw === 'optimized' ? ['baseline', 'optimized'] : raw === 'baseline' ? ['baseline'] : []
  } catch {
    return []
  }
}

export function writeDone(sub: string, kind: TryRunKind) {
  try {
    globalThis.sessionStorage?.setItem(doneKey(sub), kind)
  } catch {
    // Blocked storage: the card forgets the run on refresh.
  }
}

/**
 * Whether Home shows the card: while nothing has run in the range (a workspace fresh from setup), and between the two
 * runs, so the refresh after the first still offers the optimized one. Never in a workspace that already had spend.
 * `runs` is null while the range's totals are unknown.
 */
export function showTryRun(done: readonly TryRunKind[], runs: number | null): boolean {
  return runs === 0 || (done.includes('baseline') && !done.includes('optimized'))
}
