/**
 * Needs you, for one agent (plans/feat-agent-centric.md PR1c; CEO R2-6, design "Needs you", DX "Errors"). Pure: the
 * hub feeds it three sources, re-sourced for this agent rather than filtered from the Overview's fleet sample.
 * - Requests: this agent's pending requests (a superuser sees only those a chat proves theirs, as chat's Waiting does;
 *   requests nobody can prove yet keep the verdict back, never count as zero).
 * - Status: the agent's rating from the Overview's rules (`overview/health.ts`), on the last 7 days. An agent the rules
 *   can't rate (stopped, not deployed, a harness, or one whose spend the viewer can't see) is absent, never "checked".
 * - Budget: this agent's budget (plans/feat-budget-burn.md F8, B5), for whoever may read it (its owner or a superuser):
 *   stopped or over is action, past a mark or projected past its limit is watch, with the budget's own sentence.
 * - Sessions: failures among this agent's sessions found in the hub's bounded sample. That sample is always partial (it
 *   is the latest N sessions the viewer can see, matched by raw name), so the card says what it checked (R2-6), and a
 *   name another visible agent shares is said with the failures.
 *
 * The verdict "No issues found in checked data" shows only when every source answered and found nothing; a failed source
 * shows its own line with Retry, never the verdict.
 */
import { checkSessions, type Status } from '@nasiko/features/observability/sessions'

type SourceState<T> =
  { state: 'loading' } | { state: 'failed' } | { state: 'absent' } | { state: 'ok'; value: T }

interface Requests {
  count: number
  firstChat: string | null
  firstRequest: string | null
  /** A superuser's pending requests for this agent no chat proves theirs yet: the source can't vouch for "none". */
  unproven: number
}

interface Rating {
  rating: 'healthy' | 'watch' | 'action' | 'unknown'
  reason: string | null
  /** A cost input failed: the rating can't vouch that nothing else needs action. */
  incomplete: boolean
}

interface Sessions {
  /** This agent's sessions found in the sample. */
  found: number
  /** Of those, how many were checked (finished checking as failed or ok). */
  checked: number
  failed: number
  /** Rows in the sample (the latest N the viewer can see). */
  scanned: number
  /** Another visible agent has this raw name: its sessions may be among these. */
  ambiguous: boolean
}

interface BudgetNeed {
  /** null: the budget needs nothing. */
  level: 'action' | 'watch' | null
  sentence: string
}

export interface HubNeedsInput {
  requests: SourceState<Requests>
  status: SourceState<Rating>
  sessions: SourceState<Sessions>
  budget: SourceState<BudgetNeed>
}

export type NeedRow =
  | { kind: 'requests'; count: number; firstChat: string | null; firstRequest: string | null }
  | { kind: 'status'; level: 'action' | 'watch'; reason: string }
  | { kind: 'sessions'; failed: number; checked: number; ambiguous: boolean }
  | { kind: 'budget'; level: 'action' | 'watch'; sentence: string }

export type HubSource = 'requests' | 'status' | 'sessions' | 'budget'

export interface HubNeeds {
  rows: NeedRow[]
  failed: HubSource[]
  /** The status source failed only because a cost input is missing (its own wording). */
  statusIncomplete: boolean
  /** Requests that can't be tied to the viewer's chats yet (superusers). */
  unproven: number
  loading: boolean
  /** Every source answered (or is absent) and nothing was found. */
  clear: boolean
  /** What was checked, for the coverage line; null while loading. */
  coverage: {
    requests: boolean
    status: boolean
    sessions: Sessions | null
    budget: boolean
  } | null
}

/** The sessions source from the sample and its checks: the Overview's rule (`checkSessions`), plus coverage. */
export function sessionSource(i: {
  listError: boolean
  listPending: boolean
  found: number
  scanned: number
  verdicts: readonly (Status | undefined)[]
  ambiguous: boolean
}): SourceState<Sessions> {
  const v = checkSessions(i)
  if (v.state !== 'ok') return v
  return {
    state: 'ok',
    value: {
      found: i.found,
      checked: v.checked,
      failed: v.failed,
      scanned: i.scanned,
      ambiguous: i.ambiguous,
    },
  }
}

export function hubNeeds(i: HubNeedsInput): HubNeeds {
  const rows: NeedRow[] = []
  const failed: HubSource[] = []
  let loading = false
  const note = (s: HubSource, st: SourceState<unknown>) => {
    if (st.state === 'failed') failed.push(s)
    if (st.state === 'loading') loading = true
  }
  note('requests', i.requests)
  note('status', i.status)
  note('sessions', i.sessions)
  note('budget', i.budget)
  // An unrated agent has nothing checked: the source is absent, never "rated" (review: adversarial).
  const status: SourceState<Rating> =
    i.status.state === 'ok' && i.status.value.rating === 'unknown' ? { state: 'absent' } : i.status
  if (
    status.state === 'ok' &&
    (status.value.rating === 'action' || status.value.rating === 'watch')
  )
    rows.push({ kind: 'status', level: status.value.rating, reason: status.value.reason ?? '' })
  if (i.requests.state === 'ok' && i.requests.value.count > 0) {
    const { count, firstChat, firstRequest } = i.requests.value
    rows.push({ kind: 'requests', count, firstChat, firstRequest })
  }
  if (i.sessions.state === 'ok' && i.sessions.value.failed > 0)
    rows.push({
      kind: 'sessions',
      failed: i.sessions.value.failed,
      checked: i.sessions.value.checked,
      ambiguous: i.sessions.value.ambiguous,
    })
  if (i.budget.state === 'ok' && i.budget.value.level)
    rows.push({ kind: 'budget', level: i.budget.value.level, sentence: i.budget.value.sentence })
  const statusIncomplete = status.state === 'ok' && status.value.incomplete
  if (statusIncomplete) failed.push('status')
  const unproven = i.requests.state === 'ok' ? i.requests.value.unproven : 0
  // Action first (health, then budget), then requests, then watch (health, then budget), then sessions.
  const rank = (r: NeedRow) =>
    r.kind === 'status'
      ? r.level === 'action'
        ? 0
        : 2
      : r.kind === 'budget'
        ? r.level === 'action'
          ? 0.5
          : 2.5
        : r.kind === 'requests'
          ? 1
          : 3
  rows.sort((a, b) => rank(a) - rank(b))
  const coverage = loading
    ? null
    : {
        requests: i.requests.state === 'ok',
        status: status.state === 'ok',
        sessions: i.sessions.state === 'ok' ? i.sessions.value : null,
        budget: i.budget.state === 'ok',
      }
  return {
    rows,
    failed,
    statusIncomplete,
    unproven,
    loading,
    clear: !loading && !failed.length && !rows.length && !unproven,
    coverage,
  }
}
