/** A swimlane bar's text: its short task label and its whole story (accessible name and tooltip, F25). */
import type { Call } from './calls'
import { copy, stepStatusLabel } from './copy'
import { fmtDuration } from './precision'

const TASK_CHARS = 48

export const taskOf = (c: Call) => {
  const t = (c.input ?? '').replace(/\s+/g, ' ').trim()
  return t.length > TASK_CHARS ? `${t.slice(0, TASK_CHARS - 1)}…` : t
}

/** The bar's whole story, for its accessible name and tooltip. */
export function barLabel(c: Call, from: string | null, onPath: boolean): string {
  const parts = [
    c.agentName ?? copy.panel.unknownAgent,
    c.input ? taskOf(c) : null,
    stepStatusLabel(c.status),
    // No ticking seconds in a name: a focused bar would be re-announced every second.
    c.endMs === null ? copy.steps.stillRunning : fmtDuration(c.endMs - c.startMs, c.exactEnd),
    from ? copy.panel.from(from) : null,
    onPath ? copy.panel.critical : null,
    c.maybeSame ? copy.panel.maybeSame : null,
  ]
  return parts.filter(Boolean).join(' · ')
}
