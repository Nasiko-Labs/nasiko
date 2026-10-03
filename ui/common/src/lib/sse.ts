/**
 * text/event-stream decoding for fetch bodies (not EventSource, so 401/404 keep a status code).
 * - `parseSse(buffer)` splits complete events off a string buffer and returns the remainder.
 *   A trailing `\r` stays in the remainder: the next chunk may start with `\n`, and turning a
 *   split CRLF into two line breaks would end an event early.
 * - `SseDecoder` adds UTF-8 decoding across chunk boundaries and size caps.
 * - `readSse(res, onEvents)` reads a whole response body through a decoder.
 */

export interface SseEvent {
  event: string
  data: string
}

/** Minimal text/event-stream parser: complete events plus the unparsed remainder. */
export function parseSse(buffer: string): { events: SseEvent[]; rest: string } {
  // Hold a trailing CR back: it may be the first half of a CRLF split across chunks.
  const heldCr = buffer.endsWith('\r')
  const body = heldCr ? buffer.slice(0, -1) : buffer
  const events: SseEvent[] = []
  // The spec allows CRLF or CR line endings; normalise before splitting on blank lines.
  const chunks = body.replace(/\r\n?/g, '\n').split('\n\n')
  const rest = (chunks.pop() ?? '') + (heldCr ? '\r' : '')
  for (const chunk of chunks) {
    let event = 'message'
    const data: string[] = []
    for (const line of chunk.split('\n')) {
      // Lines starting with ':' are comments (keep-alives); other fields are ignored.
      if (line.startsWith('event:')) event = line.slice(6).trim()
      else if (line.startsWith('data:')) data.push(line.slice(5).trimStart())
    }
    if (data.length) events.push({ event, data: data.join('\n') })
  }
  return { events, rest }
}

/** Thrown when one event or a whole stream exceeds its cap. */
export class SseOverflowError extends Error {
  readonly limit: 'event' | 'stream'
  constructor(limit: 'event' | 'stream') {
    super(limit === 'event' ? 'SSE event exceeds the size cap' : 'SSE stream exceeds the size cap')
    this.name = 'SseOverflowError'
    this.limit = limit
  }
}

export interface SseLimits {
  /** Largest single unfinished event, in UTF-16 code units (close enough to bytes for a cap). */
  maxEventChars?: number
  /** Largest total body, in bytes. */
  maxStreamBytes?: number
}

/** Stateful decoder: feed byte chunks, get complete events. A partial event at the end is dropped. */
export class SseDecoder {
  private readonly text = new TextDecoder('utf-8')
  private buffer = ''
  private bytes = 0
  private readonly limits: SseLimits

  constructor(limits: SseLimits = {}) {
    this.limits = limits
  }

  push(chunk: Uint8Array | string): SseEvent[] {
    if (typeof chunk === 'string') this.buffer += chunk
    else {
      this.bytes += chunk.byteLength
      if (this.limits.maxStreamBytes !== undefined && this.bytes > this.limits.maxStreamBytes)
        throw new SseOverflowError('stream')
      // `stream: true` keeps a code point split across chunks for the next call.
      this.buffer += this.text.decode(chunk, { stream: true })
    }
    const { events, rest } = parseSse(this.buffer)
    this.buffer = rest
    if (this.limits.maxEventChars !== undefined && rest.length > this.limits.maxEventChars)
      throw new SseOverflowError('event')
    return events
  }

  /** End of stream: flush the UTF-8 decoder; an unterminated event is discarded (per spec). */
  end(): SseEvent[] {
    this.buffer += this.text.decode()
    const { events } = parseSse(this.buffer)
    this.buffer = ''
    return events
  }
}

export interface ReadSseOptions extends SseLimits {
  signal?: AbortSignal
  /**
   * What a size cap does: `cancel` (default) rejects with SseOverflowError and stops the download;
   * `drain` calls `onDrain` once, stops decoding and keeps reading to EOF, so a server that only
   * persists at the end of its stream still gets there (routed chat, v1b §5.3).
   */
  onLimit?: 'cancel' | 'drain'
  onDrain?(limit: 'event' | 'stream'): void
}

/**
 * Read a response body to the end, calling `onEvents` with each batch of complete events.
 * Resolves when the body ends; rejects on abort (AbortError), overflow (unless draining) or a
 * network error.
 */
export async function readSse(
  res: Response,
  onEvents: (events: SseEvent[]) => void,
  opts: ReadSseOptions = {},
): Promise<void> {
  if (!res.body) return
  const decoder = new SseDecoder(opts)
  let draining = false
  const reader = res.body.getReader()
  const onAbort = () => void reader.cancel().catch(() => undefined)
  opts.signal?.addEventListener('abort', onAbort, { once: true })
  let ended = false
  try {
    for (;;) {
      if (opts.signal?.aborted) throw new DOMException('Aborted', 'AbortError')
      const { value, done } = await reader.read()
      if (opts.signal?.aborted) throw new DOMException('Aborted', 'AbortError')
      if (done) {
        ended = true
        break
      }
      if (draining) continue
      let events: SseEvent[]
      try {
        events = decoder.push(value)
      } catch (err) {
        if (!(err instanceof SseOverflowError) || opts.onLimit !== 'drain') throw err
        draining = true
        opts.onDrain?.(err.limit)
        continue
      }
      if (events.length) onEvents(events)
    }
    if (draining) return
    const tail = decoder.end()
    if (tail.length) onEvents(tail)
  } finally {
    opts.signal?.removeEventListener('abort', onAbort)
    // Leaving early (overflow, a throwing onEvents): stop the download, not just the loop.
    if (!ended) void reader.cancel().catch(() => undefined)
  }
}
