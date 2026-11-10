/** NAS-1460: someone an agent is shared with is told its owner stopped it, not to start it themselves. */
import { describe, expect, it } from 'vitest'
import { stoppedByOwner } from './target'

const ME = 'user-me'
const OTHER = 'user-owner'

describe('a stopped agent', () => {
  it('someone else owns reads as stopped by its owner', () => {
    expect(stoppedByOwner({ status: 'stopped', owner_id: OTHER }, ME)).toBe(true)
  })

  it('the viewer owns does not (they can start it)', () => {
    expect(stoppedByOwner({ status: 'stopped', owner_id: ME }, ME)).toBe(false)
  })

  it('that is deploying, failed or parked, not stopped, is not blamed on the owner', () => {
    for (const status of ['deploying', 'failed', 'parked']) {
      expect(stoppedByOwner({ status, owner_id: OTHER }, ME)).toBe(false)
    }
  })
})
