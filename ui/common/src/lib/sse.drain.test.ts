// A Stop during a drain must still reject (v1b §5.3): a drain that resolved would read as a
// finished turn to the routed registry.
import { describe, expect, it } from 'vitest'
import { readSse } from './sse'

describe('readSse drain mode and abort', () => {
  it('an abort while draining rejects with AbortError and cancels the body', async () => {
    let cancelled = false
    const enc = new TextEncoder()
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(enc.encode(`data: ${'x'.repeat(50)}`))
      },
      cancel() {
        cancelled = true
      },
    })
    const ctrl = new AbortController()
    const p = readSse(new Response(body), () => undefined, {
      maxEventChars: 10,
      onLimit: 'drain',
      onDrain: () => ctrl.abort(),
      signal: ctrl.signal,
    })
    await expect(p).rejects.toMatchObject({ name: 'AbortError' })
    expect(cancelled).toBe(true)
  })
})
