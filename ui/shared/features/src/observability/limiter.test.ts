/** The one concurrency limiter (status checks, retry-waste fetches): never overshoots, never runs cancelled work. */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createLimiter } from './limiter'

afterEach(() => {
  vi.useRealTimers()
})

describe('createLimiter', () => {
  it('never runs more than max at once and skips cancelled waiters', async () => {
    vi.useFakeTimers()
    const run = createLimiter(1)
    let active = 0
    let peak = 0
    const ran: string[] = []
    const job = (name: string) => async () => {
      active++
      peak = Math.max(peak, active)
      await new Promise((r) => setTimeout(r, 5))
      ran.push(name)
      active--
      return name
    }
    const cancelled = new AbortController()
    const p1 = run(new AbortController().signal, job('a'))
    const p2 = run(cancelled.signal, job('b'))
    const p3 = run(new AbortController().signal, job('c'))
    cancelled.abort()
    await expect(p2).rejects.toBeDefined()
    // A newcomer arriving while the slot is handed over must still queue.
    await vi.advanceTimersByTimeAsync(5)
    await p1
    const p4 = run(new AbortController().signal, job('d'))
    await vi.advanceTimersByTimeAsync(10)
    await Promise.all([p3, p4])
    expect(peak).toBe(1)
    expect(ran).toEqual(['a', 'c', 'd'])
  })
})

describe('createLimiter with a signal already cancelled', () => {
  it('rejects at once instead of queueing a wait that never settles', async () => {
    const run = createLimiter(1)
    const busy = run(new AbortController().signal, () => new Promise((r) => setTimeout(r, 20)))
    const gone = new AbortController()
    gone.abort()
    const job = vi.fn(async () => 'never')
    await expect(run(gone.signal, job)).rejects.toBeDefined()
    expect(job).not.toHaveBeenCalled()
    await busy
  })
})
