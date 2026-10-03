import { readFileSync } from 'node:fs'
import { describe, expect, it, vi } from 'vitest'
import { MOCKABLE, readEnv } from './env'

describe('env', () => {
  it('waitlist: an http(s) URL gets ref=oss-app (its own query and hash kept); the Nasiko waitlist page by default', () => {
    const url = (raw: Record<string, string | boolean>) => readEnv(raw).waitlistUrl
    expect(
      url({ VITE_NASIKO_WAITLIST_URL: 'https://example.com/join?form=a#top', PROD: true }),
    ).toBe('https://example.com/join?form=a&ref=oss-app#top')
    expect(url({ VITE_NASIKO_WAITLIST_URL: 'https://example.com/?ref=x', PROD: true })).toBe(
      'https://example.com/?ref=oss-app',
    )
    expect(url({ PROD: true })).toBe('https://nasiko-waitlist.vercel.app/?ref=oss-app')
    expect(url({ DEV: true })).toBe('https://nasiko-waitlist.vercel.app/?ref=oss-app')
    for (const bad of ['javascript:alert(1)', '//example.com', 'example.com', 'https://'])
      expect(url({ VITE_NASIKO_WAITLIST_URL: bad, DEV: true }), bad).toBeNull()
  })

  it('.env.example lists exactly the partial-mock keys (auth is never partially mocked)', () => {
    const example = readFileSync('.env.example', 'utf8')
    const line = example.split('\n').find((l) => l.startsWith('# Keys:'))!
    const keys = line
      .replace('# Keys:', '')
      .split('(')[0]
      .split(',')
      .map((k) => k.trim())
      .filter(Boolean)
    expect(keys.sort()).toEqual(MOCKABLE.filter((k) => k !== 'auth').sort())
  })

  it('observability can be partially mocked in live mode', () => {
    expect(
      readEnv({ VITE_NASIKO_API_MODE: 'live', VITE_NASIKO_MOCK: 'observability,auth', DEV: true })
        .partialMocks,
    ).toEqual(['observability'])
  })

  it('harnesses is one key: an edition brings its own identity routes with it, so there is no org key', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    warn.mockClear()
    const live = (mock: string) =>
      readEnv({ VITE_NASIKO_API_MODE: 'live', VITE_NASIKO_MOCK: mock, DEV: true }).partialMocks
    expect(live('harnesses,top-traces')).toEqual(['harnesses', 'top-traces'])
    expect(live('harnesses,org')).toEqual(['harnesses'])
    expect(live('org')).toEqual([])
    expect(MOCKABLE).not.toContain('org')
    expect(warn).not.toHaveBeenCalled()
  })

  it('router needs agents and providers; alone, only router is dropped, with the working value', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    warn.mockClear()
    const live = (mock: string) =>
      readEnv({ VITE_NASIKO_API_MODE: 'live', VITE_NASIKO_MOCK: mock, DEV: true }).partialMocks
    expect(live('router,agents,providers').sort()).toEqual(['agents', 'providers', 'router'])
    expect(warn).not.toHaveBeenCalled()
    expect(live('router')).toEqual([])
    // providers survives for TokenOps (eng #13); agents survives too.
    expect(live('router,providers')).toEqual(['providers'])
    expect(live('router,agents')).toEqual(['agents'])
    expect(warn).toHaveBeenCalledTimes(3)
    expect(warn).toHaveBeenCalledWith(
      expect.stringContaining('use VITE_NASIKO_MOCK=router,agents,providers'),
    )
  })

  it('chat is mocked only with agents and observability', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(
      readEnv({
        VITE_NASIKO_API_MODE: 'live',
        VITE_NASIKO_MOCK: 'chat,agents,observability',
        DEV: true,
      }).partialMocks.sort(),
    ).toEqual(['agents', 'chat', 'observability'])
    expect(
      readEnv({ VITE_NASIKO_API_MODE: 'live', VITE_NASIKO_MOCK: 'chat', DEV: true }).partialMocks,
    ).toEqual([])
    expect(
      readEnv({ VITE_NASIKO_API_MODE: 'live', VITE_NASIKO_MOCK: 'chat,agents', DEV: true })
        .partialMocks,
    ).not.toContain('chat')
  })
})
