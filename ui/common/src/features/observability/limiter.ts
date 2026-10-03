/**
 * At most `max` async jobs in flight. A finished job hands its slot straight to the next
 * waiter (no window where a newcomer can overshoot), and a waiter whose query was
 * cancelled leaves the queue without ever running.
 */
export function createLimiter(max: number) {
  let active = 0
  const waiting: { start: () => void; signal: AbortSignal }[] = []

  const release = () => {
    // Skip waiters whose query was cancelled while queued.
    for (let next = waiting.shift(); next; next = waiting.shift()) {
      if (next.signal.aborted) continue
      next.start()
      return
    }
    active--
  }

  return async function run<T>(signal: AbortSignal, fn: () => Promise<T>): Promise<T> {
    if (active >= max) {
      await new Promise<void>((resolve, reject) => {
        const entry = { start: resolve, signal }
        waiting.push(entry)
        signal.addEventListener(
          'abort',
          () => {
            const i = waiting.indexOf(entry)
            if (i >= 0) waiting.splice(i, 1)
            reject(signal.reason ?? new DOMException('Aborted', 'AbortError'))
          },
          { once: true },
        )
      })
    } else {
      active++
    }
    try {
      return await fn()
    } finally {
      release()
    }
  }
}
