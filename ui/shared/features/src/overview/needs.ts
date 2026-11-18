/**
 * Needs you (plans/feat-overview.md §6): one inbox from four sources, most urgent first. Pure.
 *
 * - Each source loads and fails on its own; a failed source adds a "Couldn't check …" line and the rest still show
 *   (design review 5A). A source the server doesn't have (a bare 404, e.g. budgets) is absent, not failed.
 * - "Nothing needs you" is only claimed when every available source answered with nothing (5A).
 * - Requests come from Chat's Waiting pipeline, so a superuser only sees requests provably theirs (eng review R5).
 */
import type { HitlKind } from '@nasiko/features/chat/types'
import { fmtMoney, fmtShortDay } from '@nasiko/ui/lib/format'
import { copy } from './copy'
import type { Dimension } from './health'

export type SourceId = 'requests' | 'agents' | 'budgets' | 'sessions'
/** A platform feature under the alerts: a suggestion while it isn't set up, never an alert (and never "failed"). */
export type SetupId = 'router' | 'workflows' | 'mcp'
/** One setup item and how much of it exists: 0 is not set up; `more` when the count is a full page, not the total. */
export interface SetupItem {
  id: SetupId
  count: number
  more?: boolean
}
type SourceState<T> =
  { state: 'loading' } | { state: 'failed' } | { state: 'absent' } | { state: 'ok'; value: T }

export type Severity = 'action' | 'watch' | 'waiting'

interface RequestChat {
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
  /** `incomplete`: a later page failed, so the rows are real but the source can't vouch for "nothing else". */
  budgets: SourceState<BudgetItem[]> & { incomplete?: boolean }
  /**
   * `failed` is the server's `matching_count` — every failed session in the
   * window, not a sample. `agents` names the ones on the fetched page.
   */
  sessions: SourceState<{ failed: number; agents: string[] }>
}

/**
 * A budget that needs the viewer (plans/feat-budget-burn.md F8). Budgets are Enterprise only, so the shape is the
 * core's slot contract (`app/edition.ts`) and the rows come from the layer that fills it.
 */
import type { BudgetItem } from '@nasiko/app-core/edition/edition'
export type { BudgetItem }

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
  | ({ key: string; kind: 'budget'; severity: 'watch' } & Omit<BudgetItem, 'id'>)
  | {
      key: string
      kind: 'sessions'
      severity: 'action'
      failed: number
      agents: string[]
    }
  /** Watch agents, promoted into an otherwise empty inbox (`watchSummary`); never part of `mergeNeeds`. */
  | { key: string; kind: 'watch'; severity: 'watch'; count: number; detail: string }

export interface Needs {
  rows: NeedRow[]
  /** Sources that failed (or answered only partly), in inbox order. */
  failed: SourceId[]
  /** Sources the server has (not absent): all of them failing is one card error. */
  available: number
  /** Sources that answered in full, in inbox order: what an all-clear vouches for. */
  checked: SourceId[]
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
      rows.push({ ...b, key: `budget-${b.id}`, kind: 'budget', severity: 'watch' })
  if (i.sessions.state === 'ok' && i.sessions.value.failed > 0)
    rows.push({ key: 'sessions', kind: 'sessions', severity: 'action', ...i.sessions.value })

  const states = ORDER.map((id) => [id, i[id].state] as const)
  const failed = states
    .filter(
      ([id, s]) =>
        s === 'failed' ||
        (id === 'agents' && i.agents.incomplete) ||
        (id === 'budgets' && i.budgets.incomplete),
    )
    .map(([id]) => id)
  const loading = states.some(([, s]) => s === 'loading')
  return {
    rows,
    failed,
    available: states.filter(([, s]) => s !== 'absent').length,
    checked: states.filter(([id, s]) => s === 'ok' && !failed.includes(id)).map(([id]) => id),
    loading,
    empty: !loading && !failed.length && !rows.length,
    waitingCount:
      i.requests.state === 'ok'
        ? i.requests.value.chats.reduce((n, c) => n + c.count, 0) + i.requests.value.outside
        : null,
    actionCount: i.agents.state === 'ok' ? i.agents.value.length : null,
  }
}

/** A budget row's words (plans/feat-budget-burn.md F8). */
export function budgetView(b: Omit<BudgetItem, 'id'>): { title: string; detail: string } {
  const until = b.stopped && b.resetsAt ? fmtShortDay(b.resetsAt) : null
  if (b.stopped)
    return {
      title: copy.needs.budgetStopped(b.label),
      // At its limit exactly (a stop lands there), "over by $0.00" says nothing.
      detail:
        b.over && b.over >= 0.005
          ? copy.needs.budgetOverBy(fmtMoney(b.over), until)
          : copy.needs.budgetStoppedUntil(until),
    }
  if (b.state === 'exceeded')
    return {
      title: copy.needs.budget(b.label, 'exceeded', b.crossed),
      detail:
        b.over !== null
          ? copy.needs.budgetOverBy(fmtMoney(b.over), null)
          : copy.needs.budgetDetailFor(b.period, b.covers),
    }
  if (b.runsOutOn)
    return {
      title: copy.needs.budgetRunsOut(b.label, fmtShortDay(b.runsOutOn), b.stops),
      detail: copy.needs.budgetProjected,
    }
  return {
    title: copy.needs.budget(b.label, 'warning', b.crossed),
    detail: copy.needs.budgetDetailFor(b.period, b.covers),
  }
}

const DIMENSIONS: readonly Dimension[] = ['reliability', 'cost', 'activity', 'latency']

/**
 * Watch agents as one row for an inbox that is otherwise empty (user 2026-11-05): how many, and why. One shared reason
 * is said as is ("All idle 7 days while deployed"); mixed ones are counted by what they're about, by each agent's
 * first (worst) reason. Null when there are none.
 */
export function watchSummary(
  watch: readonly { reasons: readonly { dimension: Dimension; text: string }[] }[],
): { count: number; detail: string } | null {
  const first = watch.flatMap((a) => (a.reasons[0] ? [a.reasons[0]] : []))
  if (!watch.length) return null
  const texts = new Set(first.map((r) => r.text))
  const [only] = texts
  if (first.length === watch.length && texts.size === 1 && only)
    return { count: watch.length, detail: copy.needs.watchSame(watch.length, only) }
  const by = (d: Dimension) => first.filter((r) => r.dimension === d).length
  return {
    count: watch.length,
    detail: DIMENSIONS.filter((d) => by(d) > 0)
      .map((d) => copy.needs.watchBy[d](by(d)))
      .join(' · '),
  }
}
