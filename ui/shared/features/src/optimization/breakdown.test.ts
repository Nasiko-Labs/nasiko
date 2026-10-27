/** By agent's order (plans/feat-optimization-page.md R7B). */
import { describe, expect, it } from 'vitest'
import { sortAgents } from './breakdown'
import { savedOf } from '@nasiko/app-core/slice'

const row = (agent_id: string, name: string, pool: number, sent: number, requests = 1) => ({
  agent_id,
  name,
  requests,
  reports: requests,
  pool_tokens: pool,
  sent_tokens: sent,
  compress_enabled: true,
})

describe('sortAgents', () => {
  const rows = [
    row('a', 'Bravo', 100, 90, 5),
    row('b', 'Alpha', 300, 100, 1),
    row('c', 'Charlie', 200, 20, 9),
  ]

  it('history volume first by default; numbers largest first; names A–Z', () => {
    expect(sortAgents(rows, 'without').map((r) => r.agent_id)).toEqual(['b', 'c', 'a'])
    expect(sortAgents(rows, 'saved').map((r) => r.agent_id)).toEqual(['b', 'c', 'a'])
    expect(sortAgents(rows, 'sent').map((r) => r.agent_id)).toEqual(['b', 'a', 'c'])
    expect(sortAgents(rows, 'requests').map((r) => r.agent_id)).toEqual(['c', 'a', 'b'])
    expect(sortAgents(rows, 'name').map((r) => r.name)).toEqual(['Alpha', 'Bravo', 'Charlie'])
  })

  it('ties fall back to history volume, then name', () => {
    const tie = [
      row('x', 'Zed', 100, 50, 3),
      row('y', 'Amy', 200, 50, 3),
      row('z', 'Bob', 200, 50, 3),
    ]
    expect(sortAgents(tie, 'requests').map((r) => r.name)).toEqual(['Amy', 'Bob', 'Zed'])
  })

  it('saved never goes negative', () => {
    expect(savedOf({ pool_tokens: 10, sent_tokens: 12 })).toBe(0)
  })
})
