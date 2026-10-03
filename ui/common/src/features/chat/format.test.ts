/**
 * Pure Chat format helpers: Markdown export (and the download that saves it), header totals,
 * usage, the normalize quirks they read through, and the Copy buttons' useCopy.
 */
import { SAVED_NOTE_MS } from '@/features/agents/tuning'
import { act, renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { downloadText } from '@/lib/download'
import { useCopy } from '@/lib/useCopy'
import { chatTotals, exportMarkdown, usageFromMessage, usageFromMeta } from './format'
import { RECEIVING_STOPPED_MARKER, splitStopped, toNumber, unwrapSession } from './normalize'
import type { ChatMessage } from './types'

const msg = (over: Partial<ChatMessage>): ChatMessage => ({
  id: 'm',
  session_id: 's',
  role: 'assistant',
  content: '',
  timestamp: '2026-09-27T10:00:00Z',
  ...over,
})

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('exportMarkdown', () => {
  it('skips system rows, labels speakers and turns the stop marker into a note', () => {
    const md = exportMarkdown('My chat', [
      msg({ id: '1', role: 'user', content: 'hi' }),
      msg({ id: '2', role: 'system', content: 'secret system prompt' }),
      msg({ id: '3', content: `partial${RECEIVING_STOPPED_MARKER}` }),
    ])
    expect(md.startsWith('# My chat\n')).toBe(true)
    expect(md).toContain('## You · 2026-09-27T10:00:00Z')
    expect(md).toContain('## Agent · 2026-09-27T10:00:00Z')
    expect(md).not.toContain('secret system prompt')
    expect(md).not.toContain('_Receiving stopped_')
    expect(md).toContain('partial')
    expect(md).toContain('_(receiving stopped)_')
  })
})

describe('chatTotals and usage', () => {
  it('sums tokens and string costs, counts replies without usage, and flags estimates', () => {
    const t = chatTotals([
      msg({ input_tokens: 100, output_tokens: 20, cost_usd: '0.0010' }),
      msg({ input_tokens: 5, output_tokens: 5, cost_usd: 0.002, usage_estimated: true }),
      msg({}),
    ])
    expect(t).toEqual({ tokens: 130, cost: 0.003, withoutUsage: 1, estimated: true })
    // Tokens but no cost anywhere: cost stays unknown (null), not $0.
    expect(chatTotals([msg({ input_tokens: 1 })]).cost).toBeNull()
  })

  it('usageFromMessage and usageFromMeta derive totals and return null when empty', () => {
    expect(usageFromMessage(msg({}))).toBeNull()
    expect(usageFromMessage(msg({ duration_ms: 50 }))).toMatchObject({
      tokens: null,
      durationMs: 50,
    })
    expect(usageFromMessage(msg({ output_tokens: 7 }))).toMatchObject({
      tokens: 7,
      inputTokens: null,
      outputTokens: 7,
    })
    expect(usageFromMeta(null)).toBeNull()
    expect(usageFromMeta({ input_tokens: 3, output_tokens: 4 } as never)).toMatchObject({
      tokens: 7,
      cost: null,
      estimated: false,
    })
    expect(usageFromMeta({ total_tokens: 99, input_tokens: 1 } as never)).toMatchObject({
      tokens: 99,
    })
  })
})

describe('normalize quirks', () => {
  it('unwrapSession reads {data} and bare rows and rejects anything else; toNumber and splitStopped', () => {
    expect(unwrapSession({ data: { session_id: 'a' } }).session_id).toBe('a')
    expect(unwrapSession({ session_id: 'b' }).session_id).toBe('b')
    expect(() => unwrapSession({ data: null })).toThrow('Unexpected chat session response')
    expect(() => unwrapSession('nope')).toThrow()
    expect(toNumber('0.5')).toBe(0.5)
    expect(toNumber('  ')).toBeNull()
    expect(toNumber('abc')).toBeNull()
    expect(toNumber(Number.NaN)).toBeNull()
    expect(splitStopped({ content: 'x', metadata: { stopped: true } })).toEqual({
      text: 'x',
      stopped: true,
    })
    expect(splitStopped({ content: 'x', metadata: null })).toEqual({ text: 'x', stopped: false })
  })
})

describe('downloadText', () => {
  it('clicks a temporary link and revokes the object URL only after a delay', () => {
    vi.useFakeTimers()
    const create = vi.fn(() => 'blob:x')
    const revoke = vi.fn()
    Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke })
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined)
    downloadText('chat.md', '# hi', 'text/markdown')
    expect(create).toHaveBeenCalledOnce()
    expect(click).toHaveBeenCalledOnce()
    const a = click.mock.contexts[0] as HTMLAnchorElement
    expect(a.download).toBe('chat.md')
    expect(document.body.contains(a)).toBe(false)
    expect(revoke).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1000)
    expect(revoke).toHaveBeenCalledWith('blob:x')
  })
})

describe('useCopy', () => {
  const setClipboard = (value: unknown) =>
    Object.defineProperty(navigator, 'clipboard', { value, configurable: true })
  afterEach(() => setClipboard(undefined))

  it('copied fades back to idle; a missing clipboard fails and stays failed', async () => {
    vi.useFakeTimers()
    const writeText = vi.fn().mockResolvedValue(undefined)
    setClipboard({ writeText })
    const { result } = renderHook(() => useCopy('text'))
    await act(async () => expect(await result.current[1]()).toBe(true))
    expect(writeText).toHaveBeenCalledWith('text')
    expect(result.current[0]).toBe('copied')
    act(() => {
      vi.advanceTimersByTime(SAVED_NOTE_MS)
    })
    expect(result.current[0]).toBe('idle')

    setClipboard(undefined)
    await act(async () => expect(await result.current[1]()).toBe(false))
    expect(result.current[0]).toBe('failed')
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    expect(result.current[0]).toBe('failed')
  })
})
