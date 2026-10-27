/**
 * The rail's date groups and row tooltips (v1c §5.3, C3). Pure, so the calendar edges (local midnight,
 * DST) are unit-tested with a pinned time zone (E9).
 */
import { copy } from './copy'
import type { ChatSessionRow } from './types'

/**
 * A rail row's states in words, for its screen-reader text (v1c §5.8, test 13): waiting, in progress, failed and
 * new reply, in that order, joined into one phrase. The indicator shows only the first; the words say them all.
 */
export function rowStates(s: {
  waiting: number
  live: boolean
  failed: boolean
  unseen: boolean
}): string[] {
  const out: string[] = []
  if (s.waiting > 0) out.push(copy.rowWaiting)
  if (s.live) out.push(copy.rowInProgress)
  if (s.failed) out.push(copy.rowFailed)
  if (s.unseen) out.push(copy.rowNewReply)
  return out
}

export type DateGroup = 'today' | 'yesterday' | 'week' | 'older'
const DATE_GROUPS: readonly DateGroup[] = ['today', 'yesterday', 'week', 'older']

/** Characters of `last_message` a row's tooltip shows (C3). */
const PREVIEW_CHARS = 120

/** The calendar day of `ms` in `timeZone`, as days since the epoch (DST-proof: no hour arithmetic). */
function dayNumber(ms: number, timeZone?: string): number {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).formatToParts(ms)
  const get = (t: string) => Number(parts.find((p) => p.type === t)?.value)
  return Date.UTC(get('year'), get('month') - 1, get('day')) / 86_400_000
}

/** Which group a timestamp lands in: today, yesterday, the 5 days before that, or older. */
export function dateGroupOf(iso: string | undefined, now: number, timeZone?: string): DateGroup {
  const t = iso ? Date.parse(iso) : Number.NaN
  if (Number.isNaN(t)) return 'older'
  const diff = dayNumber(now, timeZone) - dayNumber(t, timeZone)
  if (diff <= 0) return 'today'
  if (diff === 1) return 'yesterday'
  if (diff <= 7) return 'week'
  return 'older'
}

/**
 * Loaded rows by `updated_at` in local time (§5.3). Each row lands in the group its own time picks,
 * so a Load more page can add rows to Today (DS12); rows keep their order; empty groups are left out.
 */
export function groupByDate<T extends Pick<ChatSessionRow, 'updated_at' | 'created_at'>>(
  rows: readonly T[],
  now: number,
  timeZone?: string,
): { group: DateGroup; rows: T[] }[] {
  const by = new Map<DateGroup, T[]>()
  for (const r of rows) {
    const g = dateGroupOf(r.updated_at ?? r.created_at, now, timeZone)
    by.set(g, [...(by.get(g) ?? []), r])
  }
  return DATE_GROUPS.flatMap((group) => {
    const rows = by.get(group)
    return rows ? [{ group, rows }] : []
  })
}

/** A row's `title` (C3): the chat title, plus the start of the last message as plain text. */
export function rowTooltip(row: Pick<ChatSessionRow, 'title' | 'last_message'>): string {
  const preview = (row.last_message ?? '')
    .replace(/[`*_#>~]+/g, '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, PREVIEW_CHARS)
  return preview ? `${row.title}\n${preview}` : row.title
}
