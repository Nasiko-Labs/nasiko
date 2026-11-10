import { describe, expect, it } from 'vitest'
import { connectWithoutLogin, serverUrl } from './commands'

describe('CLI commands that name this server (NAS-1464)', () => {
  it('names the page origin, never a hosted control plane', () => {
    expect(serverUrl()).toBe(window.location.origin)
    expect(serverUrl()).not.toMatch(/nasiko\.dev/)
  })

  it('registers this server without signing in, so the sign-in step asks once', () => {
    expect(connectWithoutLogin()).toBe(`nasiko connect ${window.location.origin} --login=false`)
  })
})
