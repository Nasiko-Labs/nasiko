import { describe, expect, it, vi } from 'vitest'
import { mockStream, type MockFrame } from '@/mocks/chat'
import { parseSse, readSse, SseDecoder, SseOverflowError, type SseEvent } from './sse'

const enc = new TextEncoder()

async function readAll(
  frames: MockFrame[],
  opts: Parameters<typeof mockStream>[1] = {},
  limits = {},
): Promise<SseEvent[]> {
  const out: SseEvent[] = []
  await readSse(new Response(mockStream(frames, opts)), (ev) => out.push(...ev), limits)
  return out
}

describe('parseSse', () => {
  it('keeps a trailing CR in the remainder so a split CRLF is not two line breaks', () => {
    const a = parseSse('data: one\r\n\r')
    expect(a.events).toEqual([])
    expect(a.rest).toBe('data: one\n\r')
    expect(parseSse(`${a.rest}\ndata: two\r\n\r\n`).events).toEqual([
      { event: 'message', data: 'one' },
      { event: 'message', data: 'two' },
    ])
  })

  it('does not end an event early when a CR lands at a chunk end', () => {
    // Old behaviour: "data: a\r" → "data: a\n", then "\ndata: b\n\n" made a false boundary.
    const first = parseSse('data: a\r')
    const second = parseSse(`${first.rest}\ndata: b\n\n`)
    expect(second.events).toEqual([{ event: 'message', data: 'a\nb' }])
  })
})

describe('parseSse: spec edges', () => {
  it('handles CRLF and CR line endings, multi-line data, comments, data-less events and empty input', () => {
    expect(parseSse('data: a\r\n\r\ndata: b\r\rdata: c')).toEqual({
      events: [
        { event: 'message', data: 'a' },
        { event: 'message', data: 'b' },
      ],
      rest: 'data: c',
    })
    expect(parseSse('data: one\ndata:two\n\n').events).toEqual([
      { event: 'message', data: 'one\ntwo' },
    ])
    // A comment (keep-alive) and an event with no data dispatch nothing.
    expect(parseSse(': ping\n\nevent: close\n\n').events).toEqual([])
    expect(parseSse('')).toEqual({ events: [], rest: '' })
  })
})

describe('SseDecoder', () => {
  it('decodes a UTF-8 code point split across chunks', () => {
    const d = new SseDecoder()
    const bytes = enc.encode('data: café ☕\n\n')
    const out: SseEvent[] = []
    for (let i = 0; i < bytes.length; i++) out.push(...d.push(bytes.slice(i, i + 1)))
    out.push(...d.end())
    expect(out).toEqual([{ event: 'message', data: 'café ☕' }])
  })

  it('ignores comment lines and joins multi-line data', () => {
    const d = new SseDecoder()
    expect(d.push(': keep-alive\n\nevent: error\ndata: {"a":\ndata: 1}\n\n')).toEqual([
      { event: 'error', data: '{"a":\n1}' },
    ])
  })

  it('drops an unterminated event at the end of the stream', () => {
    const d = new SseDecoder()
    expect(d.push('data: done\n\ndata: partial')).toHaveLength(1)
    expect(d.end()).toEqual([])
  })

  it('caps one event and the whole stream', () => {
    expect(() => new SseDecoder({ maxEventChars: 10 }).push('data: 0123456789abc')).toThrow(
      SseOverflowError,
    )
    let caught: unknown
    try {
      new SseDecoder({ maxStreamBytes: 8 }).push(enc.encode('data: 0123456789\n\n'))
    } catch (e) {
      caught = e
    }
    expect(caught).toBeInstanceOf(SseOverflowError)
    expect((caught as SseOverflowError).limit).toBe('stream')
  })
})

describe('readSse', () => {
  const frames: MockFrame[] = [
    { data: { a: 'é' } },
    { event: 'close', data: 'bye' },
    { data: { b: 2 } },
  ]

  it('reads every event with CRLF endings split one byte at a time', async () => {
    const out = await readAll(frames, { chunkBytes: 1, lineEnding: '\r\n' })
    expect(out.map((e) => [e.event, e.data])).toEqual([
      ['message', '{"a":"é"}'],
      ['close', '"bye"'],
      ['message', '{"b":2}'],
    ])
  })

  it('rejects on a dropped connection after delivering what arrived', async () => {
    const out: SseEvent[] = []
    await expect(
      readSse(new Response(mockStream(frames, { failAtEnd: true })), (ev) => out.push(...ev)),
    ).rejects.toThrow('network error')
    expect(out).toHaveLength(3)
  })

  it('rejects with AbortError when the signal aborts', async () => {
    const ctrl = new AbortController()
    const p = readSse(
      new Response(
        mockStream([
          { data: 1, delayMs: 5 },
          { data: 2, delayMs: 50 },
        ]),
      ),
      () => ctrl.abort(),
      { signal: ctrl.signal },
    )
    await expect(p).rejects.toMatchObject({ name: 'AbortError' })
  })
})

describe('readSse cleanup', () => {
  it('cancels the download when it stops early (overflow), not just the loop', async () => {
    const cancel = vi.fn()
    let n = 0
    const body = new ReadableStream<Uint8Array>({
      pull(c) {
        c.enqueue(new TextEncoder().encode(`data: ${'x'.repeat(40)}\n\n`))
        if (++n > 50) c.close()
      },
      cancel,
    })
    await expect(
      readSse(new Response(body), () => undefined, { maxStreamBytes: 100 }),
    ).rejects.toBeInstanceOf(SseOverflowError)
    expect(cancel).toHaveBeenCalled()
  })
})

describe('readSse drain mode (v1b §5.3)', () => {
  function body(chunks: string[]) {
    let cancelled = false
    const enc = new TextEncoder()
    const stream = new ReadableStream<Uint8Array>({
      pull(c) {
        const n = chunks.shift()
        if (n === undefined) c.close()
        else c.enqueue(enc.encode(n))
      },
      cancel() {
        cancelled = true
      },
    })
    return { res: new Response(stream), cancelled: () => cancelled }
  }

  it('an event over the cap drains to EOF without cancelling, and stops delivering events', async () => {
    // The event cap is on an unfinished event: a long one still arriving.
    const b = body(['data: ok\n\n', `data: ${'x'.repeat(50)}`, 'x\n\ndata: after\n\n'])
    const seen: string[] = []
    const onDrain = vi.fn()
    await readSse(b.res, (evs) => seen.push(...evs.map((e) => e.data)), {
      maxEventChars: 10,
      onLimit: 'drain',
      onDrain,
    })
    expect(onDrain).toHaveBeenCalledOnce()
    expect(onDrain).toHaveBeenCalledWith('event')
    expect(seen).toEqual(['ok'])
    expect(b.cancelled()).toBe(false)
  })

  it('the stream byte cap drains too, and says which limit', async () => {
    const b = body(['data: a\n\n', 'data: bbbbbbbbbb\n\n', 'data: c\n\n'])
    const onDrain = vi.fn()
    const seen: string[] = []
    await readSse(b.res, (evs) => seen.push(...evs.map((e) => e.data)), {
      maxStreamBytes: 12,
      onLimit: 'drain',
      onDrain,
    })
    expect(onDrain).toHaveBeenCalledWith('stream')
    expect(seen).toEqual(['a'])
    expect(b.cancelled()).toBe(false)
  })

  it('without drain, both caps still reject with SseOverflowError', async () => {
    await expect(
      readSse(body([`data: ${'x'.repeat(50)}`]).res, () => undefined, { maxEventChars: 10 }),
    ).rejects.toBeInstanceOf(SseOverflowError)
    await expect(
      readSse(body(['data: aaaaaaaaaaaaaaaa\n\n']).res, () => undefined, { maxStreamBytes: 5 }),
    ).rejects.toBeInstanceOf(SseOverflowError)
  })
})
