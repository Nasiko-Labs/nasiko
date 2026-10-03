/**
 * A recorded harness chat's tool calls as step chips (v1c §5.7, ND-10, R2, DP11). Pure. Raw values are
 * kept and each detail section is stringified and cut only when it opens (E8), so a message with 2000
 * calls and 1 MB fields maps quickly. Malformed entries are skipped with a warning naming the entry and
 * the field, never the payload (DX4).
 */
import type { Step } from './a2aReducer'
import { copy } from './copy'
import { chatKind } from './format'
import { CODING_SESSION_TITLE } from './serverContract'
import { tuning } from './tuning'
import type { ChatSessionRow } from './types'

/** `types/src/coding_agent.rs` ToolCall `status` (cb3aaf0c). */
export const RECORDED_STATUSES = [
  'succeeded',
  'failed',
  'denied',
  'timed_out',
  'cancelled',
  'pending',
  'running',
  'unknown',
] as const
type RecordedStatus = (typeof RECORDED_STATUSES)[number]

const ERROR_STATUSES: readonly RecordedStatus[] = ['failed', 'denied', 'timed_out', 'cancelled']

export type SectionLabel = 'Arguments' | 'Output' | 'Error'

export interface RecordedStep extends Omit<Step, 'status' | 'detail' | 'kind'> {
  kind: 'tool'
  status: 'ok' | 'error' | 'neutral'
  /** The server's status. */
  recordedStatus: RecordedStatus
  /** Shown next to the name for failures and calls with no result (R2, DP11). */
  statusWord?: string
  /** Only when `timestamp_quality === 'exact'`. */
  startedAt?: string
  association: 'exact' | 'turn' | 'unknown'
  /** The sections the call has, raw; `sectionText` formats one when it opens. */
  sections: { label: SectionLabel; raw: unknown }[]
}

export interface RecordedCalls {
  steps: RecordedStep[]
  /** `capture_policy === 'content'`. Stored messages always say so (§2.3); defensive. */
  captured: boolean
}

const STATUS_WORD: Partial<Record<RecordedStatus, string>> = {
  failed: copy.recordedFailed,
  denied: copy.recordedDenied,
  timed_out: copy.recordedTimedOut,
  cancelled: copy.recordedCancelled,
  pending: copy.noResultRecorded,
  running: copy.noResultRecorded,
  unknown: copy.statusUnknown,
}

type Json = Record<string, unknown>
const isObject = (v: unknown): v is Json => !!v && typeof v === 'object' && !Array.isArray(v)

/**
 * The calls on an assistant message: `metadata.coding_agent.tool_calls`, only when it is an array
 * (else null: no chips at all).
 */
export function toolCallsToSteps(
  metadata: unknown,
  warn: (msg: string) => void = (m) => console.warn(m),
): RecordedCalls | null {
  const agent = isObject(metadata) && isObject(metadata.coding_agent) ? metadata.coding_agent : null
  const calls = agent?.tool_calls
  if (!Array.isArray(calls)) return null
  const steps: RecordedStep[] = []
  calls.forEach((c: unknown, i) => {
    const bad = (field: string) => warn(`[chat] recorded tool call ${i}: invalid ${field}; skipped`)
    if (!isObject(c)) return bad('entry')
    if (typeof c.name !== 'string' || !c.name.trim()) return bad('name')
    if (typeof c.status !== 'string') return bad('status')
    const recordedStatus: RecordedStatus = (RECORDED_STATUSES as readonly string[]).includes(
      c.status,
    )
      ? (c.status as RecordedStatus)
      : 'unknown'
    const exact = c.timestamp_quality === 'exact'
    const started = typeof c.started_at === 'string' ? Date.parse(c.started_at) : Number.NaN
    const ended = typeof c.ended_at === 'string' ? Date.parse(c.ended_at) : Number.NaN
    const durationMs =
      typeof c.duration_ms === 'number' && c.duration_ms >= 0
        ? c.duration_ms
        : exact && !Number.isNaN(started) && !Number.isNaN(ended) && ended >= started
          ? ended - started
          : undefined
    const sections: RecordedStep['sections'] = []
    if ('arguments' in c) sections.push({ label: 'Arguments', raw: c.arguments })
    if ('output' in c) sections.push({ label: 'Output', raw: c.output })
    if ('error' in c) sections.push({ label: 'Error', raw: c.error })
    steps.push({
      key: typeof c.id === 'string' && c.id ? c.id : `call-${i}`,
      kind: 'tool',
      name: c.name,
      recordedStatus,
      status:
        recordedStatus === 'succeeded'
          ? 'ok'
          : ERROR_STATUSES.includes(recordedStatus)
            ? 'error'
            : 'neutral',
      statusWord: STATUS_WORD[recordedStatus],
      durationMs,
      startedAt: exact && !Number.isNaN(started) ? (c.started_at as string) : undefined,
      association:
        c.association === 'turn' || c.association === 'unknown' ? c.association : 'exact',
      sections,
    })
  })
  return { steps, captured: agent?.capture_policy === 'content' }
}

/** Empty: nothing to show but "No output". */
export const isEmptyValue = (v: unknown) =>
  v === null ||
  v === undefined ||
  v === '' ||
  (Array.isArray(v) && !v.length) ||
  (isObject(v) && !Object.keys(v).length)

/**
 * One detail section's text, built when it opens (E8): a string as given (a CLI-truncated value stays a
 * string), anything else as indented JSON, cut at SAVED_DETAIL_MAX. `full` is for Copy full value.
 */
export function sectionText(raw: unknown): { text: string; cut: boolean; full: string } {
  let full: string
  if (typeof raw === 'string') full = raw
  else {
    try {
      full = JSON.stringify(raw, null, 2) ?? String(raw)
    } catch {
      full = String(raw)
    }
  }
  const max = tuning.SAVED_DETAIL_MAX
  return full.length > max
    ? { text: full.slice(0, max), cut: true, full }
    : { text: full, cut: false, full }
}

/**
 * A session recorded under the metadata-only policy (§2.2): `message_count` 0. Once its harness agent is
 * deleted the row loses `is_coding_agent` (R-6), so the server's title "Coding session" identifies it (E16),
 * but only on a row whose agent is gone: a user can give any chat that title.
 */
export function isMetadataOnly(
  row:
    | Pick<ChatSessionRow, 'is_coding_agent' | 'message_count' | 'title' | 'agent_id' | 'agent_url'>
    | undefined,
): boolean {
  if (!row || row.message_count !== 0) return false
  return (
    !!row.is_coding_agent || (row.title === CODING_SESSION_TITLE && chatKind(row) === 'removed')
  )
}
