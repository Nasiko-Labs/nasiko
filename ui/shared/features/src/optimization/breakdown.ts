/**
 * By agent (plans/feat-optimization-page.md R7B): the sort and the cut. Pure; tested in breakdown.test.ts.
 */
import { savedOf } from '@nasiko/app-core/slice'
import type { AgentSort } from './search'
import type { SavingsAgent } from './types'

/** Rows shown before "Show all N" (R7B). */
export const TOP_AGENTS = 6

const value: Record<Exclude<AgentSort, 'name'>, (r: SavingsAgent) => number> = {
  without: (r) => r.pool_tokens,
  sent: (r) => r.sent_tokens,
  saved: savedOf,
  requests: (r) => r.requests,
}

/** Numbers sort largest first, names A–Z; ties fall back to history volume, then name, so the order never jumps. */
export function sortAgents(rows: readonly SavingsAgent[], by: AgentSort): SavingsAgent[] {
  const name = (a: SavingsAgent, b: SavingsAgent) => a.name.localeCompare(b.name)
  return [...rows].sort((a, b) =>
    by === 'name'
      ? name(a, b) || a.agent_id.localeCompare(b.agent_id)
      : value[by](b) - value[by](a) || b.pool_tokens - a.pool_tokens || name(a, b),
  )
}
