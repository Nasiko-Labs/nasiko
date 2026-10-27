/**
 * The only place Chat code tolerates server quirks (plan §5, R6). Each quirk names the §10
 * recommendation that would remove it.
 */
import type { ChatMessage, ChatSessionRow, CursorPage, HitlDto, MessagesPage } from './types'

/** Numbers that may arrive as strings. */
export function toNumber(v: unknown): number | null {
  if (typeof v === 'number') return Number.isFinite(v) ? v : null
  // quirk: §10.12 — `cost_usd` is a rust_decimal JSON string on chat_messages, a number in usage_meta.
  if (typeof v === 'string' && v.trim() !== '') {
    const n = Number(v)
    return Number.isFinite(n) ? n : null
  }
  return null
}

/** quirk: §10.12 — POST /chat/sessions wraps the row in `{data}`; PUT returns it bare. */
export function unwrapSession(body: unknown): ChatSessionRow {
  const row =
    body && typeof body === 'object' && 'data' in body ? (body as { data: unknown }).data : body
  if (!row || typeof row !== 'object' || typeof (row as ChatSessionRow).session_id !== 'string')
    throw new Error('Unexpected chat session response')
  return row as ChatSessionRow
}

export function unwrapMessagesPage(body: unknown): MessagesPage {
  const b = (body ?? {}) as Partial<MessagesPage>
  return {
    data: Array.isArray(b.data) ? b.data : [],
    has_more: b.has_more === true,
    next_cursor: b.next_cursor ?? null,
    prev_cursor: b.prev_cursor ?? null,
    hitl: Array.isArray(b.hitl) ? b.hitl : [],
  }
}

export function unwrapSessionsPage(body: unknown): CursorPage<ChatSessionRow> {
  const b = (body ?? {}) as Partial<CursorPage<ChatSessionRow>>
  return {
    data: Array.isArray(b.data) ? b.data : [],
    has_more: b.has_more === true,
    next_cursor: b.next_cursor ?? null,
    prev_cursor: b.prev_cursor ?? null,
  }
}

/** Marker appended to a partial reply the user stopped receiving (plan §6.4). */
// quirk: §10.11 — no structured `stopped` status on messages, so the marker lives in the content.
export const RECEIVING_STOPPED_MARKER = '\n\n_Receiving stopped_'

/** Split a saved reply into its text and whether it was stopped (reads a future structured status too). */
export function splitStopped(m: Pick<ChatMessage, 'content' | 'metadata'>): {
  text: string
  stopped: boolean
} {
  const structured = m.metadata && (m.metadata as { stopped?: unknown }).stopped === true
  if (m.content.endsWith(RECEIVING_STOPPED_MARKER))
    return { text: m.content.slice(0, -RECEIVING_STOPPED_MARKER.length), stopped: true }
  return { text: m.content, stopped: Boolean(structured) }
}

/**
 * `GET /api/hitl/pending` rows, safe to render on every chat route (v1c §5.9). The question is the agent's own
 * JSON, so a non-string `message` becomes none, and a row without an id, `created_at` or `execution` is dropped.
 */
// quirk: v1c R-7 — the server stores `question` as the agent sent it, unchecked.
export function unwrapPending(body: unknown): HitlDto[] {
  const data = (body as { data?: unknown } | null)?.data
  if (!Array.isArray(data)) return []
  const out: HitlDto[] = []
  for (const r of data as Partial<HitlDto>[]) {
    if (
      !r ||
      typeof r !== 'object' ||
      typeof r.id !== 'string' ||
      typeof r.created_at !== 'string' ||
      !r.execution ||
      typeof r.execution !== 'object'
    )
      continue
    const q = r.question
    const question =
      q && typeof q === 'object'
        ? typeof q.message === 'string' || q.message === undefined
          ? q
          : { ...q, message: undefined }
        : null
    out.push({ ...(r as HitlDto), question })
  }
  return out
}
