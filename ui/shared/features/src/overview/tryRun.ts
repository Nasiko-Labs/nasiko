/**
 * The try-a-run card's rules (components/TryAgentRun.tsx): a call to run the agent the viewer picked during setup, then
 * the same task with Token optimization on. Each run is real (tryRunApi.ts): sent to the agent, metered by the LLM
 * router, and read back from the stream as tokens and cost. Which runs finished, and what each cost, is kept per viewer
 * in this tab (sessionStorage), so the optimized run can be compared with the first and the card outlives a refresh,
 * while a new tab or browser starts over.
 */
import { DRAFT_PREFIX } from '@nasiko/app-core/draftKeys'
import { MANDATORY } from '@nasiko/features/onboarding/logic'

/** The agent the viewer picked during setup: the guide's always-included prebuilt one, which its Continue deploys. */
export const TRY_RUN_AGENT = MANDATORY

export type TryRunKind = 'baseline' | 'optimized'
/** Starting until the agent picks the task up, running until its reply ends, then completed for a moment. */
export type TryRunPhase = 'idle' | 'starting' | 'running' | 'completed'

/** What a finished run cost (null where the stream carried no usage). */
export interface TryRunFigures {
  tokens: number | null
  costUsd: number | null
}

/** How much less the optimized run cost than the first, as a whole percent; null when either cost is unknown or zero,
 *  or when it didn't cost less. */
export function savedPercent(baseline?: TryRunFigures, optimized?: TryRunFigures): number | null {
  const before = baseline?.costUsd
  const after = optimized?.costUsd
  if (before == null || after == null || before <= 0 || after >= before) return null
  return Math.round(((before - after) / before) * 100)
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

const figuresKey = (sub: string) => `${DRAFT_PREFIX}${sub}:meta:tryRunFigures`

/** What each finished run cost, for the optimized run's comparison; blocked or malformed storage reads as none. */
export function readFigures(sub: string): Partial<Record<TryRunKind, TryRunFigures>> {
  try {
    const raw = globalThis.sessionStorage?.getItem(figuresKey(sub))
    const parsed: unknown = raw ? JSON.parse(raw) : null
    return parsed && typeof parsed === 'object'
      ? (parsed as Partial<Record<TryRunKind, TryRunFigures>>)
      : {}
  } catch {
    return {}
  }
}

export function writeFigures(sub: string, kind: TryRunKind, figures: TryRunFigures) {
  try {
    globalThis.sessionStorage?.setItem(
      figuresKey(sub),
      JSON.stringify({ ...readFigures(sub), [kind]: figures }),
    )
  } catch {
    // Blocked storage: the optimized run is shown without the comparison.
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
