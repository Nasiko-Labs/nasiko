import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  CONSENT_KEY,
  identifySignIn,
  loadReo,
  marketingConsent,
  onConsentChange,
  privacySignal,
  setMarketingConsent,
} from './analytics'

const reoScript = () =>
  document.querySelector<HTMLScriptElement>('script[src^="https://static.reo.dev/"]')
const agree = () =>
  localStorage.setItem(CONSENT_KEY, JSON.stringify({ v: 1, marketing: true, at: 'x' }))

beforeEach(() => localStorage.clear())
afterEach(() => {
  delete window.Reo
  document.head.innerHTML = ''
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('consent', () => {
  it('is undecided until the person chooses, and remembers the choice', () => {
    expect(marketingConsent()).toBeNull()
    const seen: (boolean | null)[] = []
    const off = onConsentChange((m) => seen.push(m))
    setMarketingConsent(true)
    off()
    expect(marketingConsent()).toBe(true)
    expect(seen).toEqual([true])
  })

  it('treats a stored value it does not recognise as undecided', () => {
    localStorage.setItem(CONSENT_KEY, 'accepted')
    expect(marketingConsent()).toBeNull()
    localStorage.setItem(CONSENT_KEY, JSON.stringify({ v: 9, marketing: true }))
    expect(marketingConsent()).toBeNull()
  })

  it('withdrawing a yes clears Reo’s cookies and storage and reloads', () => {
    agree()
    document.cookie = '__sec__id=abc; path=/'
    localStorage.setItem('__sec__visitor', '1')
    const reload = vi.fn()
    vi.stubGlobal('location', { ...location, hostname: 'localhost', reload })
    setMarketingConsent(false)
    expect(document.cookie).not.toContain('__sec__id')
    expect(localStorage.getItem('__sec__visitor')).toBeNull()
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it('reads Global Privacy Control and Do Not Track as a signal', () => {
    expect(privacySignal()).toBe(false)
    vi.stubGlobal('navigator', { ...navigator, globalPrivacyControl: true })
    expect(privacySignal()).toBe(true)
  })
})

describe('loadReo', () => {
  it('loads nothing without consent', () => {
    loadReo('f4cf7633edc0a46')
    expect(reoScript()).toBeNull()
  })

  it('loads reo.js once with consent, with copy tracking off', () => {
    agree()
    loadReo('f4cf7633edc0a46')
    loadReo('f4cf7633edc0a46')
    expect(document.querySelectorAll('script[src^="https://static.reo.dev/"]')).toHaveLength(1)
    const init = vi.fn()
    window.Reo = { init, identify: vi.fn() }
    reoScript()!.dispatchEvent(new Event('load'))
    expect(init).toHaveBeenCalledWith({ clientID: 'f4cf7633edc0a46', dnt: ['copy'] })
  })

  it('refuses a client id that is not one', () => {
    agree()
    loadReo('x/../evil.js')
    expect(reoScript()).toBeNull()
  })
})

describe('identifySignIn', () => {
  it('sends nothing without consent, even when Reo is on the page', () => {
    const identify = vi.fn()
    window.Reo = { init: vi.fn(), identify }
    identifySignIn('ada@acme.com', 'Ada Lovelace')
    expect(identify).not.toHaveBeenCalled()
  })

  it('sends the lowercased address, the display name split into first and last', () => {
    agree()
    const identify = vi.fn()
    window.Reo = { init: vi.fn(), identify }
    identifySignIn('Ada@Acme.com', 'Ada King Lovelace')
    expect(identify).toHaveBeenCalledWith({
      username: 'ada@acme.com',
      type: 'email',
      firstname: 'Ada',
      lastname: 'King Lovelace',
    })
  })

  it('sends nothing for a username that is not an address, or a made-up placeholder', () => {
    agree()
    const identify = vi.fn()
    window.Reo = { init: vi.fn(), identify }
    identifySignIn('admin')
    identifySignIn(null)
    identifySignIn('ada@okta.users')
    expect(identify).not.toHaveBeenCalled()
  })

  it('waits for reo.js when the sign-in lands before it has loaded', () => {
    agree()
    loadReo('f4cf7633edc0a46')
    identifySignIn('ada@acme.com')
    const identify = vi.fn()
    window.Reo = { init: vi.fn(), identify }
    reoScript()!.dispatchEvent(new Event('load'))
    expect(identify).toHaveBeenCalledTimes(1)
    expect(identify).toHaveBeenCalledWith({ username: 'ada@acme.com', type: 'email' })
  })
})
