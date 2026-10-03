/**
 * Needs you (plans/feat-overview.md §6): one inbox from four sources, most urgent first. Pure.
 *
 * - Each source loads and fails on its own; a failed source adds a "Couldn't check …" line and the rest still show
 *   (design review 5A). A source the server doesn't have (a bare 404, e.g. budgets) is absent, not failed.
 * - "Nothing needs you" is only claimed when every available source answered with nothing (5A).
 * - Requests come from Chat's Waiting pipeline, so a superuser only sees requests provably theirs (eng review R5).
 */
import type { HitlKind } from '@/features/chat/types'

export type SourceId = 'requests' | 'agents' | 'budgets' | 'sessions'
export type SourceState<T> =
  { state: 'loading' } | { state: 'failed' } | { state: 'absent' } | { state: 'ok'; value: T }

export type Severity = 'action' | 'watch' | 'waiting'

export interface RequestChat {
  sessionId: string
  /** The oldest pending request (the card it opens on). */
  firstId: string
  kind: HitlKind
  count: number
  chatTitle: string
  at: string
}

export interface NeedsInput {
  requests: SourceState<{ chats: RequestChat[]; outside: number }>
  /** `incomplete`: some rating inputs failed, so the rows are real but the source can't vouch for "nothing else". */
  agents: SourceState<{ id: string; name: string; reason: string; budget: boolean }[]> & {
    incomplete?: boolean
  }
  budgets: SourceState<
    { id: string; label: string; crossed: number | null; state: 'warning' | 'exceeded' }[]
  >
  sessions: SourceState<{ failed: number; checked: number; agents: string[] }>
}

export type NeedRow =
  | { key: string; kind: 'request'; severity: 'waiting'; chat: RequestChat }
  | { key: string; kind: 'outside'; severity: 'waiting'; count: number }
  | {
      key: string
      kind: 'agent'
      severity: 'action'
      id: string
      name: string
      reason: string
      budget: boolean
    }
  | {
      key: string
      kind: 'budget'
      severity: 'watch'
      label: string
      crossed: number | null
      state: 'warning' | 'exceeded'
    }
  | {
      key: string
      kind: 'sessions'
      severity: 'action'
      failed: number
      checked: number
      agents: string[]
    }

export interface Needs {
  rows: NeedRow[]
  /** Sources that failed (or answered only partly), in inbox order. */
  failed: SourceId[]
  /** Sources the server has (not absent): all of them failing is one card error. */
  available: number
  /** Some available source hasn't answered yet. */
  loading: boolean
  /** Every available source answered and none has anything: the only time "Nothing needs you" may show. */
  empty: boolean
  /** Waiting requests (matched + outside Chat), or null when that source didn't answer. */
  waitingCount: number | null
  /** Agents needing action, or null when unknown. */
  actionCount: number | null
}

const ORDER: readonly SourceId[] = ['requests', 'agents', 'budgets', 'sessions']

export function mergeNeeds(i: NeedsInput): Needs {
  const rows: NeedRow[] = []
  if (i.requests.state === 'ok') {
    const chats = [...i.requests.value.chats].sort((a, b) => a.at.localeCompare(b.at))
    for (const chat of chats)
      rows.push({ key: `request-${chat.sessionId}`, kind: 'request', severity: 'waiting', chat })
    if (i.requests.value.outside > 0)
      rows.push({
        key: 'outside',
        kind: 'outside',
        severity: 'waiting',
        count: i.requests.value.outside,
      })
  }
  if (i.agents.state === 'ok')
    for (const a of i.agents.value)
      rows.push({ key: `agent-${a.id}`, kind: 'agent', severity: 'action', ...a })
  if (i.budgets.state === 'ok')
    for (const b of i.budgets.value)
      rows.push({
        key: `budget-${b.id}`,
        kind: 'budget',
        severity: 'watch',
        label: b.label,
        crossed: b.crossed,
        state: b.state,
      })
  if (i.sessions.state === 'ok' && i.sessions.value.failed > 0)
    rows.push({ key: 'sessions', kind: 'sessions', severity: 'action', ...i.sessions.value })

  const states = ORDER.map((id) => [id, i[id].state] as const)
  const failed = states
    .filter(([id, s]) => s === 'failed' || (id === 'agents' && i.agents.incomplete))
    .map(([id]) => id)
  const loading = states.some(([, s]) => s === 'loading')
  return {
    rows,
    failed,
    available: states.filter(([, s]) => s !== 'absent').length,
    loading,
    empty: !loading && !failed.length && !rows.length,
    waitingCount:
      i.requests.state === 'ok'
        ? i.requests.value.chats.reduce((n, c) => n + c.count, 0) + i.requests.value.outside
        : null,
    actionCount: i.agents.state === 'ok' ? i.agents.value.length : null,
  }
}
