/**
 * An SSE Response whose frames are released on demand (v1b E-A5): tests step a stream through
 * "working" → "asking" → "writing" and assert each state before the next frame arrives.
 */
import { encodeFrame, type MockFrame } from '@/mocks/chat'

export interface GatedSse {
  response: Response
  /** Send the next `n` frames (default 1). Resolves once they're enqueued. */
  release(n?: number): Promise<void>
  /** Send every remaining frame and close the stream. */
  releaseAll(): Promise<void>
  /** End the stream with a network error (a dropped connection). */
  fail(): void
  /** Frames not yet sent. */
  readonly remaining: number
  /** Whether the reader cancelled the body. */
  readonly cancelled: boolean
}

export function gatedSseResponse(frames: readonly MockFrame[]): GatedSse {
  const enc = new TextEncoder()
  let i = 0
  let cancelled = false
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c
    },
    cancel() {
      cancelled = true
    },
  })
  const tick = () => new Promise<void>((r) => setTimeout(r, 0))
  const gate: GatedSse = {
    response: new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } }),
    async release(n = 1) {
      for (let k = 0; k < n && i < frames.length && !cancelled; k++)
        controller.enqueue(enc.encode(encodeFrame(frames[i++]!)))
      await tick()
    },
    async releaseAll() {
      await gate.release(frames.length - i)
      if (!cancelled) controller.close()
      await tick()
    },
    fail() {
      if (!cancelled) controller.error(new TypeError('network error'))
    },
    get remaining() {
      return frames.length - i
    },
    get cancelled() {
      return cancelled
    },
  }
  return gate
}
