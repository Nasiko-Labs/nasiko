/**
 * The DEV tuning overrides (tuning.ts): `localStorage['ui-lab:chat-tuning']` and `?tune=`.
 */
import { describe, expect, it, vi } from 'vitest'
import { tuneParams } from './tuning'

describe('tuning overrides (DEV)', () => {
  const load = async () => {
    vi.resetModules()
    return (await import('./tuning')).tuning
  }

  it('takes finite non-negative numbers for known keys only; bad JSON falls back to defaults', async () => {
    // setup.ts gives each test its own in-memory localStorage.
    localStorage.setItem(
      'ui-lab:chat-tuning',
      JSON.stringify({ SLOW_MS: 10, LONG_MS: -1, STREAM_IDLE_MS: '5', NOPE: 1 }),
    )
    const t = await load()
    expect(t.SLOW_MS).toBe(10)
    expect(t.LONG_MS).toBe(30_000)
    expect(t.STREAM_IDLE_MS).toBe(90_000)
    expect('NOPE' in t).toBe(false)

    localStorage.setItem('ui-lab:chat-tuning', '{not json')
    expect((await load()).SLOW_MS).toBe(8_000)
    localStorage.setItem('ui-lab:chat-tuning', '42')
    expect((await load()).SLOW_MS).toBe(8_000)
  })

  it('?tune= parses key:ms pairs and ignores the rest', () => {
    expect(
      tuneParams(
        '?tune=LOST_REPLY_AFTER_MS:5000&tune=MAX_LIVE_TURNS:1&tune=NOPE:1&tune=SLOW_MS:-1&tune=LONG_MS:',
      ),
    ).toEqual({ LOST_REPLY_AFTER_MS: 5000, MAX_LIVE_TURNS: 1 })
  })
})
