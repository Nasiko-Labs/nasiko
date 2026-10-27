/** Every flows query key (plans/feat-flows.md), in a module of its own so a reader of one key doesn't load the data
 * layer (the Overview's paused-flow links). */
import type { FlowListParams } from './api'

/** @public The flows pages read and invalidate these. */
export const flowKeys = {
  all: ['flows'] as const,
  list: (p: FlowListParams) => ['flows', 'list', p.status ?? '', p.q ?? '', p.sinceMs] as const,
  detail: (id: string) => ['flows', 'detail', id] as const,
  paused: ['flows', 'paused'] as const,
}
