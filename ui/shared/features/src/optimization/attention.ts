/**
 * Needs attention (plans/feat-optimization-page.md P5, R1C, R2C, R2F; eng E2): which agents have Token optimization
 * off, split by whose chats they affect. History compression is decided per owner (`compression_opt_in` over the
 * owner's live agents), so line 1 is your own agents (your chats) and line 2, for superusers only, every other owner's
 * (their chats). Pure; tested in attention.test.ts.
 */

/** A `GET /api/agents` row as this rule reads it (the generated type predates `compress_enabled`). */
export interface AgentRow {
  id: string
  name: string
  display_name?: string | null
  owner_id: string
}

export interface OffAgent {
  id: string
  name: string
  ownerId: string
}

/**
 * Off only when the row says so: a row without the field (a server that predates it) is unknown, never a target for
 * a bulk write to someone else's agent (review: security). The read-only compression rule in logic.ts keeps its
 * "missing reads as off" default, the server's own.
 */
const isOff = (row: AgentRow) => (row as { compress_enabled?: unknown }).compress_enabled === false

const toOff = (row: AgentRow): OffAgent => ({
  id: row.id,
  name: row.display_name || row.name,
  ownerId: row.owner_id,
})

const byName = (a: OffAgent, b: OffAgent) =>
  a.name.localeCompare(b.name) || a.id.localeCompare(b.id)

/**
 * `owned` is your own list (`?owner=` you, what the compression rule counts); `directory` is everything you can access
 * (read only for superusers). `hidden` are agents a batch just turned on, dropped before the refetch lands.
 */
export function attentionLines(input: {
  owned: readonly AgentRow[]
  directory: readonly AgentRow[] | undefined
  me: string
  superuser: boolean
  hidden: ReadonlySet<string>
}): { mine: OffAgent[]; others: OffAgent[] } {
  const keep = (r: AgentRow) => isOff(r) && !input.hidden.has(r.id)
  const mine = input.owned.filter(keep).map(toOff).sort(byName)
  const others =
    input.superuser && input.directory
      ? input.directory
          .filter((r) => !!r.owner_id && r.owner_id !== input.me && keep(r))
          .map(toOff)
          .sort(byName)
      : []
  return { mine, others }
}

/** The first few names in the strip; the rest are counted (the confirm lists them all). */
export const SHOWN_NAMES = 5

export function shownNames<T>(list: readonly T[]): { shown: T[]; more: number } {
  return list.length > SHOWN_NAMES + 1
    ? { shown: list.slice(0, SHOWN_NAMES), more: list.length - SHOWN_NAMES }
    : { shown: [...list], more: 0 }
}
