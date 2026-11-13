import { afterEach, describe, expect, it, vi } from 'vitest'
import { identifySignIn } from './analytics'

const g = globalThis as { Reo?: { identify: (identity: unknown) => void } }

afterEach(() => {
  delete g.Reo
  document.head.innerHTML = ''
})

describe('identifySignIn', () => {
  it('sends the address as the email identity, the display name split into first and last', () => {
    const identify = vi.fn()
    g.Reo = { identify }
    identifySignIn('ada@acme.com', 'Ada King Lovelace')
    expect(identify).toHaveBeenCalledWith({
      username: 'ada@acme.com',
      type: 'email',
      firstname: 'Ada',
      lastname: 'King Lovelace',
    })
  })

  it('sends nothing for a username that is not an address, or a made-up placeholder', () => {
    const identify = vi.fn()
    g.Reo = { identify }
    identifySignIn('admin')
    identifySignIn(null)
    identifySignIn('ada@okta.users')
    expect(identify).not.toHaveBeenCalled()
  })

  it('waits for reo.js when the sign-in lands before it has loaded, and does nothing without a loader', () => {
    identifySignIn('ada@acme.com')
    const script = document.createElement('script')
    script.src = 'https://static.reo.dev/abc/reo.js'
    document.head.appendChild(script)
    identifySignIn('ada@acme.com')
    const identify = vi.fn()
    g.Reo = { identify }
    script.dispatchEvent(new Event('load'))
    expect(identify).toHaveBeenCalledTimes(1)
    expect(identify).toHaveBeenCalledWith({ username: 'ada@acme.com', type: 'email' })
  })
})
