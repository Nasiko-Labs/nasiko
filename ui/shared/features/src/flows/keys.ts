/**
 * The swimlane's keyboard model (plans/feat-flows.md F25), pure: ←/→ move through the drawn calls in start order,
 * ↑/↓ move to the lane above or below and land on its call that starts nearest the current one. Lanes are in display
 * order; calls inside a lane in any order.
 */
import type { Call } from './calls'

export type Move = 'prev' | 'next' | 'up' | 'down' | 'first' | 'last'

export function moveFrom(
  lanes: readonly (readonly Call[])[],
  from: string | null,
  move: Move,
): string | null {
  const all = lanes.flat().sort((a, b) => a.startMs - b.startMs || a.key.localeCompare(b.key))
  if (!all.length) return null
  const i = all.findIndex((c) => c.key === from)
  const at = all[i]
  if (move === 'first' || !at) return all[0]?.key ?? null
  if (move === 'last') return all[all.length - 1]?.key ?? null
  if (move === 'prev') return all[Math.max(0, i - 1)]?.key ?? null
  if (move === 'next') return all[Math.min(all.length - 1, i + 1)]?.key ?? null
  const lane = lanes.findIndex((l) => l.some((c) => c.key === from))
  for (
    let l = lane + (move === 'up' ? -1 : 1);
    l >= 0 && l < lanes.length;
    l += move === 'up' ? -1 : 1
  ) {
    const near = [...(lanes[l] ?? [])].sort(
      (a, b) => Math.abs(a.startMs - at.startMs) - Math.abs(b.startMs - at.startMs),
    )[0]
    if (near) return near.key
  }
  return at.key
}
