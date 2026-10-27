/**
 * The hub's bounded session sample (plans/feat-agent-centric.md eng "Hub sessions"; replaces `useAgentSessions`, whose
 * start could exceed Tempo's 7-day search and lose every row's trace data). Pure.
 * - Start: the later of the agent's creation and 7 days back (Sessions' Tempo-safe start), so every row can be checked.
 * - Rows: `session/list` names agents by raw name ("" when unresolved); only this agent's rows created after it.
 *   Another visible agent with the same raw name makes the match ambiguous: the rows can't be told apart, so the lists
 *   and Needs you say so with what they show.
 */
import { tempoSafeStart } from '@nasiko/features/sessions/api'
import type { SessionSummary } from '@nasiko/features/observability/types'
import { SESSIONS_DAYS } from './tuning'

const MINUTE = 60_000
const DAY = 86_400_000

/** Floored to the minute, so the sample's key holds still across renders and a remount (review: performance). */
export function sampleStart(createdAt: string, nowInput: Date): Date {
  const now = new Date(Math.floor(nowInput.getTime() / MINUTE) * MINUTE)
  const week = tempoSafeStart(new Date(now.getTime() - SESSIONS_DAYS * DAY), now)
  const created = Date.parse(createdAt)
  return Number.isNaN(created) || created < week.getTime() ? week : new Date(created)
}

export function agentRows(
  rows: readonly SessionSummary[],
  name: string,
  start: Date,
): SessionSummary[] {
  return rows.filter(
    (s) => s.agent_id === name && (!s.start_time || Date.parse(s.start_time) >= start.getTime()),
  )
}
